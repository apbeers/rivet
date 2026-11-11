//! Native production workspace. The separate appkit module retains the mock benchmark.
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
            20 => {
                println!("RIVET_LIVE_UI_SMOKE_PASS (test backend; no model/network verification)");
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
            _ => {}
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
        let _ = self.ivars().controls.set(Controls {
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
