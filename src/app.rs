//! Application shell: window + GL context, the render loop, input routing,
//! timers and IPC. Rendering is strictly on demand: a frame is drawn only when
//! mpv has a new video frame, the UI changed, or an animation is running.

use crate::ipc;
use crate::mpv::{self, Event, Mpv, RenderContext};
use crate::platform::{self, is_media, is_subtitle, is_url};
use crate::player::{self, State, HOOK_LOAD, HOOK_UNLOAD};
use crate::store::Store;
use crate::thumbs::{self, Thumbnailer};
use crate::ui::{self, Action, Menu, UiState, View};
use crate::logf;

use glow::HasContext;
use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext, Version};
use glutin::display::{Display, GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CStr};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey, PhysicalKey};
use winit::window::{CursorIcon, Fullscreen, ResizeDirection, Window, WindowAttributes, WindowId, WindowLevel};

#[derive(Debug)]
pub enum UserEvent {
    Mpv,
    /// Files handed over by the OS (macOS Finder open events).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Open(Vec<String>),
    Frame,
    Ipc(ipc::Request),
    Thumbs,
}

struct Gfx {
    window: Window,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    gl: Arc<glow::Context>,
    painter: egui_glow::Painter,
    egui_state: egui_winit::State,
    render: Option<RenderContext>,
    _display: Box<Display>,
}

unsafe extern "C" fn gl_proc(ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    let display = &*(ctx as *const Display);
    display.get_proc_address(CStr::from_ptr(name)) as *mut c_void
}

enum OpenMode {
    Replace,
    Append,
}

const HIDE_AFTER: Duration = Duration::from_millis(2200);
const BURST: Duration = Duration::from_millis(160);

pub struct App {
    proxy: EventLoopProxy<UserEvent>,
    gfx: Option<Gfx>,
    egui: egui::Context,
    mpv_init: Option<std::thread::JoinHandle<Result<Mpv, String>>>,
    mpv: Option<Mpv>,
    st: State,
    store: Store,
    ui: UiState,
    thumbs: Thumbnailer,
    mpv_flag: Arc<AtomicBool>,
    frame_flag: Arc<AtomicBool>,

    posters: HashMap<String, (String, egui::TextureHandle)>,
    poster_ids: HashMap<String, egui::TextureId>,
    poster_requested: HashSet<String>,
    hover_tex: Option<(egui::TextureHandle, egui::Vec2, f64)>,
    last_hover_req: Option<f64>,

    pending: Vec<String>,
    pending_mode: OpenMode,
    pending_at: Option<Instant>,
    dropped: Vec<PathBuf>,
    drop_at: Option<Instant>,

    last_activity: Instant,
    controls: f32,
    interacting: bool,
    cursor: Option<PhysicalPosition<f64>>,
    cursor_hidden: bool,
    modifiers: ModifiersState,
    resize_dir: Option<ResizeDirection>,

    osd: Option<(String, Instant)>,
    error: Option<String>,
    loading_since: Option<Instant>,
    resume_note: Option<String>,
    sleep_at: Option<Instant>,
    last_record: Instant,
    last_flush: Instant,
    repaint_at: Option<Instant>,
    last_frame: Instant,
    shown: bool,
    awake: bool,
    on_top: bool,
    shot: Vec<(PathBuf, mpsc::Sender<String>)>,
    quit_requested: bool,
    load_started: Option<Instant>,
    await_first_frame: bool,
    idle_since: Option<Instant>,
    trimmed: bool,
    sub_pos_applied: i64,
}

impl App {
    pub fn new(proxy: EventLoopProxy<UserEvent>, paths: Vec<String>) -> Self {
        let store = Store::load();
        let settings = store.settings.clone();
        let shots = platform::pictures_dir().join("VELA").to_string_lossy().into_owned();
        // Start libmpv on a worker thread while the window and GL context come up.
        let mpv_init = std::thread::spawn(move || player::create(&settings, &shots));
        let thumbs_proxy = proxy.clone();
        let thumbs = Thumbnailer::new(Arc::new(move || {
            let _ = thumbs_proxy.send_event(UserEvent::Thumbs);
        }));
        let now = Instant::now();
        Self {
            proxy,
            gfx: None,
            egui: egui::Context::default(),
            mpv_init: Some(mpv_init),
            mpv: None,
            st: State::default(),
            store,
            ui: UiState::default(),
            thumbs,
            mpv_flag: Arc::new(AtomicBool::new(false)),
            frame_flag: Arc::new(AtomicBool::new(false)),
            posters: HashMap::new(),
            poster_ids: HashMap::new(),
            poster_requested: HashSet::new(),
            hover_tex: None,
            last_hover_req: None,
            pending: paths,
            pending_mode: OpenMode::Replace,
            pending_at: Some(now + BURST),
            dropped: vec![],
            drop_at: None,
            last_activity: now,
            controls: 1.0,
            interacting: false,
            cursor: None,
            cursor_hidden: false,
            modifiers: ModifiersState::empty(),
            resize_dir: None,
            osd: None,
            error: None,
            loading_since: None,
            resume_note: None,
            sleep_at: None,
            last_record: now,
            last_flush: now,
            repaint_at: None,
            last_frame: now,
            shown: false,
            awake: false,
            on_top: false,
            shot: vec![],
            quit_requested: false,
            load_started: None,
            await_first_frame: false,
            idle_since: Some(now),
            trimmed: false,
            sub_pos_applied: i64::MIN,
        }
    }

    fn mpv(&self) -> &Mpv {
        self.mpv.as_ref().expect("mpv not ready")
    }

