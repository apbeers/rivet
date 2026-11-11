//! Minimal Win32 ABI. Handles and all borrowed control pointers stay on the UI thread.
#![allow(dead_code, non_snake_case)]
use std::ffi::c_void;
pub type Handle = *mut c_void;
pub type Hwnd = Handle;
pub type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;
pub type SubclassProc = unsafe extern "system" fn(Hwnd, u32, usize, isize, usize, usize) -> isize;
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[repr(C)]
pub struct WndClass {
    pub style: u32,
    pub proc: Option<WndProc>,
    pub class_extra: i32,
    pub window_extra: i32,
    pub instance: Handle,
    pub icon: Handle,
    pub cursor: Handle,
    pub background: Handle,
    pub menu: *const u16,
    pub name: *const u16,
}
#[repr(C)]
#[derive(Default)]
pub struct Msg {
    pub window: Hwnd,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub point: Point,
    pub private: u32,
}
#[repr(C)]
pub struct CreateStruct {
    pub param: *mut c_void,
    pub instance: Handle,
    pub menu: Handle,
    pub parent: Hwnd,
    pub cy: i32,
    pub cx: i32,
    pub y: i32,
    pub x: i32,
    pub style: i32,
    pub name: *const u16,
    pub class: *const u16,
    pub ex_style: u32,
}
#[repr(C)]
pub struct NmHdr {
    pub from: Hwnd,
    pub id: usize,
    pub code: u32,
}
#[repr(C)]
#[derive(Default)]
pub struct CharRange {
    pub start: i32,
    pub end: i32,
}
#[repr(C)]
pub struct StartupInfo {
    pub cb: u32,
    pub reserved: *mut u16,
    pub desktop: *mut u16,
    pub title: *mut u16,
    pub x: u32,
    pub y: u32,
    pub xsize: u32,
    pub ysize: u32,
    pub xchars: u32,
    pub ychars: u32,
    pub fill: u32,
    pub flags: u32,
    pub show: u16,
    pub reserved_size: u16,
    pub reserved_bytes: *mut u8,
    pub input: Handle,
    pub output: Handle,
    pub error: Handle,
}
#[repr(C)]
pub struct StartupInfoEx {
    pub info: StartupInfo,
    pub attributes: *mut c_void,
}
#[repr(C)]
pub struct ProcessInfo {
    pub process: Handle,
    pub thread: Handle,
    pub process_id: u32,
    pub thread_id: u32,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Coord {
    pub x: i16,
    pub y: i16,
}
#[repr(C)]
pub struct BitmapInfoHeader {
    pub size: u32,
    pub width: i32,
    pub height: i32,
    pub planes: u16,
    pub bit_count: u16,
    pub compression: u32,
    pub image_size: u32,
    pub x_pixels: i32,
    pub y_pixels: i32,
    pub colors: u32,
    pub important: u32,
}
#[repr(C)]
pub struct BitmapInfo {
    pub header: BitmapInfoHeader,
    pub colors: [u32; 1],
}
#[link(name = "user32")]
unsafe extern "system" {
    pub fn RegisterClassW(class: *const WndClass) -> u16;
    pub fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: Hwnd,
        menu: Handle,
        instance: Handle,
        param: *mut c_void,
    ) -> Hwnd;
    pub fn DefWindowProcW(window: Hwnd, message: u32, w: usize, l: isize) -> isize;
    pub fn DestroyWindow(window: Hwnd) -> i32;
    pub fn ShowWindow(window: Hwnd, command: i32) -> i32;
    pub fn UpdateWindow(window: Hwnd) -> i32;
    pub fn GetMessageW(msg: *mut Msg, window: Hwnd, min: u32, max: u32) -> i32;
    pub fn TranslateMessage(msg: *const Msg) -> i32;
    pub fn DispatchMessageW(msg: *const Msg) -> isize;
    pub fn IsDialogMessageW(window: Hwnd, msg: *mut Msg) -> i32;
    pub fn PostQuitMessage(code: i32);
    pub fn SetWindowLongPtrW(window: Hwnd, index: i32, value: isize) -> isize;
    pub fn GetWindowLongPtrW(window: Hwnd, index: i32) -> isize;
    pub fn SendMessageW(window: Hwnd, message: u32, w: usize, l: isize) -> isize;
    pub fn PostMessageW(window: Hwnd, message: u32, w: usize, l: isize) -> i32;
    pub fn SetWindowTextW(window: Hwnd, text: *const u16) -> i32;
    pub fn GetWindowTextW(window: Hwnd, text: *mut u16, count: i32) -> i32;
    pub fn GetWindowTextLengthW(window: Hwnd) -> i32;
    pub fn EnableWindow(window: Hwnd, enable: i32) -> i32;
    pub fn MoveWindow(window: Hwnd, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
    pub fn GetClientRect(window: Hwnd, rect: *mut Rect) -> i32;
    pub fn GetWindowRect(window: Hwnd, rect: *mut Rect) -> i32;
    pub fn SetTimer(window: Hwnd, id: usize, milliseconds: u32, proc: *const c_void) -> usize;
    pub fn KillTimer(window: Hwnd, id: usize) -> i32;
    pub fn LoadCursorW(instance: Handle, name: *const u16) -> Handle;
    pub fn GetDpiForWindow(window: Hwnd) -> u32;
    pub fn SetProcessDpiAwarenessContext(context: Handle) -> i32;
    pub fn SetWindowPos(
        window: Hwnd,
        after: Hwnd,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        flags: u32,
    ) -> i32;
    pub fn SetFocus(window: Hwnd) -> Hwnd;
    pub fn GetKeyState(key: i32) -> i16;
    pub fn MessageBoxW(window: Hwnd, text: *const u16, title: *const u16, kind: u32) -> i32;
    pub fn OpenClipboard(window: Hwnd) -> i32;
    pub fn CloseClipboard() -> i32;
    pub fn GetClipboardData(format: u32) -> Handle;
    pub fn EmptyClipboard() -> i32;
    pub fn SetClipboardData(format: u32, data: Handle) -> Handle;
    pub fn GetDC(window: Hwnd) -> Handle;
    pub fn ReleaseDC(window: Hwnd, dc: Handle) -> i32;
    pub fn PrintWindow(window: Hwnd, dc: Handle, flags: u32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> Handle;
    pub fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
    pub fn GetProcAddress(module: Handle, name: *const u8) -> *const c_void;
    pub fn CloseHandle(handle: Handle) -> i32;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GlobalLock(handle: Handle) -> *mut c_void;
    pub fn GlobalUnlock(handle: Handle) -> i32;
    pub fn GlobalSize(handle: Handle) -> usize;
    pub fn GlobalAlloc(flags: u32, bytes: usize) -> Handle;
    pub fn GlobalFree(handle: Handle) -> Handle;
    pub fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        attributes: *const c_void,
        size: u32,
    ) -> i32;
    pub fn CreatePseudoConsole(
        size: Coord,
        input: Handle,
        output: Handle,
        flags: u32,
        console: *mut Handle,
    ) -> i32;
    pub fn ClosePseudoConsole(console: Handle);
    pub fn InitializeProcThreadAttributeList(
        list: *mut c_void,
        count: u32,
        flags: u32,
        size: *mut usize,
    ) -> i32;
    pub fn UpdateProcThreadAttribute(
        list: *mut c_void,
        flags: u32,
        attribute: usize,
        value: *mut c_void,
        size: usize,
        previous: *mut c_void,
        returned: *mut usize,
    ) -> i32;
    pub fn DeleteProcThreadAttributeList(list: *mut c_void);
    pub fn CreateProcessW(
        application: *const u16,
        command: *mut u16,
        process_attributes: *const c_void,
        thread_attributes: *const c_void,
        inherit: i32,
        flags: u32,
        environment: *const c_void,
        cwd: *const u16,
        startup: *mut StartupInfo,
        process: *mut ProcessInfo,
    ) -> i32;
    pub fn PeekNamedPipe(
        pipe: Handle,
        buffer: *mut c_void,
        size: u32,
        read: *mut u32,
        available: *mut u32,
        left: *mut u32,
    ) -> i32;
    pub fn ReadFile(
        file: Handle,
        buffer: *mut c_void,
        size: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn WriteFile(
        file: Handle,
        buffer: *const c_void,
        size: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    pub fn WaitForSingleObject(handle: Handle, milliseconds: u32) -> u32;
    pub fn TerminateProcess(process: Handle, code: u32) -> i32;
    pub fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
    pub fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    pub fn TerminateJobObject(job: Handle, code: u32) -> i32;
    pub fn GetLastError() -> u32;
    pub fn ResumeThread(thread: Handle) -> u32;
    pub fn CancelSynchronousIo(thread: Handle) -> i32;
    pub fn SetInformationJobObject(job: Handle, class: u32, info: *const c_void, size: u32) -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    pub fn GetStockObject(index: i32) -> Handle;
    pub fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strike: u32,
        charset: u32,
        out: u32,
        clip: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> Handle;
    pub fn CreateCompatibleDC(dc: Handle) -> Handle;
    pub fn CreateCompatibleBitmap(dc: Handle, width: i32, height: i32) -> Handle;
    pub fn SelectObject(dc: Handle, object: Handle) -> Handle;
    pub fn DeleteObject(object: Handle) -> i32;
    pub fn DeleteDC(dc: Handle) -> i32;
    pub fn GetDIBits(
        dc: Handle,
        bitmap: Handle,
        start: u32,
        lines: u32,
        bits: *mut c_void,
        info: *mut BitmapInfo,
        usage: u32,
    ) -> i32;
}
#[link(name = "comctl32")]
unsafe extern "system" {
    pub fn SetWindowSubclass(
        window: Hwnd,
        callback: Option<SubclassProc>,
        id: usize,
        data: usize,
    ) -> i32;
    pub fn DefSubclassProc(window: Hwnd, message: u32, w: usize, l: isize) -> isize;
    pub fn RemoveWindowSubclass(window: Hwnd, callback: Option<SubclassProc>, id: usize) -> i32;
}
#[link(name = "ole32")]
unsafe extern "system" {
    pub fn CoInitializeEx(reserved: *mut c_void, flags: u32) -> i32;
    pub fn CoUninitialize();
    pub fn CoTaskMemFree(memory: *mut c_void);
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn send(h: Hwnd, m: u32, w: usize, l: isize) -> isize {
    unsafe { SendMessageW(h, m, w, l) }
}
pub fn text(h: Hwnd) -> String {
    let len = unsafe { GetWindowTextLengthW(h) }.clamp(0, 16 * 1024 * 1024);
    let mut v = vec![0; len as usize + 1];
    let n = unsafe { GetWindowTextW(h, v.as_mut_ptr(), len + 1) };
    String::from_utf16_lossy(&v[..n.max(0) as usize])
}
pub fn set_text(h: Hwnd, s: &str) {
    unsafe {
        SetWindowTextW(h, wide(s).as_ptr());
    }
}
pub fn show(h: Hwnd, on: bool) {
    unsafe {
        ShowWindow(h, if on { 5 } else { 0 });
    }
}
pub fn enable(h: Hwnd, on: bool) {
    unsafe {
        EnableWindow(h, i32::from(on));
    }
}
pub fn place(h: Hwnd, x: i32, y: i32, w: i32, ht: i32) {
    unsafe {
        MoveWindow(h, x, y, w.max(1), ht.max(1), 1);
    }
}
pub fn selection(h: Hwnd) -> (usize, usize) {
    let mut r = CharRange::default();
    send(h, 0x434, 0, &mut r as *mut _ as isize);
    (r.start.max(0) as usize, r.end.max(0) as usize)
}
pub fn set_selection(h: Hwnd, a: usize, b: usize) {
    let r = CharRange {
        start: a as i32,
        end: b as i32,
    };
    send(h, 0x437, 0, &r as *const _ as isize);
}
pub fn clipboard() -> Option<String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let h = GetClipboardData(13);
        let p = GlobalLock(h) as *const u16;
        let size = GlobalSize(h).min(1024 * 1024) / 2;
        let value = if p.is_null() {
            None
        } else {
            let v = std::slice::from_raw_parts(p, size);
            Some(String::from_utf16_lossy(
                &v[..v.iter().position(|&n| n == 0).unwrap_or(size)],
            ))
        };
        if !p.is_null() {
            GlobalUnlock(h);
        }
        CloseClipboard();
        value
    }
}
pub fn copy(s: &str) {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }
        let data = wide(s);
        let h = GlobalAlloc(2, data.len() * 2);
        let p = GlobalLock(h);
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(data.as_ptr(), p.cast(), data.len());
            GlobalUnlock(h);
            EmptyClipboard();
            if SetClipboardData(13, h).is_null() {
                GlobalFree(h);
            }
        } else if !h.is_null() {
            GlobalFree(h);
        }
        CloseClipboard();
    }
}
