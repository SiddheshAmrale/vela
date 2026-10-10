//! OS integration: media file detection, folder scanning, clipboard, keeping
//! the display awake, memory stats, reveal-in-file-manager and logging.
//! Everything OS-specific lives here behind small cross-platform functions.

use std::cmp::Ordering;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub const VIDEO_EXT: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "wmv", "flv", "f4v", "mpg", "mpeg", "m2v", "mpe", "ts", "mts", "m2ts", "vob", "3gp",
    "3g2", "ogv", "asf", "rm", "rmvb", "divx", "xvid", "mxf", "dv", "hevc", "h264", "h265", "y4m", "nut", "ivf", "evo", "ogm", "qt",
];
pub const AUDIO_EXT: &[&str] = &[
    "mp3", "flac", "wav", "aac", "m4a", "ogg", "oga", "opus", "wma", "aiff", "aif", "alac", "ape", "ac3", "eac3", "dts", "mka", "wv",
    "amr", "mpc", "tta", "m4b", "dsf", "dff", "caf", "mid",
];
pub const SUB_EXT: &[&str] = &["srt", "vtt", "ass", "ssa", "sub", "idx", "sup", "smi", "sbv", "ttml", "dfxp", "lrc", "txt"];
pub const PLAYLIST_EXT: &[&str] = &["m3u", "m3u8", "pls", "cue"];

fn ext_of(p: &Path) -> String {
    p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub fn is_media(p: &Path) -> bool {
    let e = ext_of(p);
    VIDEO_EXT.contains(&e.as_str()) || AUDIO_EXT.contains(&e.as_str()) || PLAYLIST_EXT.contains(&e.as_str())
}

pub fn is_subtitle(p: &Path) -> bool {
    SUB_EXT.contains(&ext_of(p).as_str())
}

pub fn is_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    ["http://", "https://", "rtmp://", "rtsp://", "rtp://", "udp://", "mms://", "ftp://", "smb://", "ytdl://"].iter().any(|p| l.starts_with(p))
}

/// Natural ("Episode 2" < "Episode 10"), case-insensitive comparison.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

fn sort_paths(v: &mut [PathBuf]) {
    v.sort_by(|a, b| natural_cmp(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy()));
}

/// Media files in the same folder as `file` (sorted naturally), plus the
/// index of `file` within that list.
pub fn folder_siblings(file: &Path) -> (Vec<PathBuf>, usize) {
    let Some(dir) = file.parent() else { return (vec![file.to_path_buf()], 0) };
    let want_audio = AUDIO_EXT.contains(&ext_of(file).as_str());
    let mut list: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .filter(|p| {
                    // keep video with video, audio with audio
                    let e = ext_of(p);
                    if want_audio { AUDIO_EXT.contains(&e.as_str()) } else { VIDEO_EXT.contains(&e.as_str()) }
                })
                .collect()
        })
        .unwrap_or_default();
    sort_paths(&mut list);
    let same = |p: &PathBuf| p.as_path() == file || (cfg!(windows) && p.to_string_lossy().eq_ignore_ascii_case(&file.to_string_lossy()));
    match list.iter().position(same) {
        Some(i) => (list, i),
        None => (vec![file.to_path_buf()], 0),
    }
}

/// Recursively collect media files from a folder (depth-limited).
pub fn walk_media(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        if depth > 6 || out.len() > 5000 {
            return;
        }
        let Ok(rd) = fs::read_dir(dir) else { return };
        let (mut files, mut dirs) = (Vec::new(), Vec::new());
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.starts_with('$') {
                continue;
            }
            if p.is_dir() {
                dirs.push(p);
            } else if is_media(&p) && !PLAYLIST_EXT.contains(&ext_of(&p).as_str()) {
                files.push(p);
            }
        }
        sort_paths(&mut files);
        sort_paths(&mut dirs);
        out.extend(files);
        for d in dirs {
            walk(&d, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out
}

pub fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

pub fn pictures_dir() -> PathBuf {
    dirs::picture_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."))
}

/// Open the system file manager with `path` selected.
pub fn show_in_folder(path: &str) {
    use std::process::Command;
    #[cfg(windows)]
    let _ = Command::new("explorer.exe").arg(format!("/select,{path}")).spawn();
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let uri = format!("['file://{path}']");
        let shown = Command::new("gdbus")
            .args(["call", "--session", "--dest", "org.freedesktop.FileManager1", "--object-path", "/org/freedesktop/FileManager1"])
            .args(["--method", "org.freedesktop.FileManager1.ShowItems", &uri, ""])
            .status()
            .map_or(false, |s| s.success());
        if !shown {
            let dir = Path::new(path).parent().unwrap_or(Path::new("/"));
            let _ = Command::new("xdg-open").arg(dir).spawn();
        }
    }
}