    fn request_redraw(&self) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }

    fn osd(&mut self, text: impl Into<String>) {
        self.osd = Some((text.into(), Instant::now()));
        self.request_redraw();
    }

    fn home(&self) -> bool {
        self.st.idle
    }

    fn fullscreen(&self) -> bool {
        self.gfx.as_ref().map_or(false, |g| g.window.fullscreen().is_some())
    }

    // -----------------------------------------------------------------------
    // Startup

    fn create_window(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let cfg = &self.store.settings;
        let (pos, size) = initial_geometry(el, cfg.window);
        let mut attrs: WindowAttributes = Window::default_attributes()
            .with_title("VELA")
            .with_inner_size(size)
            .with_min_inner_size(LogicalSize::new(480.0, 280.0))
            .with_visible(false)
            .with_theme(Some(winit::window::Theme::Dark));
        if let Some(pos) = pos {
            attrs = attrs.with_position(pos);
        }
        attrs = platform_window(attrs);
        if cfg.maximized {
            attrs = attrs.with_maximized(true);
        }

        let template = ConfigTemplateBuilder::new().with_alpha_size(8).with_depth_size(0).with_stencil_size(0);
        let (window, config) = DisplayBuilder::new()
            .with_window_attributes(Some(attrs))
            .build(el, template, |configs| configs.reduce(|a, b| if b.num_samples() < a.num_samples() { b } else { a }).unwrap())
            .map_err(|e| format!("display: {e}"))?;
        let window = window.ok_or("no window")?;
        let raw = window.window_handle().map_err(|e| e.to_string())?.as_raw();
        let display = Box::new(config.display());
        let ctx_attrs = [
            ContextAttributesBuilder::new().with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3)))).build(Some(raw)),
            ContextAttributesBuilder::new().build(Some(raw)),
        ];
        let mut not_current = None;
        for a in &ctx_attrs {
            if let Ok(c) = unsafe { display.create_context(&config, a) } {
                not_current = Some(c);
                break;
            }
        }
        let not_current = not_current.ok_or("no OpenGL context")?;
        let surf_attrs = window.build_surface_attributes(Default::default()).map_err(|e| e.to_string())?;
        let surface = unsafe { display.create_window_surface(&config, &surf_attrs) }.map_err(|e| e.to_string())?;
        let context = not_current.make_current(&surface).map_err(|e| e.to_string())?;
        let _ = surface.set_swap_interval(&context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

        let gl = Arc::new(unsafe { glow::Context::from_loader_function_cstr(|s| display.get_proc_address(s)) });
        let painter = egui_glow::Painter::new(gl.clone(), "", None, false).map_err(|e| e.to_string())?;
        ui::setup(&self.egui);
        let egui_state = egui_winit::State::new(
            self.egui.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            Some(winit::window::Theme::Dark),
            Some(painter.max_texture_side()),
        );

        // libmpv (initialised in parallel) + its GL renderer
        let mut mpv = self.mpv_init.take().ok_or("mpv init lost")?.join().map_err(|_| "mpv thread panicked")??;
        logf!("libmpv api {:x}", mpv::api_version());
        let flag = self.mpv_flag.clone();
        let proxy = self.proxy.clone();
        mpv.set_wakeup(Box::new(move || {
            if !flag.swap(true, Ordering::AcqRel) {
                let _ = proxy.send_event(UserEvent::Mpv);
            }
        }));
        let display_ptr = &*display as *const Display as *mut c_void;
        let mut render = RenderContext::new_gl(&mpv, gl_proc, display_ptr)?;
        let flag = self.frame_flag.clone();
        let proxy = self.proxy.clone();
        render.set_update_callback(Box::new(move || {
            if !flag.swap(true, Ordering::AcqRel) {
                let _ = proxy.send_event(UserEvent::Frame);
            }
        }));
        self.mpv = Some(mpv);
        self.gfx = Some(Gfx { window, surface, context, gl, painter, egui_state, render: Some(render), _display: display });
        self.drain_mpv();
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Opening files

    fn queue_open(&mut self, paths: Vec<String>, mode: OpenMode) {
        if paths.is_empty() {
            return;
        }
        if self.pending.is_empty() {
            self.pending_mode = mode;
        }
        self.pending.extend(paths);
        self.pending_at = Some(Instant::now() + BURST);
    }

    /// Open everything collected during the burst window (Explorer launches one
    /// process per selected file; we merge them into a single sorted playlist).
    fn flush_pending(&mut self) {
        self.pending_at = None;
        let raw = std::mem::take(&mut self.pending);
        if raw.is_empty() || self.mpv.is_none() {
            return;
        }
        let mut subs = Vec::new();
        let mut media: Vec<String> = Vec::new();
        for p in raw {
            if is_url(&p) {
                media.push(p);
                continue;
            }
            let path = PathBuf::from(&p);
            if path.is_dir() {
                media.extend(platform::walk_media(&path).into_iter().map(|x| x.to_string_lossy().into_owned()));
            } else if is_subtitle(&path) && !is_media(&path) {
                subs.push(p);
            } else if path.exists() {
                media.push(p);
            }
        }
        media.dedup();
        let replace = matches!(self.pending_mode, OpenMode::Replace) || self.st.idle;
        if !media.is_empty() {
            if replace {
                self.load_list(media);
            } else {
                for m in &media {
                    let _ = self.mpv().command(&["loadfile", m, "append"]);
                }
                self.osd(format!("Added {} to playlist", media.len()));
            }
        }
        for s in subs {
            self.add_subtitle(&s);
        }
    }

    fn load_list(&mut self, mut media: Vec<String>) {
        self.load_started = Some(Instant::now());
        // A single local file: queue its folder siblings so "next" just works.
        let mut start = 0usize;
        if media.len() == 1 && !is_url(&media[0]) && self.store.settings.folder_playlist {
            let (list, idx) = platform::folder_siblings(Path::new(&media[0]));
            if list.len() > 1 {
                media = list.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
                start = idx;
            }
        } else if media.len() > 1 {
            let all_local = media.iter().all(|m| !is_url(m));
            if all_local {
                let first = media[0].clone();
                media.sort_by(|a, b| platform::natural_cmp(a, b));
                start = media.iter().position(|m| *m == first).unwrap_or(0).min(0);
            }
        }
        self.error = None;
        let mpv = self.mpv();
        let _ = mpv.command(&["playlist-clear"]);
        let _ = mpv.command(&["loadfile", &media[start], "replace"]);
        for (i, m) in media.iter().enumerate() {
            if i == start {
                continue;
            }
            if i < start {
                let _ = mpv.command(&["loadfile", m, "insert-at", &i.to_string()]);
            } else {
                let _ = mpv.command(&["loadfile", m, "append"]);
            }
        }
        let _ = mpv.set_str("pause", "no");
        self.loading_since = Some(Instant::now());
        self.ui.close_all();
        self.request_redraw();
    }

    fn add_subtitle(&mut self, path: &str) {
        if self.st.idle {
            return;
        }
        match self.mpv().command(&["sub-add", path, "select"]) {
            Ok(_) => self.osd(format!("Subtitles: {}", Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default())),
            Err(_) => self.osd("Couldn't load that subtitle file"),
        }
    }

    // -----------------------------------------------------------------------
    // mpv events

    fn drain_mpv(&mut self) {
        self.mpv_flag.store(false, Ordering::Release);
        let Some(mpv) = self.mpv.as_ref() else { return };
        let mut events = Vec::new();
        while let Some(ev) = mpv.next_event(0.0) {
            events.push(ev);
        }
        let mut redraw = false;
        for ev in events {
            match ev {
                Event::Property { name, value, .. } => {
                    self.st.apply(&name, &value);
                    match name.as_str() {
                        "pause" | "vid" => self.update_awake(),
                        "media-title" | "path" | "track-list" => {
                            if let Some(g) = &self.gfx {
                                g.window.set_title(&if self.st.idle { "VELA".to_string() } else { format!("{} — VELA", self.st.display_title()) });
                            }
                        }
                        "idle-active" => {
                            self.update_awake();
                            if self.st.idle {
                                self.loading_since = None;
                            }
                        }
                        _ => {}
                    }
                    if name == "idle-active" {
                        self.on_idle_change();
                    }
                    redraw = true;
                }
                Event::Hook { userdata, id, .. } => {
                    if userdata == HOOK_LOAD {
                        self.on_load_hook();
                    } else if userdata == HOOK_UNLOAD {
                        self.record_position(true);
                    }
                    self.mpv().hook_continue(id);
                }
                Event::StartFile => {
                    self.loading_since = Some(Instant::now());
                    self.load_started.get_or_insert_with(Instant::now);
                    self.await_first_frame = true;
                    self.error = None;
                }
                Event::FileLoaded => {
                    self.loading_since = None;
                    self.last_activity = Instant::now();
                    if let Some(note) = self.resume_note.take() {
                        self.osd(note);
                    }
                    redraw = true;
                }
                Event::PlaybackRestart => {
                    self.loading_since = None;
                    redraw = true;
                }
                Event::EndFile { reason, error } => {
                    if reason == mpv::END_ERROR {
                        let msg = mpv::error_string(error);
                        logf!("playback error: {msg}");
                        self.error = Some(format!("{msg}"));
                        self.osd(format!("Can't play this file ({msg})"));
                    }
                    self.loading_since = None;
                    redraw = true;
                }
                Event::Log { level, prefix, text } => logf!("[mpv/{level}] {prefix}: {}", text.trim_end()),
                Event::Shutdown => {}
                _ => {}
            }
        }
        if redraw {
            self.request_redraw();
        }
    }

    fn on_idle_change(&mut self) {
        self.idle_since = if self.st.idle { Some(Instant::now()) } else { None };
        self.trimmed = false;
        if self.st.idle {
            self.ui.close_all();
            self.controls = 1.0;
            self.store.flush();
            self.poster_requested.clear();
            if let Some(g) = &self.gfx {
                g.window.set_title("VELA");
                g.window.set_cursor_visible(true);
                self.cursor_hidden = false;
            }
        }
    }

    fn on_load_hook(&mut self) {
        let path = self.mpv().get_str("path").unwrap_or_default();
        self.resume_note = None;
        if !self.store.settings.remember_position {
            return;
        }
        if let Some(h) = self.store.find(&path).cloned() {
            if h.resumable() {
                let _ = self.mpv().set_str("file-local-options/start", &format!("{:.3}", h.position));
                self.resume_note = Some(format!("Resumed at {}  ·  press Home to start over", ui::fmt_time(h.position)));
            }
        }
    }

    fn record_position(&mut self, unloading: bool) {
        let Some(mpv) = self.mpv.as_ref() else { return };
        let path = mpv.get_str("path").unwrap_or_default();
        if path.is_empty() {
            return;
        }
        let dur = mpv.get_f64("duration").unwrap_or(self.st.duration);
        let mut pos = mpv.get_f64("time-pos").unwrap_or(0.0);
        if mpv.get_flag("eof-reached").unwrap_or(false) || (dur > 0.0 && pos > dur - 2.0) {
            pos = dur;
        }
        let video = self.st.has_video();
        let title = if video && !is_url(&path) { player::display_name(&path) } else { mpv.get_str("media-title").unwrap_or_default() };
        self.store.record(&path, &title, pos, dur, video);
        if unloading {
            self.store.flush();
        }
        self.last_record = Instant::now();
    }

    fn update_awake(&mut self) {
        let want = !self.st.idle && !self.st.pause && self.st.vid.is_some();
        if want != self.awake {
            self.awake = want;
            platform::keep_awake(want);
        }
    }

    // -----------------------------------------------------------------------
    // Actions

    fn act(&mut self, a: Action) {
        let Some(mpv) = self.mpv.as_ref() else { return };
        let st = &self.st;
        logf!("action {:?}", a);
        match a {
            Action::TogglePause => {
                if st.eof || (st.duration > 0.0 && mpv.get_f64("time-pos").unwrap_or(0.0) >= st.duration - 0.3 && st.pause) {
                    let _ = mpv.command(&["seek", "0", "absolute"]);
                    mpv.set_flag("pause", false);
                } else {
                    let _ = mpv.command(&["cycle", "pause"]);
                }
            }
            Action::SeekTo(t, exact) => {
                let _ = mpv.command(&["seek", &format!("{t:.3}"), if exact { "absolute+exact" } else { "absolute+keyframes" }]);
            }
            Action::SeekBy(d) => {
                let _ = mpv.command(&["seek", &format!("{d}"), "relative+exact"]);
                self.osd(format!("{}{}s", if d > 0.0 { "+" } else { "−" }, d.abs()));
            }
            Action::Next => {
                if mpv.command(&["playlist-next", "force"]).is_err() {
                    self.osd("End of playlist");
                }
            }
            Action::Prev => {
                let t = mpv.get_f64("time-pos").unwrap_or(0.0);
                if t > 5.0 || self.st.pl_pos <= 0 {
                    let _ = mpv.command(&["seek", "0", "absolute"]);
                } else {
                    let _ = mpv.command(&["playlist-prev", "force"]);
                }
            }
            Action::PlayIndex(i) => {
                let _ = mpv.command(&["playlist-play-index", &i.to_string()]);
            }
            Action::RemoveIndex(i) => {
                let _ = mpv.command(&["playlist-remove", &i.to_string()]);
            }
            Action::SetVolume(v) => {
                let v = v.clamp(0.0, 200.0);
                mpv.set_f64("volume", v);
                mpv.set_flag("mute", false);
                self.store.settings.volume = v;
                self.store.settings.muted = false;
                self.store.mark_settings();
                self.osd(format!("Volume {}%", v.round()));
            }
            Action::VolumeBy(d) => {
                let v = (if st.mute { 0.0 } else { st.volume } + d).clamp(0.0, 200.0);
                self.act(Action::SetVolume((v / 5.0).round() * 5.0));
            }
            Action::ToggleMute => {
                let m = !st.mute;
                mpv.set_flag("mute", m);
                self.store.settings.muted = m;
                self.store.mark_settings();
                self.osd(if m { "Muted".to_string() } else { format!("Volume {}%", st.volume.round()) });
            }
            Action::SetSpeed(s) => {
                let s = (s.clamp(0.25, 4.0) * 100.0).round() / 100.0;
                mpv.set_f64("speed", s);
                self.osd(format!("Speed {}×", s));
            }
            Action::ToggleFullscreen => {
                if let Some(g) = &self.gfx {
                    let fs = g.window.fullscreen().is_some();
                    g.window.set_fullscreen(if fs { None } else { Some(Fullscreen::Borderless(None)) });
                }
            }
            Action::Minimize => {
                if let Some(g) = &self.gfx {
                    g.window.set_minimized(true);
                }
            }
            Action::ToggleMaximize => {
                if let Some(g) = &self.gfx {
                    g.window.set_maximized(!g.window.is_maximized());
                }
            }
            Action::Close => {
                if let Some(g) = &self.gfx {
                    g.window.set_visible(false);
                }
                self.quit_requested = true;
            }
            Action::ToggleOnTop => {
                self.on_top = !self.on_top;
                if let Some(g) = &self.gfx {
                    g.window.set_window_level(if self.on_top { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal });
                }
                self.osd(if self.on_top { "Always on top" } else { "Always on top off" });
            }
            Action::StartDrag => {
                if let Some(g) = &mut self.gfx {
                    let _ = g.window.drag_window();
                    // Windows swallows the button-up during the move loop; tell egui.
                    let pos = g.egui_state.egui_input_mut().events.iter().rev().find_map(|e| match e {
                        egui::Event::PointerMoved(p) => Some(*p),
                        _ => None,
                    });
                    g.egui_state.egui_input_mut().events.push(egui::Event::PointerButton {
                        pos: pos.unwrap_or_default(),
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    });
                }
            }
            Action::SetSub(id) => {
                match id {
                    Some(id) => {
                        mpv.set_i64("sid", id);
                        mpv.set_flag("sub-visibility", true);
                        let label = st.tracks.iter().find(|t| t.kind == "sub" && t.id == id).map(|t| t.label()).unwrap_or_default();
                        self.osd(format!("Subtitles: {label}"));
                    }
                    None => {
                        let _ = mpv.set_str("sid", "no");
                        self.osd("Subtitles off");
                    }
                }
            }
            Action::LoadSubtitle => {
                let start = Path::new(&self.st.path).parent().map(|p| p.to_path_buf());
                let mut d = rfd::FileDialog::new()
                    .set_title("Load subtitles")
                    .add_filter("Subtitles", platform::SUB_EXT)
                    .add_filter("All files", &["*"]);
                if let Some(s) = start {
                    d = d.set_directory(s);
                }
                if let Some(p) = d.pick_file() {
                    self.add_subtitle(&p.to_string_lossy());
                }
            }
            Action::SubDelay(d) => {
                let d = (d * 10.0).round() / 10.0;
                mpv.set_f64("sub-delay", d);
                self.osd(format!("Subtitle delay {d:+.1}s"));
            }
            Action::SetAudio(id) => {
                mpv.set_i64("aid", id);
                let label = st.tracks.iter().find(|t| t.kind == "audio" && t.id == id).map(|t| t.label()).unwrap_or_default();
                self.osd(format!("Audio: {label}"));
            }
            Action::AudioDelay(d) => {
                let d = (d * 20.0).round() / 20.0;
                mpv.set_f64("audio-delay", d);
                self.osd(format!("Audio delay {d:+.2}s"));
            }
            Action::SetAspect(a) => {
                let _ = mpv.set_str("keepaspect", "yes");
                let _ = mpv.set_str("panscan", "0");
                match a {
                    "auto" => {
                        let _ = mpv.set_str("video-aspect-override", "-1");
                    }
                    "fill" => {
                        let _ = mpv.set_str("video-aspect-override", "-1");
                        let _ = mpv.set_str("panscan", "1");
                    }
                    "stretch" => {
                        let _ = mpv.set_str("video-aspect-override", "-1");
                        let _ = mpv.set_str("keepaspect", "no");
                    }
                    r => {
                        let _ = mpv.set_str("video-aspect-override", r);
                    }
                }
                let label = match a {
                    "auto" => "Original",
                    "fill" => "Fill screen",
                    "stretch" => "Stretch",
                    r => r,
                };
                self.osd(format!("Aspect: {label}"));
            }
            Action::SetZoom(z) => {
                mpv.set_f64("video-zoom", z);
                self.osd(format!("Zoom {:.0}%", 2f64.powf(z) * 100.0));
            }
            Action::SetRotate(r) => {
                mpv.set_i64("video-rotate", r);
                self.osd(format!("Rotate {r}°"));
            }
            Action::SetVideoEq(name, v) => {
                mpv.set_i64(name, v.clamp(-100, 100));
            }
            Action::ResetVideo => {
                for n in ["brightness", "contrast", "saturation", "gamma", "hue"] {
                    mpv.set_i64(n, 0);
                }
                mpv.set_f64("video-zoom", 0.0);
                mpv.set_i64("video-rotate", 0);
                let _ = mpv.set_str("video-aspect-override", "-1");
                let _ = mpv.set_str("panscan", "0");
                let _ = mpv.set_str("keepaspect", "yes");
                self.osd("Picture reset");
            }
            Action::SetDeinterlace(on) => {
                let _ = mpv.set_str("deinterlace", if on { "yes" } else { "no" });
                self.osd(if on { "Deinterlace on" } else { "Deinterlace off" });
            }
            Action::SetEqPreset(key) => {
                if let Some((_, label, gains)) = player::EQ_PRESETS.iter().find(|p| p.0 == key) {
                    self.store.settings.eq_preset = key.to_string();
                    self.store.settings.eq_gains = *gains;
                    self.store.mark_settings();
                    self.apply_af();
                    self.osd(format!("Equalizer: {label}"));
                }
            }
            Action::SetEqBand(i, g) => {
                if i < 10 {
                    self.store.settings.eq_gains[i] = g.clamp(-12, 12);
                    self.store.settings.eq_preset = "custom".into();
                    self.store.mark_settings();
                    self.apply_af();
                }
            }
            Action::SetNormalize(on) => {
                self.store.settings.normalize = on;
                self.store.mark_settings();
                self.apply_af();
                self.osd(if on { "Loudness normalization on" } else { "Loudness normalization off" });
            }
            Action::SetSubScale(v) => self.sub_setting(|s| s.sub_scale = v.clamp(0.3, 3.0)),
            Action::SetSubPos(v) => self.sub_setting(|s| s.sub_pos = v.clamp(0, 150)),
            Action::SetSubColor(c) => self.sub_setting(|s| s.sub_color = c.to_string()),
            Action::SetSubBack(v) => self.sub_setting(|s| s.sub_back = v.clamp(0.0, 1.0)),
            Action::SetLoopFile(on) => {
                let _ = mpv.set_str("loop-file", if on { "inf" } else { "no" });
                if on {
                    self.osd("Repeat this file");
                }
            }
            Action::SetLoopPlaylist(on) => {
                let _ = mpv.set_str("loop-playlist", if on { "inf" } else { "no" });
                self.osd(if on { "Repeat playlist" } else { "Repeat off" });
            }
            Action::AbLoop => {
                let _ = mpv.command(&["ab-loop"]);
                let (a, b) = (mpv.get_f64("ab-loop-a"), mpv.get_f64("ab-loop-b"));
                self.osd(match (a, b) {
                    (Some(a), None) => format!("A-B repeat: A = {}", ui::fmt_time(a)),
                    (Some(a), Some(b)) => format!("A-B repeat: {} → {}", ui::fmt_time(a), ui::fmt_time(b)),
                    _ => "A-B repeat off".to_string(),
                });
            }
            Action::Shuffle => {
                let _ = mpv.command(&["playlist-shuffle"]);
                self.osd("Playlist shuffled");
            }
            Action::SetSleep(m) => {
                self.sleep_at = m.map(|m| Instant::now() + Duration::from_secs(m as u64 * 60));
                self.osd(match m {
                    Some(m) => format!("Sleep in {m} min"),
                    None => "Sleep timer off".into(),
                });
            }
            Action::SetAutoplay(on) => {
                self.store.settings.autoplay_next = on;
                self.store.mark_settings();
                let _ = mpv.set_str("keep-open", if on { "yes" } else { "always" });
            }
            Action::SetHwdec(on) => {
                self.store.settings.hwdec = on;
                self.store.mark_settings();
                let _ = mpv.set_str("hwdec", if on { "auto-safe" } else { "no" });
                self.osd(if on { "Hardware decoding on" } else { "Hardware decoding off" });
            }
            Action::SetHighQuality(on) => {
                self.store.settings.high_quality = on;
                self.store.mark_settings();
                if on {
                    let _ = mpv.command(&["apply-profile", "high-quality"]);
                } else {
                    let _ = mpv.command(&["apply-profile", "high-quality", "restore"]);
                }
                self.osd(if on { "High-quality scaling" } else { "Standard scaling" });
            }
            Action::SetRemember(on) => {
                self.store.settings.remember_position = on;
                self.store.mark_settings();
            }
            Action::SetFolderPlaylist(on) => {
                self.store.settings.folder_playlist = on;
                self.store.mark_settings();
            }
            Action::Screenshot => {
                if self.st.idle {
                    return;
                }
                let r = mpv.command(&["screenshot", "subtitles"]);
                self.osd(if r.is_ok() { "Screenshot saved to Pictures/VELA" } else { "Screenshot failed" });
            }
            Action::OpenFiles => {
                let mut exts: Vec<&str> = platform::VIDEO_EXT.to_vec();
                exts.extend_from_slice(platform::AUDIO_EXT);
                exts.extend_from_slice(platform::PLAYLIST_EXT);
                if let Some(files) = rfd::FileDialog::new().set_title("Open media").add_filter("Media files", &exts).add_filter("All files", &["*"]).pick_files() {
                    let paths: Vec<String> = files.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    self.pending_mode = OpenMode::Replace;
                    self.pending = paths;
                    self.flush_pending();
                }
            }
            Action::OpenFolder => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Play a folder").pick_folder() {
                    let files: Vec<String> = platform::walk_media(&dir).into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
                    if files.is_empty() {
                        self.osd("No media files in that folder");
                    } else {
                        self.load_list_exact(files);
                    }
                }
            }
            Action::OpenUrl(url) => {
                if url.is_empty() {
                    self.ui.open(Menu::Url);
                } else {
                    self.load_list_exact(vec![url]);
                }
            }
            Action::OpenPaths(paths) => {
                self.pending_mode = OpenMode::Replace;
                self.pending = paths;
                self.flush_pending();
            }
            Action::ForgetHistory(p) => {
                self.store.forget(&p);
                self.store.flush();
            }
            Action::ClearHistory => {
                self.store.clear_history();
                self.store.flush();
            }
            Action::Stop => {
                let _ = mpv.command(&["stop"]);
            }
            Action::ShowInFolder => {
                if !self.st.path.is_empty() && !is_url(&self.st.path) {
                    platform::show_in_folder(&self.st.path);
                }
            }
            Action::ChapterBy(d) => {
                if self.st.chapters.len() > 1 {
                    let _ = mpv.command(&["add", "chapter", &d.to_string()]);
                    let c = mpv.get_i64("chapter").unwrap_or(0);
                    if let Some(ch) = self.st.chapters.get(c.max(0) as usize) {
                        let t = ch.title.clone();
                        self.osd(t);
                    }
                } else {
                    self.act(Action::SeekBy(if d > 0 { 60.0 } else { -60.0 }));
                }
            }
        }
        self.request_redraw();
    }

    /// Play exactly this list (no folder expansion), from the first item.
    fn load_list_exact(&mut self, media: Vec<String>) {
        let folder = self.store.settings.folder_playlist;
        self.store.settings.folder_playlist = false;
        self.load_list(media);
        self.store.settings.folder_playlist = folder;
    }

    fn apply_af(&mut self) {
        let af = player::audio_filters(&self.store.settings.eq_gains, self.store.settings.normalize);
        if let Err(e) = self.mpv().set_str("af", &af) {
            logf!("af failed: {e}");
        }
    }

    fn sub_setting(&mut self, f: impl FnOnce(&mut crate::store::Settings)) {
        f(&mut self.store.settings);
        self.store.mark_settings();
        player::apply_settings(self.mpv(), &self.store.settings);
        self.sub_pos_applied = i64::MIN;
    }

    // -----------------------------------------------------------------------
    // Keyboard

    fn on_key(&mut self, key: &Key, code: Option<KeyCode>, repeat: bool) -> bool {
        let ctrl = primary_modifier(self.modifiers);
        let shift = self.modifiers.shift_key();
        let alt = self.modifiers.alt_key();
        let idle = self.st.idle;
        // Global shortcuts
        if ctrl {
            match code {
                Some(KeyCode::KeyO) if shift => self.act(Action::OpenFolder),
                Some(KeyCode::KeyO) => self.act(Action::OpenFiles),
                Some(KeyCode::KeyV) => {
                    if let Some(t) = platform::clipboard_text() {
                        let t = t.trim().trim_matches('"').to_string();
                        if is_url(&t) || Path::new(&t).exists() {
                            self.act(Action::OpenPaths(vec![t]));
                        } else {
                            self.osd("Clipboard doesn't contain a link or path");
                        }
                    }
                }
                Some(KeyCode::KeyQ) => self.act(Action::Close),
                Some(KeyCode::KeyW) => self.act(Action::Stop),
                Some(KeyCode::ArrowRight) => self.act(Action::ChapterBy(1)),
                Some(KeyCode::ArrowLeft) => self.act(Action::ChapterBy(-1)),
                _ => return false,
            }
            return true;
        }
        if alt {
            if let Key::Named(NamedKey::Enter) = key {
                self.act(Action::ToggleFullscreen);
                return true;
            }
            return false;
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                if self.ui.close_all() || self.ui.info {
                    self.ui.info = false;
                } else if self.fullscreen() {
                    self.act(Action::ToggleFullscreen);
                } else if !idle {
                    self.act(Action::Stop);
                }
                self.request_redraw();
            }
            Key::Named(NamedKey::F11) => self.act(Action::ToggleFullscreen),
            Key::Named(NamedKey::Enter) if !repeat => self.act(Action::ToggleFullscreen),
            _ if idle => match key {
                Key::Character(c) if c.as_str() == "?" => {
                    self.ui.help = !self.ui.help;
                    self.request_redraw();
                }
                _ => return false,
            },
            Key::Named(NamedKey::Space) | Key::Named(NamedKey::MediaPlayPause) if !repeat => self.act(Action::TogglePause),
            Key::Named(NamedKey::ArrowRight) => self.act(Action::SeekBy(if shift { 60.0 } else { 5.0 })),
            Key::Named(NamedKey::ArrowLeft) => self.act(Action::SeekBy(if shift { -60.0 } else { -5.0 })),
            Key::Named(NamedKey::ArrowUp) => self.act(Action::VolumeBy(5.0)),
            Key::Named(NamedKey::ArrowDown) => self.act(Action::VolumeBy(-5.0)),
            Key::Named(NamedKey::Home) => self.act(Action::SeekTo(0.0, true)),
            Key::Named(NamedKey::End) => {
                let d = self.st.duration;
                self.act(Action::SeekTo((d - 1.0).max(0.0), true))
            }
            Key::Named(NamedKey::PageUp) => self.act(Action::ChapterBy(1)),
            Key::Named(NamedKey::PageDown) => self.act(Action::ChapterBy(-1)),
            Key::Named(NamedKey::Backspace) => self.act(Action::SetSpeed(1.0)),
            Key::Named(NamedKey::MediaTrackNext) => self.act(Action::Next),
            Key::Named(NamedKey::MediaTrackPrevious) => self.act(Action::Prev),
            Key::Named(NamedKey::MediaStop) => self.act(Action::Stop),
            Key::Character(c) => {
                let c = c.as_str();
                match c {
                    "k" | "K" if !repeat => self.act(Action::TogglePause),
                    "j" | "J" => self.act(Action::SeekBy(-10.0)),
                    "l" | "L" => self.act(Action::SeekBy(10.0)),
                    "m" | "M" if !repeat => self.act(Action::ToggleMute),
                    "f" | "F" if !repeat => self.act(Action::ToggleFullscreen),
                    "n" | "N" if !repeat => self.act(Action::Next),
                    "p" | "P" if !repeat => self.act(Action::Prev),
                    "s" | "S" if !repeat => self.act(Action::Screenshot),
                    "t" | "T" if !repeat => self.act(Action::ToggleOnTop),
                    "i" | "I" if !repeat => {
                        self.ui.info = !self.ui.info;
                        self.request_redraw();
                    }
                    "?" if !repeat => {
                        self.ui.help = !self.ui.help;
                        self.request_redraw();
                    }
                    "," => {
                        let _ = self.mpv().command(&["frame-back-step"]);
                    }
                    "." => {
                        let _ = self.mpv().command(&["frame-step"]);
                    }
                    "[" | "<" => self.step_speed(-1),
                    "]" | ">" => self.step_speed(1),
                    "=" => self.act(Action::SetSpeed(1.0)),
                    "c" | "C" if !repeat => self.toggle_subs(),
                    "v" | "V" if !repeat => self.cycle_track("sub"),
                    "b" | "B" if !repeat => self.cycle_track("audio"),
                    "g" | "G" => {
                        let d = self.st.sub_delay - 0.1;
                        self.act(Action::SubDelay(d))
                    }
                    "h" | "H" => {
                        let d = self.st.sub_delay + 0.1;
                        self.act(Action::SubDelay(d))
                    }
                    "a" | "A" if !repeat => self.cycle_aspect(),
                    "r" | "R" if !repeat => self.cycle_repeat(),
                    d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => {
                        let pct = (d.as_bytes()[0] - b'0') as f64 / 10.0;
                        let t = pct * self.st.duration;
                        self.act(Action::SeekTo(t, true));
                        self.osd(format!("{}  /  {}", ui::fmt_time(t), ui::fmt_time(self.st.duration)));
                    }
                    _ => return false,
                }
            }
            _ => return false,
        }
        true
    }

    fn step_speed(&mut self, dir: i32) {
        const SPEEDS: [f64; 11] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0];
        let cur = self.st.speed;
        let next = if dir > 0 {
            SPEEDS.iter().copied().find(|s| *s > cur + 0.001).unwrap_or(4.0)
        } else {
            SPEEDS.iter().rev().copied().find(|s| *s < cur - 0.001).unwrap_or(0.25)
        };
        self.act(Action::SetSpeed(next));
    }

    fn toggle_subs(&mut self) {
        if self.st.sid.is_none() {
            let first = self.st.tracks_of("sub").next().map(|t| t.id);
            if let Some(id) = first {
                self.act(Action::SetSub(Some(id)));
            } else {
                self.osd("No subtitles");
            }
            return;
        }
        let vis = !self.st.sub_visible;
        self.mpv().set_flag("sub-visibility", vis);
        self.osd(if vis { "Subtitles on" } else { "Subtitles hidden" });
    }

    fn cycle_track(&mut self, kind: &str) {
        let ids: Vec<i64> = self.st.tracks_of(kind).map(|t| t.id).collect();
        let cur = if kind == "sub" { self.st.sid } else { self.st.aid };
        if ids.is_empty() || (kind == "audio" && ids.len() < 2) {
            self.osd(if kind == "sub" { "No subtitles" } else { "Only one audio track" });
            return;
        }
        let idx = cur.and_then(|c| ids.iter().position(|i| *i == c));
        if kind == "sub" {
            // Off -> 1 -> 2 -> ... -> Off
            let next = match idx {
                None => Some(ids[0]),
                Some(i) if i + 1 < ids.len() => Some(ids[i + 1]),
                Some(_) => None,
            };
            self.act(Action::SetSub(next));
        } else {
            let next = ids[idx.map_or(0, |i| (i + 1) % ids.len())];
            self.act(Action::SetAudio(next));
        }
    }

    fn cycle_aspect(&mut self) {
        const ORDER: [&str; 6] = ["auto", "16:9", "4:3", "21:9", "fill", "stretch"];
        let cur = if !self.st.keepaspect {
            5
        } else if self.st.panscan > 0.5 {
            4
        } else if self.st.aspect <= 0.0 {
            0
        } else if (self.st.aspect - 16.0 / 9.0).abs() < 0.01 {
            1
        } else if (self.st.aspect - 4.0 / 3.0).abs() < 0.01 {
            2
        } else {
            3
        };
        self.act(Action::SetAspect(ORDER[(cur + 1) % ORDER.len()]));
    }

    fn cycle_repeat(&mut self) {
        if self.st.loop_file {
            self.act(Action::SetLoopFile(false));
            self.act(Action::SetLoopPlaylist(true));
        } else if self.st.loop_playlist {
            self.act(Action::SetLoopPlaylist(false));
        } else {
            self.act(Action::SetLoopFile(true));
        }
    }

    // -----------------------------------------------------------------------
    // Thumbnails

    fn take_thumbs(&mut self) {
        for t in self.thumbs.take() {
            let img = egui::ColorImage::from_rgba_unmultiplied([t.w as usize, t.h as usize], &t.rgba);
            match t.kind {
                thumbs::Kind::Hover => {
                    let time: f64 = t.key.trim_start_matches("hover:").parse().unwrap_or(0.0);
                    match &mut self.hover_tex {
                        Some((h, size, tt)) => {
                            h.set(img, egui::TextureOptions::LINEAR);
                            *size = egui::vec2(t.w as f32, t.h as f32);
                            *tt = time;
                        }
                        None => {
                            let h = self.egui.load_texture("hover", img, egui::TextureOptions::LINEAR);
                            self.hover_tex = Some((h, egui::vec2(t.w as f32, t.h as f32), time));
                        }
                    }
                }
                thumbs::Kind::Poster => {
                    let path = t.key.split_once('@').map(|(p, _)| p.to_string()).unwrap_or_default();
                    let h = self.egui.load_texture(format!("poster-{}", thumbs::hash(&path)), img, egui::TextureOptions::LINEAR);
                    self.poster_ids.insert(path.clone(), h.id());
                    self.posters.insert(path, (t.key, h));
                }
            }
        }
        self.request_redraw();
    }

    fn request_hover_thumb(&mut self, t: f64) {
        if !self.st.has_video() || is_url(&self.st.path) {
            return;
        }
        let bucket = (self.st.duration / 300.0).max(1.0);
        let tq = (t / bucket).round() * bucket;
        if self.last_hover_req.map_or(false, |x| (x - tq).abs() < 0.01) {
            return;
        }
        self.last_hover_req = Some(tq);
        let aspect = if self.st.dheight > 0 { self.st.dwidth as f64 / self.st.dheight as f64 } else { 16.0 / 9.0 };
        let w = 224u32;
        let h = ((w as f64 / aspect).round() as u32).clamp(60, 240) & !1;
        self.thumbs.request(thumbs::Request {
            key: format!("hover:{tq}"),
            path: self.st.path.clone(),
            time: tq,
            w,
            h,
            kind: thumbs::Kind::Hover,
        });
    }

    fn request_posters(&mut self) {
        let wanted: Vec<(String, f64)> = self
            .store
            .history
            .iter()
            .filter(|h| h.resumable() && h.video && Path::new(&h.path).exists())
            .take(8)
            .map(|h| (h.path.clone(), h.position))
            .collect();
        for (path, pos) in wanted {
            let key = format!("{path}@{}", (pos / 30.0) as i64);
            if self.posters.get(&path).map_or(false, |(k, _)| *k == key) || self.poster_requested.contains(&key) {
                continue;
            }
            self.poster_requested.insert(key.clone());
            self.thumbs.request(thumbs::Request { key, path, time: pos, w: 384, h: 216, kind: thumbs::Kind::Poster });
        }
    }

    // -----------------------------------------------------------------------
    // Rendering

    fn controls_target(&self) -> bool {
        if self.home() {
            return true;
        }
        self.st.pause
            || self.st.eof
            || self.interacting
            || self.ui.menu != Menu::None
            || self.ui.seek_drag.is_some()
            || self.last_activity.elapsed() < HIDE_AFTER
            || self.cursor.is_none() && self.last_activity.elapsed() < HIDE_AFTER
    }

    /// Lift subtitles above the control bar while it is visible (like YouTube).
    fn update_sub_lift(&mut self) {
        let (Some(mpv), Some(g)) = (self.mpv.as_ref(), self.gfx.as_ref()) else { return };
        let h = g.window.inner_size().height as f64;
        let lift = if !self.st.idle && self.controls_target() && h > 0.0 {
            (100.0 * g.window.scale_factor() / h * 100.0).round() as i64
        } else {
            0
        };
        let pos = (self.store.settings.sub_pos - lift).clamp(0, 150);
        if pos != self.sub_pos_applied {
            self.sub_pos_applied = pos;
            let _ = mpv.set_str("sub-pos", &pos.to_string());
        }
    }

    fn redraw(&mut self) {
        self.frame_flag.store(false, Ordering::Release);
        self.update_sub_lift();
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // controls fade
        let target = if self.controls_target() { 1.0 } else { 0.0 };
        if self.controls != target {
            let step = dt / 0.16;
            self.controls = if target > self.controls { (self.controls + step).min(1.0) } else { (self.controls - step).max(0.0) };
        }
        let fading = self.controls != target;

        // cursor auto-hide while playing fullscreen/windowed
        let hide_cursor = !self.home() && self.controls == 0.0 && !self.st.pause;
        if hide_cursor != self.cursor_hidden {
            if let Some(g) = &self.gfx {
                g.window.set_cursor_visible(!hide_cursor);
            }
            self.cursor_hidden = hide_cursor;
        }

        let osd_alpha = self.osd.as_ref().map(|(_, at)| {
            let e = at.elapsed().as_secs_f32();
            if e < 1.3 {
                1.0
            } else {
                (1.0 - (e - 1.3) / 0.3).max(0.0)
            }
        });
        if osd_alpha == Some(0.0) {
            self.osd = None;
        }

        let up_next = self.up_next();
        let busy = self.loading_since.map_or(false, |t| t.elapsed() > Duration::from_millis(350)) || self.st.buffering;
        let ui_needed = self.home()
            || self.controls > 0.0
            || self.osd.is_some()
            || self.ui.menu != Menu::None
            || self.ui.help
            || self.ui.info
            || up_next.is_some()
            || busy
            || !self.st.has_video();

        let Some(gfx) = self.gfx.as_mut() else { return };
        let size = gfx.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        let (w, h) = (size.width as i32, size.height as i32);
        let show_video = !self.st.idle;
        unsafe {
            gfx.gl.disable(glow::SCISSOR_TEST);
            // Everything (mpv output and egui) is gamma-encoded already; the
            // WGL default framebuffer may come up sRGB-enabled, which would
            // brighten every translucent pixel.
            gfx.gl.disable(glow::FRAMEBUFFER_SRGB);
            gfx.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gfx.gl.viewport(0, 0, w, h);
        }
        if show_video {
            if let Some(r) = &gfx.render {
                r.render_gl(0, w, h, true);
            }
        } else {
            unsafe {
                gfx.gl.clear_color(10.0 / 255.0, 10.0 / 255.0, 13.0 / 255.0, 1.0);
                gfx.gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }

        unsafe { gfx.gl.disable(glow::FRAMEBUFFER_SRGB) };
        let mut actions: Vec<Deferred> = Vec::new();
        let mut repaint_after: Option<Duration> = None;
        if ui_needed {
            let mpv = self.mpv.as_ref().unwrap();
            let time = if self.st.idle { 0.0 } else { mpv.get_f64("time-pos").unwrap_or(0.0) };
            let cache = if self.controls > 0.0 && !self.st.idle { cache_range(mpv, time) } else { None };
            let info = if self.ui.info && !self.st.idle { Some(stats(mpv, &self.st)) } else { None };
            let artist = if !self.st.has_video() && !self.st.idle {
                mpv.get_str("metadata/by-key/Artist").or_else(|| mpv.get_str("metadata/by-key/artist")).unwrap_or_default()
            } else {
                String::new()
            };
            let cover = self.st.tracks.iter().any(|t| t.kind == "video" && t.selected && t.albumart);
            let hover_thumb = self.hover_tex.as_ref().map(|(h, s, t)| (h.id(), *s, *t));
            let sleep_left = self.sleep_at.map(|t| t.saturating_duration_since(Instant::now()).as_secs_f64());
            let view = View {
                st: &self.st,
                settings: &self.store.settings,
                history: &self.store.history,
                home: self.st.idle,
                time,
                cache,
                fullscreen: gfx.window.fullscreen().is_some(),
                maximized: gfx.window.is_maximized(),
                on_top: self.on_top,
                controls: self.controls,
                osd: self.osd.as_ref().map(|(t, _)| (t.as_str(), osd_alpha.unwrap_or(1.0))),
                hover_thumb,
                posters: &self.poster_ids,
                info,
                sleep_left,
                error: if self.st.idle { None } else { self.error.as_deref() },
                up_next,
                artist,
                busy,
                cover,
            };
            let raw = gfx.egui_state.take_egui_input(&gfx.window);
            let ui_state = &mut self.ui;
            let mut out = ui::UiOut::default();
            let full = self.egui.run(raw, |ctx| {
                out = ui::draw(ctx, &view, ui_state);
            });
            gfx.egui_state.handle_platform_output(&gfx.window, full.platform_output);
            let prims = self.egui.tessellate(full.shapes, full.pixels_per_point);
            gfx.painter.paint_and_update_textures([w as u32, h as u32], full.pixels_per_point, &prims, &full.textures_delta);
            if let Some(vo) = full.viewport_output.get(&egui::ViewportId::ROOT) {
                if vo.repaint_delay < Duration::from_secs(10) {
                    repaint_after = Some(vo.repaint_delay);
                }
            }
            if out.animating {
                repaint_after = Some(Duration::from_millis(16));
            }
            self.interacting = out.interacting;
            if let Some(t) = out.hover_time {
                actions.push(Deferred::Hover(t));
            } else {
                self.last_hover_req = None;
            }
            actions.extend(out.actions.into_iter().map(Deferred::Act));
        }

        // test hook: capture the composed frame before presenting it
        if !self.shot.is_empty() {
            let shots = std::mem::take(&mut self.shot);
            let mut buf = vec![0u8; (w * h * 4) as usize];
            unsafe {
                gfx.gl.read_pixels(0, 0, w, h, glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut buf)));
            }
            for (path, reply) in shots {
                let res = write_bmp(&path, w as u32, h as u32, &buf);
                let _ = reply.send(match res {
                    Ok(_) => path.to_string_lossy().into_owned(),
                    Err(e) => format!("error: {e}"),
                });
            }
        }

        let _ = gfx.surface.swap_buffers(&gfx.context);
        if show_video {
            if let Some(r) = &gfx.render {
                r.report_swap();
            }
        }
        if !self.shown {
            gfx.window.set_visible(true);
            self.shown = true;
            logf!("first frame on screen {} ms after launch", platform::since_start_ms());
        }
        if self.await_first_frame && show_video && self.st.vid.is_some() && self.loading_since.is_none() {
            self.await_first_frame = false;
            if let Some(t) = self.load_started.take() {
                logf!("open -> first video frame: {} ms", t.elapsed().as_millis());
            }
        }

        for a in actions {
            match a {
                Deferred::Hover(t) => self.request_hover_thumb(t),
                Deferred::Act(a) => self.act(a),
            }
        }
        if self.home() {
            self.request_posters();
        }

        // Schedule the next frame only if something is moving.
        let mut next: Option<Instant> = repaint_after.map(|d| Instant::now() + d.max(Duration::from_millis(8)));
        if fading || self.osd.is_some() || busy {
            next = Some(Instant::now() + Duration::from_millis(16));
        }
        if self.controls > 0.0 && !self.st.pause && !self.st.idle && !self.st.has_video() {
            // audio-only: tick the clock a few times per second
            let t = Instant::now() + Duration::from_millis(250);
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        if self.ui.info {
            let t = Instant::now() + Duration::from_millis(500);
            next = Some(next.map_or(t, |n| n.min(t)));
        }
        self.repaint_at = next;
    }

    fn up_next(&self) -> Option<(String, f64)> {
        if self.st.idle || !self.store.settings.autoplay_next || self.st.loop_file || self.st.pause {
            return None;
        }
        let pos = self.st.pl_pos;
        let next = if pos >= 0 && ((pos + 1) as usize) < self.st.playlist.len() {
            self.st.playlist.get((pos + 1) as usize)
        } else if self.st.loop_playlist && self.st.playlist.len() > 1 {
            self.st.playlist.first()
        } else {
            None
        }?;
        if self.st.duration < 60.0 {
            return None;
        }
        let t = self.mpv.as_ref()?.get_f64("time-pos")?;
        let left = (self.st.duration - t) / self.st.speed.max(0.1);
        (left < 10.0 && left > 0.0).then(|| (player::display_name(&next.filename), left))
    }

    fn activity(&mut self) {
        self.last_activity = Instant::now();
        if self.controls < 1.0 || self.cursor_hidden {
            self.request_redraw();
        }
    }

    // -----------------------------------------------------------------------
    // IPC (single instance + automation for tests)

    fn handle_ipc(&mut self, req: ipc::Request) {
        let mut lines = req.text.lines();
        let verb = lines.next().unwrap_or("").trim().to_string();
        let rest: Vec<String> = lines.map(|l| l.to_string()).filter(|l| !l.is_empty()).collect();
        let reply = match verb.as_str() {
            "open" | "enqueue" => {
                if let Some(g) = &self.gfx {
                    if g.window.is_minimized() == Some(true) {
                        g.window.set_minimized(false);
                    }
                    g.window.focus_window();
                }
                self.queue_open(rest, if verb == "open" { OpenMode::Replace } else { OpenMode::Append });
                "ok".to_string()
            }
            "state" => self.state_json(),
            "get" => rest.first().and_then(|p| self.mpv.as_ref().and_then(|m| m.get_node(p))).map(|v| v.to_string()).unwrap_or_else(|| "null".into()),
            "mpv" => {
                let args: Vec<&str> = rest.iter().map(|s| s.as_str()).collect();
                match self.mpv().command(&args) {
                    Ok(_) => "ok".into(),
                    Err(e) => format!("error: {e}"),
                }
            }
            "action" => self.test_action(rest.first().map(|s| s.as_str()).unwrap_or("")),
            "shot" => {
                let path = PathBuf::from(rest.first().cloned().unwrap_or_else(|| "shot.bmp".into()));
                self.shot.push((path, req.reply.clone()));
                self.request_redraw();
                return;
            }
            "quit" => {
                self.quit_requested = true;
                "ok".into()
            }
            _ => format!("unknown verb {verb}"),
        };
        let _ = req.reply.send(reply);
        self.request_redraw();
    }

    fn state_json(&self) -> String {
        let Some(mpv) = self.mpv.as_ref() else { return "{}".into() };
        let g = self.gfx.as_ref();
        serde_json::json!({
            "idle": self.st.idle,
            "path": self.st.path,
            "title": self.st.title,
            "pause": self.st.pause,
            "time": mpv.get_f64("time-pos"),
            "duration": self.st.duration,
            "volume": self.st.volume,
            "mute": self.st.mute,
            "speed": self.st.speed,
            "vid": self.st.vid, "aid": self.st.aid, "sid": self.st.sid,
            "sub_visible": self.st.sub_visible,
            "sub_delay": self.st.sub_delay,
            "audio_delay": self.st.audio_delay,
            "tracks": self.st.tracks.iter().map(|t| serde_json::json!({"id": t.id, "type": t.kind, "lang": t.lang, "title": t.title, "codec": t.codec, "selected": t.selected, "external": t.external})).collect::<Vec<_>>(),
            "chapters": self.st.chapters.len(),
            "chapter": self.st.chapter,
            "playlist": self.st.playlist.iter().map(|e| e.filename.clone()).collect::<Vec<_>>(),
            "playlist_pos": self.st.pl_pos,
            "loop_file": self.st.loop_file,
            "loop_playlist": self.st.loop_playlist,
            "ab": [self.st.ab_a, self.st.ab_b],
            "aspect": self.st.aspect, "panscan": self.st.panscan, "keepaspect": self.st.keepaspect,
            "zoom": self.st.zoom, "rotate": self.st.rotate,
            "brightness": self.st.brightness, "contrast": self.st.contrast, "saturation": self.st.saturation,
            "deinterlace": self.st.deinterlace,
            "hwdec": self.st.hwdec,
            "af": mpv.get_str("af"),
            "eof": self.st.eof,
            "error": self.error,
            "fullscreen": g.map(|g| g.window.fullscreen().is_some()),
            "maximized": g.map(|g| g.window.is_maximized()),
            "on_top": self.on_top,
            "controls": self.controls,
            "menu": format!("{:?}", self.ui.menu),
            "page": format!("{:?}", self.ui.page),
            "help": self.ui.help,
            "info": self.ui.info,
            "osd": self.osd.as_ref().map(|o| o.0.clone()),
            "sleep": self.sleep_at.map(|t| t.saturating_duration_since(Instant::now()).as_secs()),
            "hover_thumb": self.hover_tex.as_ref().map(|h| h.2),
            "posters": self.posters.len(),
            "history": self.store.history.len(),
            "memory_mb": platform::resident_mb(),
            "settings": serde_json::to_value(&self.store.settings).ok(),
        })
        .to_string()
    }

    /// Automation entry: "name arg" -> UI action / key press / menu.
    fn test_action(&mut self, line: &str) -> String {
        let (name, arg) = line.split_once(' ').unwrap_or((line, ""));
        let num = || arg.trim().parse::<f64>().unwrap_or(0.0);
        let a = match name {
            "key" => {
                let key = match arg {
                    "space" => Key::Named(NamedKey::Space),
                    "left" => Key::Named(NamedKey::ArrowLeft),
                    "right" => Key::Named(NamedKey::ArrowRight),
                    "up" => Key::Named(NamedKey::ArrowUp),
                    "down" => Key::Named(NamedKey::ArrowDown),
                    "esc" => Key::Named(NamedKey::Escape),
                    "enter" => Key::Named(NamedKey::Enter),
                    "home" => Key::Named(NamedKey::Home),
                    "end" => Key::Named(NamedKey::End),
                    "backspace" => Key::Named(NamedKey::Backspace),
                    c => Key::Character(c.into()),
                };
                return if self.on_key(&key, None, false) { "ok".into() } else { "unhandled".into() };
            }
            "menu" => {
                if arg == "context" {
                    self.ui.ctx_pos = egui::pos2(200.0, 150.0);
                }
                self.ui.open(match arg {
                    "settings" => Menu::Settings,
                    "subs" => Menu::Subs,
                    "audio" => Menu::Audio,
                    "speed" => Menu::Speed,
                    "playlist" => Menu::Playlist,
                    "context" => Menu::Context,
                    "url" => Menu::Url,
                    _ => {
                        self.ui.close_all();
                        self.request_redraw();
                        return "ok".into();
                    }
                });
                self.activity();
                self.request_redraw();
                return "ok".into();
            }
            "page" => {
                self.ui.menu = Menu::Settings;
                self.ui.page = match arg {
                    "speed" => ui::Page::Speed,
                    "subs" => ui::Page::Subs,
                    "substyle" => ui::Page::SubStyle,
                    "audio" => ui::Page::Audio,
                    "video" => ui::Page::Video,
                    "eq" => ui::Page::Eq,
                    "repeat" => ui::Page::Repeat,
                    "sleep" => ui::Page::Sleep,
                    "prefs" => ui::Page::Prefs,
                    _ => ui::Page::Main,
                };
                self.activity();
                self.request_redraw();
                return "ok".into();
            }
            "hover" => {
                self.request_hover_thumb(num());
                return "ok".into();
            }
            "activity" => {
                self.activity();
                return "ok".into();
            }
            "toggle-pause" => Action::TogglePause,
            "seek" => Action::SeekTo(num(), true),
            "seek-by" => Action::SeekBy(num()),
            "next" => Action::Next,
            "prev" => Action::Prev,
            "play-index" => Action::PlayIndex(num() as i64),
            "volume" => Action::SetVolume(num()),
            "mute" => Action::ToggleMute,
            "speed" => Action::SetSpeed(num()),
            "fullscreen" => Action::ToggleFullscreen,
            "maximize" => Action::ToggleMaximize,
            "ontop" => Action::ToggleOnTop,
            "sub" => Action::SetSub(if arg == "no" { None } else { Some(num() as i64) }),
            "sub-delay" => Action::SubDelay(num()),
            "sub-add" => {
                self.add_subtitle(arg.trim());
                return "ok".into();
            }
            "audio" => Action::SetAudio(num() as i64),
            "audio-delay" => Action::AudioDelay(num()),
            "aspect" => Action::SetAspect(match arg {
                "16:9" => "16:9",
                "4:3" => "4:3",
                "21:9" => "21:9",
                "2.35:1" => "2.35:1",
                "1:1" => "1:1",
                "fill" => "fill",
                "stretch" => "stretch",
                _ => "auto",
            }),
            "zoom" => Action::SetZoom(num()),
            "rotate" => Action::SetRotate(num() as i64),
            "brightness" => Action::SetVideoEq("brightness", num() as i64),
            "reset-video" => Action::ResetVideo,
            "deinterlace" => Action::SetDeinterlace(arg == "yes"),
            "eq" => Action::SetEqPreset(match arg {
                "bass" => "bass",
                "treble" => "treble",
                "voice" => "voice",
                "cinema" => "cinema",
                "rock" => "rock",
                "pop" => "pop",
                "classical" => "classical",
                _ => "flat",
            }),
            "eq-band" => {
                let (i, g) = arg.split_once(' ').unwrap_or(("0", "0"));
                Action::SetEqBand(i.parse().unwrap_or(0), g.parse().unwrap_or(0))
            }
            "normalize" => Action::SetNormalize(arg == "yes"),
            "sub-scale" => Action::SetSubScale(num()),
            "sub-back" => Action::SetSubBack(num()),
            "loop-file" => Action::SetLoopFile(arg == "yes"),
            "loop-playlist" => Action::SetLoopPlaylist(arg == "yes"),
            "ab" => Action::AbLoop,
            "shuffle" => Action::Shuffle,
            "sleep" => Action::SetSleep(if num() > 0.0 { Some(num() as u32) } else { None }),
            "autoplay" => Action::SetAutoplay(arg == "yes"),
            "hwdec" => Action::SetHwdec(arg == "yes"),
            "hq" => Action::SetHighQuality(arg == "yes"),
            "screenshot" => Action::Screenshot,
            "stop" => Action::Stop,
            "chapter" => Action::ChapterBy(num() as i64),
            "url" => Action::OpenUrl(arg.to_string()),
            _ => return format!("unknown action {name}"),
        };
        self.act(a);
        "ok".into()
    }

    fn save_window(&mut self) {
        if let Some(g) = &self.gfx {
            let max = g.window.is_maximized();
            let fs = g.window.fullscreen().is_some();
            self.store.settings.maximized = max;
            if !max && !fs {
                if let Ok(pos) = g.window.outer_position() {
                    let sz = g.window.inner_size();
                    self.store.settings.window = Some([pos.x, pos.y, sz.width as i32, sz.height as i32]);
                }
            }
            self.store.mark_settings();
        }
    }

    fn shutdown(&mut self, el: &ActiveEventLoop) {
        self.record_position(false);
        self.save_window();
        self.store.flush();
        platform::keep_awake(false);
        // GL objects first, then the render context (needs current GL), then mpv.
        if let Some(mut g) = self.gfx.take() {
            g.painter.destroy();
            drop(g.render.take());
            drop(g);
        }
        self.mpv.take();
        el.exit();
    }

    fn edge_at(&self, p: PhysicalPosition<f64>) -> Option<ResizeDirection> {
        let g = self.gfx.as_ref()?;
        if NATIVE_CHROME || g.window.fullscreen().is_some() || g.window.is_maximized() {
            return None;
        }
        let s = g.window.inner_size();
        let m = 6.0 * g.window.scale_factor();
        let (l, r, t, b) = (p.x < m, p.x > s.width as f64 - m, p.y < m, p.y > s.height as f64 - m);
        Some(match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return None,
        })
    }
}

