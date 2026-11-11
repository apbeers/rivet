//! Darwin PTY integration. No native UI object is touched by its worker.
use std::{
    ffi::{c_char, c_int, c_void},
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
static SESSIONS: AtomicUsize = AtomicUsize::new(0);
#[repr(C)]
struct Size {
    rows: u16,
    columns: u16,
    x: u16,
    y: u16,
}
unsafe extern "C" {
    fn openpty(
        master: *mut c_int,
        slave: *mut c_int,
        name: *mut c_char,
        term: *const c_void,
        size: *const Size,
    ) -> c_int;
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    fn setsid() -> c_int;
    fn ioctl(fd: c_int, request: u64, ...) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
}
pub struct Terminal {
    input: SyncSender<Vec<u8>>,
    output: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}
impl Terminal {
    pub fn spawn(root: &Path) -> Result<Self, String> {
        if SESSIONS.fetch_add(1, Ordering::SeqCst) >= 4 {
            SESSIONS.fetch_sub(1, Ordering::SeqCst);
            return Err("Terminal budget reached (4)".into());
        }
        let (tx, rx) = mpsc::sync_channel(16);
        let (out, output) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (flag, finished, root) = (stop.clone(), done.clone(), root.to_path_buf());
        let spawned = std::thread::Builder::new()
            .name("rivet-pty".into())
            .spawn(move || {
                if let Err(e) = Self::serve(root, rx, &out, &flag) {
                    let _ = out.try_send(format!("\r\n[PTY error: {e}]\r\n").into_bytes());
                }
                finished.store(true, Ordering::Release);
                SESSIONS.fetch_sub(1, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            SESSIONS.fetch_sub(1, Ordering::SeqCst);
            return Err(e.to_string());
        }
        Ok(Self {
            input: tx,
            output,
            stop,
            done,
        })
    }
    fn serve(
        root: PathBuf,
        rx: Receiver<Vec<u8>>,
        out: &SyncSender<Vec<u8>>,
        flag: &AtomicBool,
    ) -> Result<(), String> {
        let (mut master, mut slave) = (-1, -1);
        let size = Size {
            rows: 40,
            columns: 100,
            x: 0,
            y: 0,
        };
        // SAFETY: descriptor outputs and Darwin winsize layout are valid.
        if unsafe {
            openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        } < 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: openpty transferred two unique owned descriptors.
        let (mut master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        unsafe {
            if fcntl(master.as_raw_fd(), 2, 1) < 0 || fcntl(slave.as_raw_fd(), 2, 1) < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let flags = fcntl(master.as_raw_fd(), 3);
            if flags < 0 || fcntl(master.as_raw_fd(), 4, flags | 4) < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
        let mut command = Command::new(shell);
        command
            .arg("-i")
            .current_dir(root)
            .env("TERM", "xterm")
            .stdin(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
            .stdout(Stdio::from(slave.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(slave));
        // SAFETY: these are async-signal-safe operations after stdio assignment, before exec.
        unsafe {
            command.pre_exec(|| {
                if setsid() < 0 || ioctl(0, 0x20007461u64, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        drop(command);
        let pid = child.id() as c_int;
        let mut pending = Vec::new();
        let mut buffer = [0; 4096];
        let mut exited = false;
        let mut failure = None;
        while !flag.load(Ordering::Relaxed) {
            for chunk in rx.try_iter().take(16) {
                if pending.len() + chunk.len() > 65536 {
                    failure = Some("Terminal input budget exceeded; session stopped".to_string());
                    flag.store(true, Ordering::Relaxed);
                    break;
                }
                pending.extend(chunk);
            }
            if !pending.is_empty() {
                match master.write(&pending) {
                    Ok(n) => {
                        pending.drain(..n);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        failure = Some(e.to_string());
                        break;
                    }
                }
            }
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    let mut chunk = buffer[..n].to_vec();
                    loop {
                        match out.try_send(chunk) {
                            Ok(()) => break,
                            Err(mpsc::TrySendError::Disconnected(_)) => {
                                flag.store(true, Ordering::Relaxed);
                                break;
                            }
                            Err(mpsc::TrySendError::Full(c)) => {
                                chunk = c;
                                if flag.load(Ordering::Relaxed) {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => break,
            }
            if matches!(child.try_wait(), Ok(Some(_))) {
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if !exited {
            // Best-effort foreground/shell-group cleanup, not a process/filesystem sandbox.
            unsafe {
                let mut foreground = 0;
                if ioctl(master.as_raw_fd(), 0x40047477u64, &mut foreground) == 0 && foreground > 0
                {
                    kill(-foreground, 15);
                }
                kill(-pid, 15);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
    pub fn write(&self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > 4096 {
            return Err("Terminal input exceeds 4 KiB".into());
        }
        self.input
            .try_send(bytes)
            .map_err(|_| "Terminal input queue full or session exited".into())
    }
    pub fn sender(&self) -> SyncSender<Vec<u8>> {
        self.input.clone()
    }
    pub fn try_output(&self) -> Option<Vec<u8>> {
        self.output.try_recv().ok()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.stop();
    }
}
