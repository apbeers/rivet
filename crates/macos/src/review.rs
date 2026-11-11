use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSForegroundColorAttributeName, NSScrollView, NSTextView,
    NSView,
};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize, NSString};
use rivet_core::diff::Diff;
pub struct Review {
    left: Retained<NSScrollView>,
    right: Retained<NSScrollView>,
    a: Retained<NSTextView>,
    b: Retained<NSTextView>,
    diff: Option<Diff>,
    pub inline: bool,
}
impl Review {
    pub fn new(root: &NSView, mtm: MainThreadMarker) -> Self {
        let build = || {
            let scroll = NSScrollView::new(mtm);
            scroll.setHasVerticalScroller(true);
            let v = NSTextView::initWithFrame(
                NSTextView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 500.0)),
            );
            v.setEditable(false);
            v.setVerticallyResizable(true);
            v.setHorizontallyResizable(false);
            v.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            unsafe {
                if let Some(c) = v.textContainer() {
                    c.setWidthTracksTextView(true);
                }
            }
            scroll.setDocumentView(Some(&v));
            root.addSubview(&scroll);
            (scroll, v)
        };
        let (left, a) = build();
        let (right, b) = build();
        Self {
            left,
            right,
            a,
            b,
            diff: None,
            inline: false,
        }
    }
    pub fn show(&mut self, diff: Diff) {
        self.diff = Some(diff);
        self.render();
    }
    pub fn toggle(&mut self) {
        self.inline = !self.inline;
        self.render();
    }
    fn color(view: &NSTextView, text: &str) {
        if let Some(storage) = unsafe { view.textStorage() } {
            let mut offset = 0;
            for line in text.split_inclusive('\n').take(1024) {
                let len = line.encode_utf16().count();
                let color = if line.starts_with("+ ") {
                    Some(NSColor::systemGreenColor())
                } else if line.starts_with("- ") {
                    Some(NSColor::systemRedColor())
                } else {
                    None
                };
                if let Some(color) = color {
                    unsafe {
                        storage.addAttribute_value_range(
                            NSForegroundColorAttributeName,
                            &color,
                            NSRange::new(offset, len),
                        );
                    }
                }
                offset += len;
            }
        }
    }
    fn render(&self) {
        if let Some(d) = &self.diff {
            self.a.setString(&NSString::from_str(if self.inline {
                &d.inline
            } else {
                &d.left
            }));
            self.b.setString(&NSString::from_str(&d.right));
            Self::color(&self.a, if self.inline { &d.inline } else { &d.left });
            Self::color(&self.b, &d.right);
        }
    }
    pub fn layout(&self, frame: NSRect, visible: bool) {
        self.left.setHidden(!visible);
        self.right.setHidden(!visible || self.inline);
        let mut left = frame;
        if !self.inline {
            left.size.width = (frame.size.width - 8.0) / 2.0;
        }
        self.left.setFrame(left);
        let mut right = left;
        right.origin.x += left.size.width + 8.0;
        self.right.setFrame(right);
    }
}
