//! Native editing surface backed by the first-party UTF-8 piece table.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::*;
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize, NSString};
use rivet_core::editor::{Buffer, Snapshot, TokenKind, tokens};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};
pub(crate) struct State {
    buffer: RefCell<Buffer>,
    base: RefCell<Arc<str>>,
    revision: Cell<u64>,
    saved_revision: Cell<u64>,
}
define_class!(
 // SAFETY: all text storage and piece-table mutation is confined to the main thread.
 #[unsafe(super(NSTextView))] #[thread_kind=MainThreadOnly] #[name="RivetEditorView"] #[ivars=State]
 pub(crate) struct EditorView;
 impl EditorView{
  #[unsafe(method(undo:))] fn undo_action(&self,_sender:Option<&AnyObject>){self.undo_edit();}
  #[unsafe(method(redo:))] fn redo_action(&self,_sender:Option<&AnyObject>){self.redo_edit();}
 }
 unsafe impl NSObjectProtocol for EditorView{}
 unsafe impl NSTextDelegate for EditorView{
 #[unsafe(method(textDidChange:))] fn changed(&self,_notification:&objc2_foundation::NSNotification){self.highlight();}
 }
 unsafe impl NSTextViewDelegate for EditorView{
  #[unsafe(method(textView:shouldChangeTextInRange:replacementString:))]
  fn changing(&self,_view:&NSTextView,range:NSRange,replacement:Option<&NSString>)->bool{
   (|| { let Some(text)=replacement else{return true;};let mut buffer=self.ivars().buffer.borrow_mut();
   let end=range.location.checked_add(range.length);let bytes=end.and_then(|end|Some(buffer.utf16_to_byte(range.location).ok()?..buffer.utf16_to_byte(end).ok()?));
   let Some(bytes)=bytes else{return false;};if buffer.replace(bytes,&text.to_string()).is_err(){return false;}
   self.ivars().revision.set(self.ivars().revision.get()+1);true })()
  }
 }
);
impl EditorView {
    pub fn new(mtm: MainThreadMarker, source: Arc<str>, buffer: Buffer) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(State {
            buffer: RefCell::new(buffer),
            base: RefCell::new(source.clone()),
            revision: Cell::new(0),
            saved_revision: Cell::new(0),
        });
        // SAFETY: designated native initializer after required ivars are installed.
        let this: Retained<Self> = unsafe {
            msg_send![super(this),initWithFrame:NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(700.0,500.0))]
        };
        this.setRichText(false);
        this.setEditable(true);
        this.setAllowsUndo(false);
        this.setVerticallyResizable(true);
        this.setHorizontallyResizable(false);
        this.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        this.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        unsafe {
            if let Some(container) = this.textContainer() {
                container.setWidthTracksTextView(true);
            }
        }
        this.setString(&NSString::from_str(&source));
        this.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(13.0, 0.0)));
        this.setDelegate(Some(ProtocolObject::from_ref(&*this)));
        this.highlight();
        this
    }
    pub fn snapshot(&self) -> Snapshot {
        self.ivars().buffer.borrow().snapshot()
    }
    pub fn base(&self) -> Arc<str> {
        self.ivars().base.borrow().clone()
    }
    pub fn revision(&self) -> u64 {
        self.ivars().revision.get()
    }
    pub fn is_dirty(&self) -> bool {
        self.revision() != self.ivars().saved_revision.get()
    }
    pub fn mark_saved(&self, text: Arc<str>, revision: u64) {
        *self.ivars().base.borrow_mut() = text;
        self.ivars().saved_revision.set(revision);
    }
    fn refresh(&self) {
        let text = self.snapshot().text();
        self.setString(&NSString::from_str(&text));
        self.ivars().revision.set(self.revision() + 1);
        self.highlight();
    }
    pub fn undo_edit(&self) {
        let changed = self.ivars().buffer.borrow_mut().undo();
        if changed {
            self.refresh();
        }
    }
    pub fn redo_edit(&self) {
        let changed = self.ivars().buffer.borrow_mut().redo();
        if changed {
            self.refresh();
        }
    }
    pub fn revert(&self) {
        let base = self.base();
        let len = self.ivars().buffer.borrow().len();
        let changed = self
            .ivars()
            .buffer
            .borrow_mut()
            .replace(0..len, &base)
            .is_ok();
        if changed {
            self.refresh();
            self.ivars().saved_revision.set(self.revision());
        }
    }
    pub fn find(&self, query: &str) -> bool {
        let buffer = self.ivars().buffer.borrow();
        let Some(range) = buffer.find(query) else {
            return false;
        };
        let Ok(start) = buffer.byte_to_utf16(range.start) else {
            return false;
        };
        let Ok(end) = buffer.byte_to_utf16(range.end) else {
            return false;
        };
        let range = NSRange::new(start, end - start);
        self.setSelectedRange(range);
        self.scrollRangeToVisible(range);
        true
    }
    pub fn highlight(&self) {
        let buffer = self.ivars().buffer.borrow();
        let mut end = buffer.len().min(8192);
        while end > 0 && buffer.byte_to_utf16(end).is_err() {
            end -= 1;
        }
        let Ok(text) = buffer.read(0..end) else {
            return;
        };
        // Bounded lexical coloring; no parsing or whole-file work in draw callbacks.
        if let Some(storage) = unsafe { self.textStorage() } {
            for (range, kind) in tokens(&text) {
                let Ok(a) = buffer.byte_to_utf16(range.start) else {
                    continue;
                };
                let Ok(b) = buffer.byte_to_utf16(range.end) else {
                    continue;
                };
                let color = match kind {
                    TokenKind::Comment => NSColor::systemGreenColor(),
                    TokenKind::String => NSColor::systemRedColor(),
                    TokenKind::Number => NSColor::systemOrangeColor(),
                    TokenKind::Keyword => NSColor::systemPurpleColor(),
                };
                // SAFETY: the color is a valid value for the native foreground-color attribute.
                unsafe {
                    storage.addAttribute_value_range(
                        NSForegroundColorAttributeName,
                        &color,
                        NSRange::new(a, b - a),
                    );
                }
            }
        }
    }
}
