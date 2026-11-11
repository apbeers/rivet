//! Read-only console display with direct PTY keystrokes, so password echo is owned by the TTY.
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::AnyObject,
};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSPasteboard, NSPasteboardTypeString, NSTextView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::{cell::RefCell, sync::mpsc::SyncSender};
pub(crate) struct State {
    input: RefCell<Option<SyncSender<Vec<u8>>>>,
}
define_class!(
 // SAFETY: all native key/clipboard access is on the main thread; only bytes cross the channel.
 #[unsafe(super(NSTextView))] #[thread_kind=MainThreadOnly] #[name="RivetConsoleView"] #[ivars=State]
 pub(crate) struct Console;
 impl Console{
  #[unsafe(method(keyDown:))] fn key_down(&self,event:&NSEvent){
   if self.ivars().input.borrow().is_none()||event.modifierFlags().contains(NSEventModifierFlags::Command){unsafe{let _:()=msg_send![super(self),keyDown:event];}}else{
    let bytes=match event.keyCode(){123=>b"\x1b[D".to_vec(),124=>b"\x1b[C".to_vec(),125=>b"\x1b[B".to_vec(),126=>b"\x1b[A".to_vec(),51=>vec![127],_=>event.characters().map(|s|s.to_string().into_bytes()).unwrap_or_default()};self.write(bytes);
   }
  }
  #[unsafe(method(paste:))] fn paste_action(&self,_sender:Option<&AnyObject>){if self.ivars().input.borrow().is_some() && let Some(text)=unsafe{NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString)}{if text.length()<=4096{self.write(text.to_string().into_bytes());}else{objc2_app_kit::NSBeep();}}}
 }
);
impl Console {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(State {
            input: RefCell::new(None),
        });
        unsafe {
            msg_send![super(this),initWithFrame:NSRect::new(NSPoint::new(0.0,0.0),NSSize::new(800.0,400.0))]
        }
    }
    pub fn bind(&self, sender: Option<SyncSender<Vec<u8>>>) {
        *self.ivars().input.borrow_mut() = sender;
    }
    fn write(&self, bytes: Vec<u8>) {
        if bytes.len() > 4096 {
            objc2_app_kit::NSBeep();
            return;
        }
        if let Some(tx) = self.ivars().input.borrow().as_ref()
            && tx.try_send(bytes).is_err()
        {
            objc2_app_kit::NSBeep();
        }
    }
}
