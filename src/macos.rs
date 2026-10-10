//! macOS: files opened from Finder ("Open With", double-click, drag onto the
//! Dock icon) arrive as `application:openURLs:` on the app delegate, not in
//! argv. winit 0.30 doesn't surface them, so we add that method to winit's
//! delegate class before the event loop starts and forward the paths.

use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{msg_send, sel};
use std::ffi::{c_char, CStr};
use std::sync::OnceLock;

type Sink = Box<dyn Fn(Vec<String>) + Send + Sync>;
static SINK: OnceLock<Sink> = OnceLock::new();

unsafe extern "C" fn open_urls(_this: *mut AnyObject, _cmd: Sel, _app: *mut AnyObject, urls: *mut AnyObject) {
    let count: usize = msg_send![urls, count];
    let mut paths = Vec::with_capacity(count);
    for i in 0..count {
        let url: *mut AnyObject = msg_send![urls, objectAtIndex: i];
        let is_file: bool = msg_send![url, isFileURL];
        let text: *mut AnyObject = if is_file { msg_send![url, path] } else { msg_send![url, absoluteString] };
        if text.is_null() {
            continue;
        }
        let utf8: *const c_char = msg_send![text, UTF8String];
        if !utf8.is_null() {
            paths.push(CStr::from_ptr(utf8).to_string_lossy().into_owned());
        }
    }
    if let Some(sink) = SINK.get() {
        sink(paths);
    }
}

/// Call after the winit event loop is created (that registers the class)
/// and before it runs.
pub fn install_open_handler(sink: impl Fn(Vec<String>) + Send + Sync + 'static) {
    if SINK.set(Box::new(sink)).is_err() {
        return;
    }
    let Some(cls) = AnyClass::get("WinitApplicationDelegate") else { return };
    unsafe {
        let imp: unsafe extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject) = open_urls;
        objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut objc2::ffi::objc_class,
            sel!(application:openURLs:).as_ptr(),
            Some(std::mem::transmute::<_, unsafe extern "C" fn()>(imp)),
            c"v@:@@".as_ptr(),
        );
    }
}
