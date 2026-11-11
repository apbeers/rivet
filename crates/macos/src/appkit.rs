//! All Objective-C interaction is confined here. AppKit objects stay on the main
//! thread; target/action selectors below are registered on their actual classes.
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2::{
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::*;
use objc2_foundation::{
    NSDictionary, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSRunLoop,
    NSRunLoopCommonModes, NSSize, NSString, NSTimer,
};
use rivet_prototype::{Metrics, MockWorker, Prototype, ROW_HEIGHT, SESSION_COUNT, visible_rows};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::VecDeque,
    rc::Rc,
    time::Instant,
};

fn click(button: &NSButton) {
    // SAFETY: only controls constructed with Shell's registered target/actions reach here.
    // NSButton.performClick animates and blocks this timer-driven test. Dispatch
    // the registered native action instead; OS mouse delivery is a separate gate.
    let target = button.target();
    assert!(unsafe { button.sendAction_to(button.action(), target.as_deref()) });
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(0.0), h.max(0.0)))
}
struct Shared {
    state: RefCell<Prototype>,
    metrics: RefCell<Metrics>,
    pending: Cell<Option<Instant>>,
    switch_requested: Cell<Option<Instant>>,
    started: Instant,
    first_draw: Cell<bool>,
}
struct ConversationIvars {
    shared: Rc<Shared>,
    labels: RefCell<VecDeque<(usize, Retained<NSString>)>>,
    texts: Vec<Retained<NSString>>,
    attributes: Retained<NSDictionary<NSString, AnyObject>>,
    heading: Retained<NSDictionary<NSString, AnyObject>>,
}

