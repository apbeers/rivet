//! First-party JSON-RPC/LSP process transport with bounded queues.
use crate::json::{self, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
pub fn object(items: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Value {
    Value::object(items)
}
pub fn notification(method: &str, params: Value) -> Value {
    object([
        ("jsonrpc", Value::from("2.0")),
        ("method", Value::from(method)),
        ("params", params),
    ])
}
pub fn request(id: u32, method: &str, params: Value) -> Value {
    let mut v = notification(method, params);
    if let Value::Object(m) = &mut v {
        m.insert("id".into(), id.into());
    }
    v
}
pub fn file_uri(path: &Path) -> Result<String, String> {
    let path = path
        .to_str()
        .ok_or("LSP requires UTF-8 paths")?
        .replace('\\', "/");
    let mut encoded = String::new();
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    Ok(format!(
        "file://{}{}",
        if encoded.starts_with('/') { "" } else { "/" },
        encoded
    ))
}
pub fn uri_path(uri: &str) -> Result<PathBuf, String> {
    let path = uri.strip_prefix("file://").ok_or("Not a file location")?;
    if !path.starts_with('/') {
        return Err("Remote file authorities are unsupported".into());
    }
    let bytes = path.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let pair = std::str::from_utf8(bytes.get(i + 1..i + 3).ok_or("Bad URI escape")?)
                .map_err(|_| "Bad URI escape")?;
            out.push(u8::from_str_radix(pair, 16).map_err(|_| "Bad URI escape")?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(out).map_err(|_| "URI path is not UTF-8")?;
    #[cfg(windows)]
    let text = if text.as_bytes().get(2) == Some(&b':') {
        text[1..].to_owned()
    } else {
        text
    };
    Ok(PathBuf::from(text))
}
pub struct Connection {
    tx: SyncSender<Value>,
    rx: Receiver<Result<Value, String>>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}
impl Connection {
    pub fn spawn(program: &str, args: &[String], root: &Path) -> Result<Self, String> {
        let mut count = CONNECTIONS.load(Ordering::SeqCst);
        loop {
            if count >= 3 {
                return Err("Language server connection budget reached".into());
            }
            match CONNECTIONS.compare_exchange(count, count + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => count = actual,
            }
        }
        let child = Command::new(program)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                return Err(e.to_string());
            }
        };
        let (mut input, mut output) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        let (tx, commands) = mpsc::sync_channel::<Value>(16);
        let (out, rx) = mpsc::sync_channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        struct Child {
            child: std::process::Child,
            stop: Arc<AtomicBool>,
        }
        impl std::ops::Deref for Child {
            type Target = std::process::Child;
            fn deref(&self) -> &Self::Target {
                &self.child
            }
        }
        impl std::ops::DerefMut for Child {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.child
            }
        }
        impl Drop for Child {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::Relaxed);
                let _ = self.child.kill();
                let _ = self.child.wait();
                CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let mut child = Child {
            child,
            stop: stop.clone(),
        };
        let flag = stop.clone();
        let writer = std::thread::Builder::new()
            .name("rivet-lsp-write".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    match commands.recv_timeout(Duration::from_millis(50)) {
                        Ok(message) => {
                            let frame = json::frame(&message);
                            if frame.len() > 16 * 1024 * 1024
                                || input.write_all(&frame).and_then(|_| input.flush()).is_err()
                            {
                                flag.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let (flag, replies, events) = (stop.clone(), tx.clone(), out.clone());
        let reader = std::thread::Builder::new()
            .name("rivet-lsp-read".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    let message = json::read_frame(&mut output);
                    if let Ok(v) = &message
                        && v.get("method").is_some()
                        && let Some(id) = v.get("id")
                    {
                        // Server requests never authorize edits, processes or privileged application actions.
                        let method = v.get("method").and_then(Value::str).unwrap_or("");
                        let reply = if method == "workspace/configuration" {
                            let count = v
                                .get("params")
                                .and_then(|p| p.get("items"))
                                .and_then(Value::array)
                                .map_or(0, |a| a.len().min(64));
                            object([
                                ("jsonrpc", Value::from("2.0")),
                                ("id", id.clone()),
                                ("result", Value::Array(vec![Value::Null; count])),
                            ])
                        } else if method == "workspace/applyEdit" {
                            object([
                                ("jsonrpc", Value::from("2.0")),
                                ("id", id.clone()),
                                (
                                    "result",
                                    object([
                                        ("applied", Value::Bool(false)),
                                        (
                                            "failureReason",
                                            Value::from("Explicit source review required"),
                                        ),
                                    ]),
                                ),
                            ])
                        } else {
                            object([
                                ("jsonrpc", Value::from("2.0")),
                                ("id", id.clone()),
                                (
                                    "error",
                                    object([
                                        ("code", Value::Number(-32601.0)),
                                        ("message", Value::from("Unsupported client request")),
                                    ]),
                                ),
                            ])
                        };
                        if replies.try_send(reply).is_err() {
                            flag.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                    let failed = message.is_err();
                    let mut item = message;
                    loop {
                        match events.try_send(item) {
                            Ok(()) => break,
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                flag.store(true, Ordering::Relaxed);
                                return;
                            }
                            Err(mpsc::TrySendError::Full(e)) => {
                                item = e;
                                if flag.load(Ordering::Relaxed) {
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                    }
                    if failed {
                        flag.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let flag = stop.clone();
        let done = Arc::new(AtomicBool::new(false));
        let finished = done.clone();
        std::thread::Builder::new()
            .name("rivet-lsp-supervisor".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    match child.try_wait() {
                        Ok(Some(_)) | Err(_) => {
                            flag.store(true, Ordering::Relaxed);
                            break;
                        }
                        _ => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                let _ = reader.join();
                drop(child);
                finished.store(true, Ordering::SeqCst);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { tx, rx, stop, done })
    }
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
    pub fn send(&self, message: Value) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("Language server stopped".into());
        }
        self.tx
            .try_send(message)
            .map_err(|_| "Language server queue full or disconnected".into())
    }
    pub fn try_event(&self) -> Option<Result<Value, String>> {
        self.rx.try_recv().ok()
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
pub fn initialize(root: &Path) -> Result<Value, String> {
    Ok(request(
        1,
        "initialize",
        object([
            ("processId", (std::process::id()).into()),
            ("rootUri", file_uri(root)?.into()),
            (
                "capabilities",
                object([(
                    "general",
                    object([("positionEncodings", Value::Array(vec!["utf-16".into()]))]),
                )]),
            ),
        ]),
    ))
}
pub fn did_open(path: &Path, text: String) -> Result<Value, String> {
    let language = match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "c" | "h" => "c",
        "cpp" | "cc" => "cpp",
        _ => "plaintext",
    };
    Ok(notification(
        "textDocument/didOpen",
        object([(
            "textDocument",
            object([
                ("uri", file_uri(path)?.into()),
                ("languageId", language.into()),
                ("version", 0u32.into()),
                ("text", text.into()),
            ]),
        )]),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_stdio_transport_and_shutdown() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixture = root.join("tools/lsp_fixture.py");
        let server = Connection::spawn(
            if cfg!(windows) { "python" } else { "python3" },
            &[fixture.to_string_lossy().into()],
            &root,
        )
        .unwrap();
        server.send(initialize(&root).unwrap()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let receive = || loop {
            if let Some(event) = server.try_event() {
                break event.unwrap();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "server did not answer"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(receive().get("id").and_then(Value::integer), Some(1));
        server
            .send(request(2, "textDocument/hover", Value::Null))
            .unwrap();
        assert_eq!(
            receive()
                .get("result")
                .and_then(|v| v.get("contents"))
                .and_then(Value::str),
            Some("Fixture hover")
        );
        server.shutdown();
        while !server.is_finished() {
            assert!(std::time::Instant::now() < deadline, "server did not stop");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn local_unicode_uris_roundtrip() {
        let p = std::env::temp_dir().join("space 🦀.rs");
        assert_eq!(uri_path(&file_uri(&p).unwrap()).unwrap(), p);
        assert!(uri_path("https://example.com/a").is_err());
        assert!(uri_path("file://remote/a").is_err());
    }
}
