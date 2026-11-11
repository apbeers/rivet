//! Win32 controls on the UI thread; owned events bridge workers and native callbacks.
use super::{browser::Browser, sys::*, terminal::Terminal};
use rivet_core::{
    AgentBackend, AgentCommand, AgentEvent, LiveWorkspace,
    documents::{FileCommand, FileEvent, FileWorker},
    editor::Buffer,
    json::{self, Value},
    lsp,
    process::{self, Spec, Worker},
    task_service::{self, Host},
    terminal::Screen,
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    ptr,
    sync::{Arc, mpsc::Receiver},
    time::{Duration, Instant},
};
const ALL: u32 = 63;
const AGENTS: u32 = 1;
const BROWSER: u32 = 2;
const DOCUMENTS: u32 = 4;
const REVIEW: u32 = 8;
const TOOLS: u32 = 16;
const TASKS: u32 = 32;
#[derive(Clone, Copy)]
struct Control {
    window: Hwnd,
    modes: u32,
}
struct Document {
    path: PathBuf,
    view: Hwnd,
    base: Arc<str>,
    buffer: Buffer,
    revision: u64,
    saved: u64,
    highlight: bool,
    read_only: bool,
}
impl Document {
    fn dirty(&self) -> bool {
        self.revision != self.saved
    }
}
struct Term {
    terminal: Terminal,
    screen: Screen,
}
enum Event {
    Action(u16, u16),
    Layout,
    Close,
    Tick,
    EditFallback(Hwnd),
    Fatal,
}
struct State {
    workspace: LiveWorkspace,
    chat: BTreeMap<u64, Hwnd>,
    docs: VecDeque<Document>,
    active_doc: Option<PathBuf>,
    entries: Vec<(PathBuf, bool)>,
    directory: PathBuf,
    pending_saves: BTreeSet<PathBuf>,
    discard: Option<(PathBuf, u64)>,
    permissions: VecDeque<(u64, u64, String, Instant)>,
    task_permissions: VecDeque<(u64, String, Instant)>,
    task_report: String,
    task_log: String,
    jobs: Vec<u64>,
    next_job: u64,
    confirmation: Option<(Spec, bool)>,
    integration: Option<(u64, String)>,
    terminals: Vec<Term>,
    retired: Vec<Terminal>,
    active_terminal: usize,
    terminal_visible: bool,
    lsp: Option<lsp::Connection>,
    lsp_start: Option<Receiver<Result<lsp::Connection, String>>>,
    lsp_ready: bool,
    lsp_open: Option<(PathBuf, u64)>,
    requests: BTreeMap<u32, (u16, PathBuf, u64)>,
    next_request: u32,
    legend: Vec<String>,
    review: Option<rivet_core::diff::Diff>,
    inline: bool,
}
impl Default for State {
    fn default() -> Self {
        Self {
            workspace: LiveWorkspace::default(),
            chat: BTreeMap::new(),
            docs: VecDeque::new(),
            active_doc: None,
            entries: Vec::new(),
            directory: PathBuf::new(),
            pending_saves: BTreeSet::new(),
            discard: None,
            permissions: VecDeque::new(),
            task_permissions: VecDeque::new(),
            task_report: String::new(),
            task_log: String::new(),
            jobs: Vec::new(),
            next_job: 1,
            confirmation: None,
            integration: None,
            terminals: Vec::new(),
            retired: Vec::new(),
            active_terminal: 0,
            terminal_visible: false,
            lsp: None,
            lsp_start: None,
            lsp_ready: false,
            lsp_open: None,
            requests: BTreeMap::new(),
            next_request: 2,
            legend: Vec::new(),
            review: None,
            inline: false,
        }
    }
}
struct App {
    window: Cell<Hwnd>,
    controls: OnceCell<BTreeMap<u16, Control>>,
    state: RefCell<State>,
    backend: RefCell<Box<dyn AgentBackend>>,
    tasks: RefCell<Option<Host>>,
    files: FileWorker,
    processes: Worker,
    browser: RefCell<Browser>,
    queue: RefCell<VecDeque<Event>>,
    suppress: Cell<u32>,
    mode: Cell<u8>,
    closing: Cell<bool>,
    backend_stopped: Cell<bool>,
    fatal: Cell<bool>,
    high: Cell<Option<u16>>,
    terminal_high: Cell<Option<u16>>,
    font: Cell<Handle>,
    mono: Cell<Handle>,
    switching: RefCell<rivet_prototype::Samples>,
    polling: RefCell<rivet_prototype::Samples>,
    browser_switch: RefCell<rivet_prototype::Samples>,
    smoke: RefCell<Option<Smoke>>,
}
impl App {
    fn new(
        backend: Box<dyn AgentBackend>,
        tasks: Option<Host>,
        smoke: bool,
    ) -> Result<Box<Self>, String> {
        Ok(Box::new(Self {
            window: Cell::new(ptr::null_mut()),
            controls: OnceCell::new(),
            state: RefCell::new(State::default()),
            backend: RefCell::new(backend),
            tasks: RefCell::new(tasks),
            files: FileWorker::spawn_with_save_and_normalization(super::save::checked, true)
                .map_err(|e| e.to_string())?,
            processes: Worker::spawn().map_err(|e| e.to_string())?,
            browser: RefCell::new(Browser::new(ptr::null_mut(), None)),
            queue: RefCell::new(VecDeque::new()),
            suppress: Cell::new(0),
            mode: Cell::new(0),
            closing: Cell::new(false),
            backend_stopped: Cell::new(false),
            fatal: Cell::new(false),
            high: Cell::new(None),
            terminal_high: Cell::new(None),
            font: Cell::new(ptr::null_mut()),
            mono: Cell::new(ptr::null_mut()),
            switching: RefCell::new(Default::default()),
            polling: RefCell::new(Default::default()),
            browser_switch: RefCell::new(Default::default()),
            smoke: RefCell::new(if smoke { Some(Smoke::new()?) } else { None }),
        }))
    }
    fn h(&self, id: u16) -> Hwnd {
        self.controls
            .get()
            .and_then(|c| c.get(&id))
            .map_or(ptr::null_mut(), |c| c.window)
    }
    fn enqueue(&self, e: Event) {
        if let Ok(mut q) = self.queue.try_borrow_mut() {
            if matches!(e, Event::Tick | Event::Layout)
                && q.iter()
                    .any(|v| std::mem::discriminant(v) == std::mem::discriminant(&e))
            {
                return;
            }
            if q.len() < 512 {
                q.push_back(e);
            } else {
                self.fatal.set(true);
            }
        } else {
            self.fatal.set(true);
        }
    }
    fn guarded<T>(&self, f: impl FnOnce() -> T) -> T {
        struct Reset<'a>(&'a Cell<u32>);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() - 1);
            }
        }
        self.suppress.set(self.suppress.get() + 1);
        let _reset = Reset(&self.suppress);
        f()
    }
    fn put(&self, id: u16, s: &str) {
        self.guarded(|| set_text(self.h(id), s));
    }
    fn status(&self, s: &str) {
        self.put(900, s);
    }
    fn root(&self) -> PathBuf {
        PathBuf::from(text(self.h(110)))
    }
    fn initialize(&self) -> Result<(), String> {
        let mut controls = BTreeMap::new();
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        for (id, class, title, style, modes) in [
            (101, "LISTBOX", "", 0x1 | 0x100 | 0x200000, ALL),
            (102, "BUTTON", "New session", 0, ALL),
            (103, "COMBOBOX", "", 3 | 0x200000, ALL),
            (104, "BUTTON", "Resume saved", 0, ALL),
            (110, "EDIT", "", 0x80, ALL),
            (111, "BUTTON", "Connect", 0, AGENTS),
            (112, "COMBOBOX", "", 3 | 0x200000, AGENTS),
            (113, "EDIT", "", 0x80, AGENTS),
            (115, "EDIT", "", 4 | 0x40 | 0x1000 | 0x200000, AGENTS),
            (116, "BUTTON", "Send (Ctrl+Enter)", 0, AGENTS),
            (117, "BUTTON", "Cancel turn", 0, AGENTS),
            (118, "RICHEDIT50W", "", 4 | 0x40 | 0x800 | 0x200000, AGENTS),
            (119, "BUTTON", "Allow once", 0, AGENTS),
            (120, "BUTTON", "Reject", 0, AGENTS),
            (200, "BUTTON", "Agents", 0, ALL),
            (201, "BUTTON", "Browser", 0, ALL),
            (202, "BUTTON", "Documents", 0, ALL),
            (203, "BUTTON", "Review", 0, ALL),
            (204, "BUTTON", "Tools", 0, ALL),
            (205, "BUTTON", "Tasks", 0, ALL),
            (300, "BUTTON", "New tab", 0, BROWSER),
            (301, "COMBOBOX", "", 3 | 0x200000, BROWSER),
            (302, "EDIT", "about:blank", 0x80, BROWSER),
            (303, "BUTTON", "Go", 0, BROWSER),
            (304, "BUTTON", "Back", 0, BROWSER),
            (305, "BUTTON", "Forward", 0, BROWSER),
            (306, "BUTTON", "Reload", 0, BROWSER),
            (400, "EDIT", "", 0x80, DOCUMENTS),
            (401, "BUTTON", "Open path", 0, DOCUMENTS),
            (402, "COMBOBOX", "", 3 | 0x200000, DOCUMENTS),
            (403, "BUTTON", "List workspace", 0, DOCUMENTS),
            (404, "BUTTON", "Up", 0, DOCUMENTS),
            (405, "COMBOBOX", "", 3 | 0x200000, DOCUMENTS),
            (406, "BUTTON", "Save", 0, DOCUMENTS | REVIEW),
            (407, "BUTTON", "Undo", 0, DOCUMENTS | REVIEW),
            (408, "BUTTON", "Redo", 0, DOCUMENTS | REVIEW),
            (409, "EDIT", "", 0x80, DOCUMENTS | REVIEW),
            (410, "BUTTON", "Find", 0, DOCUMENTS | REVIEW),
            (411, "BUTTON", "Unsaved diff", 0, DOCUMENTS | REVIEW),
            (412, "BUTTON", "Reject / Confirm", 0, DOCUMENTS | REVIEW),
            (413, "BUTTON", "Inline / Split", 0, REVIEW),
            (500, "EDIT", "cargo", 0x80, TOOLS),
            (501, "EDIT", "[\"test\"]", 0x80, TOOLS),
            (502, "BUTTON", "Run / Confirm", 0, TOOLS),
            (503, "BUTTON", "Cancel jobs", 0, TOOLS),
            (504, "BUTTON", "Git status", 0, TOOLS),
            (505, "BUTTON", "Git diff HEAD", 0, TOOLS),
            (506, "BUTTON", "Start LSP / Confirm", 0, TOOLS),
            (507, "BUTTON", "Hover", 0, TOOLS),
            (508, "BUTTON", "Definition", 0, TOOLS),
            (509, "BUTTON", "References", 0, TOOLS),
            (510, "BUTTON", "Completion", 0, TOOLS),
            (511, "BUTTON", "Semantic tokens", 0, TOOLS),
            (520, "BUTTON", "New terminal", 0, TOOLS),
            (521, "BUTTON", "Close terminal", 0, TOOLS),
            (522, "COMBOBOX", "", 3 | 0x200000, TOOLS),
            (523, "EDIT", "", 0x80, TOOLS),
            (524, "BUTTON", "Send line", 0, TOOLS),
            (525, "BUTTON", "Ctrl-C", 0, TOOLS),
            (600, "EDIT", "", 0x80, TASKS),
            (601, "EDIT", "[]", 0x80, TASKS),
            (602, "EDIT", "[\"cargo\",\"test\"]", 0x80, TASKS),
            (603, "BUTTON", "Add task", 0, TASKS),
            (604, "BUTTON", "Run ready tasks", 0, TASKS),
            (605, "BUTTON", "Cancel running", 0, TASKS),
            (606, "EDIT", "", 0x80, TASKS),
            (607, "BUTTON", "Retry ID", 0, TASKS),
            (608, "EDIT", "", 0x80, TASKS),
            (609, "BUTTON", "Integrate / Confirm", 0, TASKS),
            (610, "BUTTON", "Allow task tool", 0, TASKS),
            (611, "BUTTON", "Reject task tool", 0, TASKS),
            (700, "RICHEDIT50W", "", 4 | 0x40 | 0x800 | 0x200000, REVIEW),
            (701, "RICHEDIT50W", "", 4 | 0x40 | 0x800 | 0x200000, REVIEW),
            (702, "RICHEDIT50W", "", 4 | 0x40 | 0x800 | 0x200000, TOOLS),
            (703, "RICHEDIT50W", "", 4 | 0x40 | 0x800 | 0x200000, TASKS),
            (900, "STATIC", "Ready", 0, ALL),
        ] {
            let h = unsafe {
                CreateWindowExW(
                    if class == "EDIT" || class == "RICHEDIT50W" {
                        0x200
                    } else {
                        0
                    },
                    wide(class).as_ptr(),
                    wide(title).as_ptr(),
                    0x40000000 | 0x10000000 | if class == "STATIC" { 0 } else { 0x10000 } | style,
                    0,
                    0,
                    20,
                    20,
                    self.window.get(),
                    id as usize as Handle,
                    instance,
                    ptr::null_mut(),
                )
            };
            if h.is_null() {
                return Err(format!(
                    "Native control {id}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if class == "RICHEDIT50W" {
                send(h, 0x435, 0, 2 * 1024 * 1024);
            }
            controls.insert(id, Control { window: h, modes });
        }
        self.controls
            .set(controls)
            .map_err(|_| "Controls already initialized")?;
        for (id, cue) in [
            (110, "Workspace directory"),
            (113, "Instructions (set before connecting)"),
            (115, "Describe a development objective"),
            (400, "Workspace file path"),
            (409, "Find in file"),
            (523, "Terminal line (focus console for direct keys)"),
            (600, "Task objective and completion criteria"),
            (601, "Dependency task IDs, e.g. [1,2]"),
            (602, "Validation executable and arguments as JSON"),
            (606, "Task ID"),
            (608, "Exact reviewed worktree HEAD SHA"),
        ] {
            send(self.h(id), 0x1501, 0, wide(cue).as_ptr() as isize);
        }
        self.combo(112, &["SDK default".into()], 0);
        self.put(
            110,
            &std::env::current_dir()
                .map_err(|e| e.to_string())?
                .display()
                .to_string(),
        );
        *self.browser.borrow_mut() = Browser::new(
            self.window.get(),
            self.smoke
                .borrow()
                .as_ref()
                .map(|s| s.root.join("browser-profile")),
        );
        self.fonts();
        self.add_session()?;
        let ok = unsafe {
            SetWindowSubclass(
                self.h(702),
                Some(console_proc),
                1,
                self as *const _ as usize,
            )
        };
        if ok == 0 {
            return Err("Cannot initialize terminal input surface".into());
        }
        if unsafe { SetTimer(self.window.get(), 1, 16, ptr::null()) } == 0 {
            return Err("Cannot initialize UI timer".into());
        }
        self.layout();
        self.permissions();
        Ok(())
    }
    fn fonts(&self) {
        let dpi = unsafe { GetDpiForWindow(self.window.get()) }.max(96);
        let font = unsafe {
            CreateFontW(
                -(9 * dpi as i32 / 72),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                wide("Segoe UI").as_ptr(),
            )
        };
        let mono = unsafe {
            CreateFontW(
                -(10 * dpi as i32 / 72),
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                wide("Consolas").as_ptr(),
            )
        };
        if font.is_null() || mono.is_null() {
            return;
        }
        if let Some(c) = self.controls.get() {
            for (&id, c) in c {
                send(
                    c.window,
                    0x30,
                    if matches!(id, 700..=702) { mono } else { font } as usize,
                    1,
                );
            }
        }
        for d in &self.state.borrow().docs {
            send(d.view, 0x30, mono as usize, 1);
        }
        for &h in self.state.borrow().chat.values() {
            send(h, 0x30, font as usize, 1);
        }
        let (old, old_mono) = (self.font.replace(font), self.mono.replace(mono));
        unsafe {
            if !old.is_null() {
                DeleteObject(old);
            }
            if !old_mono.is_null() {
                DeleteObject(old_mono);
            }
        }
    }
    fn combo(&self, id: u16, items: &[String], selected: usize) {
        self.guarded(|| {
            let h = self.h(id);
            send(h, 0x14b, 0, 0);
            for s in items {
                send(h, 0x143, 0, wide(s).as_ptr() as isize);
            }
            if !items.is_empty() {
                send(h, 0x14e, selected.min(items.len() - 1), 0);
            }
        });
    }
    fn choice(&self, id: u16) -> usize {
        send(self.h(id), 0x147, 0, 0).max(0) as usize
    }
    fn add_session(&self) -> Result<(), String> {
        if self.state.borrow().workspace.sessions.len() >= 20 {
            return Err("Session cache full (20)".into());
        }
        self.save_draft();
        let key = self.state.borrow_mut().workspace.add();
        let h = self.rich(AGENTS)?;
        self.state.borrow_mut().chat.insert(key, h);
        self.guarded(|| {
            send(
                self.h(101),
                0x180,
                0,
                wide(&format!("Session {key}")).as_ptr() as isize,
            );
            send(self.h(101), 0x186, key as usize - 1, 0);
        });
        self.select_session(key);
        self.layout();
        Ok(())
    }
    fn rich(&self, _mode: u32) -> Result<Hwnd, String> {
        let h = unsafe {
            CreateWindowExW(
                0x200,
                wide("RICHEDIT50W").as_ptr(),
                wide("").as_ptr(),
                0x40000000 | 0x10000 | 0x200000 | 4 | 0x40 | 0x800,
                0,
                0,
                100,
                100,
                self.window.get(),
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null_mut(),
            )
        };
        if h.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        send(h, 0x435, 0, 16 * 1024 * 1024);
        send(h, 0x30, self.font.get() as usize, 1);
        Ok(h)
    }
    fn save_draft(&self) {
        let draft = text(self.h(115));
        let instructions = text(self.h(113));
        let index = self.choice(112);
        let mut s = self.state.borrow_mut();
        let key = s.workspace.active;
        if let Some(session) = s.workspace.sessions.get_mut(&key) {
            session.draft = draft;
            if !session.connecting && session.sdk_id.is_none() {
                session.config.instructions = instructions;
                session.config.model = if index > 0 {
                    safely_model(self.h(112), index)
                } else {
                    String::new()
                };
            }
        }
    }
    fn select_session(&self, key: u64) {
        let start = Instant::now();
        self.state.borrow_mut().workspace.active = key;
        let (draft, instructions, path, editable, model) = {
            let s = self.state.borrow();
            let session = &s.workspace.sessions[&key];
            (
                session.draft.clone(),
                session.config.instructions.clone(),
                session.workspace.clone(),
                !session.connecting && session.sdk_id.is_none(),
                session.config.model.clone(),
            )
        };
        self.put(115, &draft);
        self.put(113, &instructions);
        if let Some(path) = path {
            self.put(110, &path.to_string_lossy());
        }
        self.guarded(|| {
            send(self.h(110), 0xcf, usize::from(!editable), 0);
            send(self.h(113), 0xcf, usize::from(!editable), 0);
        });
        enable(self.h(111), editable);
        enable(self.h(112), editable);
        let model_index = self
            .state
            .borrow()
            .workspace
            .models
            .iter()
            .position(|m| m == &model)
            .map_or(0, |i| i + 1);
        send(self.h(112), 0x14e, model_index, 0);
        for (&id, &view) in &self.state.borrow().chat {
            show(view, self.mode.get() == 0 && id == key);
        }
        self.switching.borrow_mut().record(start.elapsed());
    }
    fn layout(&self) {
        if self.controls.get().is_none() {
            return;
        }
        let mut rect = Rect::default();
        unsafe {
            GetClientRect(self.window.get(), &mut rect);
        }
        let scale = unsafe { GetDpiForWindow(self.window.get()) }.max(96) as f64 / 96.;
        let w = (rect.right as f64 / scale) as i32;
        let h = (rect.bottom as f64 / scale) as i32;
        let put = |id, x, y, width, height| {
            place(
                self.h(id),
                (x as f64 * scale) as i32,
                (y as f64 * scale) as i32,
                (width as f64 * scale) as i32,
                (height as f64 * scale) as i32,
            )
        };
        put(102, 12, 10, 180, 28);
        put(101, 12, 44, 180, 160);
        put(103, 12, 214, 180, 180);
        put(104, 12, 246, 180, 28);
        for i in 0..6 {
            put(200 + i, 12, 292 + i as i32 * 38, 180, 30);
        }
        put(110, 212, 10, w - 340, 28);
        put(111, w - 120, 10, 108, 28);
        put(900, 12, h - 28, w - 24, 22);
        put(112, 212, 48, 220, 220);
        put(113, 440, 48, w - 452, 28);
        put(118, 212, h - 240, w - 340, 100);
        put(119, w - 120, h - 240, 108, 28);
        put(120, w - 120, h - 204, 108, 28);
        put(115, 212, h - 126, w - 340, 84);
        put(116, w - 120, h - 126, 108, 34);
        put(117, w - 120, h - 84, 108, 34);
        for (id, x, y, width) in [
            (300, 212, 50, 88),
            (301, 305, 50, 110),
            (304, 420, 50, 52),
            (305, 477, 50, 72),
            (306, 554, 50, 70),
        ] {
            put(id, x, y, width, if id == 301 { 240 } else { 28 });
        }
        put(302, 632, 50, w - 710, 28);
        put(303, w - 72, 50, 60, 28);
        put(402, 212, 50, 260, 240);
        put(403, 480, 50, 125, 28);
        put(404, 612, 50, 50, 28);
        put(405, 670, 50, w - 682, 240);
        put(400, 212, 86, w - 340, 28);
        put(401, w - 120, 86, 108, 28);
        for (id, x, width) in [
            (406, 212, 60),
            (407, 278, 60),
            (408, 344, 60),
            (411, 410, 110),
            (412, 526, 125),
            (413, 657, 105),
        ] {
            put(id, x, 122, width, 28);
        }
        put(409, 212, 158, w - 340, 28);
        put(410, w - 120, 158, 108, 28);
        put(500, 212, 50, 180, 28);
        put(501, 400, 50, w - 665, 28);
        put(502, w - 257, 50, 125, 28);
        put(503, w - 125, 50, 113, 28);
        put(504, 212, 86, 110, 28);
        put(505, 330, 86, 125, 28);
        put(506, 463, 86, 180, 28);
        for (i, id) in (507..=511).enumerate() {
            put(id, 212 + i as i32 * 127, 122, 120, 28);
        }
        put(520, 212, 158, 125, 28);
        put(521, 345, 158, 135, 28);
        put(522, 488, 158, 150, 240);
        put(523, 212, h - 68, w - 445, 28);
        put(524, w - 225, h - 68, 105, 28);
        put(525, w - 112, h - 68, 100, 28);
        put(600, 212, 50, w - 224, 28);
        put(602, 212, 86, w - 224, 28);
        put(601, 212, 122, w - 355, 28);
        put(603, w - 135, 122, 123, 28);
        put(604, 212, 158, 145, 28);
        put(605, 365, 158, 140, 28);
        put(606, 513, 158, 90, 28);
        put(607, 611, 158, 110, 28);
        put(608, 212, 194, w - 405, 28);
        put(609, w - 185, 194, 173, 28);
        put(610, 212, 230, 150, 28);
        put(611, 370, 230, 150, 28);
        let mode = self.mode.get();
        for c in self.controls.get().unwrap().values() {
            show(c.window, c.modes & (1 << mode) != 0);
        }
        let state = self.state.borrow();
        for (&key, &view) in &state.chat {
            place(
                view,
                (212. * scale) as i32,
                (86. * scale) as i32,
                ((w - 224) as f64 * scale) as i32,
                ((h - 340) as f64 * scale) as i32,
            );
            show(view, mode == 0 && key == state.workspace.active);
        }
        for d in &state.docs {
            place(
                d.view,
                (212. * scale) as i32,
                (196. * scale) as i32,
                ((w - 224) as f64 * scale) as i32,
                ((h - 235) as f64 * scale) as i32,
            );
            show(
                d.view,
                mode == 2 && state.active_doc.as_ref() == Some(&d.path),
            );
        }
        let left = if state.inline { w - 224 } else { (w - 232) / 2 };
        put(700, 212, 196, left, h - 235);
        put(701, 220 + left, 196, w - 232 - left, h - 235);
        show(self.h(701), mode == 3 && !state.inline);
        put(702, 212, 196, w - 224, h - 274);
        put(703, 212, 268, w - 224, h - 307);
        drop(state);
        self.browser.borrow_mut().layout(
            Rect {
                left: (212. * scale) as i32,
                top: (90. * scale) as i32,
                right: rect.right - (12. * scale) as i32,
                bottom: rect.bottom - (40. * scale) as i32,
            },
            mode == 1,
        );
    }
    fn submit(&self, cmd: AgentCommand) -> Result<(), String> {
        self.backend.borrow().submit(cmd)
    }
    fn connect(&self, resume: bool) -> Result<(), String> {
        self.save_draft();
        let (key, config, connected) = {
            let s = self.state.borrow();
            let session = &s.workspace.sessions[&s.workspace.active];
            (
                s.workspace.active,
                session.config.clone(),
                session.connecting || session.sdk_id.is_some(),
            )
        };
        if connected {
            return Err("Create a new session to connect another workspace".into());
        }
        let path = self.root();
        let resume = if resume {
            self.state
                .borrow()
                .workspace
                .saved
                .get(self.choice(103))
                .cloned()
                .ok_or("Choose a saved session")?
                .into()
        } else {
            None
        };
        self.submit(AgentCommand::Open {
            key,
            workspace: path.clone(),
            resume,
            config,
        })?;
        {
            let mut s = self.state.borrow_mut();
            let session = s.workspace.sessions.get_mut(&key).unwrap();
            session.workspace = Some(path);
            session.connecting = true;
        }
        self.select_session(key);
        let _ = self.submit(AgentCommand::Models);
        self.status("Connecting in background");
        Ok(())
    }
    fn send_prompt(&self) -> Result<(), String> {
        self.save_draft();
        let (key, prompt, old, epoch) = {
            let s = self.state.borrow();
            let session = &s.workspace.sessions[&s.workspace.active];
            if session.sdk_id.is_none() {
                return Err("Connect the session first".into());
            }
            if s.workspace.sessions.values().any(|s| s.busy) {
                return Err(
                    "Another interactive turn is running; use Tasks for isolated parallel work"
                        .into(),
                );
            }
            (
                s.workspace.active,
                session.draft.clone(),
                session.transcript.len(),
                session.transcript_epoch,
            )
        };
        if prompt.trim().is_empty() {
            return Ok(());
        }
        self.submit(AgentCommand::Send {
            key,
            prompt: prompt.clone(),
        })?;
        {
            let mut s = self.state.borrow_mut();
            let session = s.workspace.sessions.get_mut(&key).unwrap();
            session.append(&format!("\nYou: {prompt}\n\n"));
            session.draft.clear();
            session.busy = true;
        }
        self.update_chat(key, old, epoch);
        self.put(115, "");
        Ok(())
    }
    fn update_chat(&self, key: u64, old: usize, epoch: u64) {
        let (view, append, reset) = {
            let s = self.state.borrow();
            let Some(session) = s.workspace.sessions.get(&key) else {
                return;
            };
            let Some(&view) = s.chat.get(&key) else {
                return;
            };
            let reset = epoch != session.transcript_epoch || old > session.transcript.len();
            (
                view,
                if reset {
                    session.transcript.clone()
                } else {
                    session.transcript[old..].to_string()
                },
                reset,
            )
        };
        if append.is_empty() {
            return;
        }
        let selected = selection(view);
        let line = send(view, 0xce, 0, 0);
        self.guarded(|| {
            send(view, 0xcf, 0, 0);
            if reset {
                set_text(view, &append);
            } else {
                set_selection(view, usize::MAX as u32 as usize, usize::MAX as u32 as usize);
                send(view, 0xc2, 0, wide(&append).as_ptr() as isize);
            }
            send(view, 0xcf, 1, 0);
            set_selection(view, selected.0, selected.1);
            let now = send(view, 0xce, 0, 0);
            send(view, 0xb6, 0, line - now);
        });
    }
    fn permissions(&self) {
        self.put(
            200,
            if self.state.borrow().permissions.is_empty() {
                "Agents"
            } else {
                "Agents (approval)"
            },
        );
        let pending = self.state.borrow().permissions.front().cloned();
        if let Some((key, _, description, _)) = pending {
            let root = self
                .state
                .borrow()
                .workspace
                .sessions
                .get(&key)
                .and_then(|s| s.workspace.as_ref())
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            self.put(118,&format!("Session {key}\nWorkspace: {root}\n{description}\nAllow once or reject. Permission hooks are not an OS sandbox."));
            enable(self.h(119), true);
            enable(self.h(120), true);
        } else {
            self.put(
                118,
                "No pending tool permissions. Requests expire denied after 120 seconds.",
            );
            enable(self.h(119), false);
            enable(self.h(120), false);
        }
    }
    fn file(&self, c: FileCommand) -> Result<(), String> {
        self.files.submit(c)
    }
    fn action(&self, id: u16, notification: u16) -> Result<(), String> {
        if self.closing.get() {
            return Ok(());
        }
        match id {
            101 if notification == 1 => {
                self.save_draft();
                let selected = send(self.h(101), 0x188, 0, 0);
                if selected < 0 {
                    return Ok(());
                }
                let key = selected as u64 + 1;
                if self.state.borrow().workspace.sessions.contains_key(&key) {
                    self.select_session(key);
                }
            }
            102 => self.add_session()?,
            104 => self.connect(true)?,
            111 => self.connect(false)?,
            116 => self.send_prompt()?,
            117 => self.submit(AgentCommand::Cancel(self.state.borrow().workspace.active))?,
            119 | 120 => {
                let request = self
                    .state
                    .borrow()
                    .permissions
                    .front()
                    .map(|p| p.1)
                    .ok_or("No pending permission")?;
                self.submit(AgentCommand::Decide {
                    request,
                    allow: id == 119,
                })?;
                self.state.borrow_mut().permissions.pop_front();
                self.permissions();
            }
            200..=205 => {
                self.mode.set((id - 200) as u8);
                self.layout();
            }
            300 => {
                let result = self.browser.borrow_mut().create();
                match result {
                    Ok(_) => {
                        self.refresh_browser();
                        self.mode.set(1);
                        self.layout();
                    }
                    Err(e) => self.status(&e),
                }
            }
            301 if notification == 1 => {
                let start = Instant::now();
                let id = self.browser.borrow().ids().get(self.choice(301)).copied();
                if let Some(id) = id {
                    self.browser.borrow_mut().select(id);
                    if let Some(url) = self.browser.borrow().source() {
                        self.put(302, &url);
                    }
                    self.browser_switch.borrow_mut().record(start.elapsed());
                }
            }
            303 => self.browser.borrow_mut().navigate(&text(self.h(302)))?,
            304 => self.browser.borrow().action(40),
            305 => self.browser.borrow().action(41),
            306 => self.browser.borrow().action(31),
            401 => self.file(FileCommand::Read {
                root: self.root(),
                path: PathBuf::from(text(self.h(400))),
            })?,
            403 => {
                let root = self.root();
                self.file(FileCommand::List {
                    root: root.clone(),
                    path: root,
                })?;
            }
            404 => {
                let path = self.state.borrow().directory.parent().map(|p| p.to_owned());
                if let Some(path) = path {
                    self.file(FileCommand::List {
                        root: self.root(),
                        path,
                    })?;
                }
            }
            402 if notification == 1 => {
                let entry = self.state.borrow().entries.get(self.choice(402)).cloned();
                if let Some((path, folder)) = entry {
                    self.file(if folder {
                        FileCommand::List {
                            root: self.root(),
                            path,
                        }
                    } else {
                        FileCommand::Read {
                            root: self.root(),
                            path,
                        }
                    })?;
                }
            }
            405 if notification == 1 => {
                let path = self
                    .state
                    .borrow()
                    .docs
                    .get(self.choice(405))
                    .map(|d| d.path.clone());
                if let Some(path) = path {
                    self.state.borrow_mut().active_doc = Some(path);
                    self.layout();
                }
            }
            406 => self.save()?,
            407 | 408 => self.undo(id == 407),
            409 => {}
            410 => self.find(),
            411 => {
                let s = self.state.borrow();
                if let Some(d) = active_doc(&s) {
                    self.file(FileCommand::Diff {
                        before: d.base.clone(),
                        snapshot: d.buffer.snapshot(),
                    })?;
                }
            }
            412 => self.discard()?,
            413 => {
                let mut s = self.state.borrow_mut();
                s.inline = !s.inline;
                drop(s);
                self.review();
            }
            502 => {
                let spec = self.spec()?;
                if self.confirm(&spec, false) {
                    self.execute(spec)?;
                }
            }
            503 => {
                for id in self.state.borrow().jobs.clone() {
                    self.processes.cancel(id);
                }
            }
            504 => self.execute(process::git(
                self.root(),
                vec!["status".into(), "--short".into(), "--branch".into()],
            ))?,
            505 => self.execute(process::git(
                self.root(),
                vec![
                    "diff".into(),
                    "--no-ext-diff".into(),
                    "--no-textconv".into(),
                    "HEAD".into(),
                    "--".into(),
                ],
            ))?,
            506 => self.start_lsp()?,
            507..=511 => self.language(id - 507)?,
            520 => self.new_terminal()?,
            521 => {
                let mut s = self.state.borrow_mut();
                let i = s.active_terminal;
                if i < s.terminals.len() {
                    let t = s.terminals.remove(i);
                    t.terminal.shutdown();
                    s.retired.push(t.terminal);
                }
                drop(s);
                self.terminal_tabs();
            }
            522 if notification == 1 => {
                self.state.borrow_mut().active_terminal = self.choice(522);
                self.show_terminal();
            }
            524 => {
                let mut line = text(self.h(523)).into_bytes();
                line.push(b'\r');
                self.terminal_input(line)?;
                self.put(523, "");
            }
            525 => self.terminal_input(vec![3])?,
            603 => {
                let deps = json::parse(&text(self.h(601)))?;
                let deps = deps
                    .array()
                    .ok_or("Dependencies must be a JSON array")?
                    .iter()
                    .map(|v| {
                        v.integer()
                            .ok_or("Dependencies must be positive integer task IDs")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let validation = args(&text(self.h(602)))?;
                self.task(task_service::Command::Add {
                    root: self.root(),
                    objective: text(self.h(600)),
                    dependencies: deps,
                    validation,
                })?;
            }
            604 => self.task(task_service::Command::RunReady)?,
            605 => self.task(task_service::Command::CancelAll)?,
            607 => self.task(task_service::Command::Retry(
                text(self.h(606)).parse().map_err(|_| "Enter a task ID")?,
            ))?,
            609 => {
                let id = text(self.h(606)).parse().map_err(|_| "Enter a task ID")?;
                let commit = text(self.h(608));
                if !rivet_core::tasks::valid_oid(&commit) {
                    return Err("Enter the exact reviewed worktree HEAD SHA".into());
                }
                let confirmed =
                    self.state.borrow().integration.as_ref() == Some(&(id, commit.clone()));
                if confirmed {
                    self.state.borrow_mut().integration = None;
                    self.task(task_service::Command::Integrate { id, commit })?;
                } else {
                    self.state.borrow_mut().integration = Some((id, commit.clone()));
                    self.task_log(&format!("Confirm integration of task #{id}, commit {commit}. Review the target workspace and changes. Press Integrate again to cherry-pick and run combined validation."));
                }
            }
            610 | 611 => {
                let request = self
                    .state
                    .borrow()
                    .task_permissions
                    .front()
                    .map(|p| p.0)
                    .ok_or("No task tool permission pending")?;
                self.task(task_service::Command::Decide {
                    request,
                    allow: id == 610,
                })?;
                self.state.borrow_mut().task_permissions.pop_front();
                self.task_render();
            }
            _ => {}
        }
        Ok(())
    }
    fn save(&self) -> Result<(), String> {
        let command = {
            let s = self.state.borrow();
            let d = active_doc(&s).ok_or("Open a document first")?;
            if !d.dirty() {
                return Ok(());
            }
            if s.pending_saves.contains(&d.path) {
                return Err("Save already pending for this document".into());
            }
            FileCommand::Save {
                root: self.root(),
                path: d.path.clone(),
                expected: d.base.clone(),
                snapshot: d.buffer.snapshot(),
                revision: d.revision,
            }
        };
        let path = if let FileCommand::Save { path, .. } = &command {
            path.clone()
        } else {
            unreachable!()
        };
        self.files.submit(command)?;
        self.state.borrow_mut().pending_saves.insert(path);
        self.status("Saving in background");
        Ok(())
    }
    fn undo(&self, undo: bool) {
        let view_text = {
            let mut s = self.state.borrow_mut();
            let Some(d) = active_doc_mut(&mut s) else {
                return;
            };
            if if undo {
                d.buffer.undo()
            } else {
                d.buffer.redo()
            } {
                d.revision += 1;
                d.highlight = true;
                Some((d.view, d.buffer.snapshot().text()))
            } else {
                None
            }
        };
        if let Some((view, text)) = view_text {
            self.guarded(|| set_text(view, &text));
        }
    }
    fn find(&self) {
        let query = text(self.h(409));
        let found = {
            let s = self.state.borrow();
            active_doc(&s).and_then(|d| {
                d.buffer.find(&query).map(|r| {
                    (
                        d.view,
                        d.buffer.byte_to_utf16(r.start).unwrap_or(0),
                        d.buffer.byte_to_utf16(r.end).unwrap_or(0),
                    )
                })
            })
        };
        if let Some((view, a, b)) = found {
            set_selection(view, a, b);
            send(view, 0xb7, 0, 0);
            unsafe {
                SetFocus(view);
            }
        } else {
            self.status("Text not found");
        }
    }
    fn discard(&self) -> Result<(), String> {
        let identity = {
            let s = self.state.borrow();
            let d = active_doc(&s).ok_or("Open a source document first")?;
            if s.pending_saves.contains(&d.path) {
                return Err("Wait for the pending save before rejecting edits".into());
            }
            (d.path.clone(), d.revision)
        };
        if self.state.borrow().discard.as_ref() != Some(&identity) {
            self.state.borrow_mut().discard = Some(identity.clone());
            self.status(&format!(
                "Confirm rejection of unsaved edits in {} by pressing Reject again",
                identity.0.display()
            ));
            return Ok(());
        }
        let (view, text) = {
            let mut s = self.state.borrow_mut();
            s.discard = None;
            let d = active_doc_mut(&mut s).unwrap();
            let text = normalize(&d.base);
            d.buffer = Buffer::new(Arc::from(text.as_str()));
            d.revision += 1;
            d.saved = d.revision;
            d.highlight = true;
            (d.view, text)
        };
        self.guarded(|| set_text(view, &text));
        self.status("Unsaved edits rejected; disk unchanged");
        Ok(())
    }
    fn edit(
        &self,
        view: Hwnd,
        mut range: (usize, usize),
        insert: &str,
        delete: i32,
    ) -> Result<(), String> {
        let insert = normalize(insert);
        let changed = {
            let mut s = self.state.borrow_mut();
            let d = s
                .docs
                .iter_mut()
                .find(|d| d.view == view)
                .ok_or("Editor document unavailable")?;
            if d.read_only {
                return Err("Mixed or legacy CR line endings are read-only in this version".into());
            }
            if range.0 == range.1 && delete < 0 && range.0 > 0 {
                range.0 -= 1;
                if d.buffer.utf16_to_byte(range.0).is_err() {
                    range.0 = range.0.saturating_sub(1);
                }
            }
            if range.0 == range.1 && delete > 0 {
                let end = d.buffer.byte_to_utf16(d.buffer.len())?;
                if range.1 < end {
                    range.1 += 1;
                    if d.buffer.utf16_to_byte(range.1).is_err() {
                        range.1 += 1;
                    }
                }
            }
            let a = d.buffer.utf16_to_byte(range.0)?;
            let b = d.buffer.utf16_to_byte(range.1)?;
            d.buffer.replace(a..b, &insert)?;
            d.revision += 1;
            d.highlight = true;
            range
        };
        self.guarded(|| {
            set_selection(view, changed.0, changed.1);
            send(view, 0xc2, 0, wide(&insert).as_ptr() as isize);
        });
        Ok(())
    }
    fn fallback(&self, view: Hwnd) {
        let content = normalize(&text(view));
        let mut error = None;
        {
            let mut s = self.state.borrow_mut();
            if let Some(d) = s.docs.iter_mut().find(|d| d.view == view) {
                let previous = d.buffer.snapshot().text();
                if previous != content {
                    if content.len() > 256 * 1024 || d.read_only {
                        error=Some((previous,"This native edit path is limited to 256 KiB; use ordinary typing/paste or an external editor".to_string()));
                    } else {
                        let mut a = previous
                            .bytes()
                            .zip(content.bytes())
                            .take_while(|(a, b)| a == b)
                            .count();
                        while !previous.is_char_boundary(a) || !content.is_char_boundary(a) {
                            a -= 1;
                        }
                        let mut suffix = previous[a..]
                            .bytes()
                            .rev()
                            .zip(content[a..].bytes().rev())
                            .take_while(|(a, b)| a == b)
                            .count();
                        while !previous.is_char_boundary(previous.len() - suffix)
                            || !content.is_char_boundary(content.len() - suffix)
                        {
                            suffix -= 1;
                        }
                        match d.buffer.replace(
                            a..previous.len() - suffix,
                            &content[a..content.len() - suffix],
                        ) {
                            Ok(()) => {
                                d.revision += 1;
                                d.highlight = true;
                            }
                            Err(e) => error = Some((previous, e)),
                        }
                    }
                }
            }
        }
        if let Some((previous, e)) = error {
            self.guarded(|| set_text(view, &previous));
            self.status(&e);
        }
    }
    fn review(&self) {
        let content = {
            let s = self.state.borrow();
            s.review.as_ref().map(|d| {
                if s.inline {
                    (d.inline.clone(), String::new())
                } else {
                    (d.left.clone(), d.right.clone())
                }
            })
        };
        if let Some((a, b)) = content {
            self.put(700, &a);
            self.put(701, &b);
            self.guarded(|| {
                diff_colors(self.h(700), &a);
                diff_colors(self.h(701), &b);
            });
        }
        self.mode.set(3);
        self.layout();
    }
    fn document(
        &self,
        path: PathBuf,
        base: Arc<str>,
        buffer: Buffer,
        cursor: Option<(u32, u32)>,
    ) -> Result<(), String> {
        if base.contains('\0') {
            return Err("NUL-containing files are not supported by the native editor".into());
        }
        let existing = self.state.borrow().docs.iter().position(|d| d.path == path);
        if let Some(i) = existing {
            let keep = {
                let s = self.state.borrow();
                s.docs[i].dirty() || s.docs[i].base == base
            };
            if keep {
                self.state.borrow_mut().active_doc = Some(path);
                self.go_to(cursor);
                self.layout();
                return Ok(());
            }
            let old = self.state.borrow_mut().docs.remove(i).unwrap();
            unsafe {
                DestroyWindow(old.view);
            }
        }
        loop {
            let s = self.state.borrow();
            let bytes: usize = s.docs.iter().map(|d| d.base.len()).sum();
            if s.docs.len() < 20 && bytes + base.len() <= 32 * 1024 * 1024 {
                break;
            }
            let index = s
                .docs
                .iter()
                .position(|d| !d.dirty() && !s.pending_saves.contains(&d.path));
            drop(s);
            if let Some(index) = index {
                let old = self.state.borrow_mut().docs.remove(index).unwrap();
                unsafe {
                    DestroyWindow(old.view);
                }
            } else {
                return Err(
                    "Document cache is full of unsaved edits; save or reject a document first"
                        .into(),
                );
            }
        }
        let view = self.rich(DOCUMENTS)?;
        send(view, 0xcf, 0, 0);
        send(view, 0x445, 0, 1);
        send(view, 0x30, self.mono.get() as usize, 1);
        send(view, 0x445, 0, 1);
        send(view, 0x452, 0, 0); // Rust owns undo/redo, not RichEdit.
        if unsafe { SetWindowSubclass(view, Some(editor_proc), 1, self as *const _ as usize) } == 0
        {
            unsafe {
                DestroyWindow(view);
            }
            return Err("Cannot initialize native editing delegate".into());
        }
        let content = buffer.snapshot().text();
        let read_only = mixed_endings(&base);
        self.state.borrow_mut().docs.push_back(Document {
            path: path.clone(),
            view,
            base,
            buffer,
            revision: 0,
            saved: 0,
            highlight: true,
            read_only,
        });
        self.guarded(|| set_text(view, &content));
        send(view, 0xcf, usize::from(read_only), 0);
        self.state.borrow_mut().active_doc = Some(path.clone());
        self.refresh_documents();
        self.mode.set(2);
        self.layout();
        self.go_to(cursor);
        self.status(if read_only {
            "Mixed line endings: source is read-only in this version"
        } else {
            path.to_str().unwrap_or("Source document")
        });
        Ok(())
    }
    fn go_to(&self, cursor: Option<(u32, u32)>) {
        if let Some((line, column)) = cursor {
            let value = {
                let s = self.state.borrow();
                active_doc(&s).and_then(|d| {
                    d.buffer
                        .line_utf16(line as usize)
                        .and_then(|n| n.checked_add(column as usize))
                        .filter(|&n| d.buffer.utf16_to_byte(n).is_ok())
                        .map(|n| (d.view, n))
                })
            };
            if let Some((view, n)) = value {
                set_selection(view, n, n);
                send(view, 0xb7, 0, 0);
            }
        }
    }
    fn refresh_documents(&self) {
        let (items, active) = {
            let s = self.state.borrow();
            (
                s.docs
                    .iter()
                    .enumerate()
                    .map(|(i, d)| format!("{}: {}", i + 1, d.path.display()))
                    .collect::<Vec<_>>(),
                s.docs
                    .iter()
                    .position(|d| Some(&d.path) == s.active_doc.as_ref())
                    .unwrap_or(0),
            )
        };
        self.combo(405, &items, active);
    }
    fn spec(&self) -> Result<Spec, String> {
        Ok(Spec {
            program: text(self.h(500)),
            args: args(&text(self.h(501)))?,
            cwd: self.root(),
            timeout: Duration::from_secs(300),
            capture_limit: 1024 * 1024,
        })
    }
    fn confirm(&self, spec: &Spec, lsp: bool) -> bool {
        if self.state.borrow().confirmation.as_ref() == Some(&(spec.clone(), lsp)) {
            self.state.borrow_mut().confirmation = None;
            true
        } else {
            self.state.borrow_mut().confirmation = Some((spec.clone(), lsp));
            self.output(&format!("Confirm this exact process by pressing the same action again.\nExecutable: {}\nArguments: {:?}\nWorkspace: {}\nRuns with your user permissions; this is not an OS sandbox.",spec.program,spec.args,spec.cwd.display()));
            false
        }
    }
    fn execute(&self, spec: Spec) -> Result<(), String> {
        let id = self.state.borrow().next_job;
        self.processes.submit(id, spec)?;
        let mut s = self.state.borrow_mut();
        s.next_job += 1;
        s.jobs.push(id);
        drop(s);
        self.output(&format!("Job {id} running in background"));
        Ok(())
    }
    fn output(&self, text: &str) {
        self.state.borrow_mut().terminal_visible = false;
        self.put(702, &text.chars().take(65536).collect::<String>());
    }
    fn start_lsp(&self) -> Result<(), String> {
        let spec = self.spec()?;
        if !self.confirm(&spec, true) {
            return Ok(());
        }
        if self.state.borrow().lsp_start.is_some() {
            return Err("Language server startup already pending".into());
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("rivet-windows-lsp".into())
            .spawn(move || {
                let _ = tx.send(lsp::Connection::spawn(&spec.program, &spec.args, &spec.cwd));
            })
            .map_err(|e| e.to_string())?;
        self.state.borrow_mut().lsp_start = Some(rx);
        self.output("Language server starting");
        Ok(())
    }
    fn language(&self, kind: u16) -> Result<(), String> {
        let (mut id, path, revision, position, content, reopen, old) = {
            let s = self.state.borrow();
            if !s.lsp_ready {
                return Err("Start a language server and wait for initialization".into());
            }
            let d = active_doc(&s).ok_or("Open a source document first")?;
            if d.buffer.len() > 256 * 1024 {
                return Err("LSP synchronization is limited to 256 KiB in this version".into());
            }
            let cp = selection(d.view).0;
            (
                s.next_request,
                d.path.clone(),
                d.revision,
                d.buffer.position(cp)?,
                d.buffer.snapshot().text(),
                s.lsp_open.as_ref() != Some(&(d.path.clone(), d.revision)),
                s.lsp_open.clone(),
            )
        };
        let method = [
            "textDocument/hover",
            "textDocument/definition",
            "textDocument/references",
            "textDocument/completion",
            "textDocument/semanticTokens/full",
        ][kind.min(4) as usize];
        let mut params = lsp::object([(
            "textDocument",
            lsp::object([("uri", lsp::file_uri(&path)?.into())]),
        )]);
        if let Value::Object(m) = &mut params {
            if kind < 4 {
                m.insert(
                    "position".into(),
                    lsp::object([
                        ("line", (position.0 as u32).into()),
                        ("character", (position.1 as u32).into()),
                    ]),
                );
            }
            if kind == 2 {
                m.insert(
                    "context".into(),
                    lsp::object([("includeDeclaration", Value::Bool(true))]),
                );
            }
        }
        {
            let mut s = self.state.borrow_mut();
            let server = s.lsp.as_ref().ok_or("Language server unavailable")?;
            if reopen {
                if let Some((old, _)) = old {
                    server.send(lsp::notification(
                        "textDocument/didClose",
                        lsp::object([(
                            "textDocument",
                            lsp::object([("uri", lsp::file_uri(&old)?.into())]),
                        )]),
                    ))?;
                }
                server.send(lsp::did_open(&path, content)?)?;
            }
            server.send(lsp::request(id, method, params))?;
            s.lsp_open = Some((path.clone(), revision));
            s.requests.insert(id, (kind, path, revision));
            id += 1;
            s.next_request = id;
        }
        self.output("Language request sent");
        Ok(())
    }
    fn consume_language(&self, v: &Value) {
        if v.get("id").and_then(Value::integer) == Some(1) {
            let capabilities = v.get("result").and_then(|r| r.get("capabilities"));
            if v.get("error").is_some()
                || capabilities
                    .and_then(|c| c.get("positionEncoding"))
                    .and_then(Value::str)
                    .is_some_and(|s| s != "utf-16")
            {
                self.output(
                    "Language server initialization failed or unsupported position encoding",
                );
                return;
            }
            let mut s = self.state.borrow_mut();
            s.lsp_ready = true;
            s.legend = capabilities
                .and_then(|c| c.get("semanticTokensProvider"))
                .and_then(|p| p.get("legend"))
                .and_then(|l| l.get("tokenTypes"))
                .and_then(Value::array)
                .unwrap_or(&[])
                .iter()
                .filter_map(Value::str)
                .take(128)
                .map(String::from)
                .collect();
            if let Some(server) = &s.lsp {
                let _ = server.send(lsp::notification(
                    "initialized",
                    Value::object(std::iter::empty::<(String, Value)>()),
                ));
            }
            drop(s);
            self.output("Language server ready");
            return;
        }
        let request = v
            .get("id")
            .and_then(Value::integer)
            .and_then(|id| self.state.borrow_mut().requests.remove(&(id as u32)));
        if let Some((kind, path, revision)) = request {
            let current = {
                let s = self.state.borrow();
                active_doc(&s).is_some_and(|d| d.path == path && d.revision == revision)
            };
            if current && let Some(result) = v.get("result") {
                if kind == 1 {
                    let location = result.array().and_then(|a| a.first()).unwrap_or(result);
                    let uri = location
                        .get("uri")
                        .or_else(|| location.get("targetUri"))
                        .and_then(Value::str);
                    let start = location
                        .get("range")
                        .or_else(|| location.get("targetSelectionRange"))
                        .and_then(|r| r.get("start"));
                    if let (Some(uri), Some(start)) = (uri, start)
                        && let Ok(path) = lsp::uri_path(uri)
                    {
                        let _ = self.file(FileCommand::ReadAt {
                            root: self.root(),
                            path,
                            line: start.get("line").and_then(Value::integer).unwrap_or(0) as u32,
                            character: start.get("character").and_then(Value::integer).unwrap_or(0)
                                as u32,
                        });
                    }
                } else if kind == 4 {
                    self.semantic(result);
                }
            }
        }
        self.output(&language_text(v));
    }
    fn new_terminal(&self) -> Result<(), String> {
        if self.state.borrow().terminals.len() >= 4 {
            return Err("Close a terminal first (limit 4)".into());
        }
        let terminal = Terminal::spawn(&self.root())?;
        self.state.borrow_mut().terminals.push(Term {
            terminal,
            screen: Screen::new(100, 40),
        });
        self.terminal_tabs();
        Ok(())
    }
    fn terminal_tabs(&self) {
        let len = self.state.borrow().terminals.len();
        self.combo(
            522,
            &(0..len)
                .map(|i| format!("Terminal {}", i + 1))
                .collect::<Vec<_>>(),
            len.saturating_sub(1),
        );
        self.state.borrow_mut().active_terminal = len.saturating_sub(1);
        self.show_terminal();
    }
    fn show_terminal(&self) {
        let content = {
            let mut s = self.state.borrow_mut();
            s.terminal_visible = true;
            s.terminals
                .get(s.active_terminal)
                .map(|t| t.screen.render())
        };
        if let Some(content) = content {
            self.put(702, &content);
            unsafe {
                SetFocus(self.h(702));
            }
        }
    }
    fn terminal_input(&self, bytes: Vec<u8>) -> Result<(), String> {
        let mut s = self.state.borrow_mut();
        s.terminals
            .get(s.active_terminal)
            .ok_or("Create a terminal first")?
            .terminal
            .write(bytes)?;
        s.terminal_visible = true;
        Ok(())
    }
    fn task(&self, c: task_service::Command) -> Result<(), String> {
        self.tasks
            .borrow()
            .as_ref()
            .ok_or("Task service unavailable in this test environment")?
            .submit(c)
    }
    fn task_log(&self, text: &str) {
        let mut s = self.state.borrow_mut();
        s.task_log.extend(text.chars().take(8192));
        s.task_log.push('\n');
        trim_log(&mut s.task_log);
        drop(s);
        self.task_render();
    }
    fn task_render(&self) {
        let (content, pending) = {
            let s = self.state.borrow();
            (
                format!(
                    "{}\n{}\n{}",
                    s.task_permissions
                        .front()
                        .map(|p| format!("TASK TOOL PERMISSION:\n{}\n", p.1))
                        .unwrap_or_default(),
                    s.task_report,
                    s.task_log
                ),
                !s.task_permissions.is_empty(),
            )
        };
        self.put(703, &content);
        self.put(205, if pending { "Tasks (approval)" } else { "Tasks" });
        enable(self.h(610), pending);
        enable(self.h(611), pending);
    }
    fn refresh_browser(&self) {
        let b = self.browser.borrow();
        let ids = b.ids();
        self.combo(
            301,
            &ids.iter().map(|n| format!("Tab {n}")).collect::<Vec<_>>(),
            ids.iter().position(|id| Some(*id) == b.active).unwrap_or(0),
        );
    }
    fn begin_close(&self) {
        if self.closing.get() {
            return;
        }
        let s = self.state.borrow();
        if s.docs.iter().any(Document::dirty) || !s.pending_saves.is_empty() {
            drop(s);
            self.status(
                "Wait for pending saves; save or explicitly reject unsaved files before closing",
            );
            return;
        }
        drop(s);
        self.closing.set(true);
        self.backend.borrow().shutdown();
        self.processes.shutdown();
        if let Some(host) = self.tasks.borrow().as_ref() {
            host.shutdown();
        }
        let s = self.state.borrow();
        if let Some(server) = &s.lsp {
            server.shutdown();
        }
        for t in &s.terminals {
            t.terminal.shutdown();
        }
        drop(s);
        self.browser.borrow_mut().shutdown();
        self.status("Stopping background services");
    }
    fn stopped(&self) -> bool {
        let s = self.state.borrow();
        self.backend_stopped.get()
            && self.processes.is_finished()
            && self.tasks.borrow().as_ref().is_none_or(Host::is_finished)
            && s.lsp_start.is_none()
            && s.lsp.as_ref().is_none_or(lsp::Connection::is_finished)
            && s.terminals.iter().all(|t| t.terminal.is_finished())
            && s.retired.iter().all(Terminal::is_finished)
    }
    fn poll(&self) {
        let started = Instant::now();
        for _ in 0..64 {
            let Some(event) = self.backend.borrow_mut().try_event() else {
                break;
            };
            let key = match &event {
                AgentEvent::Ready { key, .. }
                | AgentEvent::Delta { key, .. }
                | AgentEvent::Activity { key, .. }
                | AgentEvent::Error { key, .. }
                | AgentEvent::Finished { key } => Some(*key),
                _ => None,
            };
            let old = key
                .and_then(|key| {
                    self.state
                        .borrow()
                        .workspace
                        .sessions
                        .get(&key)
                        .map(|s| (s.transcript.len(), s.transcript_epoch))
                })
                .unwrap_or((0, 0));
            self.state.borrow_mut().workspace.apply(&event);
            if let Some(key) = key {
                self.update_chat(key, old.0, old.1);
            }
            match event {
                AgentEvent::Stopped => self.backend_stopped.set(true),
                AgentEvent::Status(s) => self.status(&s),
                AgentEvent::Error { message, key } => {
                    self.status(&message);
                    if key == self.state.borrow().workspace.active {
                        self.select_session(key);
                    }
                }
                AgentEvent::Ready { key, .. } => {
                    if key == self.state.borrow().workspace.active {
                        self.guarded(|| {
                            send(self.h(110), 0xcf, 1, 0);
                            send(self.h(113), 0xcf, 1, 0);
                        });
                        enable(self.h(111), false);
                        enable(self.h(112), false);
                    }
                    self.status("Session connected");
                }
                AgentEvent::Models(_) => {
                    let models = self.state.borrow().workspace.models.clone();
                    let selected = self.choice(112);
                    let mut items = vec!["SDK default".to_string()];
                    items.extend(models);
                    self.combo(112, &items, selected);
                }
                AgentEvent::SavedSession { .. } => {
                    let saved = self.state.borrow().workspace.saved.clone();
                    self.combo(103, &saved, 0);
                }
                AgentEvent::Permission {
                    key,
                    request,
                    description,
                } => {
                    if self.state.borrow().permissions.len() < 32 {
                        self.state.borrow_mut().permissions.push_back((
                            key,
                            request,
                            description,
                            Instant::now(),
                        ));
                        self.permissions();
                    } else {
                        let _ = self.submit(AgentCommand::Decide {
                            request,
                            allow: false,
                        });
                    }
                }
                AgentEvent::Finished { .. } => self.status("Turn finished"),
                _ => {}
            }
        }
        for _ in 0..4 {
            let Some(event) = self.files.try_event() else {
                break;
            };
            match event {
                FileEvent::Entries { path, entries } => {
                    self.state.borrow_mut().directory = path;
                    let list = entries.into_iter().take(200).collect::<Vec<_>>();
                    let titles = list
                        .iter()
                        .enumerate()
                        .map(|(i, (p, folder))| {
                            format!(
                                "{}: {}{}",
                                i + 1,
                                if *folder { "[dir] " } else { "" },
                                p.file_name().unwrap_or_default().to_string_lossy()
                            )
                        })
                        .collect::<Vec<_>>();
                    self.state.borrow_mut().entries = list;
                    self.combo(402, &titles, 0);
                }
                FileEvent::Loaded {
                    path,
                    text,
                    buffer,
                    cursor,
                } => {
                    if let Err(e) = self.document(path, text, buffer, cursor) {
                        self.status(&e);
                    }
                }
                FileEvent::Saved {
                    path,
                    text,
                    revision,
                } => {
                    let mut s = self.state.borrow_mut();
                    s.pending_saves.remove(&path);
                    if let Some(d) = s.docs.iter_mut().find(|d| d.path == path) {
                        d.base = text;
                        if d.revision == revision {
                            d.saved = revision;
                        }
                    }
                    drop(s);
                    self.status("File saved; newer edits remain unsaved");
                }
                FileEvent::SaveFailed { path, message } => {
                    self.state.borrow_mut().pending_saves.remove(&path);
                    self.status(&message);
                }
                FileEvent::Diff(diff) => {
                    self.state.borrow_mut().review = Some(diff);
                    self.review();
                }
                FileEvent::Error(e) => self.status(&e),
            }
        }
        for _ in 0..4 {
            let Some(done) = self.processes.try_event() else {
                break;
            };
            self.state.borrow_mut().jobs.retain(|&id| id != done.id);
            let text = match done.result {
                Ok(result) => result.display(),
                Err(e) => e,
            };
            if text.contains("RIVET_PROCESS_OK")
                && let Some(smoke) = self.smoke.borrow_mut().as_mut()
            {
                smoke.process = true;
            }
            self.output(&text);
        }
        let server = self
            .state
            .borrow()
            .lsp_start
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(server) = server {
            self.state.borrow_mut().lsp_start = None;
            match server {
                Ok(server) => {
                    if self.closing.get() {
                        server.shutdown();
                    } else if let Err(e) =
                        lsp::initialize(&self.root()).and_then(|v| server.send(v))
                    {
                        self.output(&e);
                    }
                    self.state.borrow_mut().lsp = Some(server);
                    self.state.borrow_mut().lsp_ready = false;
                    self.state.borrow_mut().lsp_open = None;
                }
                Err(e) => self.output(&e),
            }
        }
        for _ in 0..4 {
            let event = self
                .state
                .borrow()
                .lsp
                .as_ref()
                .and_then(lsp::Connection::try_event);
            let Some(event) = event else {
                break;
            };
            match event {
                Ok(v) => {
                    if language_text(&v).contains("Fixture hover")
                        && let Some(smoke) = self.smoke.borrow_mut().as_mut()
                    {
                        smoke.hover = true;
                    }
                    self.consume_language(&v);
                }
                Err(e) => self.output(&format!("Language server: {e}")),
            }
        }
        {
            let mut s = self.state.borrow_mut();
            let active = s.active_terminal;
            let visible = s.terminal_visible;
            let mut changed = false;
            for (i, t) in s.terminals.iter_mut().enumerate() {
                for _ in 0..4 {
                    let Some(bytes) = t.terminal.try_output() else {
                        break;
                    };
                    t.screen.feed(&bytes);
                    changed |= i == active;
                }
            }
            s.retired.retain(|t| !t.is_finished());
            let content = if changed && visible {
                s.terminals.get(active).map(|t| t.screen.render())
            } else {
                None
            };
            drop(s);
            if let Some(content) = content {
                self.put(702, &content);
            }
        }
        let mut tasks_dirty = false;
        for _ in 0..16 {
            let event = self.tasks.borrow().as_ref().and_then(Host::try_event);
            let Some(event) = event else {
                break;
            };
            match event {
                task_service::Event::Report(r) => self.state.borrow_mut().task_report = r,
                task_service::Event::Log(t) => {
                    let mut s = self.state.borrow_mut();
                    s.task_log.extend(t.chars().take(8192));
                    s.task_log.push('\n');
                    trim_log(&mut s.task_log);
                }
                task_service::Event::Permission {
                    request,
                    description,
                } => {
                    self.state.borrow_mut().task_permissions.push_back((
                        request,
                        description,
                        Instant::now(),
                    ));
                }
                task_service::Event::Stopped => {}
            }
            tasks_dirty = true;
        }
        if tasks_dirty {
            self.task_render();
        }
        let expired = self
            .state
            .borrow()
            .permissions
            .front()
            .is_some_and(|p| p.3.elapsed() > Duration::from_secs(120));
        if expired {
            self.state.borrow_mut().permissions.pop_front();
            self.permissions();
        }
        let expired = self
            .state
            .borrow()
            .task_permissions
            .front()
            .is_some_and(|p| p.2.elapsed() > Duration::from_secs(120));
        if expired {
            self.state.borrow_mut().task_permissions.pop_front();
            self.task_render();
        }
        self.browser.borrow_mut().tick();
        let error = self.browser.borrow_mut().error.take();
        if let Some(error) = error {
            self.status(&error);
        }
        self.highlight();
        self.polling.borrow_mut().record(started.elapsed());
        if self.closing.get() && self.stopped() {
            self.report();
            if self.smoke.borrow().as_ref().is_some_and(|s| s.stage >= 9) {
                println!(
                    "RIVET_WINDOWS_UI_SMOKE_PASS (test SDK backend; real Win32, file save, process, LSP, ConPTY and WebView2)"
                );
            }
            unsafe {
                KillTimer(self.window.get(), 1);
                DestroyWindow(self.window.get());
            }
        }
    }
    fn drain(&self) {
        for _ in 0..256 {
            let event = self.queue.borrow_mut().pop_front();
            let Some(event) = event else {
                break;
            };
            match event {
                Event::Action(id, n) => {
                    if let Err(e) = self.action(id, n) {
                        self.status(&e);
                    }
                }
                Event::Layout => self.layout(),
                Event::Close => self.begin_close(),
                Event::Tick => {
                    self.poll();
                    if !self.closing.get() {
                        self.smoke_step();
                    }
                }
                Event::EditFallback(view) => self.fallback(view),
                Event::Fatal => {
                    self.fatal.set(true);
                    self.backend.borrow().shutdown();
                    self.processes.shutdown();
                    unsafe {
                        PostQuitMessage(1);
                    }
                }
            }
        }
    }
    fn report(&self) {
        for (name, samples) in [
            ("windows_session_view_apply_elapsed", &self.switching),
            ("windows_poll_elapsed", &self.polling),
            (
                "windows_cached_browser_switch_elapsed",
                &self.browser_switch,
            ),
        ] {
            if let Some(p) = samples.borrow().percentiles() {
                println!(
                    "{name}: n={} p50={} p95={} p99={} ns (callbacks, not presented frames)",
                    p.count, p.p50, p.p95, p.p99
                );
            }
        }
    }
}
#[repr(C)]
struct CharFormat {
    size: u32,
    mask: u32,
    effects: u32,
    height: i32,
    offset: i32,
    color: u32,
    charset: u8,
    pitch: u8,
    face: [u16; 32],
}
fn color(view: Hwnd, start: usize, end: usize, rgb: u32) {
    set_selection(view, start, end);
    let cf = CharFormat {
        size: std::mem::size_of::<CharFormat>() as u32,
        mask: 0x40000000,
        effects: 0,
        height: 0,
        offset: 0,
        color: rgb,
        charset: 0,
        pitch: 0,
        face: [0; 32],
    };
    send(view, 0x444, 1, &cf as *const _ as isize);
}
impl App {
    fn highlight(&self) {
        let data = {
            let mut s = self.state.borrow_mut();
            active_doc_mut(&mut s).filter(|d| d.highlight).map(|d| {
                d.highlight = false;
                let mut end = d.buffer.len().min(8192);
                while d.buffer.byte_to_utf16(end).is_err() {
                    end -= 1;
                }
                (d.view, d.buffer.read(0..end).unwrap_or_default())
            })
        };
        if let Some((view, prefix)) = data {
            let selected = selection(view);
            self.guarded(|| {
                send(view, 0xb, 0, 0);
                color(view, 0, prefix.encode_utf16().count(), 0);
                for (range, kind) in rivet_core::editor::tokens(&prefix).into_iter().take(128) {
                    let rgb = match kind {
                        rivet_core::editor::TokenKind::Keyword => 0x800080,
                        rivet_core::editor::TokenKind::String => 0x2020b0,
                        rivet_core::editor::TokenKind::Comment => 0x408020,
                        rivet_core::editor::TokenKind::Number => 0x0080b0,
                    };
                    color(
                        view,
                        prefix[..range.start].encode_utf16().count(),
                        prefix[..range.end].encode_utf16().count(),
                        rgb,
                    );
                }
                set_selection(view, selected.0, selected.1);
                send(view, 0xb, 1, 0);
            });
        }
    }
    fn semantic(&self, result: &Value) {
        let s = self.state.borrow();
        let Some(d) = active_doc(&s) else {
            return;
        };
        let data: Vec<_> = result
            .get("data")
            .and_then(Value::array)
            .unwrap_or(&[])
            .iter()
            .take(2560)
            .filter_map(Value::integer)
            .collect();
        let mut spans = Vec::new();
        let (mut line, mut col) = (0usize, 0usize);
        for t in data.as_chunks::<5>().0 {
            if t[0] > 0 {
                line = line.saturating_add(t[0] as usize);
                col = t[1] as usize;
            } else {
                col = col.saturating_add(t[1] as usize);
            }
            if let Some(a) = d.buffer.line_utf16(line).and_then(|n| n.checked_add(col))
                && let Some(b) = a.checked_add(t[2] as usize)
                && d.buffer.utf16_to_byte(a).is_ok()
                && d.buffer.utf16_to_byte(b).is_ok()
            {
                let rgb = match s.legend.get(t[3] as usize).map(String::as_str) {
                    Some("keyword" | "class" | "type") => 0x800080,
                    Some("string") => 0x2020b0,
                    Some("comment") => 0x408020,
                    Some("number") => 0x0080b0,
                    _ => 0,
                };
                spans.push((a, b, rgb));
            }
        }
        let view = d.view;
        drop(s);
        let selected = selection(view);
        self.guarded(|| {
            for (a, b, rgb) in spans {
                color(view, a, b, rgb);
            }
            set_selection(view, selected.0, selected.1);
        });
    }
}
fn diff_colors(view: Hwnd, text: &str) {
    send(view, 0xb, 0, 0);
    color(view, 0, text.encode_utf16().count(), 0);
    let mut offset = 0;
    for line in text.split_inclusive('\n').take(512) {
        let len = line.encode_utf16().count();
        if line.starts_with("+ ") {
            color(view, offset, offset + len, 0x208020);
        } else if line.starts_with("- ") {
            color(view, offset, offset + len, 0x2020b0);
        }
        offset += len;
    }
    set_selection(view, 0, 0);
    send(view, 0xb, 1, 0);
}
fn active_doc(s: &State) -> Option<&Document> {
    s.docs
        .iter()
        .find(|d| Some(&d.path) == s.active_doc.as_ref())
}
fn active_doc_mut(s: &mut State) -> Option<&mut Document> {
    let path = s.active_doc.clone();
    s.docs.iter_mut().find(|d| Some(&d.path) == path.as_ref())
}
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}
fn mixed_endings(s: &str) -> bool {
    let crlf = s.matches("\r\n").count();
    let cr = s.bytes().filter(|&b| b == b'\r').count();
    let lf = s.bytes().filter(|&b| b == b'\n').count();
    cr > crlf || (crlf > 0 && lf > crlf)
}
fn args(s: &str) -> Result<Vec<String>, String> {
    if s.len() > 16384 {
        return Err("Argument budget exceeded (16 KiB)".into());
    }
    let v = json::parse(s)?;
    let array = v
        .array()
        .ok_or("Arguments must be a JSON array of strings")?;
    if array.len() > 256 {
        return Err("Argument limit is 256".into());
    }
    array
        .iter()
        .map(|v| {
            v.str()
                .map(String::from)
                .ok_or("Every argument must be a string".into())
        })
        .collect()
}
fn safely_model(combo: Hwnd, index: usize) -> String {
    let n = send(combo, 0x149, index, 0);
    if !(0..4096).contains(&n) {
        return String::new();
    }
    let mut data = vec![0; n as usize + 1];
    let n = send(combo, 0x148, index, data.as_mut_ptr() as isize);
    String::from_utf16_lossy(&data[..n.max(0) as usize])
}
fn trim_log(s: &mut String) {
    if s.len() > 65536 {
        let mut i = s.len() - 49152;
        while !s.is_char_boundary(i) {
            i += 1;
        }
        s.drain(..i);
    }
}
fn language_text(v: &Value) -> String {
    fn walk(v: &Value, out: &mut String, depth: usize) {
        if depth > 12 || out.len() >= 60000 {
            return;
        }
        match v {
            Value::String(s) => {
                out.extend(s.chars().take(60000usize.saturating_sub(out.len())));
                out.push('\n');
            }
            Value::Array(a) => {
                for v in a.iter().take(128) {
                    walk(v, out, depth + 1)
                }
            }
            Value::Object(m) => {
                for k in [
                    "result",
                    "error",
                    "params",
                    "contents",
                    "diagnostics",
                    "items",
                    "message",
                    "label",
                    "value",
                    "uri",
                    "targetUri",
                ] {
                    if let Some(v) = m.get(k) {
                        walk(v, out, depth + 1)
                    }
                }
            }
            _ => {}
        }
    }
    let mut s = String::new();
    walk(v, &mut s, 0);
    if s.is_empty() {
        "Language response received (no displayable text)".into()
    } else {
        s
    }
}

// SAFETY: callback data points to the boxed App owned by run_loop. The App outlives
// its HWND and all child subclasses. Reentrant window notifications only enqueue
// owned events; no mutable Rust reference crosses a native message dispatch.
unsafe extern "system" fn window_proc(window: Hwnd, message: u32, w: usize, l: isize) -> isize {
    unsafe {
        if message == 0x24 {
            let dpi = GetDpiForWindow(window).max(96) as i32;
            let points = l as *mut Point;
            *points.add(3) = Point {
                x: 900 * dpi / 96,
                y: 720 * dpi / 96,
            };
            return 0;
        }
        if message == 0x81 {
            let create = &*(l as *const CreateStruct);
            SetWindowLongPtrW(window, -21, create.param as isize);
            let app = &*(create.param as *const App);
            app.window.set(window);
        }
        let data = GetWindowLongPtrW(window, -21);
        if data == 0 {
            return DefWindowProcW(window, message, w, l);
        }
        let app = &*(data as *const App);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match message {
            5 | 0x47 => {
                app.enqueue(Event::Layout);
                0
            }
            0x2e0 => {
                let r = &*(l as *const Rect);
                SetWindowPos(
                    window,
                    ptr::null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    0x14,
                );
                app.fonts();
                app.enqueue(Event::Layout);
                0
            }
            0x111 => {
                if app.suppress.get() == 0 {
                    let id = (w & 0xffff) as u16;
                    let notification = (w >> 16) as u16;
                    if id == 0 && notification == 0x300 {
                        app.enqueue(Event::EditFallback(l as Hwnd));
                    } else {
                        app.enqueue(Event::Action(id, notification));
                    }
                }
                0
            }
            0x113 => {
                app.enqueue(Event::Tick);
                0
            }
            0x10 => {
                app.enqueue(Event::Close);
                0
            }
            2 => {
                PostQuitMessage(0);
                0
            }
            0x82 => {
                SetWindowLongPtrW(window, -21, 0);
                DefWindowProcW(window, message, w, l)
            }
            _ => DefWindowProcW(window, message, w, l),
        }));
        match result {
            Ok(result) => result,
            Err(_) => {
                app.enqueue(Event::Fatal);
                0
            }
        }
    }
}
unsafe extern "system" fn editor_proc(
    view: Hwnd,
    message: u32,
    w: usize,
    l: isize,
    _id: usize,
    data: usize,
) -> isize {
    unsafe {
        if message == 0x82 {
            RemoveWindowSubclass(view, Some(editor_proc), 1);
            return DefSubclassProc(view, message, w, l);
        }
        let app = &*(data as *const App);
        if app.suppress.get() > 0 {
            return DefSubclassProc(view, message, w, l);
        }
        if message == 0x87 {
            // Code editing owns Tab, Return and arrow keys, including dialog routing.
            return DefSubclassProc(view, message, w, l) | 0x87;
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
            || -> Option<Result<(), String>> {
                match message {
                    0x100 if GetKeyState(0x11) < 0 && w == b'Z' as usize => {
                        app.undo(true);
                        Some(Ok(()))
                    }
                    0x100 if GetKeyState(0x11) < 0 && w == b'Y' as usize => {
                        app.undo(false);
                        Some(Ok(()))
                    }
                    0x100 if GetKeyState(0x11) < 0 && w == b'S' as usize => Some(app.save()),
                    0x100 if GetKeyState(0x11) < 0 && w == b'F' as usize => {
                        SetFocus(app.h(409));
                        Some(Ok(()))
                    }
                    0x100 if w == 0x2e => Some(app.edit(view, selection(view), "", 1)),
                    0x102 => {
                        let word = w as u16;
                        if word == 8 {
                            return Some(app.edit(view, selection(view), "", -1));
                        }
                        if word == 22 || word == 24 || word == 3 || word == 1 {
                            return None;
                        }
                        if word < 32 && word != 9 && word != 13 {
                            return Some(Ok(()));
                        }
                        if (0xd800..=0xdbff).contains(&word) {
                            app.high.set(Some(word));
                            return Some(Ok(()));
                        }
                        let units = if let Some(high) = app.high.take() {
                            vec![high, word]
                        } else {
                            vec![word]
                        };
                        let text = String::from_utf16(&units)
                            .map_err(|_| "Invalid UTF-16 input".to_string());
                        Some(text.and_then(|s| {
                            app.edit(view, selection(view), if s == "\r" { "\n" } else { &s }, 0)
                        }))
                    }
                    0x302 => Some(
                        clipboard()
                            .ok_or("No Unicode clipboard text".into())
                            .and_then(|s| app.edit(view, selection(view), &s, 0)),
                    ),
                    0x300 | 0x301 => {
                        let (a, b) = selection(view);
                        let selected = {
                            let state = app.state.borrow();
                            state.docs.iter().find(|d| d.view == view).and_then(|d| {
                                let a = d.buffer.utf16_to_byte(a).ok()?;
                                let b = d.buffer.utf16_to_byte(b).ok()?;
                                d.buffer.read(a..b).ok()
                            })
                        };
                        if let Some(s) = selected {
                            copy(&s.replace('\n', "\r\n"));
                        }
                        Some(if message == 0x300 {
                            app.edit(view, (a, b), "", 0)
                        } else {
                            Ok(())
                        })
                    }
                    0x303 => Some(app.edit(view, selection(view), "", 0)),
                    0x304 | 0xc7 => {
                        app.undo(true);
                        Some(Ok(()))
                    }
                    _ => None,
                }
            },
        ));
        match result {
            Ok(Some(result)) => {
                if let Err(e) = result {
                    app.status(&e);
                }
                0
            }
            Ok(None) => DefSubclassProc(view, message, w, l),
            Err(_) => {
                app.enqueue(Event::Fatal);
                0
            }
        }
    }
}
unsafe extern "system" fn console_proc(
    view: Hwnd,
    message: u32,
    w: usize,
    l: isize,
    _id: usize,
    data: usize,
) -> isize {
    unsafe {
        if message == 0x82 {
            RemoveWindowSubclass(view, Some(console_proc), 1);
            return DefSubclassProc(view, message, w, l);
        }
        let app = &*(data as *const App);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            console_message(app, view, message, w, l)
        })) {
            Ok(result) => result,
            Err(_) => {
                app.enqueue(Event::Fatal);
                0
            }
        }
    }
}
unsafe fn console_message(app: &App, view: Hwnd, message: u32, w: usize, l: isize) -> isize {
    unsafe {
        if app.suppress.get() > 0 || !app.state.borrow().terminal_visible {
            return DefSubclassProc(view, message, w, l);
        }
        if message == 0x87 {
            return DefSubclassProc(view, message, w, l) | 0x87;
        }
        let bytes = match message {
            0x100 => match w {
                0x25 => Some(b"\x1b[D".to_vec()),
                0x26 => Some(b"\x1b[A".to_vec()),
                0x27 => Some(b"\x1b[C".to_vec()),
                0x28 => Some(b"\x1b[B".to_vec()),
                0x24 => Some(b"\x1b[H".to_vec()),
                0x23 => Some(b"\x1b[F".to_vec()),
                0x2e => Some(b"\x1b[3~".to_vec()),
                _ => None,
            },
            0x102 => {
                if w == 22 {
                    clipboard().map(String::into_bytes)
                } else if w == 3 && selection(view).0 != selection(view).1 {
                    return DefSubclassProc(view, 0x301, 0, 0);
                } else if w == 8 {
                    Some(vec![127])
                } else if (0xd800..=0xdbff).contains(&w) {
                    app.terminal_high.set(Some(w as u16));
                    return 0;
                } else if let Some(high) = app.terminal_high.take() {
                    String::from_utf16(&[high, w as u16])
                        .ok()
                        .map(String::into_bytes)
                } else {
                    char::from_u32(w as u32).map(|c| c.to_string().into_bytes())
                }
            }
            0x302 => clipboard().map(String::into_bytes),
            _ => None,
        };
        if let Some(bytes) = bytes {
            if let Err(e) = app.terminal_input(bytes) {
                app.status(&e);
            }
            0
        } else {
            DefSubclassProc(view, message, w, l)
        }
    }
}

