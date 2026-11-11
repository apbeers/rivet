use super::editor::EditorView;
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{NSScrollView, NSView};
use objc2_foundation::{NSRect, NSString};
use rivet_core::editor::Buffer;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};
struct Tab {
    path: PathBuf,
    text: Arc<str>,
    scroll: Retained<NSScrollView>,
    view: Retained<EditorView>,
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
        buffer: Option<Buffer>,
        frame: NSRect,
        mtm: MainThreadMarker,
    ) -> Result<(), &'static str> {
        if let Some(i) = self.tabs.iter().position(|t| t.path == path) {
            let tab = self.tabs.remove(i).unwrap();
            if !Arc::ptr_eq(&tab.text, &text) && !tab.view.is_dirty() {
                self.bytes -= tab.text.len();
                tab.scroll.removeFromSuperview();
            } else {
                self.tabs.push_back(tab);
                self.active = Some(path);
                self.layout(frame, true);
                return Ok(());
            }
        }
        if self.tabs.iter().filter(|t| t.view.is_dirty()).count() >= 20
            || self
                .tabs
                .iter()
                .filter(|t| t.view.is_dirty())
                .map(|t| t.text.len())
                .sum::<usize>()
                + text.len()
                > 32 * 1024 * 1024
        {
            return Err(
                "Document cache full of unsaved edits; save or reject a document before opening another",
            );
        }
        while self.tabs.len() >= 20 || self.bytes + text.len() > 32 * 1024 * 1024 {
            if let Some(index) = self.tabs.iter().position(|t| !t.view.is_dirty()) {
                let old = self.tabs.remove(index).unwrap();
                self.bytes -= old.text.len();
                old.scroll.removeFromSuperview();
            } else {
                return Err("Document cache full of unsaved edits");
            }
        }
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        let view = EditorView::new(
            mtm,
            text.clone(),
            buffer.unwrap_or_else(|| Buffer::new(text.clone())),
        );
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
        Ok(())
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
    pub fn any_dirty(&self) -> bool {
        self.tabs.iter().any(|t| t.view.is_dirty())
    }
    pub fn active_editor_retained(&self) -> Option<Retained<EditorView>> {
        self.tabs
            .iter()
            .find(|t| Some(&t.path) == self.active.as_ref())
            .map(|t| t.view.clone())
    }
    pub fn active_editor(&self) -> Option<&EditorView> {
        self.tabs
            .iter()
            .find(|t| Some(&t.path) == self.active.as_ref())
            .map(|t| &*t.view)
    }
    pub fn saved(&self, path: &Path, text: Arc<str>, revision: u64) {
        if let Some(tab) = self.tabs.iter().find(|t| t.path == path) {
            tab.view.mark_saved(text, revision);
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