define_class!(
    // SAFETY: NSView subclass with main-thread-only state; no custom deallocation.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RivetConversationView"]
    #[ivars = ConversationIvars]
    struct Conversation;
    impl Conversation {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }
        #[unsafe(method(isOpaque))]
        fn opaque(&self) -> bool { true }
        #[unsafe(method(drawRect:))]
        fn draw(&self, dirty: NSRect) {
            let started = Instant::now();let cpu=super::clock::Span::start();
            let iv = self.ivars();
            if !iv.shared.first_draw.replace(true) {
                eprintln!("native_frontend_entry_to_first_conversation_draw_us={}", iv.shared.started.elapsed().as_micros());
            }
            if let Some(requested) = iv.shared.switch_requested.take() {
                iv.shared.metrics.borrow_mut().switch_to_draw.record(requested.elapsed());
            }
            NSColor::textBackgroundColor().setFill(); NSRectFill(dirty);
            let state = iv.shared.state.borrow(); let session = state.active();
            let viewport = self.visibleRect();
            let rows = visible_rows(viewport.origin.y, viewport.size.height, session.row_count());
            let mut drawn = 0;
            for index in rows {
                let y = index as f64 * ROW_HEIGHT;
                if y + ROW_HEIGHT < dirty.origin.y || y > dirty.origin.y + dirty.size.height { continue; }
                if index % 2 == 0 {
                    NSColor::controlBackgroundColor().setFill();
                    NSRectFill(rect(0.0, y, self.bounds().size.width, ROW_HEIGHT));
                }
                let mut labels = iv.labels.borrow_mut();
                if !labels.iter().any(|(key, _)| *key == index) {
                    if labels.len() == 64 { labels.pop_front(); }
                    let label = if index == session.records.len() { "Live mock activity".into() }
                        else { format!("{} · Event {}", session.records[index].role, index + 1) };
                    labels.push_back((index, NSString::from_str(&label)));
                }
                let label = &labels.iter().find(|(key, _)| *key == index).unwrap().1;
                let width = (self.bounds().size.width - 40.0).max(20.0);
                // SAFETY: dictionaries contain native font/color values for the matching keys.
                unsafe {
                    label.drawInRect_withAttributes(rect(20.0, y + 10.0, width, 20.0), Some(&iv.heading));
                    if index < session.records.len() {
                        iv.texts[index % 3].drawInRect_withAttributes(rect(20.0, y + 34.0, width, 24.0), Some(&iv.attributes));
                    } else {
                        let text = NSString::from_str(&format!("{} updates · {}", session.updates, session.live_text));
                        text.drawInRect_withAttributes(rect(20.0, y + 34.0, width, 24.0), Some(&iv.attributes));
                        if let Some(received) = iv.shared.pending.take() {
                            iv.shared.metrics.borrow_mut().event_to_draw.record(received.elapsed());
                        }
                    }
                }
                drawn += 1;
            }
            let mut metrics = iv.shared.metrics.borrow_mut();
            metrics.rows_drawn = drawn; metrics.max_rows_drawn = metrics.max_rows_drawn.max(drawn);
            metrics.drawing.record(started.elapsed());
            if let Some(elapsed)=cpu.elapsed(){metrics.drawing_thread_cpu.record(elapsed);}
        }
    }
);
impl Conversation {
    fn new(mtm: MainThreadMarker, shared: Rc<Shared>) -> Retained<Self> {
        let font = NSFont::systemFontOfSize(13.0);
        let bold = NSFont::boldSystemFontOfSize(12.0);
        let color = NSColor::labelColor();
        let secondary = NSColor::secondaryLabelColor();
        // SAFETY: AppKit's attribute keys are process-lifetime constants, with the documented value classes.
        let attributes = unsafe {
            NSDictionary::from_slices(
                &[NSFontAttributeName, NSForegroundColorAttributeName],
                &[&*font as &AnyObject, &*color as &AnyObject],
            )
        };
        let heading = unsafe {
            NSDictionary::from_slices(
                &[NSFontAttributeName, NSForegroundColorAttributeName],
                &[&*bold as &AnyObject, &*secondary as &AnyObject],
            )
        };
        let texts = shared.state.borrow().active().records[..3]
            .iter()
            .map(|r| NSString::from_str(r.text))
            .collect();
        let this = Self::alloc(mtm).set_ivars(ConversationIvars {
            shared,
            labels: RefCell::new(VecDeque::new()),
            texts,
            attributes,
            heading,
        });
        // SAFETY: designated NSView initializer, after installing required Rust ivars.
        unsafe {
            msg_send![super(this), initWithFrame: rect(0.0, 0.0, 700.0, (rivet_prototype::EVENT_COUNT + 1) as f64 * ROW_HEIGHT)]
        }
    }
}

struct Controls {
    sidebar: Retained<NSScrollView>,
    buttons: Vec<Retained<NSButton>>,
    title: Retained<NSTextField>,
    conversation_tab: Retained<NSButton>,
    settings_tab: Retained<NSButton>,
    scroll: Retained<NSScrollView>,
    document: Retained<Conversation>,
    draft: Retained<NSTextField>,
    settings: Retained<NSView>,
    model: Retained<NSButton>,
    status: Retained<NSTextField>,
    jump: Retained<NSButton>,
}
struct ShellIvars {
    shared: Rc<Shared>,
    controls: OnceCell<Controls>,
    timer: OnceCell<Retained<NSTimer>>,
    worker: MockWorker,
    smoke: bool,
    ticks: Cell<usize>,
    started: Instant,
}

