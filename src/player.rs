//! The playback engine: one libmpv instance plus a mirror of the properties the
//! UI cares about. Fast-changing values (time-pos, cache) are *pulled* on
//! demand when the UI draws, instead of being pushed as events every frame.

use crate::mpv::Mpv;
use crate::store::Settings;
use serde_json::Value;

pub const OBSERVED: &[&str] = &[
    "idle-active",
    "pause",
    "duration",
    "path",
    "media-title",
    "volume",
    "mute",
    "speed",
    "track-list",
    "chapter-list",
    "playlist",
    "playlist-pos",
    "sub-delay",
    "audio-delay",
    "paused-for-cache",
    "seeking",
    "eof-reached",
    "dwidth",
    "dheight",
    "loop-file",
    "loop-playlist",
    "ab-loop-a",
    "ab-loop-b",
    "sub-visibility",
    "video-aspect-override",
    "video-zoom",
    "video-rotate",
    "panscan",
    "keepaspect",
    "brightness",
    "contrast",
    "saturation",
    "gamma",
    "hue",
    "deinterlace",
    "vid",
    "aid",
    "sid",
    "chapter",
    "hwdec-current",
];

pub const HOOK_LOAD: u64 = 1;
pub const HOOK_UNLOAD: u64 = 2;

#[derive(Clone, Default, Debug)]
pub struct Track {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub lang: String,
    pub codec: String,
    pub forced: bool,
    pub external: bool,
    pub selected: bool,
    pub channels: i64,
    pub albumart: bool,
}

impl Track {
    pub fn label(&self) -> String {
        let lang = crate::ui::lang_name(&self.lang);
        let mut parts: Vec<String> = Vec::new();
        if !self.title.is_empty() {
            parts.push(self.title.clone());
        }
        if !lang.is_empty() && !self.title.to_lowercase().contains(&lang.to_lowercase()) {
            parts.push(lang);
        }
        if parts.is_empty() {
            parts.push(format!("Track {}", self.id));
        }
        parts.join(" · ")
    }
    pub fn detail(&self) -> String {
        let mut d = vec![crate::ui::codec_name(&self.codec)];
        if self.kind == "audio" && self.channels > 0 {
            d.push(match self.channels {
                1 => "Mono".into(),
                2 => "Stereo".into(),
                6 => "5.1".into(),
                8 => "7.1".into(),
                n => format!("{n}ch"),
            });
        }
        if self.forced {
            d.push("Forced".into());
        }
        if self.external {
            d.push("External".into());
        }
        d.join(" · ")
    }
}

#[derive(Clone, Debug)]
pub struct Chapter {
    pub time: f64,
    pub title: String,
}

#[derive(Clone, Debug)]
pub struct PlEntry {
    pub filename: String,
    pub title: String,
    pub current: bool,
}

impl PlEntry {
    /// Playlist label: the file name (with extension, so "clip.mkv" and
    /// "clip.mp4" stay distinguishable), or the stream title for URLs.
    pub fn name(&self) -> String {
        if crate::platform::is_url(&self.filename) && !self.title.is_empty() {
            return self.title.clone();
        }
        std::path::Path::new(&self.filename).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| self.filename.clone())
    }
}

