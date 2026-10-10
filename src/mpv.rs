//! Thin, dependency-free bindings to libmpv's client + render API.
//! Only what VELA needs; everything is converted to owned Rust values at
//! the boundary so no mpv-owned memory escapes.
#![allow(non_camel_case_types)]

use serde_json::Value;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;

#[repr(C)]
pub struct mpv_handle {
    _p: [u8; 0],
}
#[repr(C)]
pub struct mpv_render_context {
    _p: [u8; 0],
}

pub const FMT_NONE: c_int = 0;
pub const FMT_STRING: c_int = 1;
pub const FMT_FLAG: c_int = 3;
pub const FMT_INT64: c_int = 4;
pub const FMT_DOUBLE: c_int = 5;
pub const FMT_NODE: c_int = 6;
pub const FMT_NODE_ARRAY: c_int = 7;
pub const FMT_NODE_MAP: c_int = 8;

#[repr(C)]
#[derive(Clone, Copy)]
pub union mpv_node_u {
    pub string: *mut c_char,
    pub flag: c_int,
    pub int64: i64,
    pub double_: f64,
    pub list: *mut mpv_node_list,
}
#[repr(C)]
pub struct mpv_node {
    pub u: mpv_node_u,
    pub format: c_int,
}
#[repr(C)]
pub struct mpv_node_list {
    pub num: c_int,
    pub values: *mut mpv_node,
    pub keys: *mut *mut c_char,
}
#[repr(C)]
pub struct mpv_event {
    pub event_id: c_int,
    pub error: c_int,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}
#[repr(C)]
pub struct mpv_event_property {
    pub name: *const c_char,
    pub format: c_int,
    pub data: *mut c_void,
}
#[repr(C)]
pub struct mpv_event_end_file {
    pub reason: c_int,
    pub error: c_int,
    pub playlist_entry_id: i64,
    pub playlist_insert_id: i64,
    pub playlist_insert_num_entries: c_int,
}
#[repr(C)]
pub struct mpv_event_log_message {
    pub prefix: *const c_char,
    pub level: *const c_char,
    pub text: *const c_char,
    pub log_level: c_int,
}
#[repr(C)]
pub struct mpv_event_hook {
    pub name: *const c_char,
    pub id: u64,
}
#[repr(C)]
pub struct mpv_render_param {
    pub type_: c_int,
    pub data: *mut c_void,
}
#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}
#[repr(C)]
pub struct mpv_opengl_fbo {
    pub fbo: c_int,
    pub w: c_int,
    pub h: c_int,
    pub internal_format: c_int,
}

pub const RENDER_PARAM_INVALID: c_int = 0;
pub const RENDER_PARAM_API_TYPE: c_int = 1;
pub const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
pub const RENDER_PARAM_OPENGL_FBO: c_int = 3;
pub const RENDER_PARAM_FLIP_Y: c_int = 4;
pub const RENDER_PARAM_ADVANCED_CONTROL: c_int = 10;
pub const RENDER_PARAM_SW_SIZE: c_int = 17;
pub const RENDER_PARAM_SW_FORMAT: c_int = 18;
pub const RENDER_PARAM_SW_STRIDE: c_int = 19;
pub const RENDER_PARAM_SW_POINTER: c_int = 20;
pub const RENDER_UPDATE_FRAME: u64 = 1;

#[link(name = "mpv")]
extern "C" {
    fn mpv_client_api_version() -> std::ffi::c_ulong;
    fn mpv_error_string(error: c_int) -> *const c_char;
    fn mpv_free(data: *mut c_void);
    fn mpv_create() -> *mut mpv_handle;
    fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut mpv_handle);
    fn mpv_set_option_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_command(ctx: *mut mpv_handle, args: *mut *const c_char) -> c_int;
    fn mpv_set_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_set_property_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_get_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_observe_property(ctx: *mut mpv_handle, reply_userdata: u64, name: *const c_char, format: c_int) -> c_int;
    fn mpv_request_log_messages(ctx: *mut mpv_handle, min_level: *const c_char) -> c_int;
    fn mpv_wait_event(ctx: *mut mpv_handle, timeout: f64) -> *mut mpv_event;
    fn mpv_set_wakeup_callback(ctx: *mut mpv_handle, cb: Option<unsafe extern "C" fn(*mut c_void)>, d: *mut c_void);
    fn mpv_free_node_contents(node: *mut mpv_node);
    fn mpv_hook_add(ctx: *mut mpv_handle, reply_userdata: u64, name: *const c_char, priority: c_int) -> c_int;
    fn mpv_hook_continue(ctx: *mut mpv_handle, id: u64) -> c_int;

    fn mpv_render_context_create(res: *mut *mut mpv_render_context, mpv: *mut mpv_handle, params: *mut mpv_render_param) -> c_int;
    fn mpv_render_context_set_update_callback(ctx: *mut mpv_render_context, cb: Option<unsafe extern "C" fn(*mut c_void)>, d: *mut c_void);
    fn mpv_render_context_update(ctx: *mut mpv_render_context) -> u64;
    fn mpv_render_context_render(ctx: *mut mpv_render_context, params: *mut mpv_render_param) -> c_int;
    fn mpv_render_context_report_swap(ctx: *mut mpv_render_context);
    fn mpv_render_context_free(ctx: *mut mpv_render_context);
}