/// Work produced while drawing, applied once the frame is presented.
enum Deferred {
    Hover(f64),
    Act(Action),
}

/// Where to open the window: the saved placement if it is still mostly on a
/// connected monitor, otherwise centred on the primary monitor and sized to
/// fit it (a fixed default overflowed 1080p screens at 125 % scaling).
fn initial_geometry(el: &ActiveEventLoop, saved: Option<[i32; 4]>) -> (Option<PhysicalPosition<i32>>, PhysicalSize<u32>) {
    let monitors: Vec<_> = el.available_monitors().collect();
    if let Some([x, y, w, h]) = saved {
        let visible = monitors.iter().any(|m| {
            let (mp, ms) = (m.position(), m.size());
            let ix = (x + w).min(mp.x + ms.width as i32) - x.max(mp.x);
            let iy = (y + h).min(mp.y + ms.height as i32) - y.max(mp.y);
            w >= 480 && h >= 280 && ix > w / 2 && iy > h / 2 && h <= ms.height as i32
        });
        if visible {
            return (Some(PhysicalPosition::new(x, y)), PhysicalSize::new(w as u32, h as u32));
        }
    }
    let Some(m) = el.primary_monitor().or_else(|| monitors.into_iter().next()) else {
        return (None, PhysicalSize::new(1280, 740));
    };
    let (mp, ms, k) = (m.position(), m.size(), m.scale_factor());
    // Ideal 1280x740 logical, but never more than ~80 % of the screen.
    let w = (1280.0 * k).min(ms.width as f64 * 0.82);
    let h = (740.0 * k).min(ms.height as f64 * 0.78).min(w * 0.62);
    let x = mp.x + ((ms.width as f64 - w) / 2.0) as i32;
    let y = mp.y + ((ms.height as f64 - h) / 2.0 - 16.0 * k).max(0.0) as i32;
    (Some(PhysicalPosition::new(x, y)), PhysicalSize::new(w as u32, h as u32))
}

