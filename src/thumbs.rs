//! Off-screen thumbnail engine.
//!
//! A second, headless libmpv instance decodes keyframes and renders them
//! through mpv's software render API straight into a small RGBA buffer - no
//! files, no image codecs. It lives on its own thread, coalesces requests
//! (only the newest seek-bar hover matters) and tears itself down after a few
//! idle seconds so it costs zero memory when not in use.

use crate::mpv::{Event, Mpv, RenderContext, END_ERROR, RENDER_UPDATE_FRAME};
use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Hover,
    Poster,
}

#[derive(Clone, Debug)]
pub struct Request {
    pub key: String,
    pub path: String,
    pub time: f64,
    pub w: u32,
    pub h: u32,
    pub kind: Kind,
}

pub struct Thumb {
    pub key: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub kind: Kind,
}

type Notify = Arc<dyn Fn() + Send + Sync>;

pub struct Thumbnailer {
    tx: Option<Sender<Request>>,
    results: Arc<Mutex<Vec<Thumb>>>,
    alive: Arc<Mutex<bool>>,
    notify: Notify,
}

const IDLE_EXIT: Duration = Duration::from_secs(12);

impl Thumbnailer {
    pub fn new(notify: Notify) -> Self {
        Self { tx: None, results: Arc::new(Mutex::new(Vec::new())), alive: Arc::new(Mutex::new(false)), notify }
    }

    pub fn request(&mut self, req: Request) {
        let running = *self.alive.lock().unwrap();
        if !running || self.tx.is_none() {
            let (tx, rx) = mpsc::channel();
            let results = self.results.clone();
            let alive = self.alive.clone();
            let notify = self.notify.clone();
            *alive.lock().unwrap() = true;
            let spawned = std::thread::Builder::new().name("vela-thumbs".into()).spawn(move || {
                worker(rx, results, notify);
                *alive.lock().unwrap() = false;
            });
            if spawned.is_err() {
                *self.alive.lock().unwrap() = false;
                return;
            }
            self.tx = Some(tx);
        }
        if let Some(tx) = &self.tx {
            if tx.send(req).is_err() {
                self.tx = None;
            }
        }
    }

    pub fn take(&self) -> Vec<Thumb> {
        std::mem::take(&mut *self.results.lock().unwrap())
    }
}

fn worker(rx: Receiver<Request>, results: Arc<Mutex<Vec<Thumb>>>, notify: Notify) {
    let Ok(mpv) = Mpv::new() else { return };
    for (k, v) in [
        ("config", "no"),
        ("terminal", "no"),
        ("msg-level", "all=no"),
        ("vo", "libmpv"),
        ("ao", "null"),
        ("aid", "no"),
        ("sid", "no"),
        ("audio", "no"),
        ("sub-auto", "no"),
        ("audio-file-auto", "no"),
        ("cover-art-auto", "no"),
        ("pause", "yes"),
        ("idle", "yes"),
        ("keep-open", "always"),
        ("hr-seek", "no"),
        ("cache", "no"),
        ("demuxer-readahead-secs", "0"),
        ("demuxer-max-bytes", "2MiB"),
        ("demuxer-max-back-bytes", "0"),
        ("hwdec", "no"),
        ("vd-lavc-threads", "2"),
        ("vd-lavc-skiploopfilter", "all"),
        ("vd-lavc-fast", "yes"),
        ("sws-scaler", "fast-bilinear"),
        ("ytdl", "no"),
        ("load-scripts", "no"),
        ("osc", "no"),
        ("load-stats-overlay", "no"),
        ("load-osd-console", "no"),
        ("load-auto-profiles", "no"),
        ("load-select", "no"),
        ("load-commands", "no"),
        ("load-context-menu", "no"),
        ("load-positioning", "no"),
        ("input-default-bindings", "no"),
        ("osd-level", "0"),
        ("background-color", "#000000"),
    ] {
        let _ = mpv.set_option(k, v);
    }
    if mpv.initialize().is_err() {
        return;
    }
    let Ok(mut rc) = RenderContext::new_sw(&mpv) else { return };
    let (ftx, frx) = mpsc::channel::<()>();
    rc.set_update_callback(Box::new(move || {
        let _ = ftx.send(());
    }));

    let mut current_path = String::new();
    let mut posters: VecDeque<Request> = VecDeque::new();
    let mut hover: Option<Request> = None;
    let mut last_work = Instant::now();

    loop {
        // Gather everything pending; block only when idle.
        if hover.is_none() && posters.is_empty() {
            match rx.recv_timeout(IDLE_EXIT.saturating_sub(last_work.elapsed()).max(Duration::from_millis(50))) {
                Ok(r) => enqueue(r, &mut hover, &mut posters),
                Err(RecvTimeoutError::Timeout) => {
                    if last_work.elapsed() >= IDLE_EXIT {
                        break;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        while let Ok(r) = rx.try_recv() {
            enqueue(r, &mut hover, &mut posters);
        }
        let Some(req) = hover.take().or_else(|| posters.pop_front()) else { continue };
        last_work = Instant::now();
        if let Some(rgba) = grab(&mpv, &rc, &frx, &mut current_path, &req) {
            results.lock().unwrap().push(Thumb { key: req.key.clone(), w: req.w, h: req.h, rgba, kind: req.kind });
            notify();
        }
    }
    drop(rc);
    drop(mpv);
}

fn enqueue(r: Request, hover: &mut Option<Request>, posters: &mut VecDeque<Request>) {
    match r.kind {
        Kind::Hover => *hover = Some(r),
        Kind::Poster => {
            if !posters.iter().any(|p| p.key == r.key) {
                posters.push_back(r)
            }
        }
    }
}

fn grab(mpv: &Mpv, rc: &RenderContext, frames: &Receiver<()>, current: &mut String, req: &Request) -> Option<Vec<u8>> {
    while frames.try_recv().is_ok() {}
    let t = format!("{:.3}", req.time.max(0.0));
    if *current != req.path {
        mpv.command(&["loadfile", &req.path, "replace", "-1", &format!("start={t}")]).ok()?;
        *current = req.path.clone();
    } else {
        mpv.command(&["seek", &t, "absolute+keyframes"]).ok()?;
    }
    let deadline = Instant::now() + Duration::from_millis(4000);
    let mut restarted_at: Option<Instant> = None;
    let mut got_frame = false;
    while Instant::now() < deadline {
        if frames.recv_timeout(Duration::from_millis(8)).is_ok() && rc.update() & RENDER_UPDATE_FRAME != 0 {
            got_frame = true;
        }
        while let Some(ev) = mpv.next_event(0.0) {
            match ev {
                Event::PlaybackRestart => restarted_at = Some(Instant::now()),
                Event::EndFile { reason, .. } if reason == END_ERROR => {
                    current.clear();
                    return None;
                }
                _ => {}
            }
        }
        // A seek landing on the frame already shown produces no new frame -
        // after the restart settles, render whatever is current.
        if got_frame && restarted_at.is_some() {
            break;
        }
        if let Some(t0) = restarted_at {
            if t0.elapsed() > Duration::from_millis(120) {
                break;
            }
        }
    }
    if restarted_at.is_none() && !got_frame {
        return None;
    }
    let mut buf = vec![0u8; (req.w * req.h * 4) as usize];
    if !rc.render_sw(req.w as i32, req.h as i32, &mut buf) {
        return None;
    }
    for px in buf.chunks_exact_mut(4) {
        px[3] = 255;
    }
    Some(buf)
}

/// Stable 64-bit FNV-1a hash used for cache keys / file names.
pub fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.to_lowercase().bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
