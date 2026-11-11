//! Workspace-scoped, bounded filesystem jobs and byte-budgeted immutable cache.
use std::{
    collections::VecDeque,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, SystemTime},
};
const FILE_LIMIT: usize = 16 * 1024 * 1024;
const CACHE_LIMIT: usize = 32 * 1024 * 1024;
#[derive(Debug)]
pub enum FileCommand {
    ReadAt {
        root: PathBuf,
        path: PathBuf,
        line: u32,
        character: u32,
    },
    Diff {
        before: Arc<str>,
        snapshot: crate::editor::Snapshot,
    },
    Save {
        root: PathBuf,
        path: PathBuf,
        expected: Arc<str>,
        snapshot: crate::editor::Snapshot,
        revision: u64,
    },
    List {
        root: PathBuf,
        path: PathBuf,
    },
    Read {
        root: PathBuf,
        path: PathBuf,
    },
}
#[derive(Debug)]
pub enum FileEvent {
    SaveFailed {
        path: PathBuf,
        message: String,
    },
    Diff(crate::diff::Diff),
    Saved {
        path: PathBuf,
        text: Arc<str>,
        revision: u64,
    },
    Entries {
        path: PathBuf,
        entries: Vec<(PathBuf, bool)>,
    },
    Loaded {
        path: PathBuf,
        text: Arc<str>,
        buffer: crate::editor::Buffer,
        cursor: Option<(u32, u32)>,
    },
    Error(String),
}
struct Cached {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    text: Arc<str>,
}
#[derive(Default)]
pub struct DocumentCache {
    items: VecDeque<Cached>,
    bytes: usize,
}
fn scoped(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let root = root.canonicalize()?;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .canonicalize()?;
    if !path.starts_with(&root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "path resolves outside workspace",
        ));
    }
    Ok(path)
}
impl DocumentCache {
    pub fn read(&mut self, root: &Path, path: &Path) -> io::Result<(PathBuf, Arc<str>)> {
        let path = scoped(root, path)?;
        let meta = fs::metadata(&path)?;
        if !meta.is_file() || meta.len() > FILE_LIMIT as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a file or exceeds 16 MiB",
            ));
        }
        if let Some(i) = self.items.iter().position(|c| c.path == path) {
            let c = self.items.remove(i).unwrap();
            if c.modified == meta.modified().ok() && c.len == meta.len() {
                let text = c.text.clone();
                self.items.push_back(c);
                return Ok((path, text));
            }
            self.bytes -= c.text.len();
        }
        let mut data = Vec::with_capacity(meta.len() as usize);
        fs::File::open(&path)?
            .take(FILE_LIMIT as u64 + 1)
            .read_to_end(&mut data)?;
        if data.len() > FILE_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file grew beyond read budget",
            ));
        }
        let text: Arc<str> = String::from_utf8(data)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "binary/non-UTF-8 file"))?
            .into();
        while self.bytes + text.len() > CACHE_LIMIT || self.items.len() >= 20 {
            if let Some(old) = self.items.pop_front() {
                self.bytes -= old.text.len();
            } else {
                break;
            }
        }
        self.bytes += text.len();
        self.items.push_back(Cached {
            path: path.clone(),
            modified: meta.modified().ok(),
            len: meta.len(),
            text: text.clone(),
        });
        Ok((path, text))
    }
}
pub struct FileWorker {
    commands: SyncSender<FileCommand>,
    events: Receiver<FileEvent>,
    stop: Arc<AtomicBool>,
}
impl FileWorker {
    pub fn spawn() -> io::Result<Self> {
        Self::spawn_with_save(save_checked)
    }
    pub fn spawn_with_save(
        save: impl Fn(&Path, &Path, &str, &crate::editor::Snapshot) -> io::Result<PathBuf>
        + Send
        + 'static,
    ) -> io::Result<Self> {
        Self::spawn_with_save_and_normalization(save, false)
    }
    pub fn spawn_with_save_and_normalization(
        save: impl Fn(&Path, &Path, &str, &crate::editor::Snapshot) -> io::Result<PathBuf>
        + Send
        + 'static,
        normalize: bool,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel(16);
        let (out, events) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("rivet-files".into())
            .spawn(move || {
                let mut cache = DocumentCache::default();
                while !flag.load(Ordering::Relaxed) {
                    let c = match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(c) => c,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(_) => break,
                    };
                    let result = match c {
                        FileCommand::Diff { before, snapshot } => {
                            Ok(FileEvent::Diff(crate::diff::compare(
                                &if normalize {
                                    normalize_lines(&before)
                                } else {
                                    before.to_string()
                                },
                                &snapshot.text(),
                            )))
                        }
                        FileCommand::Save {
                            root,
                            path,
                            expected,
                            snapshot,
                            revision,
                        } => {
                            // Normalized native editors retain the disk's CRLF style.
                            // Materialize and convert only here, off the UI thread;
                            // the acknowledgement must contain the actual saved bytes.
                            let snapshot = if normalize && expected.contains("\r\n") {
                                crate::editor::Buffer::new(Arc::from(
                                    snapshot.text().replace('\n', "\r\n"),
                                ))
                                .snapshot()
                            } else {
                                snapshot
                            };
                            Ok(match save(&root, &path, &expected, &snapshot) {
                                Ok(_) => FileEvent::Saved {
                                    path,
                                    text: Arc::from(snapshot.text()),
                                    revision,
                                },
                                Err(error) => FileEvent::SaveFailed {
                                    path,
                                    message: error.to_string(),
                                },
                            })
                        }
                        FileCommand::List { root, path } => scoped(&root, &path).and_then(|path| {
                            let mut entries = Vec::new();
                            for entry in fs::read_dir(&path)?.take(10_000) {
                                let e = entry?;
                                entries.push((e.path(), e.file_type()?.is_dir()));
                            }
                            entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                            Ok(FileEvent::Entries { path, entries })
                        }),
                        FileCommand::ReadAt {
                            root,
                            path,
                            line,
                            character,
                        } => cache
                            .read(&root, &path)
                            .map(|(path, text)| FileEvent::Loaded {
                                path,
                                buffer: crate::editor::Buffer::new(if normalize {
                                    Arc::from(normalize_lines(&text))
                                } else {
                                    text.clone()
                                }),
                                text,
                                cursor: Some((line, character)),
                            }),
                        FileCommand::Read { root, path } => {
                            cache
                                .read(&root, &path)
                                .map(|(path, text)| FileEvent::Loaded {
                                    path,
                                    buffer: crate::editor::Buffer::new(if normalize {
                                        Arc::from(normalize_lines(&text))
                                    } else {
                                        text.clone()
                                    }),
                                    cursor: None,
                                    text,
                                })
                        }
                    }
                    .unwrap_or_else(|e| FileEvent::Error(e.to_string()));
                    // No UI blocking. Backpressure stops this one worker, with cancellation polling.
                    let mut event = result;
                    loop {
                        match out.try_send(event) {
                            Ok(()) => break,
                            Err(mpsc::TrySendError::Disconnected(_)) => return,
                            Err(mpsc::TrySendError::Full(e)) => {
                                event = e;
                                if flag.load(Ordering::Relaxed) {
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            commands: tx,
            events,
            stop,
        })
    }
    pub fn submit(&self, c: FileCommand) -> Result<(), String> {
        self.commands
            .try_send(c)
            .map_err(|_| "Filesystem queue full or disconnected".into())
    }
    pub fn try_event(&self) -> Option<FileEvent> {
        self.events.try_recv().ok()
    }
}
impl Drop for FileWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
fn save_checked(
    root: &Path,
    path: &Path,
    expected: &str,
    snapshot: &crate::editor::Snapshot,
) -> io::Result<PathBuf> {
    #[cfg(windows)]
    {
        let _ = (root, path, expected, snapshot);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Native Windows replacement backend required",
        ))
    }
    #[cfg(not(windows))]
    save_with_replace(root, path, expected, snapshot, |temp, path| {
        fs::rename(temp, path)?;
        fs::File::open(path.parent().unwrap())?.sync_all()
    })
}
/// Worker-only shared validation and staging; native backends own atomic replacement.
pub fn save_with_replace(
    root: &Path,
    path: &Path,
    expected: &str,
    snapshot: &crate::editor::Snapshot,
    replace: impl Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    use std::io::Write;
    let path = scoped(root, path)?;
    let read = || -> io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take(FILE_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > FILE_LIMIT {
            return Err(io::Error::other("External file exceeds save budget"));
        }
        Ok(bytes)
    };
    let original = read()?;
    if original != expected.as_bytes() {
        return Err(io::Error::other(
            "File changed outside Rivet; reload or reconcile before saving",
        ));
    }
    let permissions = fs::metadata(&path)?.permissions();
    if permissions.readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "File is read-only",
        ));
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_file_name(format!(".rivet-save-{}-{nonce}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.set_permissions(permissions)?;
        snapshot.write_to(&mut file)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        if read()? != original {
            return Err(io::Error::other(
                "File changed during save; original preserved",
            ));
        }
        replace(&temporary, &path)?;
        Ok(path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn normalize_lines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalized_saves_acknowledge_crlf_bytes_for_subsequent_saves() {
        let root = std::env::temp_dir().join(format!("rivet-crlf-save-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("source.rs");
        fs::write(&path, "original\r\n").unwrap();
        // The native atomic-replace backend is tested on Windows separately.
        let worker = FileWorker::spawn_with_save_and_normalization(
            |_, path, expected, snapshot| {
                assert_eq!(fs::read_to_string(path)?, expected);
                fs::write(path, snapshot.text())?;
                Ok(path.to_owned())
            },
            true,
        )
        .unwrap();
        let mut expected: Arc<str> = Arc::from("original\r\n");
        for revision in 1..=2 {
            let normalized = format!("saved 🦀 {revision}\n");
            worker
                .submit(FileCommand::Save {
                    root: root.clone(),
                    path: path.clone(),
                    expected,
                    snapshot: crate::editor::Buffer::new(Arc::from(normalized.as_str())).snapshot(),
                    revision,
                })
                .unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            expected = loop {
                if let Some(event) = worker.try_event() {
                    let FileEvent::Saved { text, .. } = event else {
                        panic!("CRLF save failed: {event:?}");
                    };
                    assert_eq!(&*text, normalized.replace('\n', "\r\n"));
                    assert_eq!(&*text, fs::read_to_string(&path).unwrap());
                    break text;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            };
        }
        drop(worker);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn normalized_editor_buffers_keep_original_disk_text() {
        let root = std::env::temp_dir().join(format!("rivet-line-endings-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("source.rs");
        fs::write(&path, "let crab = \"🦀\";\r\n").unwrap();
        let worker = FileWorker::spawn_with_save_and_normalization(save_checked, true).unwrap();
        worker
            .submit(FileCommand::Read {
                root: root.clone(),
                path,
            })
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = worker.try_event() {
                let FileEvent::Loaded { text, buffer, .. } = event else {
                    panic!("Unexpected file result");
                };
                assert!(text.contains("\r\n"));
                assert_eq!(buffer.snapshot().text(), "let crab = \"🦀\";\n");
                assert_eq!(
                    buffer
                        .position(buffer.byte_to_utf16(buffer.len()).unwrap())
                        .unwrap(),
                    (1, 0)
                );
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(worker);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[cfg(not(windows))]
    fn save_worker_reports_the_exact_document_and_remains_usable() {
        let root = std::env::temp_dir().join(format!("rivet-save-worker-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let actual = root.join("actual");
        std::fs::create_dir_all(&actual).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&actual, root.join("alias")).unwrap();
        let path = if cfg!(unix) {
            root.join("alias/file")
        } else {
            actual.join("file")
        };
        std::fs::write(&path, "original").unwrap();
        let worker = FileWorker::spawn().unwrap();
        let mut buffer = crate::editor::Buffer::new(Arc::from("original"));
        buffer.replace(0..8, "saved").unwrap();
        let receive = || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(event) = worker.try_event() {
                    break event;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "filesystem worker stopped answering"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        worker
            .submit(FileCommand::Save {
                root: root.clone(),
                path: path.clone(),
                expected: Arc::from("original"),
                snapshot: buffer.snapshot(),
                revision: 7,
            })
            .unwrap();
        assert!(matches!(receive(),FileEvent::Saved{path:p,revision:7,..} if p==path));
        worker
            .submit(FileCommand::Save {
                root: root.clone(),
                path: path.clone(),
                expected: Arc::from("original"),
                snapshot: buffer.snapshot(),
                revision: 8,
            })
            .unwrap();
        assert!(matches!(receive(),FileEvent::SaveFailed{path:p,..} if p==path));
        worker
            .submit(FileCommand::Read {
                root: root.clone(),
                path: path.clone(),
            })
            .unwrap();
        assert!(matches!(receive(),FileEvent::Loaded{text,..} if &*text=="saved"));
        drop(worker);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cached_documents_invalidate_and_paths_cannot_escape() {
        let root = std::env::temp_dir().join(format!("rivet-documents-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a"), "🦀").unwrap();
        let mut c = DocumentCache::default();
        let (_, a) = c.read(&root, Path::new("a")).unwrap();
        let (_, b) = c.read(&root, Path::new("a")).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        fs::write(root.join("a"), "changed length").unwrap();
        assert_eq!(&*c.read(&root, Path::new("a")).unwrap().1, "changed length");
        assert!(c.read(&root, Path::new("../outside")).is_err());
        fs::write(root.join("binary"), [0xff]).unwrap();
        assert!(c.read(&root, Path::new("binary")).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.parent().unwrap(), root.join("escape")).unwrap();
            assert!(scoped(&root, &root.join("escape")).is_err());
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    #[cfg(not(windows))]
    fn atomic_save_refuses_external_edits_and_preserves_file_mode() {
        let root = std::env::temp_dir().join(format!("rivet-save-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("source.rs");
        fs::write(&path, "original").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        let mut b = crate::editor::Buffer::new(Arc::from("original"));
        b.replace(0..8, "updated 🦀").unwrap();
        save_checked(&root, &path, "original", &b.snapshot()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "updated 🦀");
        assert!(save_checked(&root, &path, "original", &b.snapshot()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "updated 🦀");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o640
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