pub fn error_string(code: c_int) -> String {
    unsafe { CStr::from_ptr(mpv_error_string(code)).to_string_lossy().into_owned() }
}

pub fn api_version() -> u64 {
    unsafe { mpv_client_api_version() as u64 }
}

#[derive(Debug, Clone)]
pub enum Event {
    Shutdown,
    Log { level: String, prefix: String, text: String },
    StartFile,
    EndFile { reason: i32, error: i32 },
    FileLoaded,
    PlaybackRestart,
    Seek,
    VideoReconfig,
    AudioReconfig,
    Property { name: String, value: Value },
    Hook { userdata: u64, id: u64 },
    Other,
}

pub const END_ERROR: i32 = 4;

/// A libmpv client handle. The client API is thread-safe.
pub struct Mpv {
    h: *mut mpv_handle,
    wakeup_box: Option<usize>,
}
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

impl Mpv {
    pub fn new() -> Result<Self, String> {
        let h = unsafe { mpv_create() };
        if h.is_null() {
            return Err("mpv_create failed".into());
        }
        Ok(Self { h, wakeup_box: None })
    }

    pub fn raw(&self) -> *mut mpv_handle {
        self.h
    }

    pub fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        match unsafe { mpv_set_option_string(self.h, n.as_ptr(), v.as_ptr()) } {
            0 => Ok(()),
            e => Err(format!("{name}={value}: {}", error_string(e))),
        }
    }

    pub fn initialize(&self) -> Result<(), String> {
        match unsafe { mpv_initialize(self.h) } {
            0 => Ok(()),
            e => Err(error_string(e)),
        }
    }

    fn with_args<R>(args: &[&str], f: impl FnOnce(*mut *const c_char) -> R) -> R {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        f(ptrs.as_mut_ptr())
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        match Self::with_args(args, |p| unsafe { mpv_command(self.h, p) }) {
            0 => Ok(()),
            e => Err(format!("{:?}: {}", args, error_string(e))),
        }
    }

    pub fn set_str(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        match unsafe { mpv_set_property_string(self.h, n.as_ptr(), v.as_ptr()) } {
            0 => Ok(()),
            e => Err(format!("{name}={value}: {}", error_string(e))),
        }
    }

    pub fn set_flag(&self, name: &str, value: bool) {
        let n = cstr(name);
        let mut v: c_int = value as c_int;
        unsafe { mpv_set_property(self.h, n.as_ptr(), FMT_FLAG, &mut v as *mut _ as *mut c_void) };
    }

    pub fn set_f64(&self, name: &str, value: f64) {
        let n = cstr(name);
        let mut v = value;
        unsafe { mpv_set_property(self.h, n.as_ptr(), FMT_DOUBLE, &mut v as *mut _ as *mut c_void) };
    }

    pub fn set_i64(&self, name: &str, value: i64) {
        let n = cstr(name);
        let mut v = value;
        unsafe { mpv_set_property(self.h, n.as_ptr(), FMT_INT64, &mut v as *mut _ as *mut c_void) };
    }

    pub fn get_str(&self, name: &str) -> Option<String> {
        let n = cstr(name);
        let mut out: *mut c_char = ptr::null_mut();
        let r = unsafe { mpv_get_property(self.h, n.as_ptr(), FMT_STRING, &mut out as *mut _ as *mut c_void) };
        if r < 0 || out.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(out).to_string_lossy().into_owned() };
        unsafe { mpv_free(out as *mut c_void) };
        Some(s)
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let n = cstr(name);
        let mut v = 0f64;
        let r = unsafe { mpv_get_property(self.h, n.as_ptr(), FMT_DOUBLE, &mut v as *mut _ as *mut c_void) };
        (r >= 0).then_some(v)
    }

    pub fn get_i64(&self, name: &str) -> Option<i64> {
        let n = cstr(name);
        let mut v = 0i64;
        let r = unsafe { mpv_get_property(self.h, n.as_ptr(), FMT_INT64, &mut v as *mut _ as *mut c_void) };
        (r >= 0).then_some(v)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        let n = cstr(name);
        let mut v: c_int = 0;
        let r = unsafe { mpv_get_property(self.h, n.as_ptr(), FMT_FLAG, &mut v as *mut _ as *mut c_void) };
        (r >= 0).then_some(v != 0)
    }

    pub fn get_node(&self, name: &str) -> Option<Value> {
        let n = cstr(name);
        let mut node = mpv_node { u: mpv_node_u { int64: 0 }, format: FMT_NONE };
        let r = unsafe { mpv_get_property(self.h, n.as_ptr(), FMT_NODE, &mut node as *mut _ as *mut c_void) };
        if r < 0 {
            return None;
        }
        let v = unsafe { node_to_value(&node) };
        unsafe { mpv_free_node_contents(&mut node) };
        Some(v)
    }

    pub fn observe(&self, id: u64, name: &str) {
        let n = cstr(name);
        unsafe { mpv_observe_property(self.h, id, n.as_ptr(), FMT_NODE) };
    }

    pub fn request_log(&self, level: &str) {
        let l = cstr(level);
        unsafe { mpv_request_log_messages(self.h, l.as_ptr()) };
    }

    pub fn hook_add(&self, userdata: u64, name: &str, priority: i32) {
        let n = cstr(name);
        unsafe { mpv_hook_add(self.h, userdata, n.as_ptr(), priority) };
    }

    pub fn hook_continue(&self, id: u64) {
        unsafe { mpv_hook_continue(self.h, id) };
    }

    /// Install a wakeup callback (called from mpv's threads when events are pending).
    pub fn set_wakeup(&mut self, f: Box<dyn Fn() + Send + Sync>) {
        unsafe extern "C" fn tramp(d: *mut c_void) {
            let f = &*(d as *const Box<dyn Fn() + Send + Sync>);
            f();
        }
        // Boxed twice so a thin pointer can cross the C boundary; freed in Drop.
        let raw = Box::into_raw(Box::new(f));
        unsafe { mpv_set_wakeup_callback(self.h, Some(tramp), raw as *mut c_void) };
        if let Some(old) = self.wakeup_box.replace(raw as usize) {
            unsafe { drop(Box::from_raw(old as *mut Box<dyn Fn() + Send + Sync>)) };
        }
    }

    /// Non-blocking event poll. Returns None when the queue is empty.
    pub fn next_event(&self, timeout: f64) -> Option<Event> {
        let ev = unsafe { &*mpv_wait_event(self.h, timeout) };
        if ev.event_id == 0 {
            return None;
        }
        Some(unsafe { convert_event(ev) })
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe {
            mpv_set_wakeup_callback(self.h, None, ptr::null_mut());
            mpv_terminate_destroy(self.h);
            if let Some(raw) = self.wakeup_box.take() {
                drop(Box::from_raw(raw as *mut Box<dyn Fn() + Send + Sync>));
            }
        }
    }
}