define_class!(
    // SAFETY: all state and native object ownership are main-thread-only.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RivetShellView"]
    #[ivars = ShellIvars]
    struct Shell;
    impl Shell {
        #[unsafe(method(isFlipped))]
        fn flipped(&self) -> bool { true }
        #[unsafe(method(resizeSubviewsWithOldSize:))]
        fn resized(&self, _old: NSSize) { self.layout_controls(); }
        #[unsafe(method(selectSession:))]
        fn select_session(&self, sender: &NSButton) {
            let start = Instant::now();let cpu=super::clock::Span::start(); let Some(c) = self.ivars().controls.get() else { return; };
            self.save_view();
            self.ivars().shared.pending.set(None);
            self.ivars().shared.state.borrow_mut().select(sender.tag() as usize);
            self.restore_view();
            if !self.ivars().shared.state.borrow().active().settings_open {
                self.ivars().shared.switch_requested.set(Some(start));
            }
            self.ivars().shared.metrics.borrow_mut().switching.record(start.elapsed());
            if let Some(elapsed)=cpu.elapsed(){self.ivars().shared.metrics.borrow_mut().switching_thread_cpu.record(elapsed);}
            c.document.setNeedsDisplay(true);
        }
        #[unsafe(method(showConversation:))]
        fn conversation_tab(&self, _sender: &NSButton) { self.show_settings(false); }
        #[unsafe(method(showSettings:))]
        fn settings_tab(&self, _sender: &NSButton) { self.show_settings(true); }
        #[unsafe(method(cycleModel:))]
        fn cycle_model(&self, _sender: &NSButton) {
            let mut state = self.ivars().shared.state.borrow_mut();
            state.active_mut().model = (state.active().model + 1) % 3;
            drop(state); self.restore_view();
        }
        #[unsafe(method(jumpToActivity:))]
        fn jump_to_activity(&self, _sender: &NSButton) {
            let c = self.ivars().controls.get().unwrap();
            self.ivars().shared.state.borrow_mut().active_mut().settings_open = false;
            self.restore_view();
            c.document.scrollPoint(NSPoint::new(0.0, c.document.frame().size.height));
            self.save_view();
        }
        #[unsafe(method(tick:))]
        fn tick(&self, _timer: &NSTimer) {
            let iv = self.ivars();
            let Some(c) = iv.controls.get() else { return; };
            let mut changed = false;
            {
                let mut state = iv.shared.state.borrow_mut();
                for update in iv.worker.drain() {
                    if update.session == state.active_index() { iv.shared.pending.set(Some(update.received)); changed = true; }
                    state.apply(&update);
                }
            }
            if changed {
                let state = iv.shared.state.borrow();
                let y = state.active().records.len() as f64 * ROW_HEIGHT;
                let viewport = c.document.visibleRect();
                if !state.active().settings_open && viewport.origin.y + viewport.size.height >= y {
                    c.document.setNeedsDisplayInRect(rect(0.0, y, c.document.bounds().size.width, ROW_HEIGHT));
                } else {
                    // Hidden/offscreen time is not event presentation latency.
                    iv.shared.pending.set(None);
                }
            }
            let ticks = iv.ticks.get() + 1; iv.ticks.set(ticks);
            if ticks.is_multiple_of(60) {
                let state = iv.shared.state.borrow();
                c.status.setStringValue(&NSString::from_str(&format!("MOCK DATA · {} events · {} updates · native UI prototype", state.active().row_count(), state.active().updates)));
            }
            if iv.smoke { self.smoke_step(ticks); }
        }
    }
    unsafe impl NSObjectProtocol for Shell {}
    unsafe impl NSWindowDelegate for Shell {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _notification: &NSNotification) {
            self.shutdown();
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }
    }
);
impl Shell {
    fn new(mtm: MainThreadMarker, shared: Rc<Shared>, smoke: bool) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ShellIvars {
            shared,
            controls: OnceCell::new(),
            timer: OnceCell::new(),
            worker: MockWorker::new(),
            smoke,
            ticks: Cell::new(0),
            started: Instant::now(),
        });
        // SAFETY: designated NSView initializer on a main-thread allocated instance.
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, 1120.0, 760.0)] };
        this.create_controls();
        this
    }
    fn button(&self, title: &str, action: objc2::runtime::Sel) -> Retained<NSButton> {
        // SAFETY: this Shell implements each selector passed by create_controls.
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        };
        button.setBezelStyle(NSBezelStyle::Push);
        button
    }
    fn create_controls(&self) {
        let mtm = self.mtm();
        let sidebar =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0.0, 0.0, 224.0, 650.0));
        sidebar.setHasVerticalScroller(true);
        sidebar.setAutohidesScrollers(true);
        let list = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 224.0, 840.0));
        let mut buttons = Vec::new();
        for (index, session) in self
            .ivars()
            .shared
            .state
            .borrow()
            .sessions
            .iter()
            .enumerate()
        {
            let button = self.button(&session.name, sel!(selectSession:));
            button.setTag(index as isize);
            button.setFrame(rect(12.0, 800.0 - index as f64 * 40.0, 195.0, 32.0));
            list.addSubview(&button);
            buttons.push(button);
        }
        sidebar.setDocumentView(Some(&list));
        list.scrollPoint(NSPoint::new(0.0, 840.0));
        let title =
            NSTextField::labelWithString(&NSString::from_str("Rivet · Mock agent workspace"), mtm);
        title.setFont(Some(&NSFont::boldSystemFontOfSize(19.0)));
        let conversation_tab = self.button("Conversation", sel!(showConversation:));
        let settings_tab = self.button("Agent settings", sel!(showSettings:));
        let jump = self.button("Live activity ↓", sel!(jumpToActivity:));
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let document = Conversation::new(mtm, self.ivars().shared.clone());
        scroll.setDocumentView(Some(&document));
        let draft = NSTextField::new(mtm);
        draft.setPlaceholderString(Some(&NSString::from_str(
            "Draft an objective · preserved per session · no messages sent in this prototype",
        )));
        let settings = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, 600.0, 500.0));
        let note = NSTextField::wrappingLabelWithString(
            &NSString::from_str(
                "Agent configuration\n\nCached per session. These are mock settings; no Copilot session is created.\n\nTool policy: no execution in Stage 2.",
            ),
            mtm,
        );
        note.setFrame(rect(24.0, 200.0, 500.0, 180.0));
        settings.addSubview(&note);
        let model = self.button("Mock model: Balanced", sel!(cycleModel:));
        model.setFrame(rect(24.0, 140.0, 260.0, 36.0));
        settings.addSubview(&model);
        let status = NSTextField::labelWithString(
            &NSString::from_str(
                "MOCK DATA · 20 persistent sessions · 100,000 events per conversation",
            ),
            mtm,
        );
        for view in [
            &*sidebar as &NSView,
            &*title,
            &*conversation_tab,
            &*settings_tab,
            &*jump,
            &*scroll,
            &*settings,
            &*draft,
            &*status,
        ] {
            self.addSubview(view);
        }
        let _ = self.ivars().controls.set(Controls {
            sidebar,
            buttons,
            title,
            conversation_tab,
            settings_tab,
            scroll,
            document,
            draft,
            settings,
            model,
            status,
            jump,
        });
        self.layout_controls();
        self.restore_view();
    }
    fn layout_controls(&self) {
        let Some(c) = self.ivars().controls.get() else {
            return;
        };
        let size = self.bounds().size;
        let x = 240.0;
        let width = (size.width - x - 20.0).max(100.0);
        c.sidebar
            .setFrame(rect(0.0, 0.0, 224.0, size.height - 30.0));
        c.title.setFrame(rect(x, 18.0, width, 30.0));
        c.conversation_tab.setFrame(rect(x, 58.0, 130.0, 30.0));
        c.settings_tab.setFrame(rect(x + 140.0, 58.0, 140.0, 30.0));
        c.jump.setFrame(rect(x + width - 145.0, 58.0, 145.0, 30.0));
        let content = rect(x, 100.0, width, size.height - 192.0);
        c.scroll.setFrame(content);
        c.settings.setFrame(content);
        c.document.setFrameSize(NSSize::new(
            c.scroll.contentSize().width,
            self.ivars().shared.state.borrow().active().content_height(),
        ));
        c.draft.setFrame(rect(x, size.height - 78.0, width, 36.0));
        c.status
            .setFrame(rect(12.0, size.height - 27.0, size.width - 24.0, 20.0));
    }
    fn save_view(&self) {
        let c = self.ivars().controls.get().unwrap();
        // Commit the native field editor so switching during text entry preserves the draft.
        if let Some(window) = self.window() {
            window.makeFirstResponder(None);
        }
        let mut state = self.ivars().shared.state.borrow_mut();
        state.active_mut().draft = c.draft.stringValue().to_string();
        state.remember_scroll(
            c.scroll.contentView().bounds().origin.y,
            c.scroll.contentSize().height,
        );
    }
    fn restore_view(&self) {
        let c = self.ivars().controls.get().unwrap();
        let state = self.ivars().shared.state.borrow();
        let session = state.active();
        c.title.setStringValue(&NSString::from_str(&format!(
            "{} · Mock session",
            session.name
        )));
        c.draft.setStringValue(&NSString::from_str(&session.draft));
        c.model.setTitle(&NSString::from_str(&format!(
            "Mock model: {}",
            ["Balanced", "Fast", "Thorough"][session.model]
        )));
        c.settings.setHidden(!session.settings_open);
        c.scroll.setHidden(session.settings_open);
        c.draft.setHidden(session.settings_open);
        c.jump.setHidden(session.settings_open);
        for (index, button) in c.buttons.iter().enumerate() {
            button.setState(if index == state.active_index() {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
        c.document.scrollPoint(NSPoint::new(0.0, session.scroll_y));
        c.document.setNeedsDisplay(true);
    }
    fn show_settings(&self, show: bool) {
        let start = Instant::now();
        let cpu = super::clock::Span::start();
        self.save_view();
        self.ivars()
            .shared
            .state
            .borrow_mut()
            .active_mut()
            .settings_open = show;
        // Toggle only the affected views. Session labels, models and sidebar selection
        // are already current and need no update for a cached tab change.
        let c = self.ivars().controls.get().unwrap();
        c.settings.setHidden(!show);
        c.scroll.setHidden(show);
        c.draft.setHidden(show);
        c.jump.setHidden(show);
        if !show {
            c.document.setNeedsDisplay(true);
        }
        let mut metrics = self.ivars().shared.metrics.borrow_mut();
        metrics.settings.record(start.elapsed());
        if let Some(elapsed) = cpu.elapsed() {
            metrics.settings_thread_cpu.record(elapsed);
        }
    }
    fn start_timer(&self) {
        // SAFETY: tick: is implemented above; timer is retained and invalidated at shutdown.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(
                1.0 / 120.0,
                self,
                sel!(tick:),
                None,
                true,
            )
        };
        // Common modes keep background polling alive during native scrolling/tracking.
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
        let _ = self.ivars().timer.set(timer);
    }
    fn shutdown(&self) {
        if let Some(timer) = self.ivars().timer.get() {
            timer.invalidate();
        }
        eprintln!("{}", self.ivars().shared.metrics.borrow().report());
    }
    fn smoke_step(&self, tick: usize) {
        let c = self.ivars().controls.get().unwrap();
        match tick {
            10 => {
                c.draft
                    .setStringValue(&NSString::from_str("Persistent draft 🦀 日本語"));
                c.document.scrollPoint(NSPoint::new(0.0, 7200.0));
            }
            20 => click(&c.buttons[1]),
            30 => click(&c.buttons[0]),
            40 => {
                let state = self.ivars().shared.state.borrow();
                assert_eq!(
                    c.draft.stringValue().to_string(),
                    "Persistent draft 🦀 日本語"
                );
                assert!((c.scroll.contentView().bounds().origin.y - 7200.0).abs() < 1.0);
                assert_eq!(state.active_index(), 0);
            }
            50 => click(&c.settings_tab),
            60 => click(&c.model),
            70 => click(&c.buttons[1]),
            80 => click(&c.buttons[0]),
            90 => {
                assert!(!c.settings.isHidden());
                assert_eq!(self.ivars().shared.state.borrow().active().model, 1);
            }
            100 => click(&c.conversation_tab),
            110 => click(&c.jump),
            120..=219 => {
                click(&c.buttons[tick % SESSION_COUNT]);
                c.document.scrollPoint(NSPoint::new(
                    0.0,
                    ((tick * 7919) % rivet_prototype::EVENT_COUNT) as f64 * ROW_HEIGHT,
                ));
            }
            220 => click(&c.jump),
            240..=339 => {
                if tick.is_multiple_of(2) {
                    click(&c.settings_tab);
                } else {
                    click(&c.conversation_tab);
                }
            }
            340 => click(&c.jump),
            360 => {
                assert!(self.ivars().shared.state.borrow().active().updates > 0);
                assert!(self.ivars().shared.metrics.borrow().max_rows_drawn < 30);
                assert!(
                    self.ivars()
                        .shared
                        .metrics
                        .borrow()
                        .drawing
                        .percentiles()
                        .is_some()
                );
                self.snapshot();
                self.shutdown();
                println!(
                    "RIVET_UI_SMOKE_PASS elapsed_ms={}",
                    self.ivars().started.elapsed().as_millis()
                );
                NSApplication::sharedApplication(self.mtm()).terminate(None);
            }
            _ => {}
        }
    }
    fn snapshot(&self) {
        let image = self
            .bitmapImageRepForCachingDisplayInRect(self.bounds())
            .expect("native bitmap allocation");
        {
            self.cacheDisplayInRect_toBitmapImageRep(self.bounds(), &image);
            // SAFETY: empty properties dictionary is valid for PNG encoding.
            let data = unsafe {
                image.representationUsingType_properties(
                    NSBitmapImageFileType::PNG,
                    &NSDictionary::new(),
                )
            }
            .expect("native PNG encoding");
            assert!(data.writeToFile_atomically(&NSString::from_str("rivet-ui-smoke.png"), true));
        }
    }
}

