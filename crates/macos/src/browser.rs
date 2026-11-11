//! Native browser instances are retained across view switches, never used as shell UI.
use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::NSView;
use objc2_foundation::{NSRect, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
use std::collections::VecDeque;
pub struct Browsers {
    tabs: VecDeque<(u64, Retained<WKWebView>)>,
    next: u64,
    pub active: Option<u64>,
}
impl Browsers {
    pub fn new() -> Self {
        Self {
            tabs: VecDeque::new(),
            next: 1,
            active: None,
        }
    }
    pub fn create(&mut self, root: &NSView, frame: NSRect, mtm: MainThreadMarker) -> u64 {
        if self.tabs.len() == 5
            && let Some((_, old)) = self.tabs.pop_front()
        {
            unsafe {
                old.stopLoading();
            }
            old.removeFromSuperview();
        }
        // SAFETY: default configuration is created on AppKit's main thread.
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        // SAFETY: main-thread initialization with a valid retained WebKit configuration.
        let view = unsafe {
            WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &configuration)
        };
        root.addSubview(&view);
        let id = self.next;
        self.next += 1;
        self.tabs.push_back((id, view));
        self.active = Some(id);
        id
    }
    pub fn ids(&self) -> Vec<u64> {
        self.tabs.iter().map(|(id, _)| *id).collect()
    }
    pub fn select(&mut self, id: u64) {
        if let Some(i) = self.tabs.iter().position(|(key, _)| *key == id) {
            let tab = self.tabs.remove(i).unwrap();
            self.tabs.push_back(tab);
            self.active = Some(id);
        }
    }
    pub fn active_view(&self) -> Option<&WKWebView> {
        self.tabs
            .iter()
            .find(|(id, _)| Some(*id) == self.active)
            .map(|(_, v)| &**v)
    }
    pub fn layout(&self, frame: NSRect, visible: bool) {
        for (id, v) in &self.tabs {
            v.setFrame(frame);
            v.setHidden(!visible || Some(*id) != self.active);
        }
    }
    pub fn navigate(&self, address: &str) -> Result<(), String> {
        let address = address.trim();
        if !(address.starts_with("https://")
            || address.starts_with("http://")
            || address == "about:blank")
        {
            return Err("Use an http:// or https:// address (including localhost)".into());
        }
        let url = NSURL::URLWithString(&NSString::from_str(address)).ok_or("Invalid URL")?;
        let request = NSURLRequest::requestWithURL(&url);
        let view = self.active_view().ok_or("Create a browser tab first")?;
        // SAFETY: WebKit owns asynchronous navigation; no privileged script/message bridge is installed.
        unsafe {
            view.loadRequest(&request);
        }
        Ok(())
    }
    pub fn back(&self) {
        if let Some(v) = self.active_view() {
            unsafe {
                v.goBack();
            }
        }
    }
    pub fn forward(&self) {
        if let Some(v) = self.active_view() {
            unsafe {
                v.goForward();
            }
        }
    }
    pub fn reload(&self) {
        if let Some(v) = self.active_view() {
            unsafe {
                v.reload();
            }
        }
    }
}
