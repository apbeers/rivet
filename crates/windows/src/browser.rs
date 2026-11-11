//! Minimal WebView2 COM ABI, verified against Microsoft's official 1.0.4314.35 header.
//! Controllers stay on their creating STA. Callbacks own channels, never App pointers.
use super::sys::*;
use std::{
    collections::VecDeque,
    ffi::c_void,
    ptr,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    a: u32,
    b: u16,
    c: u16,
    d: [u8; 8],
}
const UNKNOWN: Guid = Guid {
    a: 0,
    b: 0,
    c: 0,
    d: [0xc0, 0, 0, 0, 0, 0, 0, 0x46],
};
const ENV: Guid = Guid {
    a: 0x4e8a3389,
    b: 0xc9d8,
    c: 0x4bd2,
    d: [0xb6, 0xb5, 0x12, 0x4f, 0xee, 0x6c, 0xc1, 0x4d],
};
const CTRL: Guid = Guid {
    a: 0x6c4819f3,
    b: 0xc9b7,
    c: 0x4260,
    d: [0x81, 0x27, 0xc9, 0xf5, 0xbd, 0xe7, 0xf6, 0x8c],
};
// SAFETY: callers use documented vtable slots and their exact native signatures.
unsafe fn method<T: Copy>(object: *mut c_void, index: usize) -> T {
    unsafe {
        let vt = *(object as *const *const *const c_void);
        std::mem::transmute_copy(&*vt.add(index))
    }
}
struct Com(*mut c_void);
impl Com {
    unsafe fn retain(p: *mut c_void) -> Self {
        unsafe {
            let add: unsafe extern "system" fn(*mut c_void) -> u32 = method(p, 1);
            add(p);
            Self(p)
        }
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        unsafe {
            let release: unsafe extern "system" fn(*mut c_void) -> u32 = method(self.0, 2);
            release(self.0);
        }
    }
}
enum Event {
    Environment(Result<Com, String>),
    Controller(u64, Result<Com, String>),
}
#[repr(C)]
struct Callback {
    vtable: *const CallbackVtable,
    refs: AtomicU32,
    thread: u32,
    id: u64,
    environment: bool,
    events: SyncSender<Event>,
}
#[repr(C)]
struct CallbackVtable {
    query: unsafe extern "system" fn(*mut Callback, *const Guid, *mut *mut c_void) -> i32,
    add: unsafe extern "system" fn(*mut Callback) -> u32,
    release: unsafe extern "system" fn(*mut Callback) -> u32,
    invoke: unsafe extern "system" fn(*mut Callback, i32, *mut c_void) -> i32,
}
static CALLBACK: CallbackVtable = CallbackVtable {
    query,
    add,
    release,
    invoke,
};
unsafe extern "system" fn query(
    this: *mut Callback,
    id: *const Guid,
    out: *mut *mut c_void,
) -> i32 {
    unsafe {
        if out.is_null() || id.is_null() {
            return 0x80004003u32 as i32;
        }
        *out = ptr::null_mut();
        if *id == UNKNOWN || *id == if (*this).environment { ENV } else { CTRL } {
            *out = this.cast();
            add(this);
            0
        } else {
            0x80004002u32 as i32
        }
    }
}
unsafe extern "system" fn add(this: *mut Callback) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}
unsafe extern "system" fn release(this: *mut Callback) -> u32 {
    unsafe {
        let n = (*this).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if n == 0 {
            drop(Box::from_raw(this));
        }
        n
    }
}
unsafe extern "system" fn invoke(this: *mut Callback, result: i32, object: *mut c_void) -> i32 {
    unsafe {
        if GetCurrentThreadId() != (*this).thread {
            return 0x8001010eu32 as i32;
        }
        let value = if result >= 0 && !object.is_null() {
            Ok(Com::retain(object))
        } else {
            Err(format!(
                "WebView2 initialization failed ({result:#x}); install the Evergreen Runtime for the current user"
            ))
        };
        let event = if (*this).environment {
            Event::Environment(value)
        } else {
            Event::Controller((*this).id, value)
        };
        let _ = (*this).events.try_send(event);
        0
    }
}
fn callback(events: SyncSender<Event>, environment: bool, id: u64) -> *mut Callback {
    Box::into_raw(Box::new(Callback {
        vtable: &CALLBACK,
        refs: AtomicU32::new(1),
        thread: unsafe { GetCurrentThreadId() },
        id,
        environment,
        events,
    }))
}
struct Tab {
    id: u64,
    controller: Option<Com>,
    web: Option<Com>,
    url: String,
}
impl Drop for Tab {
    fn drop(&mut self) {
        if let Some(c) = &self.controller {
            unsafe {
                let close: unsafe extern "system" fn(*mut c_void) -> i32 = method(c.0, 24);
                close(c.0);
            }
        }
    }
}
fn loader() -> Result<Handle, String> {
    let path = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("WebView2Loader.dll");
    let path: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(path.as_os_str())
        .chain(Some(0))
        .collect();
    let module = unsafe { LoadLibraryExW(path.as_ptr(), ptr::null_mut(), 0x100 | 0x800) };
    if module.is_null() {
        Err("WebView2Loader.dll missing from the application directory; use the complete Windows package".into())
    } else {
        Ok(module)
    }
}
pub fn runtime_version() -> Result<String, String> {
    let module = loader()?;
    let address = unsafe {
        GetProcAddress(
            module,
            c"GetAvailableCoreWebView2BrowserVersionString"
                .as_ptr()
                .cast(),
        )
    };
    if address.is_null() {
        return Err("WebView2 runtime version API unavailable".into());
    }
    let f: unsafe extern "system" fn(*const u16, *mut *mut u16) -> i32 =
        unsafe { std::mem::transmute(address) };
    let mut version = ptr::null_mut();
    let hr = unsafe { f(ptr::null(), &mut version) };
    if hr < 0 || version.is_null() {
        return Err(format!("WebView2 Evergreen Runtime unavailable ({hr:#x})"));
    }
    unsafe {
        let mut n = 0;
        while n < 4096 && *version.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(version, n));
        CoTaskMemFree(version.cast());
        Ok(s)
    }
}
pub struct Browser {
    parent: Hwnd,
    tabs: VecDeque<Tab>,
    environment: Option<Com>,
    creating: bool,
    tx: SyncSender<Event>,
    rx: Receiver<Event>,
    next: u64,
    pub active: Option<u64>,
    bounds: Rect,
    visible: bool,
    pub error: Option<String>,
    data: Option<std::path::PathBuf>,
    layout_dirty: bool,
}
impl Browser {
    pub fn new(parent: Hwnd, data: Option<std::path::PathBuf>) -> Self {
        let (tx, rx) = mpsc::sync_channel(16);
        Self {
            parent,
            tabs: VecDeque::new(),
            environment: None,
            creating: false,
            tx,
            rx,
            next: 1,
            active: None,
            bounds: Rect::default(),
            visible: false,
            error: None,
            data,
            layout_dirty: true,
        }
    }
    pub fn ids(&self) -> Vec<u64> {
        let mut ids: Vec<_> = self.tabs.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids
    }
    pub fn create(&mut self) -> Result<u64, String> {
        if self.tabs.len() >= 5 {
            self.tabs.pop_front();
        }
        let id = self.next;
        self.next += 1;
        self.tabs.push_back(Tab {
            id,
            controller: None,
            web: None,
            url: "about:blank".into(),
        });
        self.active = Some(id);
        self.layout_dirty = true;
        if self.environment.is_some() {
            self.controller(id)?;
        } else if !self.creating {
            let module = loader()?;
            let address = unsafe {
                GetProcAddress(
                    module,
                    c"CreateCoreWebView2EnvironmentWithOptions".as_ptr().cast(),
                )
            };
            if address.is_null() {
                return Err("WebView2 loader API unavailable".into());
            }
            type Create = unsafe extern "system" fn(
                *const u16,
                *const u16,
                *mut c_void,
                *mut Callback,
            ) -> i32;
            let create: Create = unsafe { std::mem::transmute(address) };
            let data = if let Some(data) = &self.data {
                data.clone()
            } else {
                rivet_core::journal::user_data_dir()
                    .map_err(|e| e.to_string())?
                    .join("Browser")
            };
            let data = std::path::PathBuf::from(
                data.to_string_lossy()
                    .strip_prefix(r"\\?\")
                    .unwrap_or(&data.to_string_lossy()),
            );
            let folder: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(data.as_os_str())
                .chain(Some(0))
                .collect();
            let cb = callback(self.tx.clone(), true, 0);
            let result = unsafe { create(ptr::null(), folder.as_ptr(), ptr::null_mut(), cb) };
            unsafe {
                release(cb);
            }
            if result < 0 {
                return Err(format!(
                    "WebView2 runtime unavailable ({result:#x}); install Microsoft's Evergreen Runtime per user"
                ));
            }
            self.creating = true;
        }
        self.layout(self.bounds, self.visible);
        Ok(id)
    }
    fn controller(&self, id: u64) -> Result<(), String> {
        let env = self
            .environment
            .as_ref()
            .ok_or("Browser environment not ready")?;
        let cb = callback(self.tx.clone(), false, id);
        let result = unsafe {
            let f: unsafe extern "system" fn(*mut c_void, Hwnd, *mut Callback) -> i32 =
                method(env.0, 3);
            f(env.0, self.parent, cb)
        };
        unsafe {
            release(cb);
        }
        if result < 0 {
            Err(format!("WebView2 controller failed ({result:#x})"))
        } else {
            Ok(())
        }
    }
    pub fn tick(&mut self) {
        for _ in 0..8 {
            let Ok(event) = self.rx.try_recv() else {
                break;
            };
            match event {
                Event::Environment(Ok(env)) => {
                    self.creating = false;
                    self.environment = Some(env);
                    for id in self.ids() {
                        if let Err(e) = self.controller(id) {
                            self.error = Some(e);
                        }
                    }
                }
                Event::Environment(Err(e)) => {
                    self.creating = false;
                    self.error = Some(e);
                }
                Event::Controller(id, Ok(controller)) => {
                    self.layout_dirty = true;
                    let mut web = ptr::null_mut();
                    let hr = unsafe {
                        let get: unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> i32 =
                            method(controller.0, 25);
                        get(controller.0, &mut web)
                    };
                    if let Some(tab) = self.tabs.iter_mut().find(|t| t.id == id) {
                        if hr >= 0 && !web.is_null() {
                            tab.web = Some(Com(web));
                            tab.controller = Some(controller);
                            let _ = navigate(tab, &tab.url);
                        } else {
                            self.error = Some(format!("WebView2 view unavailable ({hr:#x})"));
                        }
                    } else {
                        if hr >= 0 && !web.is_null() {
                            drop(Com(web));
                        }
                        let tab = Tab {
                            id,
                            controller: Some(controller),
                            web: None,
                            url: String::new(),
                        };
                        drop(tab);
                    }
                }
                Event::Controller(_, Err(e)) => self.error = Some(e),
            }
        }
        self.layout(self.bounds, self.visible);
    }
    pub fn select(&mut self, id: u64) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            let tab = self.tabs.remove(i).unwrap();
            self.tabs.push_back(tab);
            self.active = Some(id);
            self.layout_dirty = true;
            self.layout(self.bounds, self.visible);
        }
    }
    pub fn layout(&mut self, rect: Rect, visible: bool) {
        if rect == self.bounds && visible == self.visible && !self.layout_dirty {
            return;
        }
        self.layout_dirty = false;
        self.bounds = rect;
        self.visible = visible;
        for tab in &self.tabs {
            if let Some(c) = &tab.controller {
                unsafe {
                    let bounds: unsafe extern "system" fn(*mut c_void, Rect) -> i32 =
                        method(c.0, 6);
                    bounds(c.0, rect);
                    let show: unsafe extern "system" fn(*mut c_void, i32) -> i32 = method(c.0, 4);
                    show(c.0, i32::from(visible && self.active == Some(tab.id)));
                }
            }
        }
    }
    pub fn navigate(&mut self, url: &str) -> Result<(), String> {
        if !valid_url(url) {
            return Err("Use an http/https address or about:blank".into());
        }
        let tab = self
            .tabs
            .iter_mut()
            .find(|t| Some(t.id) == self.active)
            .ok_or("Create a browser tab first")?;
        tab.url = url.into();
        if tab.web.is_some() {
            navigate(tab, url)?;
        }
        Ok(())
    }
    pub fn action(&self, index: usize) {
        if let Some(w) = self
            .tabs
            .iter()
            .find(|t| Some(t.id) == self.active)
            .and_then(|t| t.web.as_ref())
        {
            unsafe {
                let f: unsafe extern "system" fn(*mut c_void) -> i32 = method(w.0, index);
                f(w.0);
            }
        }
    }
    fn string(&self, index: usize) -> Option<String> {
        let web = self
            .tabs
            .iter()
            .find(|t| Some(t.id) == self.active)?
            .web
            .as_ref()?;
        let mut p = ptr::null_mut();
        unsafe {
            let f: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32 =
                method(web.0, index);
            if f(web.0, &mut p) < 0 || p.is_null() {
                return None;
            }
            let mut n = 0;
            while n < 65536 && *p.add(n) != 0 {
                n += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            CoTaskMemFree(p.cast());
            Some(text)
        }
    }
    pub fn source(&self) -> Option<String> {
        self.string(4)
    }
    pub fn title(&self) -> Option<String> {
        self.string(48)
    }
    pub fn identity(&self) -> Option<usize> {
        self.tabs
            .iter()
            .find(|t| Some(t.id) == self.active)?
            .controller
            .as_ref()
            .map(|c| c.0 as usize)
    }
    pub fn shutdown(&mut self) {
        self.tabs.clear();
        self.environment.take();
    }
}
fn valid_url(s: &str) -> bool {
    s == "about:blank"
        || ((s.starts_with("https://") || s.starts_with("http://"))
            && !s.chars().any(char::is_control)
            && s.len() < 16384)
}
fn navigate(tab: &Tab, url: &str) -> Result<(), String> {
    let web = tab.web.as_ref().ok_or("Browser is initializing")?;
    let hr = unsafe {
        let f: unsafe extern "system" fn(*mut c_void, *const u16) -> i32 = method(web.0, 5);
        f(web.0, wide(url).as_ptr())
    };
    if hr < 0 {
        Err(format!("Navigation failed ({hr:#x})"))
    } else {
        Ok(())
    }
}