/// Let another process (the running instance) bring its window to the front.
pub fn allow_foreground_handoff() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};
        AllowSetForegroundWindow(ASFW_ANY);
    }
}

// ---------------------------------------------------------------------------
// Keep the display awake while a video plays

#[cfg(windows)]
pub fn keep_awake(on: bool) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED};
    unsafe {
        SetThreadExecutionState(if on { ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS });
    }
}

#[cfg(target_os = "macos")]
pub fn keep_awake(on: bool) {
    use std::ffi::{c_char, c_void};
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(kind: *const c_void, level: u32, name: *const c_void, id: *mut u32) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(alloc: *const c_void, s: *const c_char, encoding: u32) -> *const c_void;
        fn CFRelease(p: *const c_void);
    }
    static ASSERTION: Mutex<Option<u32>> = Mutex::new(None);
    let mut held = ASSERTION.lock().unwrap();
    unsafe {
        if on && held.is_none() {
            let kind = CFStringCreateWithCString(std::ptr::null(), c"PreventUserIdleDisplaySleep".as_ptr(), 0x0800_0100);
            let name = CFStringCreateWithCString(std::ptr::null(), c"VELA playback".as_ptr(), 0x0800_0100);
            let mut id = 0u32;
            if IOPMAssertionCreateWithName(kind, 255, name, &mut id) == 0 {
                *held = Some(id);
            }
            CFRelease(kind);
            CFRelease(name);
        } else if !on {
            if let Some(id) = held.take() {
                IOPMAssertionRelease(id);
            }
        }
    }
}

/// freedesktop ScreenSaver inhibit via `gdbus` (present on every desktop that
/// has the service), on a worker thread so the UI never waits on D-Bus.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn keep_awake(on: bool) {
    use std::process::Command;
    static COOKIE: Mutex<Option<String>> = Mutex::new(None);
    std::thread::spawn(move || {
        let mut cookie = COOKIE.lock().unwrap();
        let base = ["call", "--session", "--dest", "org.freedesktop.ScreenSaver", "--object-path", "/org/freedesktop/ScreenSaver", "--method"];
        if on && cookie.is_none() {
            if let Ok(out) = Command::new("gdbus").args(base).args(["org.freedesktop.ScreenSaver.Inhibit", "VELA", "Playing video"]).output() {
                let text = String::from_utf8_lossy(&out.stdout);
                let digits: String = text.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    *cookie = Some(digits);
                }
            }
        } else if !on {
            if let Some(c) = cookie.take() {
                let _ = Command::new("gdbus").args(base).args(["org.freedesktop.ScreenSaver.UnInhibit", &c]).output();
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Memory

/// Resident memory of this process in MB (stats overlay).
pub fn resident_mb() -> f64 {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return c.WorkingSetSize as f64 / (1024.0 * 1024.0);
        }
    }
    #[cfg(target_os = "linux")]
    if let Ok(s) = fs::read_to_string("/proc/self/statm") {
        if let Some(pages) = s.split_whitespace().nth(1).and_then(|v| v.parse::<f64>().ok()) {
            return pages * 4096.0 / (1024.0 * 1024.0);
        }
    }
    0.0
}

/// Hand resident pages back to the OS (they fault back in on demand). Used
/// when minimised or idling on the home screen. Windows only; elsewhere the
/// kernel already reclaims clean pages lazily.
pub fn trim_memory() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::EmptyWorkingSet;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        EmptyWorkingSet(GetCurrentProcess());
    }
}

// ---------------------------------------------------------------------------
// Startup clock + tiny file logger (truncated each start)

pub static START: OnceLock<std::time::Instant> = OnceLock::new();

pub fn since_start_ms() -> u128 {
    START.get_or_init(std::time::Instant::now).elapsed().as_millis()
}

static LOG: Mutex<Option<fs::File>> = Mutex::new(None);

pub fn init_log() {
    let dir = crate::store::cache_dir();
    let _ = fs::create_dir_all(&dir);
    if let Ok(f) = fs::File::create(dir.join("vela.log")) {
        *LOG.lock().unwrap() = Some(f);
    }
}

pub fn log(msg: &str) {
    if let Ok(mut g) = LOG.lock() {
        if let Some(f) = g.as_mut() {
            let _ = writeln!(f, "{msg}");
        }
    }
    #[cfg(debug_assertions)]
    eprintln!("{msg}");
}

#[macro_export]
macro_rules! logf {
    ($($t:tt)*) => { $crate::platform::log(&format!($($t)*)) };
}
