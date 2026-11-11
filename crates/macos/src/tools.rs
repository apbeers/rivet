//! Native development tools; jobs execute on workers and LSP startup is asynchronous.
use super::{editor::EditorView, terminal::Terminal};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::*;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use rivet_core::{
    json::{self, Value},
    lsp,
    process::{self, Spec, Worker},
    terminal::Screen,
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::VecDeque,
    path::PathBuf,
    sync::mpsc::Receiver,
    time::Duration,
};
struct Controls {
    program: Retained<NSTextField>,
    args: Retained<NSTextField>,
    run: Retained<NSButton>,
    cancel: Retained<NSButton>,
    git_status: Retained<NSButton>,
    git_diff: Retained<NSButton>,
    lsp_start: Retained<NSButton>,
    requests: Vec<Retained<NSButton>>,
    terminal_new: Retained<NSButton>,
    terminal_close: Retained<NSButton>,
    terminal_tabs: Retained<NSPopUpButton>,
    input: Retained<NSTextField>,
    send: Retained<NSButton>,
    interrupt: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    output: Retained<super::console::Console>,
}
struct Term {
    terminal: Terminal,
    screen: Screen,
}
pub(crate) struct State {
    controls: OnceCell<Controls>,
    workspace: Retained<NSTextField>,
    processes: Worker,
    next: Cell<u64>,
    pending: RefCell<Option<(Spec, bool)>>,
    jobs: RefCell<Vec<u64>>,
    editor: RefCell<Option<Retained<EditorView>>>,
    path: RefCell<Option<PathBuf>>,
    lsp: RefCell<Option<lsp::Connection>>,
    lsp_pending: RefCell<Option<Receiver<Result<lsp::Connection, String>>>>,
    initialized: Cell<bool>,
    lsp_opened: RefCell<Option<(PathBuf, u64)>>,
    request_id: Cell<u32>,
    terminals: RefCell<Vec<Term>>,
    active_terminal: Cell<usize>,
    show_terminal: Cell<bool>,
    navigation: RefCell<VecDeque<(PathBuf, u32, u32)>>,
    requests: RefCell<std::collections::BTreeMap<u32, (usize, PathBuf, u64)>>,
    legend: RefCell<Vec<String>>,
    closing: Cell<bool>,
    retired: RefCell<Vec<Terminal>>,
    smoke_process: Cell<bool>,
    smoke_hover: Cell<bool>,
    smoke_requested: Cell<bool>,
}
define_class!(
 // SAFETY: native controls are main-thread-only; all subprocess execution uses workers.
 #[unsafe(super(NSView))] #[thread_kind=MainThreadOnly] #[name="RivetToolsPane"] #[ivars=State]
 pub(crate) struct Tools;
 impl Tools{
  #[unsafe(method(isFlipped))] fn flipped(&self)->bool{true}
  #[unsafe(method(resizeSubviewsWithOldSize:))] fn resized(&self,_old:NSSize){self.layout_controls();}
  #[unsafe(method(runTool:))] fn run_tool(&self,_sender:&NSButton){self.run_requested();}
  #[unsafe(method(cancelTool:))] fn cancel_tool(&self,_sender:&NSButton){for &id in self.ivars().jobs.borrow().iter(){self.ivars().processes.cancel(id);}self.print("Cancellation requested");}
  #[unsafe(method(gitStatus:))] fn git_status(&self,_sender:&NSButton){self.execute(process::git(self.root(),vec!["status".into(),"--short".into(),"--branch".into()]));}
  #[unsafe(method(gitDiff:))] fn git_diff(&self,_sender:&NSButton){self.execute(process::git(self.root(),vec!["diff".into(),"--no-ext-diff".into(),"--no-textconv".into(),"HEAD".into(),"--".into()]));}
  #[unsafe(method(startLanguageServer:))] fn start_lsp(&self,_sender:&NSButton){self.start_language_server();}
  #[unsafe(method(languageRequest:))] fn language_action(&self,sender:&NSButton){self.language_request(sender.tag() as usize);}
  #[unsafe(method(newTerminal:))] fn new_terminal(&self,_sender:&NSButton){self.open_terminal();}
  #[unsafe(method(closeTerminal:))] fn close_terminal(&self,_sender:&NSButton){let i=self.ivars().active_terminal.get();if i<self.ivars().terminals.borrow().len(){let term=self.ivars().terminals.borrow_mut().remove(i);term.terminal.stop();self.ivars().retired.borrow_mut().push(term.terminal);}self.update_terminal_tabs();}
  #[unsafe(method(selectTerminal:))] fn select_terminal(&self,sender:&NSPopUpButton){self.ivars().active_terminal.set(sender.indexOfSelectedItem().max(0) as usize);self.show_active_terminal();}
  #[unsafe(method(sendTerminal:))] fn send_terminal(&self,_sender:&NSButton){let mut text=self.c().input.stringValue().to_string().into_bytes();text.push(b'\n');self.terminal_write(text);self.c().input.setStringValue(&NSString::new());}
  #[unsafe(method(interruptTerminal:))] fn interrupt_terminal(&self,_sender:&NSButton){self.terminal_write(vec![3]);}
 }
);
impl Tools {
    pub fn new(
        mtm: MainThreadMarker,
        workspace: Retained<NSTextField>,
    ) -> Result<Retained<Self>, String> {
        let this = Self::alloc(mtm).set_ivars(State {
            controls: OnceCell::new(),
            workspace,
            processes: Worker::spawn().map_err(|e| e.to_string())?,
            next: Cell::new(1),
            pending: RefCell::new(None),
            jobs: RefCell::new(Vec::new()),
            editor: RefCell::new(None),
            path: RefCell::new(None),
            lsp: RefCell::new(None),
            lsp_pending: RefCell::new(None),
            initialized: Cell::new(false),
            lsp_opened: RefCell::new(None),
            request_id: Cell::new(10),
            terminals: RefCell::new(Vec::new()),
            active_terminal: Cell::new(0),
            show_terminal: Cell::new(false),
            navigation: RefCell::new(VecDeque::new()),
            requests: RefCell::new(std::collections::BTreeMap::new()),
            legend: RefCell::new(Vec::new()),
            closing: Cell::new(false),
            retired: RefCell::new(Vec::new()),
            smoke_process: Cell::new(false),
            smoke_hover: Cell::new(false),
            smoke_requested: Cell::new(false),
        });
        // SAFETY: valid NSView initializer after ivars are installed.
        let this: Retained<Self> = unsafe {
            msg_send![super(this),initWithFrame:NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(900.0,600.0))]
        };
        this.create_controls();
        Ok(this)
    }
    fn c(&self) -> &Controls {
        self.ivars().controls.get().unwrap()
    }
    fn root(&self) -> PathBuf {
        if let Some(w) = self.window() {
            w.makeFirstResponder(None);
        }
        PathBuf::from(self.ivars().workspace.stringValue().to_string())
    }
    fn print(&self, text: &str) {
        self.ivars().show_terminal.set(false);
        self.c().output.bind(None);
        let limited: String = text.chars().take(65_536).collect();
        self.c().output.setString(&NSString::from_str(&limited));
    }
    fn button(&self, title: &str, action: objc2::runtime::Sel) -> Retained<NSButton> {
        unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        }
    }
    fn create_controls(&self) {
        let mtm = self.mtm();
        let program = NSTextField::new(mtm);
        program.setStringValue(&NSString::from_str("cargo"));
        let args = NSTextField::new(mtm);
        args.setStringValue(&NSString::from_str("[\"test\"]"));
        let run = self.button("Run / Confirm", sel!(runTool:));
        let cancel = self.button("Cancel jobs", sel!(cancelTool:));
        let git_status = self.button("Git status", sel!(gitStatus:));
        let git_diff = self.button("Git diff HEAD", sel!(gitDiff:));
        let lsp_start = self.button("Start LSP / Confirm", sel!(startLanguageServer:));
        let mut requests = Vec::new();
        for (i, title) in [
            "Hover",
            "Definition",
            "References",
            "Completion",
            "Semantic tokens",
        ]
        .iter()
        .enumerate()
        {
            let b = self.button(title, sel!(languageRequest:));
            b.setTag(i as isize);
            self.addSubview(&b);
            requests.push(b);
        }
        let terminal_new = self.button("New PTY", sel!(newTerminal:));
        let terminal_close = self.button("Close PTY", sel!(closeTerminal:));
        let terminal_tabs = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            NSRect::new(Default::default(), NSSize::new(150.0, 30.0)),
            false,
        );
        unsafe {
            terminal_tabs.setTarget(Some(self));
            terminal_tabs.setAction(Some(sel!(selectTerminal:)));
        }
        let input = NSTextField::new(mtm);
        input.setPlaceholderString(Some(&NSString::from_str(
            "Optional line input; focus console for direct PTY input, including no-echo prompts",
        )));
        let send = self.button("Send line", sel!(sendTerminal:));
        let interrupt = self.button("Ctrl-C", sel!(interruptTerminal:));
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        let output = super::console::Console::new(mtm);
        output.setEditable(false);
        output.setVerticallyResizable(true);
        output.setHorizontallyResizable(false);
        output.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        unsafe {
            if let Some(c) = output.textContainer() {
                c.setWidthTracksTextView(true);
            }
        }
        scroll.setDocumentView(Some(&output));
        for v in [
            &*program as &NSView,
            &*args,
            &*run,
            &*cancel,
            &*git_status,
            &*git_diff,
            &*lsp_start,
            &*terminal_new,
            &*terminal_close,
            &*terminal_tabs,
            &*input,
            &*send,
            &*interrupt,
            &*scroll,
        ] {
            self.addSubview(v);
        }
        let _ = self.ivars().controls.set(Controls {
            program,
            args,
            run,
            cancel,
            git_status,
            git_diff,
            lsp_start,
            requests,
            terminal_new,
            terminal_close,
            terminal_tabs,
            input,
            send,
            interrupt,
            scroll,
            output,
        });
        self.layout_controls();
        self.print("Run programs with a JSON argument array. Confirm the exact command before execution. For LSP, enter the installed server executable and its arguments, then Start LSP twice. PTY sessions use your shell; the console supports direct keyboard input with a constrained escape-sequence renderer.");
    }
    fn layout_controls(&self) {
        let Some(c) = self.ivars().controls.get() else {
            return;
        };
        let w = self.bounds().size.width;
        let h = self.bounds().size.height;
        let r = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        c.program.setFrame(r(0.0, 0.0, 180.0, 28.0));
        c.args.setFrame(r(185.0, 0.0, (w - 420.0).max(50.0), 28.0));
        c.run.setFrame(r(w - 230.0, 0.0, 120.0, 30.0));
        c.cancel.setFrame(r(w - 105.0, 0.0, 105.0, 30.0));
        c.git_status.setFrame(r(0.0, 38.0, 110.0, 30.0));
        c.git_diff.setFrame(r(115.0, 38.0, 130.0, 30.0));
        c.lsp_start.setFrame(r(250.0, 38.0, 180.0, 30.0));
        for (i, b) in c.requests.iter().enumerate() {
            b.setFrame(r(i as f64 * 130.0, 76.0, 125.0, 30.0));
        }
        c.terminal_new.setFrame(r(0.0, 114.0, 100.0, 30.0));
        c.terminal_close.setFrame(r(105.0, 114.0, 100.0, 30.0));
        c.terminal_tabs.setFrame(r(210.0, 114.0, 150.0, 30.0));
        c.input
            .setFrame(r(0.0, h - 40.0, (w - 200.0).max(50.0), 30.0));
        c.send.setFrame(r(w - 195.0, h - 40.0, 100.0, 30.0));
        c.interrupt.setFrame(r(w - 90.0, h - 40.0, 90.0, 30.0));
        c.scroll.setFrame(r(0.0, 154.0, w, (h - 204.0).max(50.0)));
    }
    fn spec(&self) -> Result<Spec, String> {
        let root = self.root();
        let text = self.c().args.stringValue().to_string();
        if text.len() > 16 * 1024 {
            return Err("Argument budget exceeded (16 KiB)".into());
        }
        let args = json::parse(&text)?;
        let Some(args) = args.array() else {
            return Err("Arguments must be a JSON array of strings".into());
        };
        if args.len() > 256 {
            return Err("Argument budget exceeded (256 arguments)".into());
        }
        let args = args
            .iter()
            .map(|v| {
                v.str()
                    .map(String::from)
                    .ok_or("Every argument must be a string")
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Spec {
            program: self.c().program.stringValue().to_string(),
            args,
            cwd: root,
            timeout: Duration::from_secs(300),
            capture_limit: 1024 * 1024,
        })
    }
    fn confirmed(&self, spec: &Spec, language: bool) -> bool {
        let confirmed = self.ivars().pending.borrow().as_ref() == Some(&(spec.clone(), language));
        if !confirmed {
            *self.ivars().pending.borrow_mut() = Some((spec.clone(), language));
            self.print(&format!("Confirm this exact process by pressing the same action again:\nExecutable: {}\nArguments: {:?}\nWorkspace: {}\n\nRuns with your user permissions; this is not an OS sandbox.",spec.program,spec.args,spec.cwd.display()));
            false
        } else {
            self.ivars().pending.borrow_mut().take();
            true
        }
    }
    fn run_requested(&self) {
        match self.spec() {
            Ok(s) if self.confirmed(&s, false) => self.execute(s),
            Ok(_) => {}
            Err(e) => self.print(&e),
        }
    }
    fn execute(&self, spec: Spec) {
        let id = self.ivars().next.get();
        self.ivars().next.set(id + 1);
        match self.ivars().processes.submit(id, spec) {
            Ok(()) => {
                self.ivars().jobs.borrow_mut().push(id);
                self.print(&format!("Job {id} running in background"));
            }
            Err(e) => self.print(&e),
        }
    }
    fn start_language_server(&self) {
        let spec = match self.spec() {
            Ok(s) => s,
            Err(e) => {
                self.print(&e);
                return;
            }
        };
        if !self.confirmed(&spec, true) {
            return;
        }
        if self.ivars().lsp_pending.borrow().is_some() {
            self.print("Language server startup already pending");
            return;
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let result = std::thread::Builder::new()
            .name("rivet-lsp-start".into())
            .spawn(move || {
                let _ = tx.send(lsp::Connection::spawn(&spec.program, &spec.args, &spec.cwd));
            });
        match result {
            Ok(_) => {
                *self.ivars().lsp_pending.borrow_mut() = Some(rx);
                self.print("Language server starting in background");
            }
            Err(e) => self.print(&e.to_string()),
        }
    }
    pub fn set_document(&self, path: Option<PathBuf>, editor: Option<Retained<EditorView>>) {
        *self.ivars().path.borrow_mut() = path;
        *self.ivars().editor.borrow_mut() = editor;
    }
    fn sync_document(&self) -> Result<(), String> {
        let current = self.ivars().path.borrow();
        let path = current.as_ref().ok_or("Open a source document first")?;
        let editors = self.ivars().editor.borrow();
        let editor = editors.as_ref().ok_or("No active editor")?;
        let identity = (path.clone(), editor.revision());
        if self.ivars().lsp_opened.borrow().as_ref() == Some(&identity) {
            return Ok(());
        }
        if editor.byte_len() > 256 * 1024 {
            return Err("LSP full-document synchronization currently limited to 256 KiB; editor remains available".into());
        }
        let text = editor.snapshot().text();
        let connection = self.ivars().lsp.borrow();
        let server = connection.as_ref().ok_or("Start a language server first")?;
        if let Some((old, _)) = self.ivars().lsp_opened.borrow().as_ref() {
            server.send(lsp::notification(
                "textDocument/didClose",
                lsp::object([(
                    "textDocument",
                    lsp::object([("uri", lsp::file_uri(old)?.into())]),
                )]),
            ))?;
        }
        server.send(lsp::did_open(path, text)?)?;
        *self.ivars().lsp_opened.borrow_mut() = Some(identity);
        Ok(())
    }
    fn language_request(&self, kind: usize) {
        if !self.ivars().initialized.get() {
            self.print("Language server initialization is not complete");
            return;
        }
        if let Err(e) = self.sync_document() {
            self.print(&e);
            return;
        }
        let path = self.ivars().path.borrow().clone().unwrap();
        let selected = self
            .ivars()
            .editor
            .borrow()
            .as_ref()
            .unwrap()
            .selectedRange();
        let (method, params) = if kind == 4 {
            (
                "textDocument/semanticTokens/full",
                lsp::object([(
                    "textDocument",
                    lsp::object([("uri", lsp::file_uri(&path).unwrap_or_default().into())]),
                )]),
            )
        } else {
            let position = self
                .ivars()
                .editor
                .borrow()
                .as_ref()
                .unwrap()
                .position(selected.location)
                .unwrap_or((0, 0));
            let mut params = lsp::object([
                (
                    "textDocument",
                    lsp::object([("uri", lsp::file_uri(&path).unwrap_or_default().into())]),
                ),
                (
                    "position",
                    lsp::object([
                        ("line", (position.0 as u32).into()),
                        ("character", (position.1 as u32).into()),
                    ]),
                ),
            ]);
            if kind == 2
                && let Value::Object(m) = &mut params
            {
                m.insert(
                    "context".into(),
                    lsp::object([("includeDeclaration", Value::Bool(true))]),
                );
            }
            (
                [
                    "textDocument/hover",
                    "textDocument/definition",
                    "textDocument/references",
                    "textDocument/completion",
                ][kind.min(3)],
                params,
            )
        };
        let id = self.ivars().request_id.get();
        self.ivars().request_id.set(id + 1);
        if let Some(editor) = self.ivars().editor.borrow().as_ref() {
            self.ivars()
                .requests
                .borrow_mut()
                .insert(id, (kind, path.clone(), editor.revision()));
        }
        if let Some(server) = self.ivars().lsp.borrow().as_ref() {
            if let Err(e) = server.send(lsp::request(id, method, params)) {
                self.print(&e);
            } else {
                self.print("Language request sent");
            }
        }
    }
    pub fn shutdown(&self) {
        self.ivars().closing.set(true);
        self.ivars().processes.shutdown();
        if let Some(server) = self.ivars().lsp.borrow().as_ref() {
            server.shutdown();
        }
        for term in self.ivars().terminals.borrow().iter() {
            term.terminal.stop();
        }
    }
    pub fn is_finished(&self) -> bool {
        self.ivars().processes.is_finished()
            && self.ivars().lsp_pending.borrow().is_none()
            && self
                .ivars()
                .lsp
                .borrow()
                .as_ref()
                .is_none_or(lsp::Connection::is_finished)
            && self
                .ivars()
                .terminals
                .borrow()
                .iter()
                .all(|t| t.terminal.is_finished())
            && self.ivars().retired.borrow().is_empty()
    }
    pub fn tick(&self) {
        self.ivars()
            .retired
            .borrow_mut()
            .retain(|t| !t.is_finished());
        for _ in 0..4 {
            let Some(done) = self.ivars().processes.try_event() else {
                break;
            };
            self.ivars().jobs.borrow_mut().retain(|id| *id != done.id);
            if done
                .result
                .as_ref()
                .is_ok_and(|o| o.success() && o.stdout.contains("RIVET_PROCESS_OK"))
            {
                self.ivars().smoke_process.set(true);
            }
            self.print(&match done.result {
                Ok(o) => o.display(),
                Err(e) => e,
            });
        }
        let started = self
            .ivars()
            .lsp_pending
            .borrow()
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());
        if let Some(result) = started {
            self.ivars().lsp_pending.borrow_mut().take();
            match result {
                Ok(server) if self.ivars().closing.get() => {
                    server.shutdown();
                    *self.ivars().lsp.borrow_mut() = Some(server);
                }
                Ok(server) => match lsp::initialize(&self.root()).and_then(|v| server.send(v)) {
                    Ok(()) => {
                        *self.ivars().lsp.borrow_mut() = Some(server);
                        self.ivars().initialized.set(false);
                        self.ivars().lsp_opened.borrow_mut().take();
                        self.print("Language server initializing");
                    }
                    Err(e) => self.print(&e),
                },
                Err(e) => self.print(&e),
            }
        }
        for _ in 0..4 {
            let event = self
                .ivars()
                .lsp
                .borrow()
                .as_ref()
                .and_then(|s| s.try_event());
            let Some(event) = event else {
                break;
            };
            match event {
                Err(e) => self.print(&format!("Language server disconnected: {e}")),
                Ok(message) => {
                    if format_language(&message).contains("Fixture hover") {
                        self.ivars().smoke_hover.set(true);
                    }
                    if message.get("id").and_then(Value::integer) == Some(1) {
                        if message.get("error").is_some() {
                            self.print("Language server initialization failed");
                        } else {
                            let capabilities =
                                message.get("result").and_then(|v| v.get("capabilities"));
                            if capabilities
                                .and_then(|c| c.get("positionEncoding"))
                                .and_then(Value::str)
                                .is_some_and(|e| e != "utf-16")
                            {
                                self.print(
                                    "Language server selected an unsupported position encoding",
                                );
                                continue;
                            }
                            *self.ivars().legend.borrow_mut() = capabilities
                                .and_then(|c| c.get("semanticTokensProvider"))
                                .and_then(|p| p.get("legend"))
                                .and_then(|l| l.get("tokenTypes"))
                                .and_then(Value::array)
                                .unwrap_or(&[])
                                .iter()
                                .filter_map(Value::str)
                                .map(String::from)
                                .collect();
                            self.ivars().initialized.set(true);
                            if let Some(server) = self.ivars().lsp.borrow().as_ref() {
                                let _ = server.send(lsp::notification(
                                    "initialized",
                                    Value::object(std::iter::empty::<(String, Value)>()),
                                ));
                            }
                            self.print("Language server ready. Requests use the selected document and caret.");
                        }
                    } else {
                        self.consume_language(&message);
                        self.print(&format_language(&message));
                    }
                }
            }
        }
        let mut changed = false;
        let active = self.ivars().active_terminal.get();
        for (i, term) in self.ivars().terminals.borrow_mut().iter_mut().enumerate() {
            for _ in 0..2 {
                let Some(bytes) = term.terminal.try_output() else {
                    break;
                };
                term.screen.feed(&bytes);
                if i == active {
                    changed = true;
                }
            }
        }
        if changed
            && self.ivars().show_terminal.get()
            && let Some(term) = self.ivars().terminals.borrow().get(active)
        {
            self.c()
                .output
                .setString(&NSString::from_str(&term.screen.render()));
        }
    }
    fn consume_language(&self, message: &Value) {
        let Some(id) = message
            .get("id")
            .and_then(Value::integer)
            .and_then(|id| u32::try_from(id).ok())
        else {
            return;
        };
        let Some((kind, path, revision)) = self.ivars().requests.borrow_mut().remove(&id) else {
            return;
        };
        let editors = self.ivars().editor.borrow();
        let Some(editor) = editors.as_ref() else {
            return;
        };
        if self.ivars().path.borrow().as_ref() != Some(&path) || editor.revision() != revision {
            return;
        }
        let Some(result) = message.get("result") else {
            return;
        };
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
                self.ivars().navigation.borrow_mut().push_back((
                    path,
                    start.get("line").and_then(Value::integer).unwrap_or(0) as u32,
                    start.get("character").and_then(Value::integer).unwrap_or(0) as u32,
                ));
            }
        } else if kind == 4 {
            let data: Vec<u32> = result
                .get("data")
                .and_then(Value::array)
                .unwrap_or(&[])
                .iter()
                .take(2560)
                .filter_map(Value::integer)
                .filter_map(|n| u32::try_from(n).ok())
                .collect();
            editor.apply_semantic(&data, &self.ivars().legend.borrow());
        }
    }
    fn open_terminal(&self) {
        if self.ivars().terminals.borrow().len() >= 4 {
            self.print("Close a terminal before opening another (limit 4)");
            return;
        }
        match Terminal::spawn(&self.root()) {
            Ok(terminal) => {
                self.ivars().terminals.borrow_mut().push(Term {
                    terminal,
                    screen: Screen::new(100, 40),
                });
                self.update_terminal_tabs();
                self.ivars().show_terminal.set(true);
            }
            Err(e) => self.print(&e),
        }
    }
    fn update_terminal_tabs(&self) {
        let c = self.c();
        c.terminal_tabs.removeAllItems();
        let len = self.ivars().terminals.borrow().len();
        for id in 0..len {
            c.terminal_tabs
                .addItemWithTitle(&NSString::from_str(&format!("PTY {}", id + 1)));
        }
        self.ivars().active_terminal.set(len.saturating_sub(1));
        self.show_active_terminal();
        if len > 0 {
            c.terminal_tabs.selectItemAtIndex(len as isize - 1);
        }
    }
    fn show_active_terminal(&self) {
        let terms = self.ivars().terminals.borrow();
        if let Some(term) = terms.get(self.ivars().active_terminal.get()) {
            self.c().output.bind(Some(term.terminal.sender()));
            self.c()
                .output
                .setString(&NSString::from_str(&term.screen.render()));
            self.ivars().show_terminal.set(true);
            if let Some(window) = self.window() {
                window.makeFirstResponder(Some(&self.c().output));
            }
        } else {
            self.c().output.bind(None);
        }
    }
    fn terminal_write(&self, bytes: Vec<u8>) {
        if let Some(term) = self
            .ivars()
            .terminals
            .borrow()
            .get(self.ivars().active_terminal.get())
        {
            match term.terminal.write(bytes) {
                Ok(()) => {
                    self.ivars().show_terminal.set(true);
                    self.c().output.bind(Some(term.terminal.sender()));
                }
                Err(e) => self.print(&e),
            }
        } else {
            self.print("Create a PTY session first");
        }
    }
    pub fn smoke_start(&self) {
        self.execute(Spec {
            program: "python3".into(),
            args: vec!["-c".into(), "print('RIVET_PROCESS_OK')".into()],
            cwd: self.root(),
            timeout: Duration::from_secs(5),
            capture_limit: 4096,
        });
        self.c()
            .program
            .setStringValue(&NSString::from_str("python3"));
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/lsp_fixture.py");
        self.c().args.setStringValue(&NSString::from_str(
            &Value::Array(vec![fixture.to_string_lossy().to_string().into()]).encode(),
        ));
        self.start_language_server();
        self.start_language_server();
        self.open_terminal();
        self.terminal_write(
            b"printf '\\122\\111\\126\\105\\124\\137\\120\\124\\131\\137\\117\\113\\n'\r".to_vec(),
        );
    }
    pub fn smoke_complete(&self) -> bool {
        if self.ivars().initialized.get() && !self.ivars().smoke_requested.replace(true) {
            self.language_request(0);
        }
        self.ivars().smoke_process.get()
            && self.ivars().smoke_hover.get()
            && self
                .ivars()
                .terminals
                .borrow()
                .iter()
                .any(|t| t.screen.render().contains("RIVET_PTY_OK"))
    }
    pub fn try_navigation(&self) -> Option<(PathBuf, u32, u32)> {
        self.ivars().navigation.borrow_mut().pop_front()
    }
}
fn format_language(v: &Value) -> String {
    fn text(v: &Value, out: &mut String, depth: usize) {
        if depth > 12 || out.len() > 60000 {
            return;
        }
        match v {
            Value::String(s) => {
                out.push_str(s);
                out.push('\n');
            }
            Value::Array(a) => {
                for v in a.iter().take(128) {
                    text(v, out, depth + 1)
                }
            }
            Value::Object(m) => {
                for key in ["message", "label", "value", "uri", "targetUri"] {
                    if let Some(v) = m.get(key) {
                        text(v, out, depth + 1)
                    }
                }
                for key in [
                    "result",
                    "error",
                    "params",
                    "contents",
                    "diagnostics",
                    "items",
                ] {
                    if let Some(v) = m.get(key) {
                        text(v, out, depth + 1)
                    }
                }
            }
            _ => {}
        }
    }
    let mut out = String::new();
    text(v, &mut out, 0);
    if out.is_empty() {
        "Language response received (no displayable text)".into()
    } else {
        out
    }
}