unsafe fn cstr_lossy(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

unsafe fn node_to_value(n: &mpv_node) -> Value {
    match n.format {
        FMT_STRING => Value::String(cstr_lossy(n.u.string)),
        FMT_FLAG => Value::Bool(n.u.flag != 0),
        FMT_INT64 => Value::from(n.u.int64),
        FMT_DOUBLE => serde_json::Number::from_f64(n.u.double_).map(Value::Number).unwrap_or(Value::Null),
        FMT_NODE_ARRAY => {
            let l = &*n.u.list;
            let mut out = Vec::with_capacity(l.num as usize);
            for i in 0..l.num as usize {
                out.push(node_to_value(&*l.values.add(i)));
            }
            Value::Array(out)
        }
        FMT_NODE_MAP => {
            let l = &*n.u.list;
            let mut out = serde_json::Map::with_capacity(l.num as usize);
            for i in 0..l.num as usize {
                out.insert(cstr_lossy(*l.keys.add(i)), node_to_value(&*l.values.add(i)));
            }
            Value::Object(out)
        }
        _ => Value::Null,
    }
}

unsafe fn convert_event(ev: &mpv_event) -> Event {
    match ev.event_id {
        1 => Event::Shutdown,
        2 => {
            let m = &*(ev.data as *const mpv_event_log_message);
            Event::Log { level: cstr_lossy(m.level), prefix: cstr_lossy(m.prefix), text: cstr_lossy(m.text) }
        }
        6 => Event::StartFile,
        7 => {
            let e = &*(ev.data as *const mpv_event_end_file);
            Event::EndFile { reason: e.reason, error: e.error }
        }
        8 => Event::FileLoaded,
        17 => Event::VideoReconfig,
        18 => Event::AudioReconfig,
        20 => Event::Seek,
        21 => Event::PlaybackRestart,
        22 => {
            let p = &*(ev.data as *const mpv_event_property);
            let value = if p.format == FMT_NODE && !p.data.is_null() { node_to_value(&*(p.data as *const mpv_node)) } else { Value::Null };
            Event::Property { name: cstr_lossy(p.name), value }
        }
        25 => {
            let h = &*(ev.data as *const mpv_event_hook);
            Event::Hook { userdata: ev.reply_userdata, id: h.id }
        }
        _ => Event::Other,
    }
}

// ---------------------------------------------------------------------------
// Render API

pub type GetProcAddress = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;

pub struct RenderContext {
    ctx: *mut mpv_render_context,
    update_box: Option<usize>,
}
unsafe impl Send for RenderContext {}

impl RenderContext {
    /// OpenGL renderer. `proc_ctx` must outlive the context.
    pub fn new_gl(mpv: &Mpv, get_proc: GetProcAddress, proc_ctx: *mut c_void) -> Result<Self, String> {
        let api = cstr("opengl");
        let mut init = mpv_opengl_init_params { get_proc_address: Some(get_proc), get_proc_address_ctx: proc_ctx };
        let mut advanced: c_int = 0;
        let mut params = [
            mpv_render_param { type_: RENDER_PARAM_API_TYPE, data: api.as_ptr() as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_ADVANCED_CONTROL, data: &mut advanced as *mut _ as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        Self::create(mpv, &mut params)
    }

    /// Software renderer (used for off-screen thumbnails).
    pub fn new_sw(mpv: &Mpv) -> Result<Self, String> {
        let api = cstr("sw");
        let mut params = [
            mpv_render_param { type_: RENDER_PARAM_API_TYPE, data: api.as_ptr() as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        Self::create(mpv, &mut params)
    }

    fn create(mpv: &Mpv, params: &mut [mpv_render_param]) -> Result<Self, String> {
        let mut ctx: *mut mpv_render_context = ptr::null_mut();
        let r = unsafe { mpv_render_context_create(&mut ctx, mpv.raw(), params.as_mut_ptr()) };
        if r < 0 {
            return Err(error_string(r));
        }
        Ok(Self { ctx, update_box: None })
    }

    pub fn set_update_callback(&mut self, f: Box<dyn Fn() + Send + Sync>) {
        unsafe extern "C" fn tramp(d: *mut c_void) {
            let f = &*(d as *const Box<dyn Fn() + Send + Sync>);
            f();
        }
        let raw = Box::into_raw(Box::new(f));
        unsafe { mpv_render_context_set_update_callback(self.ctx, Some(tramp), raw as *mut c_void) };
        if let Some(old) = self.update_box.replace(raw as usize) {
            unsafe { drop(Box::from_raw(old as *mut Box<dyn Fn() + Send + Sync>)) };
        }
    }

    pub fn update(&self) -> u64 {
        unsafe { mpv_render_context_update(self.ctx) }
    }

    pub fn render_gl(&self, fbo: i32, w: i32, h: i32, flip_y: bool) {
        let mut target = mpv_opengl_fbo { fbo, w, h, internal_format: 0 };
        let mut flip: c_int = flip_y as c_int;
        let mut params = [
            mpv_render_param { type_: RENDER_PARAM_OPENGL_FBO, data: &mut target as *mut _ as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_FLIP_Y, data: &mut flip as *mut _ as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        unsafe { mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
    }

    /// Render into a caller-provided RGBX buffer (stride = w*4).
    pub fn render_sw(&self, w: i32, h: i32, buf: &mut [u8]) -> bool {
        let mut size: [c_int; 2] = [w, h];
        let fmt = cstr("rgb0");
        let mut stride: usize = (w as usize) * 4;
        let mut params = [
            mpv_render_param { type_: RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr() as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_SW_FORMAT, data: fmt.as_ptr() as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_SW_STRIDE, data: &mut stride as *mut _ as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_SW_POINTER, data: buf.as_mut_ptr() as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        unsafe { mpv_render_context_render(self.ctx, params.as_mut_ptr()) >= 0 }
    }

    pub fn report_swap(&self) {
        unsafe { mpv_render_context_report_swap(self.ctx) };
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe {
            mpv_render_context_set_update_callback(self.ctx, None, ptr::null_mut());
            mpv_render_context_free(self.ctx);
            if let Some(raw) = self.update_box.take() {
                drop(Box::from_raw(raw as *mut Box<dyn Fn() + Send + Sync>));
            }
        }
    }
}
