use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSScrollView, NSTextView, NSView};
use objc2_foundation::{NSRect, NSSize, NSString};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};
struct Tab {
    path: PathBuf,
    text: Arc<str>,
    scroll: Retained<NSScrollView>,
    view: Retained<NSTextView>,
}
pub struct Documents {
    tabs: VecDeque<Tab>,
    pub active: Option<PathBuf>,
    bytes: usize,
}
impl Documents {
    pub fn new() -> Self {
        Self {
            tabs: VecDeque::new(),
            active: None,
            bytes: 0,
        }
    }
    pub fn show(
        &mut self,
        root: &NSView,
        path: PathBuf,
        text: Arc<str>,
        frame: NSRect,
        mtm: MainThreadMarker,
    ) {
        if let Some(i) = self.tabs.iter().position(|t| t.path == path) {
            let tab = self.tabs.remove(i).unwrap();
            if !Arc::ptr_eq(&tab.text, &text) {
                self.bytes -= tab.text.len();
                tab.scroll.removeFromSuperview();
            } else {
                self.tabs.push_back(tab);
                self.active = Some(path);
                self.layout(frame, true);
                return;
            }
        }
        while self.tabs.len() >= 20 || self.bytes + text.len() > 32 * 1024 * 1024 {
            if let Some(old) = self.tabs.pop_front() {
                self.bytes -= old.text.len();
                old.scroll.removeFromSuperview();
            } else {
                break;
            }
        }
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        let view = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(Default::default(), scroll.contentSize()),
        );
        view.setEditable(false);
        view.setVerticallyResizable(true);
        view.setHorizontallyResizable(false);
        view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        view.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        // SAFETY: native text container remains owned by this text view on the main thread.
        if let Some(container) = unsafe { view.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        view.setString(&NSString::from_str(&text));
        scroll.setDocumentView(Some(&view));
        root.addSubview(&scroll);
        self.bytes += text.len();
        self.active = Some(path.clone());
        self.tabs.push_back(Tab {
            path,
            text,
            scroll,
            view,
        });
        self.layout(frame, true);
    }
    pub fn select_cached(&mut self, path: &Path, frame: NSRect) -> bool {
        let Some(i) = self.tabs.iter().position(|t| t.path == path) else {
            return false;
        };
        let tab = self.tabs.remove(i).unwrap();
        self.active = Some(tab.path.clone());
        self.tabs.push_back(tab);
        self.layout(frame, true);
        true
    }
    pub fn layout(&self, frame: NSRect, visible: bool) {
        for tab in &self.tabs {
            tab.scroll.setFrame(frame);
            tab.scroll
                .setHidden(!visible || self.active.as_ref() != Some(&tab.path));
        }
    }
    pub fn active_text(&self) -> Option<Retained<NSString>> {
        self.tabs
            .iter()
            .find(|t| Some(&t.path) == self.active.as_ref())
            .map(|t| t.view.string())
    }
    pub fn paths(&self) -> Vec<PathBuf> {
        self.tabs.iter().map(|t| t.path.clone()).collect()
    }
}