pub fn display_name(path: &str) -> String {
    if crate::platform::is_url(path) {
        return path.to_string();
    }
    std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

#[derive(Clone, Debug)]
pub struct State {
    pub idle: bool,
    pub pause: bool,
    pub duration: f64,
    pub path: String,
    pub title: String,
    pub volume: f64,
    pub mute: bool,
    pub speed: f64,
    pub tracks: Vec<Track>,
    pub chapters: Vec<Chapter>,
    pub chapter: i64,
    pub playlist: Vec<PlEntry>,
    pub pl_pos: i64,
    pub sub_delay: f64,
    pub audio_delay: f64,
    pub buffering: bool,
    pub seeking: bool,
    pub eof: bool,
    pub dwidth: i64,
    pub dheight: i64,
    pub loop_file: bool,
    pub loop_playlist: bool,
    pub ab_a: Option<f64>,
    pub ab_b: Option<f64>,
    pub sub_visible: bool,
    pub aspect: f64,
    pub zoom: f64,
    pub rotate: i64,
    pub panscan: f64,
    pub keepaspect: bool,
    pub brightness: i64,
    pub contrast: i64,
    pub saturation: i64,
    pub gamma: i64,
    pub hue: i64,
    pub deinterlace: String,
    pub vid: Option<i64>,
    pub aid: Option<i64>,
    pub sid: Option<i64>,
    pub hwdec: String,
}

impl Default for State {
    fn default() -> Self {
        Self {
            idle: true,
            pause: false,
            duration: 0.0,
            path: String::new(),
            title: String::new(),
            volume: 100.0,
            mute: false,
            speed: 1.0,
            tracks: vec![],
            chapters: vec![],
            chapter: -1,
            playlist: vec![],
            pl_pos: -1,
            sub_delay: 0.0,
            audio_delay: 0.0,
            buffering: false,
            seeking: false,
            eof: false,
            dwidth: 0,
            dheight: 0,
            loop_file: false,
            loop_playlist: false,
            ab_a: None,
            ab_b: None,
            sub_visible: true,
            aspect: -1.0,
            zoom: 0.0,
            rotate: 0,
            panscan: 0.0,
            keepaspect: true,
            brightness: 0,
            contrast: 0,
            saturation: 0,
            gamma: 0,
            hue: 0,
            deinterlace: "no".into(),
            vid: None,
            aid: None,
            sid: None,
            hwdec: String::new(),
        }
    }
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
fn i(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|x| x as i64)).unwrap_or(0)
}
fn b(v: &Value) -> bool {
    match v {
        Value::Bool(x) => *x,
        Value::String(s) => s != "no",
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        _ => false,
    }
}
fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
fn opt_id(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(x) => x.parse().ok(),
        _ => None,
    }
}
fn opt_time(v: &Value) -> Option<f64> {
    v.as_f64()
}

impl State {
    pub fn apply(&mut self, name: &str, v: &Value) {
        match name {
            "idle-active" => self.idle = b(v),
            "pause" => self.pause = b(v),
            "duration" => self.duration = f(v),
            "path" => self.path = s(v),
            "media-title" => self.title = s(v),
            "volume" => self.volume = f(v),
            "mute" => self.mute = b(v),
            "speed" => self.speed = if v.is_null() { 1.0 } else { f(v) },
            "track-list" => self.tracks = parse_tracks(v),
            "chapter-list" => {
                self.chapters = v
                    .as_array()
                    .map(|a| a.iter().map(|c| Chapter { time: f(&c["time"]), title: s(&c["title"]) }).collect())
                    .unwrap_or_default()
            }
            "chapter" => self.chapter = if v.is_null() { -1 } else { i(v) },
            "playlist" => {
                self.playlist = v
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|e| PlEntry { filename: s(&e["filename"]), title: s(&e["title"]), current: b(&e["current"]) || b(&e["playing"]) })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            "playlist-pos" => self.pl_pos = if v.is_null() { -1 } else { i(v) },
            "sub-delay" => self.sub_delay = f(v),
            "audio-delay" => self.audio_delay = f(v),
            "paused-for-cache" => self.buffering = b(v),
            "seeking" => self.seeking = b(v),
            "eof-reached" => self.eof = b(v),
            "dwidth" => self.dwidth = i(v),
            "dheight" => self.dheight = i(v),
            "loop-file" => self.loop_file = b(v),
            "loop-playlist" => self.loop_playlist = b(v),
            "ab-loop-a" => self.ab_a = opt_time(v),
            "ab-loop-b" => self.ab_b = opt_time(v),
            "sub-visibility" => self.sub_visible = b(v),
            "video-aspect-override" => self.aspect = v.as_f64().unwrap_or(-1.0),
            "video-zoom" => self.zoom = f(v),
            "video-rotate" => self.rotate = i(v),
            "panscan" => self.panscan = f(v),
            "keepaspect" => self.keepaspect = b(v),
            "brightness" => self.brightness = i(v),
            "contrast" => self.contrast = i(v),
            "saturation" => self.saturation = i(v),
            "gamma" => self.gamma = i(v),
            "hue" => self.hue = i(v),
            "deinterlace" => self.deinterlace = s(v),
            "vid" => self.vid = opt_id(v),
            "aid" => self.aid = opt_id(v),
            "sid" => self.sid = opt_id(v),
            "hwdec-current" => self.hwdec = s(v),
            _ => {}
        }
    }

