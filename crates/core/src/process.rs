//! Bounded capture and cancellable process supervision. Not an OS sandbox.
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};
static NONCE: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout: Duration,
    pub capture_limit: usize,
}
#[derive(Debug)]
pub struct Outcome {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub truncated: bool,
}
impl Outcome {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out && !self.cancelled && !self.truncated
    }
    pub fn display(&self) -> String {
        format!(
            "exit={:?} timeout={} cancelled={} truncated={}\n{}\n{}",
            self.code, self.timed_out, self.cancelled, self.truncated, self.stdout, self.stderr
        )
    }
}
fn capture(path: &std::path::Path, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
pub fn run(spec: &Spec, cancel: &AtomicBool) -> io::Result<Outcome> {
    if cancel.load(Ordering::Relaxed) {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Process cancelled before launch",
        ));
    }
    if spec.program.is_empty() || spec.capture_limit == 0 || spec.capture_limit > 4 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid process budget",
        ));
    }
    let dir = std::env::temp_dir().join(format!(
        "rivet-job-{}-{}",
        std::process::id(),
        NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir)?;
    let execute = || -> io::Result<Outcome> {
        let stdout = dir.join("stdout");
        let stderr = dir.join("stderr");
        let open = |p: &std::path::Path| {
            let mut o = OpenOptions::new();
            o.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            o.open(p)
        };
        // Files avoid pipe-reader deadlocks when descendants inherit standard handles.
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(open(&stdout)?)
            .stderr(open(&stderr)?)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env_remove("GIT_EXTERNAL_DIFF");
        struct Child(std::process::Child);
        impl std::ops::Deref for Child {
            type Target = std::process::Child;
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
        impl std::ops::DerefMut for Child {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
        impl Drop for Child {
            fn drop(&mut self) {
                if !matches!(self.0.try_wait(), Ok(Some(_))) {
                    let _ = self.0.kill();
                    let _ = self.0.wait();
                }
            }
        }
        let mut child = Child(command.spawn()?);
        let start = Instant::now();
        let (mut timed_out, mut cancelled, mut truncated) = (false, false, false);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            timed_out = start.elapsed() > spec.timeout;
            cancelled = cancel.load(Ordering::Relaxed);
            truncated = fs::metadata(&stdout)?.len() > spec.capture_limit as u64
                || fs::metadata(&stderr)?.len() > spec.capture_limit as u64;
            if timed_out || cancelled || truncated {
                let _ = child.kill();
                break child.wait()?;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        truncated |= fs::metadata(&stdout)?.len() > spec.capture_limit as u64
            || fs::metadata(&stderr)?.len() > spec.capture_limit as u64;
        Ok(Outcome {
            code: status.code(),
            stdout: capture(&stdout, spec.capture_limit)?,
            stderr: capture(&stderr, spec.capture_limit)?,
            timed_out,
            cancelled,
            truncated,
        })
    };
    let result = execute();
    let _ = fs::remove_dir_all(dir);
    result
}
pub struct Completed {
    pub id: u64,
    pub result: Result<Outcome, String>,
}
pub struct Worker {
    tx: SyncSender<(u64, Spec, Arc<AtomicBool>)>,
    rx: Receiver<Completed>,
    stop: Arc<AtomicBool>,
    alive: Arc<AtomicUsize>,
    flags: Arc<Mutex<std::collections::HashMap<u64, Arc<AtomicBool>>>>,
}
impl Worker {
    pub fn spawn() -> io::Result<Self> {
        let (tx, jobs) = mpsc::sync_channel::<(u64, Spec, Arc<AtomicBool>)>(16);
        let jobs = Arc::new(Mutex::new(jobs));
        let (out, rx) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let flags = Arc::new(Mutex::new(std::collections::HashMap::new()));
        let alive = Arc::new(AtomicUsize::new(0));
        for index in 0..2 {
            alive.fetch_add(1, Ordering::SeqCst);
            let live = alive.clone();
            let (jobs, out, stop, flags) = (jobs.clone(), out.clone(), stop.clone(), flags.clone());
            std::thread::Builder::new()
                .name(format!("rivet-process-{index}"))
                .spawn(move || {
                    struct Done(Arc<AtomicUsize>);
                    impl Drop for Done {
                        fn drop(&mut self) {
                            self.0.fetch_sub(1, Ordering::SeqCst);
                        }
                    }
                    let _done = Done(live);
                    while !stop.load(Ordering::Relaxed) {
                        let item = jobs.lock().unwrap().try_recv();
                        let (id, spec, cancel) = match item {
                            Ok(j) => j,
                            Err(mpsc::TryRecvError::Empty) => {
                                std::thread::sleep(Duration::from_millis(5));
                                continue;
                            }
                            Err(_) => break,
                        };
                        let result = if cancel.load(Ordering::Relaxed) {
                            Err("Cancelled before execution".into())
                        } else {
                            run(&spec, &cancel).map_err(|e| e.to_string())
                        };
                        flags.lock().unwrap().remove(&id);
                        let mut event = Completed { id, result };
                        loop {
                            match out.try_send(event) {
                                Ok(()) => break,
                                Err(mpsc::TrySendError::Disconnected(_)) => return,
                                Err(mpsc::TrySendError::Full(e)) => {
                                    event = e;
                                    if stop.load(Ordering::Relaxed) {
                                        return;
                                    }
                                    std::thread::sleep(Duration::from_millis(5));
                                }
                            }
                        }
                    }
                })?;
        }
        Ok(Self {
            tx,
            rx,
            stop,
            alive,
            flags,
        })
    }
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        for flag in self.flags.lock().unwrap().values() {
            flag.store(true, Ordering::Relaxed);
        }
    }
    pub fn is_finished(&self) -> bool {
        self.alive.load(Ordering::SeqCst) == 0
    }
    pub fn submit(&self, id: u64, spec: Spec) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("Process worker stopped".into());
        }
        let flag = Arc::new(AtomicBool::new(false));
        {
            let mut flags = self.flags.lock().map_err(|_| "Process state poisoned")?;
            if flags.len() >= 16 || flags.contains_key(&id) {
                return Err("Process budget reached or duplicate job ID".into());
            }
            flags.insert(id, flag.clone());
        }
        if self.tx.try_send((id, spec, flag)).is_err() {
            self.flags.lock().unwrap().remove(&id);
            return Err("Process queue unavailable".into());
        }
        Ok(())
    }
    pub fn cancel(&self, id: u64) {
        if let Some(flag) = self.flags.lock().unwrap().get(&id) {
            flag.store(true, Ordering::Relaxed);
        }
    }
    pub fn try_event(&self) -> Option<Completed> {
        self.rx.try_recv().ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}
pub fn git(root: PathBuf, args: Vec<String>) -> Spec {
    let mut base = vec![
        "--no-pager".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
    ];
    base.extend(args);
    Spec {
        program: "git".into(),
        args: base,
        cwd: root,
        timeout: Duration::from_secs(60),
        capture_limit: 1024 * 1024,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_capture_timeouts_and_missing_tools() {
        let py = if cfg!(windows) { "python" } else { "python3" };
        let mut s = Spec {
            program: py.into(),
            args: vec![
                "-c".into(),
                "import sys;print('stdout');print('stderr',file=sys.stderr)".into(),
            ],
            cwd: std::env::current_dir().unwrap(),
            timeout: Duration::from_secs(5),
            capture_limit: 4096,
        };
        let o = run(&s, &AtomicBool::new(false)).unwrap();
        assert!(o.success());
        assert!(o.stdout.contains("stdout"));
        assert!(o.stderr.contains("stderr"));
        s.args = vec!["-c".into(), "import time;time.sleep(10)".into()];
        s.timeout = Duration::from_millis(50);
        assert!(run(&s, &AtomicBool::new(false)).unwrap().timed_out);
        s.program = "rivet-nonexistent-program-12345".into();
        assert!(run(&s, &AtomicBool::new(false)).is_err());
    }
}