pub fn attach_parent_console() {
    unsafe extern "system" {
        fn AttachConsole(process: u32) -> i32;
        fn GetStdHandle(kind: u32) -> Handle;
    }
    unsafe {
        if GetStdHandle((-11i32) as u32).is_null() {
            AttachConsole(u32::MAX);
        }
    }
}
pub fn run(backend: Box<dyn AgentBackend>, tasks: Option<Host>) -> Result<(), String> {
    let result = run_loop(backend, tasks, false);
    if let Err(error) = &result {
        unsafe {
            MessageBoxW(
                ptr::null_mut(),
                wide(error).as_ptr(),
                wide("Rivet could not start").as_ptr(),
                0x10,
            );
        }
    }
    result
}
pub fn run_smoke() -> Result<(), String> {
    run_loop(fixture(), None, true)
}
fn run_loop(
    backend: Box<dyn AgentBackend>,
    tasks: Option<Host>,
    smoke: bool,
) -> Result<(), String> {
    // SAFETY: all HWND/COM creation, dispatch and destruction stays on this thread.
    unsafe {
        SetProcessDpiAwarenessContext((-4isize) as Handle);
        let hr = CoInitializeEx(ptr::null_mut(), 2 | 4);
        if hr < 0 {
            return Err(format!("Windows STA initialization failed ({hr:#x})"));
        }
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _apartment = Apartment;
        if LoadLibraryExW(wide("Msftedit.dll").as_ptr(), ptr::null_mut(), 0x800).is_null() {
            return Err("Windows RichEdit text services unavailable".into());
        }
        let app = App::new(backend, tasks, smoke)?;
        let class = wide("RivetNativeWorkspace");
        let instance = GetModuleHandleW(ptr::null());
        let wc = WndClass {
            style: 3,
            proc: Some(window_proc),
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: ptr::null_mut(),
            cursor: LoadCursorW(ptr::null_mut(), 32512usize as *const u16),
            background: 6usize as Handle,
            menu: ptr::null(),
            name: class.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let window = CreateWindowExW(
            0x10000,
            class.as_ptr(),
            wide("Rivet — Agent Workspace").as_ptr(),
            0xcf0000 | 0x02000000,
            0x80000000u32 as i32,
            0x80000000u32 as i32,
            1200,
            820,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            &*app as *const _ as *mut _,
        );
        if window.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        struct WindowGuard {
            window: Hwnd,
            app: *const App,
        }
        impl Drop for WindowGuard {
            fn drop(&mut self) {
                unsafe {
                    if GetWindowLongPtrW(self.window, -21) == self.app as isize {
                        DestroyWindow(self.window);
                    }
                }
            }
        }
        let _window = WindowGuard { window, app: &*app };
        if let Err(e) = app.initialize() {
            DestroyWindow(window);
            return Err(e);
        }
        ShowWindow(window, 5);
        UpdateWindow(window);
        let mut msg = Msg::default();
        loop {
            let result = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if result <= 0 {
                if result < 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                break;
            }
            if msg.window == app.h(115)
                && msg.message == 0x100
                && msg.wparam == 13
                && GetKeyState(0x11) < 0
            {
                app.enqueue(Event::Action(116, 0));
            } else if IsDialogMessageW(window, &mut msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            app.drain();
        }
        if app.fatal.get() || msg.wparam != 0 {
            return Err("Native Windows UI failed; see diagnostics".into());
        }
        Ok(())
    }
}
struct Smoke {
    started: Instant,
    stage: u8,
    root: PathBuf,
    process: bool,
    hover: bool,
    queried: bool,
    server_stop: Arc<std::sync::atomic::AtomicBool>,
    url: String,
    browser_identity: Option<usize>,
}
impl Smoke {
    fn new() -> Result<Self, String> {
        use std::io::{Read, Write};
        let root = std::env::temp_dir().join(format!(
            "rivet-windows-smoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let url = format!(
            "http://{}/",
            listener.local_addr().map_err(|e| e.to_string())?
        );
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((mut socket, _)) = listener.accept() {
                    let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
                    let _ = socket.read(&mut [0; 4096]);
                    let body = "<!doctype html><title>Rivet browser fixture</title><h1>Native WebView2 fixture</h1>";
                    let _ = write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Ok(Self {
            started: Instant::now(),
            stage: 0,
            root,
            process: false,
            hover: false,
            queried: false,
            server_stop: stop,
            url,
            browser_identity: None,
        })
    }
}
impl Drop for Smoke {
    fn drop(&mut self) {
        self.server_stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
impl App {
    fn smoke_step(&self) {
        let (stage, root, url, expired) = {
            let smoke = self.smoke.borrow();
            let Some(s) = smoke.as_ref() else {
                return;
            };
            (
                s.stage,
                s.root.clone(),
                s.url.clone(),
                s.started.elapsed() > Duration::from_secs(90),
            )
        };
        if expired {
            let s = self.state.borrow();
            let smoke = self.smoke.borrow();
            let smoke = smoke.as_ref().unwrap();
            panic!(
                "Windows smoke stage={stage} process={} hover={} lsp={} terminal={} pending_saves={:?} status={}",
                smoke.process,
                smoke.hover,
                s.lsp_ready,
                s.terminals
                    .iter()
                    .any(|t| t.screen.render().contains("RIVET_PTY_OK")),
                s.pending_saves,
                text(self.h(900))
            );
        }
        let mut advance = true;
        match stage {
            0 => {
                self.action(111, 0).unwrap();
            }
            1 => {
                if self.state.borrow().workspace.sessions[&1].sdk_id.is_some() {
                    assert!(!self.state.borrow().permissions.is_empty());
                    self.action(119, 0).unwrap();
                    self.put(115, "Hello 🦀");
                    self.action(116, 0).unwrap();
                } else {
                    advance = false;
                }
            }
            2 => {
                if self.state.borrow().workspace.sessions[&1]
                    .transcript
                    .contains("Test streamed response")
                {
                    self.put(115, "Retained draft 🦀");
                    let chat = self.state.borrow().chat[&1];
                    set_selection(chat, 10, 15);
                    send(chat, 0xb6, 0, 12); // Scroll twelve actual native lines.
                    let scrolled = send(chat, 0xce, 0, 0);
                    assert!(scrolled > 0, "Native conversation did not scroll");
                    for _ in 1..20 {
                        self.action(102, 0).unwrap();
                    }
                    for i in 0..100 {
                        send(self.h(101), 0x186, i % 20, 0);
                        self.action(101, 1).unwrap();
                    }
                    send(self.h(101), 0x186, 0, 0);
                    self.action(101, 1).unwrap();
                    assert_eq!(text(self.h(115)), "Retained draft 🦀");
                    assert!(text(chat).contains("Test streamed response"));
                    assert_eq!(selection(chat), (10, 15));
                    assert_eq!(send(chat, 0xce, 0, 0), scrolled);
                    let (old, epoch) = {
                        let mut s = self.state.borrow_mut();
                        let session = s.workspace.sessions.get_mut(&1).unwrap();
                        let old = (session.transcript.len(), session.transcript_epoch);
                        session.append("\nTest-only background delta while scrolled\n");
                        old
                    };
                    self.update_chat(1, old, epoch);
                    assert_eq!(selection(chat), (10, 15));
                    assert_eq!(send(chat, 0xce, 0, 0), scrolled);
                    send(chat, 0xb6, 0, -scrolled);
                    self.capture("rivet-windows-agents.bmp").unwrap();
                } else {
                    advance = false;
                }
            }
            3 => {
                std::fs::write(root.join("source.rs"), "let value = 1; // 🦀\r\n").unwrap();
                self.put(110, &root.display().to_string());
                self.put(400, "source.rs");
                self.action(401, 0).unwrap();
            }
            4 => {
                let view = self.state.borrow().docs.front().map(|d| d.view);
                if let Some(view) = view {
                    let dialog_code = send(view, 0x87, 13, 0);
                    assert_eq!(dialog_code & 0x87, 0x87);
                    assert!(
                        !active_doc(&self.state.borrow())
                            .unwrap()
                            .buffer
                            .snapshot()
                            .text()
                            .contains('\r')
                    );
                    set_selection(view, 0, 0);
                    send(view, 0x102, b'Z' as usize, 0);
                    assert!(
                        active_doc(&self.state.borrow())
                            .unwrap()
                            .buffer
                            .snapshot()
                            .text()
                            .starts_with('Z')
                    );
                    self.action(407, 0).unwrap();
                    assert!(
                        !active_doc(&self.state.borrow())
                            .unwrap()
                            .buffer
                            .snapshot()
                            .text()
                            .starts_with('Z')
                    );
                    self.action(408, 0).unwrap();
                    assert!(text(view).starts_with('Z'));
                    let end = {
                        let s = self.state.borrow();
                        let d = active_doc(&s).unwrap();
                        d.buffer.byte_to_utf16(d.buffer.len()).unwrap()
                    };
                    set_selection(view, end, end);
                    send(view, 0x102, 0xd83e, 0);
                    send(view, 0x102, 0xdd80, 0);
                    assert!(
                        active_doc(&self.state.borrow())
                            .unwrap()
                            .buffer
                            .snapshot()
                            .text()
                            .ends_with('🦀')
                    );
                    self.action(406, 0).unwrap();
                    assert!(self.action(412, 0).is_err());
                    self.begin_close();
                    assert!(!self.closing.get());
                } else {
                    advance = false;
                }
            }
            5 => {
                if self.state.borrow().pending_saves.is_empty() {
                    assert!(!active_doc(&self.state.borrow()).unwrap().dirty());
                    let saved = std::fs::read_to_string(root.join("source.rs")).unwrap();
                    assert!(saved.starts_with('Z') && saved.contains("\r\n"));
                    let view = active_doc(&self.state.borrow()).unwrap().view;
                    set_selection(view, 0, 0);
                    send(view, 0x102, b'Q' as usize, 0);
                    self.action(411, 0).unwrap();
                } else {
                    advance = false;
                }
            }
            6 => {
                if self.mode.get() == 3 {
                    assert!(text(self.h(701)).contains('Q'));
                    self.action(412, 0).unwrap();
                    self.action(412, 0).unwrap();
                    assert!(!active_doc(&self.state.borrow()).unwrap().dirty());
                    self.mode.set(4);
                    self.layout();
                    self.execute(Spec {
                        program: "python".into(),
                        args: vec!["-c".into(), "print('RIVET_PROCESS_OK')".into()],
                        cwd: root.clone(),
                        timeout: Duration::from_secs(10),
                        capture_limit: 4096,
                    })
                    .unwrap();
                    self.put(500, "python");
                    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../tools/lsp_fixture.py");
                    self.put(
                        501,
                        &Value::Array(vec![fixture.to_string_lossy().to_string().into()]).encode(),
                    );
                    self.action(506, 0).unwrap();
                    self.action(506, 0).unwrap();
                    self.action(520, 0).unwrap();
                    self.put(523, "set RV=RIVET_");
                    self.action(524, 0).unwrap();
                    self.put(523, "echo PTY_WORKSPACE>rivet-pty-cwd.txt");
                    self.action(524, 0).unwrap();
                    self.put(523, "echo %RV%PTY_OK");
                    self.action(524, 0).unwrap();
                } else {
                    advance = false;
                }
            }
            7 => {
                if self.state.borrow().lsp_ready && !self.smoke.borrow().as_ref().unwrap().queried {
                    self.language(0).unwrap();
                    self.smoke.borrow_mut().as_mut().unwrap().queried = true;
                }
                let ready = {
                    let s = self.smoke.borrow();
                    let s = s.as_ref().unwrap();
                    s.process
                        && s.hover
                        && self
                            .state
                            .borrow()
                            .terminals
                            .iter()
                            .any(|t| t.screen.render().contains("RIVET_PTY_OK"))
                };
                if ready {
                    assert_eq!(
                        std::fs::read_to_string(root.join("rivet-pty-cwd.txt")).unwrap(),
                        "PTY_WORKSPACE\r\n"
                    );
                    self.show_terminal();
                    assert_eq!(self.mode.get(), 4);
                    assert!(text(self.h(702)).contains("RIVET_PTY_OK"));
                    unsafe {
                        assert_ne!(IsWindowVisible(self.h(702)), 0);
                        assert_ne!(IsWindowVisible(self.h(523)), 0);
                        for d in &self.state.borrow().docs {
                            assert_eq!(IsWindowVisible(d.view), 0);
                        }
                    }
                    self.capture("rivet-windows-tools.bmp").unwrap();
                    for _ in 0..5 {
                        self.action(300, 0).unwrap();
                    }
                    self.put(302, &url);
                    self.action(303, 0).unwrap();
                } else {
                    advance = false;
                }
            }
            8 => {
                if self.browser.borrow().title().as_deref() == Some("Rivet browser fixture") {
                    let identity = self.browser.borrow().identity().unwrap();
                    self.smoke.borrow_mut().as_mut().unwrap().browser_identity = Some(identity);
                    send(self.h(301), 0x14e, 0, 0);
                    self.action(301, 1).unwrap();
                    send(self.h(301), 0x14e, 4, 0);
                    self.action(301, 1).unwrap();
                    assert_eq!(self.browser.borrow().identity(), Some(identity));
                    assert_eq!(self.browser.borrow().ids().len(), 5);
                    assert!(
                        self.browser
                            .borrow_mut()
                            .navigate("javascript:alert(1)")
                            .is_err()
                    );
                    self.mode.set(4);
                    self.layout();
                    self.begin_close();
                } else {
                    advance = false;
                }
            }
            _ => {}
        }
        if advance {
            println!("WINDOWS_SMOKE_STAGE {stage}");
            if let Some(s) = self.smoke.borrow_mut().as_mut() {
                s.stage += 1;
            }
        }
    }
    fn capture(&self, path: &str) -> Result<(), String> {
        use std::io::Write;
        unsafe {
            // Flush invalidation after ShowWindow/WM_SETTEXT before collecting visual
            // evidence. A synchronous test action can otherwise capture stale pixels.
            RedrawWindow(
                self.window.get(),
                ptr::null(),
                ptr::null_mut(),
                1 | 0x80 | 0x100,
            );
            let mut r = Rect::default();
            GetClientRect(self.window.get(), &mut r);
            let (w, h) = (r.right, r.bottom);
            if w <= 0 || h <= 0 || i64::from(w) * i64::from(h) > 16 * 1024 * 1024 {
                return Err("Screenshot size outside budget".into());
            }
            let screen = GetDC(self.window.get());
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            if dc.is_null() || bitmap.is_null() {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                if !dc.is_null() {
                    DeleteDC(dc);
                }
                ReleaseDC(self.window.get(), screen);
                return Err("Cannot allocate native screenshot".into());
            }
            let previous = SelectObject(dc, bitmap);
            let captured = PrintWindow(self.window.get(), dc, 1);
            SelectObject(dc, previous);
            let mut info = BitmapInfo {
                header: BitmapInfoHeader {
                    size: 40,
                    width: w,
                    height: -h,
                    planes: 1,
                    bit_count: 32,
                    compression: 0,
                    image_size: 0,
                    x_pixels: 0,
                    y_pixels: 0,
                    colors: 0,
                    important: 0,
                },
                colors: [0],
            };
            let mut pixels = vec![0; w as usize * h as usize * 4];
            let rows = GetDIBits(
                dc,
                bitmap,
                0,
                h as u32,
                pixels.as_mut_ptr().cast(),
                &mut info,
                0,
            );
            DeleteObject(bitmap);
            DeleteDC(dc);
            ReleaseDC(self.window.get(), screen);
            if captured == 0 || rows != h {
                return Err("Native screenshot failed".into());
            }
            let mut header = vec![0u8; 54];
            header[..2].copy_from_slice(b"BM");
            header[2..6].copy_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
            header[10..14].copy_from_slice(&54u32.to_le_bytes());
            header[14..18].copy_from_slice(&40u32.to_le_bytes());
            header[18..22].copy_from_slice(&w.to_le_bytes());
            header[22..26].copy_from_slice(&(-h).to_le_bytes());
            header[26..28].copy_from_slice(&1u16.to_le_bytes());
            header[28..30].copy_from_slice(&32u16.to_le_bytes());
            let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
            file.write_all(&header)
                .and_then(|_| file.write_all(&pixels))
                .map_err(|e| e.to_string())
        }
    }
}
fn fixture() -> Box<dyn AgentBackend> {
    struct Fixture(RefCell<VecDeque<AgentEvent>>);
    impl AgentBackend for Fixture {
        fn shutdown(&self) {
            self.0.borrow_mut().push_back(AgentEvent::Stopped);
        }
        fn submit(&self, cmd: AgentCommand) -> Result<(), String> {
            let mut q = self.0.borrow_mut();
            match cmd {
                AgentCommand::Open { key, .. } => {
                    q.push_back(AgentEvent::Ready {
                        key,
                        id: "Windows-UI-test-session".into(),
                    });
                    q.push_back(AgentEvent::Permission {
                        key,
                        request: 1,
                        description: "Explicit test-only permission".into(),
                    });
                }
                AgentCommand::Models => q.push_back(AgentEvent::Models(vec!["test-model".into()])),
                AgentCommand::Send { key, .. } => {
                    q.push_back(AgentEvent::Delta {
                        key,
                        text: format!(
                            "Test streamed response 🦀\n{}",
                            (1..=80)
                                .map(|i| format!("Test-only activity line {i}\n"))
                                .collect::<String>()
                        ),
                    });
                    q.push_back(AgentEvent::Finished { key });
                }
                _ => {}
            }
            Ok(())
        }
        fn try_event(&mut self) -> Option<AgentEvent> {
            self.0.borrow_mut().pop_front()
        }
    }
    Box::new(Fixture(RefCell::new(VecDeque::new())))
}