    /// What to call the current item. Embedded titles of downloaded videos are
    /// often junk (release-group tags, site watermarks), so local videos use
    /// the file name; audio and streams keep their metadata title.
    pub fn display_title(&self) -> String {
        let local = !self.path.is_empty() && !crate::platform::is_url(&self.path);
        if local && (self.has_video() || self.title.is_empty()) {
            display_name(&self.path)
        } else {
            self.title.clone()
        }
    }

    pub fn has_video(&self) -> bool {
        self.vid.is_some() && self.tracks.iter().any(|t| t.kind == "video" && t.selected && !t.albumart)
    }

    pub fn tracks_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Track> + 'a {
        self.tracks.iter().filter(move |t| t.kind == kind)
    }
}

fn parse_tracks(v: &Value) -> Vec<Track> {
    let Some(arr) = v.as_array() else { return vec![] };
    arr.iter()
        .map(|t| {
            let codec = s(&t["codec"]);
            Track {
                id: i(&t["id"]),
                kind: s(&t["type"]),
                title: s(&t["title"]),
                lang: s(&t["lang"]),
                codec,
                forced: b(&t["forced"]),
                external: b(&t["external"]),
                selected: b(&t["selected"]),
                channels: i(&t["demux-channel-count"]),
                albumart: b(&t["albumart"]) || b(&t["image"]),
            }
        })
        .collect()
}

pub const EQ_FREQS: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];

