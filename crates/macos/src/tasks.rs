//! Native task supervision; task and SDK execution live on worker threads.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
};
use objc2_app_kit::*;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use rivet_core::{
    json,
    task_service::{Command, Event, Host},
};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::VecDeque,
    path::PathBuf,
};
struct Controls {
    objective: Retained<NSTextField>,
    validation: Retained<NSTextField>,
    dependencies: Retained<NSTextField>,
    id: Retained<NSTextField>,
    commit: Retained<NSTextField>,
    buttons: Vec<Retained<NSButton>>,
    scroll: Retained<NSScrollView>,
    output: Retained<NSTextView>,
}
pub(crate) struct State {
    host: RefCell<Option<Host>>,
    controls: OnceCell<Controls>,
    workspace: Retained<NSTextField>,
    report: RefCell<String>,
    log: RefCell<String>,
    permissions: RefCell<VecDeque<(u64, String)>>,
    stopped: Cell<bool>,
    closing: Cell<bool>,
    integration: RefCell<Option<(u64, String)>>,
}
define_class!(
 // SAFETY: AppKit controls are accessed exclusively on the main thread.
 #[unsafe(super(NSView))] #[thread_kind=MainThreadOnly] #[name="RivetTasksPane"] #[ivars=State]
 pub(crate) struct Tasks;
 impl Tasks{
  #[unsafe(method(isFlipped))] fn flipped(&self)->bool{true}
  #[unsafe(method(resizeSubviewsWithOldSize:))] fn resized(&self,_old:NSSize){self.layout_controls();}
  #[unsafe(method(taskAction:))] fn task_action(&self,sender:&NSButton){if let Err(e)=self.action(sender.tag()){self.log(&e);}}
 }
);
impl Tasks {
    pub fn new(
        mtm: MainThreadMarker,
        workspace: Retained<NSTextField>,
        host: Option<Host>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(State {
            host: RefCell::new(host),
            controls: OnceCell::new(),
            workspace,
            report: RefCell::new(String::new()),
            log: RefCell::new(String::new()),
            permissions: RefCell::new(VecDeque::new()),
            stopped: Cell::new(false),
            closing: Cell::new(false),
            integration: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe {
            msg_send![super(this),initWithFrame:NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(900.0,600.0))]
        };
        this.create_controls();
        this
    }
    fn c(&self) -> &Controls {
        self.ivars().controls.get().unwrap()
    }
    fn create_controls(&self) {
        let mtm = self.mtm();
        let field = |placeholder: &str| {
            let f = NSTextField::new(mtm);
            f.setPlaceholderString(Some(&NSString::from_str(placeholder)));
            self.addSubview(&f);
            f
        };
        let objective = field("Development objective and completion criteria");
        let validation = field("Validation command as JSON array, e.g. [\"cargo\",\"test\"]");
        let dependencies = field("Dependency task IDs, e.g. [1,2]; [] for none");
        dependencies.setStringValue(&NSString::from_str("[]"));
        let id = field("Task ID");
        let commit = field("Exact reviewed worktree HEAD SHA to integrate");
        let mut buttons = Vec::new();
        for (i, title) in [
            "Add task",
            "Run ready tasks",
            "Retry ID",
            "Cancel running",
            "Integrate / Confirm",
            "Allow once",
            "Reject",
        ]
        .iter()
        .enumerate()
        {
            let b = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(self),
                    Some(sel!(taskAction:)),
                    mtm,
                )
            };
            b.setTag(i as isize);
            self.addSubview(&b);
            buttons.push(b);
        }
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        let output = NSTextView::new(mtm);
        output.setEditable(false);
        output.setVerticallyResizable(true);
        output.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        scroll.setDocumentView(Some(&output));
        self.addSubview(&scroll);
        let _ = self.ivars().controls.set(Controls {
            objective,
            validation,
            dependencies,
            id,
            commit,
            buttons,
            scroll,
            output,
        });
        self.layout_controls();
        self.log("Tasks run in separate Git worktrees (up to three agents). Validation runs before review and after integration. Review and commit changes in the task worktree, then enter its exact HEAD SHA to integrate. Failed integrations retain changes for inspection. Task actions run with your user permissions.");
    }
    fn layout_controls(&self) {
        let Some(c) = self.ivars().controls.get() else {
            return;
        };
        let w = self.bounds().size.width;
        let h = self.bounds().size.height;
        let r = |x, y, w, h| NSRect::new(NSPoint::new(x, y), NSSize::new(w, h));
        c.objective.setFrame(r(0.0, 0.0, w, 28.0));
        c.validation.setFrame(r(0.0, 36.0, w, 28.0));
        c.dependencies.setFrame(r(0.0, 72.0, w - 140.0, 28.0));
        c.buttons[0].setFrame(r(w - 135.0, 72.0, 135.0, 30.0));
        for (i, b) in c.buttons[1..4].iter().enumerate() {
            b.setFrame(r(i as f64 * 150.0, 110.0, 145.0, 30.0));
        }
        c.id.setFrame(r(450.0, 110.0, 100.0, 28.0));
        c.commit.setFrame(r(0.0, 148.0, w - 180.0, 28.0));
        c.buttons[4].setFrame(r(w - 175.0, 148.0, 175.0, 30.0));
        c.buttons[5].setFrame(r(0.0, 186.0, 120.0, 30.0));
        c.buttons[6].setFrame(r(125.0, 186.0, 120.0, 30.0));
        c.scroll.setFrame(r(0.0, 224.0, w, (h - 224.0).max(60.0)));
    }
    fn submit(&self, cmd: Command) -> Result<(), String> {
        self.ivars()
            .host
            .borrow()
            .as_ref()
            .ok_or("Task service unavailable in this test environment")?
            .submit(cmd)
    }
    fn action(&self, tag: isize) -> Result<(), String> {
        if self.ivars().closing.get() {
            return Err("Task service stopping".into());
        }
        if let Some(w) = self.window() {
            w.makeFirstResponder(None);
        }
        let c = self.c();
        match tag {
            0 => {
                let text = c.validation.stringValue().to_string();
                let deps = c.dependencies.stringValue().to_string();
                if text.len() > 16384 || deps.len() > 4096 {
                    return Err("Task configuration too large".into());
                }
                let v = json::parse(&text)?;
                let validation = v
                    .array()
                    .ok_or("Validation must be an array")?
                    .iter()
                    .map(|v| {
                        v.str()
                            .map(String::from)
                            .ok_or("Validation arguments must be strings")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let d = json::parse(&deps)?;
                let dependencies = d
                    .array()
                    .ok_or("Dependencies must be an array")?
                    .iter()
                    .map(|v| v.integer().ok_or("Invalid task dependency"))
                    .collect::<Result<Vec<_>, _>>()?;
                self.submit(Command::Add {
                    root: PathBuf::from(self.ivars().workspace.stringValue().to_string()),
                    objective: c.objective.stringValue().to_string(),
                    dependencies,
                    validation,
                })?;
            }
            1 => self.submit(Command::RunReady)?,
            2 => self.submit(Command::Retry(
                c.id.stringValue()
                    .to_string()
                    .parse()
                    .map_err(|_| "Enter a task ID")?,
            ))?,
            3 => self.submit(Command::CancelAll)?,
            4 => {
                let id =
                    c.id.stringValue()
                        .to_string()
                        .parse()
                        .map_err(|_| "Enter a task ID")?;
                let commit = c.commit.stringValue().to_string();
                if !rivet_core::tasks::valid_oid(&commit) {
                    return Err("Enter an exact commit SHA".into());
                }
                let confirmed =
                    self.ivars().integration.borrow().as_ref() == Some(&(id, commit.clone()));
                if !confirmed {
                    *self.ivars().integration.borrow_mut() = Some((id, commit.clone()));
                    self.log(&format!("Confirm integration of task #{id}, reviewed commit {commit}. This modifies the main workspace and runs the configured validation. Press Integrate again to proceed."));
                } else {
                    self.ivars().integration.borrow_mut().take();
                    self.submit(Command::Integrate { id, commit })?;
                }
            }
            5 | 6 => {
                let request = self
                    .ivars()
                    .permissions
                    .borrow()
                    .front()
                    .map(|p| p.0)
                    .ok_or("No pending task permission")?;
                self.submit(Command::Decide {
                    request,
                    allow: tag == 5,
                })?;
                self.ivars().permissions.borrow_mut().pop_front();
                self.render();
            }
            _ => {}
        }
        Ok(())
    }
    fn log(&self, text: &str) {
        let mut log = self.ivars().log.borrow_mut();
        log.push_str(text);
        log.push('\n');
        if log.len() > 65536 {
            let mut cut = log.len() - 49152;
            while !log.is_char_boundary(cut) {
                cut += 1;
            }
            log.drain(..cut);
        }
        drop(log);
        self.render();
    }
    fn render(&self) {
        let p = self.ivars().permissions.borrow();
        let text = format!(
            "{}\n{}\n{}",
            p.front()
                .map(|p| format!("PERMISSION (allow once / reject):\n{}\n", p.1))
                .unwrap_or_default(),
            self.ivars().report.borrow(),
            self.ivars().log.borrow()
        );
        self.c().output.setString(&NSString::from_str(&text));
        self.c().buttons[5].setEnabled(!p.is_empty());
        self.c().buttons[6].setEnabled(!p.is_empty());
    }
    pub fn tick(&self) {
        for _ in 0..16 {
            let e = self
                .ivars()
                .host
                .borrow()
                .as_ref()
                .and_then(Host::try_event);
            let Some(e) = e else {
                break;
            };
            match e {
                Event::Report(r) => {
                    *self.ivars().report.borrow_mut() = r;
                    self.render();
                }
                Event::Log(t) => self.log(&t),
                Event::Permission {
                    request,
                    description,
                } => {
                    if self.ivars().permissions.borrow().len() < 32 {
                        self.ivars()
                            .permissions
                            .borrow_mut()
                            .push_back((request, description));
                        self.render();
                    } else {
                        let _ = self.submit(Command::Decide {
                            request,
                            allow: false,
                        });
                    }
                }
                Event::Stopped => {
                    self.ivars().stopped.set(true);
                    self.log("Task service stopped");
                }
            }
        }
    }
    pub fn shutdown(&self) {
        self.ivars().closing.set(true);
        if let Some(h) = self.ivars().host.borrow().as_ref() {
            h.shutdown();
        }
    }
    pub fn is_finished(&self) -> bool {
        self.ivars()
            .host
            .borrow()
            .as_ref()
            .is_none_or(Host::is_finished)
    }
}