pub fn run(smoke: bool) -> Result<(), String> {
    let started = Instant::now();
    let mtm = MainThreadMarker::new().ok_or("AppKit must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let shared = Rc::new(Shared {
        state: RefCell::new(Prototype::new()),
        metrics: RefCell::new(Metrics::default()),
        pending: Cell::new(None),
        switch_requested: Cell::new(None),
        started,
        first_draw: Cell::new(false),
    });
    let shell = Shell::new(mtm, shared, smoke);
    // SAFETY: designated initializer on the main thread; the retained window stays alive throughout run().
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0.0, 0.0, 1120.0, 760.0),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(&NSString::from_str(
        "Rivet — Native UI Prototype · Mock Data",
    ));
    window.setContentMinSize(NSSize::new(820.0, 560.0));
    window.setContentView(Some(&shell));
    window.setDelegate(Some(ProtocolObject::from_ref(&*shell)));
    install_menus(&app, mtm);
    window.center();
    window.makeKeyAndOrderFront(None);
    app.activate();
    shell.start_timer();
    app.run();
    shell.shutdown();
    Ok(())
}

pub(crate) fn install_menus(app: &NSApplication, mtm: MainThreadMarker) {
    let menu = NSMenu::new(mtm);
    let app_item = NSMenuItem::new(mtm);
    menu.addItem(&app_item);
    let app_menu = NSMenu::new(mtm);
    // SAFETY: terminate: is handled by NSApplication through the native responder chain.
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit Rivet"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        )
    };
    app_menu.addItem(&quit);
    app_item.setSubmenu(Some(&app_menu));
    let edit_item = NSMenuItem::new(mtm);
    let edit_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Edit"));
    for (title, action, key) in [
        ("Undo", sel!(undo:), "z"),
        ("Redo", sel!(redo:), "Z"),
        ("Cut", sel!(cut:), "x"),
        ("Copy", sel!(copy:), "c"),
        ("Paste", sel!(paste:), "v"),
        ("Select All", sel!(selectAll:), "a"),
    ] {
        // SAFETY: standard editing actions route to AppKit's current field editor.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        edit_menu.addItem(&item);
    }
    edit_item.setSubmenu(Some(&edit_menu));
    menu.addItem(&edit_item);
    app.setMainMenu(Some(&menu));
}
