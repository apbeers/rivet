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
    List { root: PathBuf, path: PathBuf },
    Read { root: PathBuf, path: PathBuf },
}
#[derive(Debug)]
pub enum FileEvent {
    Entries {
        path: PathBuf,
        entries: Vec<(PathBuf, bool)>,
    },
    Loaded {
        path: PathBuf,
        text: Arc<str>,
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
                        FileCommand::List { root, path } => scoped(&root, &path).and_then(|path| {
                            let mut entries = Vec::new();
                            for entry in fs::read_dir(&path)?.take(10_000) {
                                let e = entry?;
                                entries.push((e.path(), e.file_type()?.is_dir()));
                            }
                            entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                            Ok(FileEvent::Entries { path, entries })
                        }),
                        FileCommand::Read { root, path } => cache
                            .read(&root, &path)
                            .map(|(path, text)| FileEvent::Loaded { path, text }),
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
#[cfg(test)]
mod tests {
    use super::*;
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
}