pub const EQ_PRESETS: &[(&str, &str, [i32; 10])] = &[
    ("flat", "Flat", [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    ("bass", "Bass boost", [7, 6, 5, 3, 1, 0, 0, 0, 0, 0]),
    ("treble", "Treble boost", [0, 0, 0, 0, 0, 1, 3, 5, 6, 7]),
    ("voice", "Dialogue", [-3, -2, -1, 1, 4, 5, 4, 2, 0, -1]),
    ("cinema", "Cinema", [4, 3, 1, 0, -1, 1, 2, 3, 3, 2]),
    ("rock", "Rock", [5, 4, 3, 1, -1, -1, 1, 3, 4, 5]),
    ("pop", "Pop", [-1, 1, 3, 4, 4, 2, 0, -1, -1, -1]),
    ("classical", "Classical", [4, 3, 2, 1, 0, 0, 0, 2, 3, 4]),
];

/// Audio filter chain for mpv's `af` property.
pub fn audio_filters(gains: &[i32; 10], normalize: bool) -> String {
    let mut parts = Vec::new();
    let bands: Vec<String> = gains
        .iter()
        .zip(EQ_FREQS)
        .filter(|(g, _)| **g != 0)
        .map(|(g, f)| format!("equalizer=f={f}:t=o:w=1:g={g}"))
        .collect();
    if !bands.is_empty() {
        parts.push(format!("@eq:lavfi=[{}]", bands.join(",")));
    }
    if normalize {
        parts.push("@norm:lavfi=[dynaudnorm=f=250:g=15:p=0.9:m=8]".to_string());
    }
    parts.join(",")
}

fn sub_style(mpv: &Mpv, st: &Settings) {
    let _ = mpv.set_str("sub-scale", &format!("{:.2}", st.sub_scale));
    let _ = mpv.set_str("sub-pos", &st.sub_pos.to_string());
    let _ = mpv.set_str("sub-color", &st.sub_color);
    if st.sub_back > 0.01 {
        let a = (st.sub_back.clamp(0.0, 1.0) * 255.0) as u8;
        let _ = mpv.set_str("sub-border-style", "background-box");
        let _ = mpv.set_str("sub-back-color", &format!("#{a:02X}000000"));
    } else {
        let _ = mpv.set_str("sub-border-style", "outline-and-shadow");
    }
}

pub fn apply_settings(mpv: &Mpv, st: &Settings) {
    sub_style(mpv, st);
    let _ = mpv.set_str("af", &audio_filters(&st.eq_gains, st.normalize));
    let _ = mpv.set_str("keep-open", if st.autoplay_next { "yes" } else { "always" });
    let _ = mpv.set_str("hwdec", if st.hwdec { "auto-safe" } else { "no" });
}

/// Create and configure the main mpv instance.
pub fn create(st: &Settings, screenshot_dir: &str) -> Result<Mpv, String> {
    let mpv = Mpv::new()?;
    let opts: Vec<(&str, String)> = vec![
        ("config", "no".into()),
        ("terminal", "no".into()),
        ("vo", "libmpv".into()),
        ("hwdec", if st.hwdec { "auto-safe" } else { "no" }.into()),
        ("idle", "yes".into()),
        ("keep-open", if st.autoplay_next { "yes" } else { "always" }.into()),
        ("input-default-bindings", "no".into()),
        ("input-vo-keyboard", "no".into()),
        ("osc", "no".into()),
        ("osd-level", "0".into()),
        ("load-scripts", "no".into()),
        ("load-stats-overlay", "no".into()),
        ("load-osd-console", "no".into()),
        ("load-auto-profiles", "no".into()),
        ("load-select", "no".into()),
        ("load-commands", "no".into()),
        ("load-context-menu", "no".into()),
        ("load-positioning", "no".into()),
        ("ytdl", "yes".into()),
        ("audio-client-name", "VELA".into()),
        ("volume-max", "200".into()),
        ("volume", format!("{}", st.volume.clamp(0.0, 200.0))),
        ("mute", if st.muted { "yes" } else { "no" }.into()),
        ("sub-auto", "fuzzy".into()),
        ("sub-file-paths", "subs;Subs;subtitles;Subtitles;sub;Sub".into()),
        ("audio-file-auto", "fuzzy".into()),
        ("slang", st.slang.clone()),
        ("alang", st.alang.clone()),
        ("subs-with-matching-audio", "no".into()),
        ("screenshot-directory", screenshot_dir.into()),
        ("screenshot-format", "png".into()),
        ("screenshot-png-compression", "5".into()),
        ("screenshot-template", "%{filename/no-ext} %wH.%wM.%wS (%n)".into()),
        ("vd-lavc-threads", std::thread::available_parallelism().map(|n| n.get().clamp(2, 8)).unwrap_or(4).to_string()),
        ("demuxer-max-bytes", "96MiB".into()),
        ("demuxer-max-back-bytes", "32MiB".into()),
        ("force-seekable", "yes".into()),
        ("cache", "auto".into()),
        ("reset-on-next-file", "video-aspect-override,video-zoom,video-pan-x,video-pan-y,video-rotate,panscan,keepaspect,sub-delay,audio-delay,ab-loop-a,ab-loop-b".into()),
        ("background-color", "#000000".into()),
        ("image-display-duration", "inf".into()),
        ("msg-level", "all=warn".into()),
    ];
    let mut opts = opts;
    if st.high_quality {
        opts.push(("profile", "high-quality".into()));
    }
    for (k, v) in &opts {
        if let Err(e) = mpv.set_option(k, v) {
            crate::logf!("mpv option ignored: {e}");
        }
    }
    mpv.initialize()?;
    apply_settings(&mpv, st);
    mpv.request_log("warn");
    for (idx, name) in OBSERVED.iter().enumerate() {
        mpv.observe(idx as u64 + 1, name);
    }
    mpv.hook_add(HOOK_LOAD, "on_load", 50);
    mpv.hook_add(HOOK_UNLOAD, "on_unload", 50);
    Ok(mpv)
}