/// Ctrl on Windows/Linux, Cmd on macOS.
fn primary_modifier(m: ModifiersState) -> bool {
    if cfg!(target_os = "macos") {
        m.super_key()
    } else {
        m.control_key()
    }
}

/// Custom-drawn chrome everywhere except macOS, which keeps its native traffic
/// lights over a transparent, full-size title bar.
pub const NATIVE_CHROME: bool = cfg!(target_os = "macos");

#[cfg(windows)]
fn platform_window(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::windows::{IconExtWindows, WindowAttributesExtWindows};
    let icon = winit::window::Icon::from_resource(1, None).ok();
    attrs
        .with_decorations(false)
        .with_window_icon(icon.clone())
        .with_taskbar_icon(icon)
        .with_undecorated_shadow(true)
        .with_class_name("VelaWindow")
        .with_drag_and_drop(true)
}

#[cfg(target_os = "macos")]
fn platform_window(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS;
    attrs.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_window(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    let icon = winit::window::Icon::from_rgba(include_bytes!("../build/icon-64.rgba").to_vec(), 64, 64).ok();
    WindowAttributesExtWayland::with_name(attrs.with_decorations(false).with_window_icon(icon), "vela", "vela")
}

fn cache_range(mpv: &Mpv, time: f64) -> Option<(f64, f64)> {
    let v = mpv.get_node("demuxer-cache-state")?;
    let ranges = v.get("seekable-ranges")?.as_array()?;
    for r in ranges {
        let (s, e) = (r.get("start")?.as_f64()?, r.get("end")?.as_f64()?);
        if time >= s - 1.0 && time <= e + 1.0 {
            return Some((s, e));
        }
    }
    None
}

fn stats(mpv: &Mpv, st: &State) -> Vec<(String, String)> {
    let g = |p: &str| mpv.get_str(p).unwrap_or_default();
    let mut rows = vec![("File".to_string(), Path::new(&st.path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(st.path.clone()))];
    rows.push(("Container".into(), g("file-format")));
    if st.vid.is_some() {
        let fps = mpv.get_f64("container-fps").unwrap_or(0.0);
        rows.push(("Video".into(), format!("{} · {}×{} · {:.3} fps", ui::codec_name(&g("video-format")), st.dwidth, st.dheight, fps)));
        let hw = if st.hwdec.is_empty() || st.hwdec == "no" { "software".to_string() } else { st.hwdec.clone() };
        rows.push(("Decoder".into(), format!("{} ({hw})", g("video-codec"))));
        let pix = mpv.get_node("video-params").map(|v| format!("{} · {} · {}", v["pixelformat"].as_str().unwrap_or(""), v["colormatrix"].as_str().unwrap_or(""), v["gamma"].as_str().unwrap_or(""))).unwrap_or_default();
        rows.push(("Pixels".into(), pix));
        let drop = mpv.get_i64("frame-drop-count").unwrap_or(0) + mpv.get_i64("decoder-frame-drop-count").unwrap_or(0);
        rows.push(("Dropped".into(), format!("{drop} frames")));
    }
    if st.aid.is_some() {
        let ap = mpv.get_node("audio-params");
        let (ch, sr) = ap.map(|v| (v["channel-count"].as_i64().unwrap_or(0), v["samplerate"].as_i64().unwrap_or(0))).unwrap_or((0, 0));
        rows.push(("Audio".into(), format!("{} · {ch}ch · {:.1} kHz", ui::codec_name(&g("audio-codec-name")), sr as f64 / 1000.0)));
    }
    let vb = mpv.get_f64("video-bitrate").unwrap_or(0.0);
    let ab = mpv.get_f64("audio-bitrate").unwrap_or(0.0);
    rows.push(("Bitrate".into(), format!("{:.2} Mb/s video · {:.0} kb/s audio", vb / 1e6, ab / 1e3)));
    rows.push(("Cache".into(), format!("{:.1} s ahead", mpv.get_f64("demuxer-cache-duration").unwrap_or(0.0))));
    rows.push(("Memory".into(), format!("{:.0} MB resident", platform::resident_mb())));
    rows
}

fn write_bmp(path: &Path, w: u32, h: u32, bgra: &[u8]) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(54 + bgra.len());
    let size = 54 + bgra.len() as u32;
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes()); // bottom-up, same as GL
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    out.extend_from_slice(bgra);
    std::fs::write(path, out)
}

