//! Native production workspace. The separate appkit module retains the mock benchmark.
use super::{browser::Browsers, documents::Documents};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained, runtime::ProtocolObject, sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    NSAttributedString, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};
use rivet_core::AgentBackend;
use rivet_core::documents::{FileCommand, FileEvent, FileWorker};
use rivet_core::{AgentCommand, AgentConfig, AgentEvent, LiveWorkspace};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(1.0), h.max(1.0)))
}
struct Controls {
    sessions: Retained<NSPopUpButton>,
    saved: Retained<NSPopUpButton>,
    model: Retained<NSPopUpButton>,
    new: Retained<NSButton>,
    connect: Retained<NSButton>,
    resume: Retained<NSButton>,
    send: Retained<NSButton>,
    cancel: Retained<NSButton>,
    allow: Retained<NSButton>,
    deny: Retained<NSButton>,
    workspace: Retained<NSTextField>,
    instructions: Retained<NSTextField>,
    draft: Retained<NSTextField>,
    status: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    permission_scroll: Retained<NSScrollView>,
    permission: Retained<NSTextView>,
    modes: Vec<Retained<NSButton>>,
    browser_tabs: Retained<NSPopUpButton>,
    browser_new: Retained<NSButton>,
    address: Retained<NSTextField>,
    go: Retained<NSButton>,
    back: Retained<NSButton>,
    forward: Retained<NSButton>,
    reload: Retained<NSButton>,
    files: Retained<NSPopUpButton>,
    refresh_files: Retained<NSButton>,
    parent: Retained<NSButton>,
    cached: Retained<NSPopUpButton>,
    path: Retained<NSTextField>,
    open_path: Retained<NSButton>,
}
struct State {
    workspace: RefCell<LiveWorkspace>,
    host: RefCell<Box<dyn AgentBackend>>,
    controls: OnceCell<Controls>,
    views: RefCell<BTreeMap<u64, Retained<NSTextView>>>,
    permissions: RefCell<VecDeque<(u64, u64, String)>>,
    timer: OnceCell<Retained<NSTimer>>,
    smoke_tick: Cell<Option<u16>>,
    closing: Cell<bool>,
    mode: Cell<u8>,
    browsers: RefCell<Browsers>,
    documents: RefCell<Documents>,
    files_worker: FileWorker,
    entries: RefCell<Vec<(PathBuf, bool)>>,
    directory: RefCell<PathBuf>,
}
define_class!(
    // SAFETY: native view state remains on the main thread; SDK work uses AgentHost.
    #[unsafe(super(NSView))] #[thread_kind = MainThreadOnly] #[name="RivetLiveWorkspace"] #[ivars=State]
    struct Workspace;
    impl Workspace {
        #[unsafe(method(isFlipped))] fn flipped(&self)->bool{true}
        #[unsafe(method(resizeSubviewsWithOldSize:))] fn resized(&self,_old:NSSize){self.layout_controls();}
        #[unsafe(method(newSession:))] fn new_session(&self,_sender:&NSButton){
            if self.ivars().workspace.borrow().sessions.len()>=20 {self.status("Session budget reached (20)");return;}
            self.save_draft();let id=self.ivars().workspace.borrow_mut().add();let c=self.controls();
            c.sessions.addItemWithTitle(&NSString::from_str(&format!("Session {id}"))); c.sessions.selectItemAtIndex(id as isize-1);
            self.show_active();
        }
        #[unsafe(method(selectSession:))] fn select_session(&self,sender:&NSPopUpButton){self.save_draft();self.ivars().workspace.borrow_mut().active=sender.indexOfSelectedItem() as u64+1;self.show_active();}
        #[unsafe(method(connectSession:))] fn connect_session(&self,_sender:&NSButton){self.connect(None);}
        #[unsafe(method(resumeSession:))] fn resume_session(&self,_sender:&NSButton){let c=self.controls();if let Some(id)=c.saved.titleOfSelectedItem(){self.connect(Some(id.to_string()));}}
        #[unsafe(method(sendPrompt:))] fn send_prompt(&self,_sender:&NSButton){
            self.save_draft(); let mut state=self.ivars().workspace.borrow_mut();let key=state.active;let s=state.sessions.get_mut(&key).unwrap();
            if s.sdk_id.is_none(){drop(state);self.status("Connect this session before sending");return;}
            if s.busy{drop(state);self.status("A turn is already running");return;}
            let prompt=s.draft.clone();if prompt.trim().is_empty(){return;} let old=s.transcript.len();
            if let Err(e)=self.ivars().host.borrow().submit(AgentCommand::Send{key,prompt:prompt.clone()}){drop(state);self.status(&e);return;}
            s.busy=true;s.append(&format!("\nYou: {prompt}\n\n"));s.draft.clear();drop(state);self.update_view(key,old);self.controls().draft.setStringValue(&NSString::new());self.status("Agent working; review tool permissions below");
        }
        #[unsafe(method(cancelTurn:))] fn cancel_turn(&self,_sender:&NSButton){let key=self.ivars().workspace.borrow().active;self.submit(AgentCommand::Cancel(key));}
        #[unsafe(method(allowOnce:))] fn allow_once(&self,_sender:&NSButton){self.decide(true);}
        #[unsafe(method(denyRequest:))] fn deny_request(&self,_sender:&NSButton){self.decide(false);}
        #[unsafe(method(showMode:))] fn show_mode(&self,sender:&NSButton){self.ivars().mode.set(sender.tag() as u8);self.apply_mode();}
        #[unsafe(method(newBrowser:))] fn new_browser(&self,_sender:&NSButton){
            let frame=self.content_frame();let id=self.ivars().browsers.borrow_mut().create(self,frame,self.mtm());let ids=self.ivars().browsers.borrow().ids();let c=self.controls();c.browser_tabs.removeAllItems();for key in ids{c.browser_tabs.addItemWithTitle(&NSString::from_str(&key.to_string()));}c.browser_tabs.selectItemWithTitle(&NSString::from_str(&id.to_string()));self.ivars().mode.set(1);self.apply_mode();
        }
        #[unsafe(method(selectBrowser:))] fn select_browser(&self,sender:&NSPopUpButton){if let Some(id)=sender.titleOfSelectedItem().and_then(|s|s.to_string().parse().ok()){self.ivars().browsers.borrow_mut().select(id);self.apply_mode();}}
        #[unsafe(method(navigateBrowser:))] fn navigate_browser(&self,_sender:&NSButton){if let Err(e)=self.ivars().browsers.borrow().navigate(&self.controls().address.stringValue().to_string()){self.status(&e);}}
        #[unsafe(method(browserBack:))] fn browser_back(&self,_sender:&NSButton){self.ivars().browsers.borrow().back();}
        #[unsafe(method(browserForward:))] fn browser_forward(&self,_sender:&NSButton){self.ivars().browsers.borrow().forward();}
        #[unsafe(method(browserReload:))] fn browser_reload(&self,_sender:&NSButton){self.ivars().browsers.borrow().reload();}
        #[unsafe(method(refreshFiles:))] fn refresh_files(&self,_sender:&NSButton){let root=self.root_path();*self.ivars().directory.borrow_mut()=root.clone();self.file_job(FileCommand::List{root:root.clone(),path:root});}
        #[unsafe(method(parentDirectory:))] fn parent_directory(&self,_sender:&NSButton){let parent=self.ivars().directory.borrow().parent().map(|p|p.to_path_buf());if let Some(path)=parent{self.file_job(FileCommand::List{root:self.root_path(),path});}}
        #[unsafe(method(selectFile:))] fn select_file(&self,sender:&NSPopUpButton){let item=self.ivars().entries.borrow().get(sender.indexOfSelectedItem() as usize).cloned();if let Some((path,folder))=item{self.file_job(if folder{FileCommand::List{root:self.root_path(),path}}else{FileCommand::Read{root:self.root_path(),path}});}}
        #[unsafe(method(openPath:))] fn open_path(&self,_sender:&NSButton){self.file_job(FileCommand::Read{root:self.root_path(),path:PathBuf::from(self.controls().path.stringValue().to_string())});}
        #[unsafe(method(selectCached:))] fn select_cached(&self,sender:&NSPopUpButton){let path=self.ivars().documents.borrow().paths().get(sender.indexOfSelectedItem() as usize).cloned();if let Some(path)=path{self.ivars().documents.borrow_mut().select_cached(&path,self.content_frame());self.apply_mode();}}
        #[unsafe(method(tick:))] fn tick(&self,_timer:&NSTimer){
            for _ in 0..64 {
                let event=self.ivars().host.borrow_mut().try_event();let Some(event)=event else {break;};
                let key=match &event{AgentEvent::Ready{key,..}|AgentEvent::Delta{key,..}|AgentEvent::Activity{key,..}|AgentEvent::Finished{key}|AgentEvent::Error{key,..}=>Some(*key),_=>None};
                let old=key.and_then(|k|self.ivars().workspace.borrow().sessions.get(&k).map(|s|s.transcript.len())).unwrap_or(0);
                self.ivars().workspace.borrow_mut().apply(&event);
                if let Some(key)=key{self.update_view(key,old);}
                match event {
                    AgentEvent::Stopped=>{if let Some(t)=self.ivars().timer.get(){t.invalidate();}NSApplication::sharedApplication(self.mtm()).replyToApplicationShouldTerminate(true);},
                    AgentEvent::Status(text)=>self.status(&text),
                    AgentEvent::Error{message,..}=>self.status(&message),
                    AgentEvent::Ready{..}=>{self.status("Session connected; model/instructions are fixed for this connection");self.show_active();},
                    AgentEvent::Models(models)=>{let c=self.controls();c.model.removeAllItems();c.model.addItemWithTitle(&NSString::from_str("SDK default"));for model in models{c.model.addItemWithTitle(&NSString::from_str(&model));}self.show_active();},
                    AgentEvent::SavedSession{id}=>{let c=self.controls();if c.saved.indexOfItemWithTitle(&NSString::from_str(&id))<0{c.saved.addItemWithTitle(&NSString::from_str(&id));}},
                    AgentEvent::Permission{key,request,description}=>{self.ivars().permissions.borrow_mut().push_back((key,request,description));self.show_permission();},
                    AgentEvent::Finished{..}=>self.status("Turn finished"),_=>{},
                }
            }
            self.poll_files();
            self.smoke_step();
        }
    }
    unsafe impl NSObjectProtocol for Workspace{}
    unsafe impl NSApplicationDelegate for Workspace {
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self,_app:&NSApplication)->NSApplicationTerminateReply {
            self.ivars().closing.set(true); self.ivars().host.borrow().shutdown(); NSApplicationTerminateReply::TerminateLater
        }
    }
    unsafe impl NSWindowDelegate for Workspace{
        #[unsafe(method(windowWillClose:))] fn closed(&self,_notification:&NSNotification){self.ivars().permissions.borrow_mut().clear();if !self.ivars().closing.get(){NSApplication::sharedApplication(self.mtm()).terminate(None);}}
    }
);
impl Workspace {
    fn smoke_step(&self) {
        let Some(tick) = self.ivars().smoke_tick.get() else {
            return;
        };
        self.ivars().smoke_tick.set(Some(tick + 1));
        let c = self.controls();
        let dispatch = |control: &NSControl| {
            // SAFETY: controls below have registered target/action pairs.
            let target = control.target();
            assert!(unsafe { control.sendAction_to(control.action(), target.as_deref()) });
        };
        match tick {
            1 => dispatch(&c.connect),
            3 => {
                assert!(!self.ivars().permissions.borrow().is_empty());
                dispatch(&c.allow);
            }
            5 => {
                c.draft.setStringValue(&NSString::from_str("Hello 🦀"));
                dispatch(&c.send);
            }
            7 => {
                assert!(
                    self.ivars().workspace.borrow().sessions[&1]
                        .transcript
                        .contains("Test streamed response")
                );
                c.draft
                    .setStringValue(&NSString::from_str("Retained draft 🦀"));
                dispatch(&c.new);
            }
            9 => {
                c.sessions.selectItemAtIndex(0);
                dispatch(&c.sessions);
            }
            11 => {
                assert_eq!(c.draft.stringValue().to_string(), "Retained draft 🦀");
                assert!(
                    self.ivars().workspace.borrow().sessions[&1]
                        .sdk_id
                        .is_some()
                );
            }
            13..=17 => {
                dispatch(&c.browser_new);
            }
            18 => {
                let mut browsers = self.ivars().browsers.borrow_mut();
                assert_eq!(browsers.ids().len(), 5);
                let id = browsers.active.unwrap();
                let ptr = browsers.active_view().unwrap() as *const _;
                let first = browsers.ids()[0];
                browsers.select(first);
                browsers.select(id);
                assert_eq!(browsers.active_view().unwrap() as *const _, ptr);
                assert!(browsers.navigate("javascript:alert(1)").is_err());
                browsers.navigate("about:blank").unwrap();
            }
            20 => {
                let path = std::env::temp_dir().join("rivet-ui-document-fixture");
                self.ivars().documents.borrow_mut().show(
                    self,
                    path.clone(),
                    std::sync::Arc::from("Native document fixture 🦀"),
                    self.content_frame(),
                    self.mtm(),
                );
                self.ivars().mode.set(2);
                self.apply_mode();
                assert_eq!(
                    self.ivars()
                        .documents
                        .borrow()
                        .active_text()
                        .unwrap()
                        .to_string(),
                    "Native document fixture 🦀"
                );
                assert!(
                    self.ivars()
                        .documents
                        .borrow_mut()
                        .select_cached(&path, self.content_frame())
                );
            }
            30 => {
                println!("RIVET_LIVE_UI_SMOKE_PASS (test backend; no model/network verification)");
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
            _ => {}
        }
    }
    fn root_path(&self) -> PathBuf {
        PathBuf::from(self.controls().workspace.stringValue().to_string())
    }
    fn content_frame(&self) -> NSRect {
        let z = self.bounds().size;
        rect(220.0, 165.0, z.width - 232.0, z.height - 210.0)
    }
    fn apply_mode(&self) {
        let c = self.controls();
        let mode = self.ivars().mode.get();
        for v in [
            &*c.model as &NSView,
            &*c.instructions,
            &*c.scroll,
            &*c.permission_scroll,
            &*c.allow,
            &*c.deny,
            &*c.draft,
            &*c.send,
            &*c.cancel,
        ] {
            v.setHidden(mode != 0);
        }
        for v in [
            &*c.browser_tabs as &NSView,
            &*c.browser_new,
            &*c.address,
            &*c.go,
            &*c.back,
            &*c.forward,
            &*c.reload,
        ] {
            v.setHidden(mode != 1);
        }
        for v in [
            &*c.files as &NSView,
            &*c.refresh_files,
            &*c.parent,
            &*c.cached,
            &*c.path,
            &*c.open_path,
        ] {
            v.setHidden(mode != 2);
        }
        self.ivars()
            .browsers
            .borrow()
            .layout(self.content_frame(), mode == 1);
        self.ivars()
            .documents
            .borrow()
            .layout(self.content_frame(), mode == 2);
    }
    fn file_job(&self, job: FileCommand) {
        if let Err(e) = self.ivars().files_worker.submit(job) {
            self.status(&e);
        }
    }
    fn poll_files(&self) {
        for _ in 0..4 {
            let Some(event) = self.ivars().files_worker.try_event() else {
                break;
            };
            match event {
                FileEvent::Error(e) => self.status(&e),
                FileEvent::Entries { path, mut entries } => {
                    let total = entries.len();
                    entries.truncate(200);
                    *self.ivars().directory.borrow_mut() = path.clone();
                    let c = self.controls();
                    c.files.removeAllItems();
                    for (p, folder) in &entries {
                        c.files.addItemWithTitle(&NSString::from_str(&format!(
                            "{}{}",
                            if *folder { "▸ " } else { "" },
                            p.file_name().unwrap_or_default().to_string_lossy()
                        )));
                    }
                    *self.ivars().entries.borrow_mut() = entries;
                    self.status(&format!("{} · showing up to 200 of {total} entries; Open path supports any workspace file",path.display()));
                }
                FileEvent::Loaded { path, text } => {
                    self.ivars().documents.borrow_mut().show(
                        self,
                        path.clone(),
                        text,
                        self.content_frame(),
                        self.mtm(),
                    );
                    let c = self.controls();
                    c.cached.removeAllItems();
                    for path in self.ivars().documents.borrow().paths() {
                        c.cached
                            .addItemWithTitle(&NSString::from_str(&path.to_string_lossy()));
                    }
                    c.cached
                        .selectItemWithTitle(&NSString::from_str(&path.to_string_lossy()));
                    self.ivars().mode.set(2);
                    self.apply_mode();
                    self.status(&path.to_string_lossy());
                }
            }
        }
    }
    fn controls(&self) -> &Controls {
        self.ivars().controls.get().unwrap()
    }
    fn status(&self, text: &str) {
        self.controls()
            .status
            .setStringValue(&NSString::from_str(text));
    }
    fn submit(&self, command: AgentCommand) {
        if let Err(e) = self.ivars().host.borrow().submit(command) {
            self.status(&e);
        }
    }
    fn button(&self, title: &str, action: objc2::runtime::Sel) -> Retained<NSButton> {
        // SAFETY: selectors and sender types match the methods registered above.
        unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        }
    }
    fn text_view(&self) -> Retained<NSTextView> {
        let v =
            NSTextView::initWithFrame(NSTextView::alloc(self.mtm()), rect(0.0, 0.0, 700.0, 500.0));
        v.setEditable(false);
        v.setVerticallyResizable(true);
        v.setHorizontallyResizable(false);
        v.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        v.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        if let Some(container) = unsafe { v.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        v
    }
    fn new(
        mtm: MainThreadMarker,
        host: Box<dyn AgentBackend>,
        smoke: bool,
    ) -> Result<Retained<Self>, String> {
        let this = Self::alloc(mtm).set_ivars(State {
            workspace: RefCell::new(LiveWorkspace::default()),
            host: RefCell::new(host),
            controls: OnceCell::new(),
            views: RefCell::new(BTreeMap::new()),
            permissions: RefCell::new(VecDeque::new()),
            timer: OnceCell::new(),
            closing: Cell::new(false),
            mode: Cell::new(0),
            browsers: RefCell::new(Browsers::new()),
            documents: RefCell::new(Documents::new()),
            files_worker: FileWorker::spawn().map_err(|e| e.to_string())?,
            entries: RefCell::new(Vec::new()),
            directory: RefCell::new(PathBuf::new()),
            smoke_tick: Cell::new(if smoke { Some(0) } else { None }),
        });
        // SAFETY: initialize NSView after installing its Rust ivars.
        let this: Retained<Self> =
            unsafe { msg_send![super(this),initWithFrame:rect(0.0,0.0,1120.0,800.0)] };
        this.create_controls();
        unsafe {
            let _: () = msg_send![&this, newSession: &*this.controls().new];
        }
        Ok(this)
    }
    fn create_controls(&self) {
        let mtm = self.mtm();
        let sessions = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(0.0, 0.0, 200.0, 30.0),
            false,
        );
        // SAFETY: this view implements selectSession: with the matching sender.
        unsafe {
            sessions.setTarget(Some(self));
            sessions.setAction(Some(sel!(selectSession:)));
        }
        let saved = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(0.0, 0.0, 200.0, 30.0),
            false,
        );
        let model = NSPopUpButton::initWithFrame_pullsDown(
            NSPopUpButton::alloc(mtm),
            rect(0.0, 0.0, 200.0, 30.0),
            false,
        );
        model.addItemWithTitle(&NSString::from_str("SDK default"));
        let new = self.button("New session", sel!(newSession:));
        let connect = self.button("Connect", sel!(connectSession:));
        let resume = self.button("Resume saved", sel!(resumeSession:));
        let send = self.button("Send", sel!(sendPrompt:));
        let cancel = self.button("Cancel", sel!(cancelTurn:));
        let allow = self.button("Allow once", sel!(allowOnce:));
        let deny = self.button("Reject", sel!(denyRequest:));
        let workspace = NSTextField::new(mtm);
        workspace.setStringValue(&NSString::from_str(
            &std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy(),
        ));
        let instructions = NSTextField::new(mtm);
        instructions.setPlaceholderString(Some(&NSString::from_str(
            "Agent instructions (set before connecting)",
        )));
        let draft = NSTextField::new(mtm);
        draft.setPlaceholderString(Some(&NSString::from_str(
            "Describe a development objective",
        )));
        let status = NSTextField::labelWithString(
            &NSString::from_str("Rivet · Copilot workspace · connect a session to begin"),
            mtm,
        );
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        let permission_scroll = NSScrollView::new(mtm);
        permission_scroll.setHasVerticalScroller(true);
        let permission = self.text_view();
        permission_scroll.setDocumentView(Some(&permission));
        for view in [
            &*sessions as &NSView,
            &*saved,
            &*model,
            &*new,
            &*connect,
            &*resume,
            &*send,
            &*cancel,
            &*allow,
            &*deny,
            &*workspace,
            &*instructions,
            &*draft,
            &*status,
            &*scroll,
            &*permission_scroll,
        ] {
            self.addSubview(view);
        }
        let mut modes = Vec::new();
        for (id, title) in ["Agents", "Browser", "Documents"].iter().enumerate() {
            let b = self.button(title, sel!(showMode:));
            b.setTag(id as isize);
            self.addSubview(&b);
            modes.push(b);
        }
        let popup = || {
            NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(mtm),
                rect(0.0, 0.0, 160.0, 30.0),
                false,
            )
        };
        let browser_tabs = popup();
        let files = popup();
        let cached = popup();
        // SAFETY: all selectors below are registered on this view with popup senders.
        unsafe {
            for (control, action) in [
                (&browser_tabs, sel!(selectBrowser:)),
                (&files, sel!(selectFile:)),
                (&cached, sel!(selectCached:)),
            ] {
                control.setTarget(Some(self));
                control.setAction(Some(action));
            }
        }
        let browser_new = self.button("New tab", sel!(newBrowser:));
        let go = self.button("Go", sel!(navigateBrowser:));
        let back = self.button("←", sel!(browserBack:));
        let forward = self.button("→", sel!(browserForward:));
        let reload = self.button("Reload", sel!(browserReload:));
        let address = NSTextField::new(mtm);
        address.setStringValue(&NSString::from_str("https://github.com"));
        let refresh_files = self.button("List workspace", sel!(refreshFiles:));
        let parent = self.button("Up", sel!(parentDirectory:));
        let path = NSTextField::new(mtm);
        path.setPlaceholderString(Some(&NSString::from_str("Workspace-relative file path")));
        let open_path = self.button("Open path", sel!(openPath:));
        for v in [
            &*browser_tabs as &NSView,
            &*files,
            &*cached,
            &*browser_new,
            &*go,
            &*back,
            &*forward,
            &*reload,
            &*address,
            &*refresh_files,
            &*parent,
            &*path,
            &*open_path,
        ] {
            self.addSubview(v);
        }
        let _ = self.ivars().controls.set(Controls {
            modes,
            browser_tabs,
            browser_new,
            address,
            go,
            back,
            forward,
            reload,
            files,
            refresh_files,
            parent,
            cached,
            path,
            open_path,
            sessions,
            saved,
            model,
            new,
            connect,
            resume,
            send,
            cancel,
            allow,
            deny,
            workspace,
            instructions,
            draft,
            status,
            scroll,
            permission_scroll,
            permission,
        });
        self.layout_controls();
        self.show_permission();
        self.apply_mode();
    }
    fn layout_controls(&self) {
        let Some(c) = self.ivars().controls.get() else {
            return;
        };
        let size = self.bounds().size;
        let w = size.width;
        let h = size.height;
        c.sessions.setFrame(rect(12.0, 12.0, 190.0, 30.0));
        c.new.setFrame(rect(12.0, 50.0, 190.0, 32.0));
        c.saved.setFrame(rect(12.0, 100.0, 190.0, 30.0));
        c.resume.setFrame(rect(12.0, 138.0, 190.0, 32.0));
        c.workspace.setFrame(rect(220.0, 12.0, w - 350.0, 28.0));
        c.connect.setFrame(rect(w - 120.0, 12.0, 108.0, 30.0));
        c.model.setFrame(rect(220.0, 50.0, 220.0, 30.0));
        c.instructions.setFrame(rect(450.0, 50.0, w - 462.0, 28.0));
        c.scroll.setFrame(rect(220.0, 90.0, w - 232.0, h - 310.0));
        c.permission_scroll
            .setFrame(rect(220.0, h - 210.0, w - 450.0, 100.0));
        c.allow.setFrame(rect(w - 215.0, h - 200.0, 100.0, 32.0));
        c.deny.setFrame(rect(w - 110.0, h - 200.0, 98.0, 32.0));
        c.draft.setFrame(rect(220.0, h - 90.0, w - 450.0, 36.0));
        c.send.setFrame(rect(w - 215.0, h - 90.0, 100.0, 36.0));
        c.cancel.setFrame(rect(w - 110.0, h - 90.0, 98.0, 36.0));
        c.status.setFrame(rect(12.0, h - 36.0, w - 24.0, 28.0));
        for (i, b) in c.modes.iter().enumerate() {
            b.setFrame(rect(12.0, 210.0 + i as f64 * 40.0, 190.0, 32.0));
        }
        c.browser_tabs.setFrame(rect(220.0, 90.0, 100.0, 30.0));
        c.browser_new.setFrame(rect(325.0, 90.0, 85.0, 30.0));
        c.back.setFrame(rect(415.0, 90.0, 40.0, 30.0));
        c.forward.setFrame(rect(460.0, 90.0, 40.0, 30.0));
        c.reload.setFrame(rect(505.0, 90.0, 75.0, 30.0));
        c.address.setFrame(rect(585.0, 90.0, w - 665.0, 28.0));
        c.go.setFrame(rect(w - 75.0, 90.0, 63.0, 30.0));
        c.files.setFrame(rect(220.0, 90.0, 280.0, 30.0));
        c.refresh_files.setFrame(rect(505.0, 90.0, 125.0, 30.0));
        c.parent.setFrame(rect(635.0, 90.0, 50.0, 30.0));
        c.cached.setFrame(rect(690.0, 90.0, w - 702.0, 30.0));
        c.path.setFrame(rect(220.0, 125.0, w - 340.0, 28.0));
        c.open_path.setFrame(rect(w - 115.0, 125.0, 103.0, 30.0));
        self.ivars()
            .browsers
            .borrow()
            .layout(self.content_frame(), self.ivars().mode.get() == 1);
        self.ivars()
            .documents
            .borrow()
            .layout(self.content_frame(), self.ivars().mode.get() == 2);
    }
    fn save_draft(&self) {
        let c = self.controls();
        if let Some(window) = self.window() {
            window.makeFirstResponder(None);
        }
        let mut state = self.ivars().workspace.borrow_mut();
        let key = state.active;
        if let Some(s) = state.sessions.get_mut(&key) {
            s.draft = c.draft.stringValue().to_string();
            s.config.instructions = c.instructions.stringValue().to_string();
            s.config.model = if c.model.indexOfSelectedItem() > 0 {
                c.model
                    .titleOfSelectedItem()
                    .map(|x| x.to_string())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            s.scroll_y = c.scroll.contentView().bounds().origin.y;
        }
    }
    fn show_active(&self) {
        let c = self.controls();
        let state = self.ivars().workspace.borrow();
        let key = state.active;
        let Some(s) = state.sessions.get(&key) else {
            return;
        };
        c.draft.setStringValue(&NSString::from_str(&s.draft));
        c.instructions
            .setStringValue(&NSString::from_str(&s.config.instructions));
        c.instructions.setEditable(s.sdk_id.is_none());
        c.model.setEnabled(s.sdk_id.is_none());
        if s.config.model.is_empty() {
            c.model.selectItemAtIndex(0);
        } else {
            c.model
                .selectItemWithTitle(&NSString::from_str(&s.config.model));
        }
        let mut views = self.ivars().views.borrow_mut();
        let view = views.entry(key).or_insert_with(|| self.text_view());
        c.scroll.setDocumentView(Some(view));
        view.scrollPoint(NSPoint::new(0.0, s.scroll_y));
    }
    fn update_view(&self, key: u64, old: usize) {
        let state = self.ivars().workspace.borrow();
        let Some(s) = state.sessions.get(&key) else {
            return;
        };
        let mut views = self.ivars().views.borrow_mut();
        let view = views.entry(key).or_insert_with(|| self.text_view());
        if old > s.transcript.len() {
            view.setString(&NSString::from_str(&s.transcript));
        } else if let Some(storage) = unsafe { view.textStorage() } {
            let text = NSAttributedString::initWithString(
                NSAttributedString::alloc(),
                &NSString::from_str(&s.transcript[old..]),
            );
            storage.appendAttributedString(&text);
        }
    }
    fn connect(&self, resume: Option<String>) {
        self.save_draft();
        let state = self.ivars().workspace.borrow();
        let key = state.active;
        let config: AgentConfig = state.sessions[&key].config.clone();
        let workspace = PathBuf::from(self.controls().workspace.stringValue().to_string());
        drop(state);
        self.submit(AgentCommand::Open {
            key,
            workspace,
            resume,
            config,
        });
        self.submit(AgentCommand::Models);
        self.status("Connecting in background…");
    }
    fn show_permission(&self) {
        let c = self.controls();
        let pending = self.ivars().permissions.borrow();
        if let Some((key, id, text)) = pending.front() {
            c.permission.setString(&NSString::from_str(&format!("Session {key} · request {id}\n{text}\n\nReview before allowing. Permission hooks are not an OS sandbox.")));
            c.allow.setEnabled(true);
            c.deny.setEnabled(true);
        } else {
            c.permission.setString(&NSString::from_str("No tool permissions awaiting your decision. Requests expire after 120 seconds and default to rejection."));
            c.allow.setEnabled(false);
            c.deny.setEnabled(false);
        }
    }
    fn decide(&self, allow: bool) {
        let request = self
            .ivars()
            .permissions
            .borrow()
            .front()
            .map(|(_, id, _)| *id);
        if let Some(request) = request
            && let Err(e) = self
                .ivars()
                .host
                .borrow()
                .submit(AgentCommand::Decide { request, allow })
        {
            self.status(&e);
            return;
        }
        self.ivars().permissions.borrow_mut().pop_front();
        self.show_permission();
    }
}
pub fn run(host: Box<dyn AgentBackend>, smoke: bool) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("AppKit requires the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let root = Workspace::new(mtm, host, smoke)?;
    // SAFETY: main-thread designated initializer, retained through the event loop.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0.0, 0.0, 1120.0, 800.0),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::Miniaturizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(&NSString::from_str("Rivet — Copilot Workspace"));
    window.setContentMinSize(NSSize::new(900.0, 650.0));
    window.setContentView(Some(&root));
    window.setDelegate(Some(ProtocolObject::from_ref(&*root)));
    app.setDelegate(Some(ProtocolObject::from_ref(&*root)));
    window.center();
    window.makeKeyAndOrderFront(None);
    super::appkit::install_menus(&app, mtm);
    app.activate();
    // SAFETY: tick: is registered and its retained target is invalidated on close.
    let timer = unsafe {
        NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
            1.0 / 60.0,
            &root,
            sel!(tick:),
            None,
            true,
        )
    };
    unsafe {
        NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
    }
    let _ = root.ivars().timer.set(timer);
    app.run();
    Ok(())
}

/// Explicit UI-only fixture, never selected by the production launcher.
pub fn smoke_backend() -> Box<dyn AgentBackend> {
    struct Fixture(RefCell<VecDeque<AgentEvent>>);
    impl AgentBackend for Fixture {
        fn shutdown(&self) {
            self.0.borrow_mut().push_back(AgentEvent::Stopped);
        }
        fn submit(&self, command: AgentCommand) -> Result<(), String> {
            let mut events = self.0.borrow_mut();
            match command {
                AgentCommand::Open { key, .. } => {
                    events.push_back(AgentEvent::Ready {
                        key,
                        id: "UI-test-session".into(),
                    });
                    events.push_back(AgentEvent::Permission {
                        key,
                        request: 1,
                        description: "UI-only test permission".into(),
                    });
                }
                AgentCommand::Models => {
                    events.push_back(AgentEvent::Models(vec!["test-model".into()]))
                }
                AgentCommand::Send { key, .. } => {
                    events.push_back(AgentEvent::Delta {
                        key,
                        text: "Test streamed response 🦀".into(),
                    });
                    events.push_back(AgentEvent::Finished { key });
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
