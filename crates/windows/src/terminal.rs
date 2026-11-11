//! ConPTY sessions, bounded channels, job ownership and independent pipe draining.
use super::sys::*;
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};
static COUNT: AtomicUsize = AtomicUsize::new(0);
fn error() -> String {
    io::Error::last_os_error().to_string()
}
unsafe fn owned(h: Handle) -> OwnedHandle {
    unsafe { OwnedHandle::from_raw_handle(h) }
}
#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min: usize,
    max: usize,
    processes: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
#[derive(Default)]
struct Limits {
    basic: BasicLimits,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process: usize,
    peak_job: usize,
}
struct Console(Handle);
impl Drop for Console {
    fn drop(&mut self) {
        unsafe {
            ClosePseudoConsole(self.0);
        }
    }
}
pub struct Terminal {
    input: SyncSender<Vec<u8>>,
    output: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}
impl Terminal {
    pub fn spawn(root: &Path) -> Result<Self, String> {
        if COUNT.fetch_add(1, Ordering::SeqCst) >= 4 {
            COUNT.fetch_sub(1, Ordering::SeqCst);
            return Err("Terminal budget reached (4)".into());
        }
        let (tx, rx) = mpsc::sync_channel(16);
        let (out, output) = mpsc::sync_channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (flag, finished, root) = (stop.clone(), done.clone(), root.to_owned());
        if let Err(e) = std::thread::Builder::new()
            .name("rivet-conpty".into())
            .spawn(move || {
                if let Err(e) = serve(&root, rx, out.clone(), flag) {
                    let _ = out.try_send(format!("\r\n[ConPTY: {e}]\r\n").into_bytes());
                }
                finished.store(true, Ordering::Release);
                COUNT.fetch_sub(1, Ordering::SeqCst);
            })
        {
            COUNT.fetch_sub(1, Ordering::SeqCst);
            return Err(e.to_string());
        }
        Ok(Self {
            input: tx,
            output,
            stop,
            done,
        })
    }
    pub fn write(&self, bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() > 4096 {
            return Err("Terminal input exceeds 4 KiB".into());
        }
        self.input
            .try_send(bytes)
            .map_err(|_| "Terminal input full or disconnected".into())
    }
    pub fn try_output(&self) -> Option<Vec<u8>> {
        self.output.try_recv().ok()
    }
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
    pub fn is_finished(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}
fn serve(
    root: &Path,
    input: Receiver<Vec<u8>>,
    out: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
) -> Result<(), String> {
    // SAFETY: API structures follow the Windows x64 ABI. Kernel handles are uniquely
    // owned; no handle is closed while a pipe worker uses it. Everything executes off UI.
    unsafe {
        let (mut ir, mut iw, mut or, mut ow) = (
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        if CreatePipe(&mut ir, &mut iw, ptr::null(), 65536) == 0 {
            return Err(error());
        }
        let (ir, iw) = (owned(ir), owned(iw));
        if CreatePipe(&mut or, &mut ow, ptr::null(), 65536) == 0 {
            return Err(error());
        }
        let (or, ow) = (owned(or), owned(ow));
        let mut hpc = ptr::null_mut();
        let hr = CreatePseudoConsole(
            Coord { x: 100, y: 40 },
            ir.as_raw_handle(),
            ow.as_raw_handle(),
            0,
            &mut hpc,
        );
        if hr < 0 {
            return Err(format!(
                "ConPTY unavailable: HRESULT {hr:#x}; requires Windows 10 1809 or later"
            ));
        }
        let console = Console(hpc);
        drop(ir);
        drop(ow);
        let mut size = 0;
        InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut size);
        if size == 0 {
            return Err(error());
        }
        let mut attributes = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        let list = attributes.as_mut_ptr().cast();
        if InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0 {
            return Err(error());
        }
        struct Attributes(*mut std::ffi::c_void);
        impl Drop for Attributes {
            fn drop(&mut self) {
                unsafe {
                    DeleteProcThreadAttributeList(self.0);
                }
            }
        }
        let _attributes = Attributes(list);
        if UpdateProcThreadAttribute(
            list,
            0,
            0x20016,
            hpc,
            std::mem::size_of::<Handle>(),
            ptr::null_mut(),
            ptr::null_mut(),
        ) == 0
        {
            return Err(error());
        }
        let mut startup: StartupInfoEx = std::mem::zeroed();
        startup.info.cb = std::mem::size_of::<StartupInfoEx>() as u32;
        // Force ConPTY connections instead of duplicating a launcher's redirected
        // stdio. Keep all three explicit handles null; ConPTY initializes them.
        // https://github.com/microsoft/terminal/discussions/15814
        startup.info.flags = 0x100; // STARTF_USESTDHANDLES
        startup.attributes = list;
        let mut pi: ProcessInfo = std::mem::zeroed();
        let shell =
            std::env::var("COMSPEC").unwrap_or_else(|_| "C:\\Windows\\System32\\cmd.exe".into());
        let mut command = wide(&format!("\"{}\" /D /Q", shell));
        let mut cwd: Vec<u16> =
            std::os::windows::ffi::OsStrExt::encode_wide(root.as_os_str()).collect();
        // cmd.exe treats verbatim disk paths as UNC and silently changes directory.
        // Preserve UTF-16 filenames while removing only a verified disk prefix.
        if cwd.starts_with(&wide("\\\\?\\")[..4])
            && cwd.len() > 6
            && cwd[4] < 128
            && (cwd[4] as u8).is_ascii_alphabetic()
            && cwd[5] == b':' as u16
            && cwd[6] == b'\\' as u16
        {
            cwd.drain(..4);
        }
        if cwd.starts_with(&[b'\\' as u16, b'\\' as u16]) {
            return Err("The initial cmd.exe terminal requires a local disk workspace".into());
        }
        cwd.push(0);
        let job = CreateJobObjectW(ptr::null(), ptr::null());
        if job.is_null() {
            return Err(error());
        }
        let job = owned(job);
        let mut limits = Limits::default();
        limits.basic.flags = 0x2000;
        if SetInformationJobObject(
            job.as_raw_handle(),
            9,
            &limits as *const _ as *const _,
            std::mem::size_of::<Limits>() as u32,
        ) == 0
        {
            return Err(error());
        }
        if CreateProcessW(
            wide(&shell).as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            0x80000 | 0x400 | 4,
            ptr::null(),
            cwd.as_ptr(),
            &mut startup.info,
            &mut pi,
        ) == 0
        {
            return Err(error());
        }
        let (process, thread) = (owned(pi.process), owned(pi.thread));
        if AssignProcessToJobObject(job.as_raw_handle(), process.as_raw_handle()) == 0 {
            TerminateProcess(process.as_raw_handle(), 1);
            return Err(format!("Cannot supervise terminal process: {}", error()));
        }
        let flag = stop.clone();
        let writer = std::thread::Builder::new()
            .name("rivet-conpty-input".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    match input.recv_timeout(Duration::from_millis(25)) {
                        Ok(bytes) => {
                            let mut offset = 0;
                            while offset < bytes.len() && !flag.load(Ordering::Relaxed) {
                                let mut n = 0;
                                if WriteFile(
                                    iw.as_raw_handle(),
                                    bytes[offset..].as_ptr().cast(),
                                    (bytes.len() - offset) as u32,
                                    &mut n,
                                    ptr::null_mut(),
                                ) == 0
                                    || n == 0
                                {
                                    return;
                                }
                                offset += n as usize;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let flag = stop.clone();
        let reader = std::thread::Builder::new()
            .name("rivet-conpty-output".into())
            .spawn(move || {
                let mut bytes = [0; 4096];
                loop {
                    let mut n = 0;
                    if ReadFile(
                        or.as_raw_handle(),
                        bytes.as_mut_ptr().cast(),
                        4096,
                        &mut n,
                        ptr::null_mut(),
                    ) == 0
                        || n == 0
                    {
                        break;
                    }
                    if flag.load(Ordering::Relaxed) {
                        continue;
                    }
                    let mut chunk = bytes[..n as usize].to_vec();
                    loop {
                        match out.try_send(chunk) {
                            Ok(()) => break,
                            Err(mpsc::TrySendError::Full(c)) => {
                                chunk = c;
                                if flag.load(Ordering::Relaxed) {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(5));
                            }
                            Err(_) => return,
                        }
                    }
                }
            })
            .map_err(|e| {
                stop.store(true, Ordering::Relaxed);
                CancelSynchronousIo(writer.as_raw_handle());
                e.to_string()
            })?;
        if ResumeThread(thread.as_raw_handle()) == u32::MAX {
            stop.store(true, Ordering::Relaxed);
        }
        while !stop.load(Ordering::Relaxed)
            && WaitForSingleObject(process.as_raw_handle(), 10) == 258
        {}
        stop.store(true, Ordering::Relaxed);
        TerminateJobObject(job.as_raw_handle(), 1);
        CancelSynchronousIo(writer.as_raw_handle());
        // The reader keeps draining during ClosePseudoConsole, which can otherwise block.
        drop(console);
        CancelSynchronousIo(reader.as_raw_handle());
        let _ = writer.join();
        let _ = reader.join();
        WaitForSingleObject(process.as_raw_handle(), 5000);
        Ok(())
    }
}