impl ApplicationHandler<UserEvent> for App {

    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        if let Err(e) = self.create_window(el) {
            logf!("fatal: {e}");
            rfd::MessageDialog::new().set_title("VELA").set_description(format!("VELA could not start:\n{e}")).show();
            el.exit();
            return;
        }
        // Hidden windows get no RedrawRequested: paint the first frame by hand,
        // then show it (no white flash).
        self.redraw();
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Mpv => self.drain_mpv(),
            UserEvent::Frame => {
                if let Some(g) = &self.gfx {
                    if let Some(r) = &g.render {
                        r.update();
                    }
                }
                self.request_redraw();
            }
            UserEvent::Ipc(req) => self.handle_ipc(req),
            UserEvent::Open(paths) => self.queue_open(paths, OpenMode::Replace),
            UserEvent::Thumbs => self.take_thumbs(),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Route to egui first (it tracks pointer/keyboard state).
        // RedrawRequested is our own output, not input: egui answers it with
        // "repaint", which would turn every frame into a request for the next.
        // Keyboard goes to egui only while its one text field (the URL dialog)
        // is open; otherwise a clicked widget keeps focus and eats Space/arrows.
        let skip_egui = matches!(event, WindowEvent::RedrawRequested)
            || (matches!(event, WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_)) && self.ui.menu != Menu::Url)
            || matches!(event, WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } if self.resize_dir.is_some())
            || (matches!(event, WindowEvent::MouseWheel { .. }) && self.ui.menu == Menu::None && !self.home());
        if !skip_egui {
            if let Some(g) = &mut self.gfx {
                let r = g.egui_state.on_window_event(&g.window, &event);
                if r.repaint {
                    g.window.request_redraw();
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => self.quit_requested = true,
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) => {
                if let Some(g) = &self.gfx {
                    g.window.resize_surface(&g.surface, &g.context);
                    g.window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    self.activity();
                    if self.ui.menu == Menu::Url {
                        // Text field focused (URL dialog): only intercept paste/escape.
                        if primary_modifier(self.modifiers) && event.physical_key == PhysicalKey::Code(KeyCode::KeyV) {
                            if let (Some(t), Some(g)) = (platform::clipboard_text(), self.gfx.as_mut()) {
                                g.egui_state.egui_input_mut().events.push(egui::Event::Paste(t));
                            }
                        } else if event.logical_key == Key::Named(NamedKey::Escape) {
                            self.ui.close_all();
                        }
                        self.request_redraw();
                    } else {
                        let code = match event.physical_key {
                            PhysicalKey::Code(c) => Some(c),
                            _ => None,
                        };
                        self.on_key(&event.logical_key, code, event.repeat);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let moved = self.cursor.map_or(true, |c| (c.x - position.x).abs() > 1.0 || (c.y - position.y).abs() > 1.0);
                self.cursor = Some(position);
                if moved {
                    self.activity();
                }
                let dir = self.edge_at(position);
                if dir != self.resize_dir {
                    self.resize_dir = dir;
                }
                if let (Some(d), Some(g)) = (dir, &self.gfx) {
                    g.window.set_cursor(CursorIcon::from(match d {
                        ResizeDirection::East | ResizeDirection::West => CursorIcon::EwResize,
                        ResizeDirection::North | ResizeDirection::South => CursorIcon::NsResize,
                        ResizeDirection::NorthEast | ResizeDirection::SouthWest => CursorIcon::NeswResize,
                        _ => CursorIcon::NwseResize,
                    }));
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.resize_dir = None;
                self.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.activity();
                if state == ElementState::Pressed {
                    if button == MouseButton::Left {
                        if let (Some(d), Some(g)) = (self.resize_dir, &self.gfx) {
                            let _ = g.window.drag_resize_window(d);
                        }
                    } else if button == MouseButton::Back {
                        self.act(Action::SeekBy(-10.0));
                    } else if button == MouseButton::Forward {
                        self.act(Action::SeekBy(10.0));
                    }
                }
                self.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                self.activity();
                if self.ui.menu == Menu::None && !self.home() {
                    let dy = match delta {
                        MouseScrollDelta::LineDelta(_, y) => y,
                        MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                    };
                    if dy != 0.0 {
                        self.act(Action::VolumeBy(if dy > 0.0 { 5.0 } else { -5.0 }));
                    }
                }
                self.request_redraw();
            }
            WindowEvent::DroppedFile(p) => {
                self.dropped.push(p);
                self.drop_at = Some(Instant::now() + Duration::from_millis(40));
            }
            WindowEvent::HoveredFile(_) => self.osd("Drop to play"),
            WindowEvent::Focused(_) | WindowEvent::ScaleFactorChanged { .. } | WindowEvent::ThemeChanged(_) => self.request_redraw(),
            _ => {}
        }
        if self.quit_requested {
            self.shutdown(el);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none() {
            return;
        }
        let now = Instant::now();
        if self.quit_requested {
            self.shutdown(el);
            return;
        }
        // burst-collected opens / drops
        if self.pending_at.map_or(false, |t| now >= t) {
            self.flush_pending();
        }
        if self.drop_at.map_or(false, |t| now >= t) {
            self.drop_at = None;
            let dropped = std::mem::take(&mut self.dropped);
            let subs: Vec<String> = dropped.iter().filter(|p| is_subtitle(p) && !is_media(p)).map(|p| p.to_string_lossy().into_owned()).collect();
            let media: Vec<String> = dropped.iter().filter(|p| !(is_subtitle(p) && !is_media(p))).map(|p| p.to_string_lossy().into_owned()).collect();
            if !media.is_empty() {
                self.queue_open(media, OpenMode::Replace);
                self.flush_pending();
            }
            for s in subs {
                self.add_subtitle(&s);
            }
        }
        // sleep timer
        if self.sleep_at.map_or(false, |t| now >= t) {
            self.sleep_at = None;
            if let Some(m) = &self.mpv {
                m.set_flag("pause", true);
            }
            self.osd("Sleep timer — paused");
        }
        // periodic position snapshot (cheap, in memory) + rare disk flush
        if !self.st.idle && !self.st.pause && now.duration_since(self.last_record) > Duration::from_secs(10) {
            self.record_position(false);
        }
        if now.duration_since(self.last_flush) > Duration::from_secs(60) {
            self.store.flush();
            self.last_flush = now;
        }
        // Frugality: give memory back while minimised or idling on the home screen.
        let minimized = self.gfx.as_ref().and_then(|g| g.window.is_minimized()).unwrap_or(false);
        let idle_long = self.idle_since.map_or(false, |t| now.duration_since(t) > Duration::from_secs(4));
        if (minimized || idle_long) && !self.trimmed {
            self.trimmed = true;
            platform::trim_memory();
        } else if !minimized && !idle_long && self.trimmed && !self.st.idle {
            self.trimmed = false;
        }
        // controls auto-hide transition needs a frame
        let target = self.controls_target();
        if (target && self.controls < 1.0) || (!target && self.controls > 0.0) {
            self.request_redraw();
        }
        if self.repaint_at.map_or(false, |t| now >= t) {
            self.repaint_at = None;
            self.request_redraw();
        }

        // Sleep until the next deadline.
        let mut wake: Vec<Instant> = vec![];
        if let Some(t) = self.pending_at {
            wake.push(t);
        }
        if let Some(t) = self.drop_at {
            wake.push(t);
        }
        if let Some(t) = self.sleep_at {
            wake.push(t);
        }
        if let Some(t) = self.repaint_at {
            wake.push(t);
        }
        if let (Some(t), false) = (self.idle_since, self.trimmed) {
            wake.push(t + Duration::from_secs(4) + Duration::from_millis(50));
        }
        if !self.home() && self.controls > 0.0 && !self.st.pause && target {
            wake.push(self.last_activity + HIDE_AFTER);
        }
        if !self.st.idle && !self.st.pause {
            wake.push(self.last_record + Duration::from_secs(10));
        }
        match wake.into_iter().min() {
            Some(t) => el.set_control_flow(ControlFlow::WaitUntil(t.max(now + Duration::from_millis(1)))),
            None => el.set_control_flow(ControlFlow::Wait),
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.store.flush();
    }
}

impl App {
    pub fn spawn_ipc(&self, server: ipc::Server) {
        let proxy = self.proxy.clone();
        server.spawn(move |req| {
            let _ = proxy.send_event(UserEvent::Ipc(req));
        });
    }
}
