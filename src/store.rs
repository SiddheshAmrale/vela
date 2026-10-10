//! Settings + watch history, persisted as small JSON files in the OS config
//! directory (%APPDATA%\Vela, ~/Library/Application Support/Vela, ~/.config/vela).
//! Writes are atomic (temp file + rename) and only happen when something changed.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub volume: f64,
    pub muted: bool,
    pub autoplay_next: bool,
    pub remember_position: bool,
    pub folder_playlist: bool,
    pub sub_scale: f64,
    pub sub_pos: i64,
    pub sub_color: String,
    pub sub_back: f64,
    pub eq_preset: String,
    pub eq_gains: [i32; 10],
    pub normalize: bool,
    pub alang: String,
    pub slang: String,
    pub hwdec: bool,
    pub high_quality: bool,
    pub window: Option<[i32; 4]>,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 100.0,
            muted: false,
            autoplay_next: true,
            remember_position: true,
            folder_playlist: true,
            sub_scale: 1.0,
            sub_pos: 100,
            sub_color: "#FFFFFF".into(),
            sub_back: 0.0,
            eq_preset: "flat".into(),
            eq_gains: [0; 10],
            normalize: false,
            alang: String::new(),
            slang: "eng,en".into(),
            hwdec: true,
            high_quality: false,
            window: None,
            maximized: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HistoryEntry {
    pub path: String,
    pub title: String,
    pub position: f64,
    pub duration: f64,
    pub updated: u64,
    #[serde(default)]
    pub video: bool,
}

impl HistoryEntry {
    pub fn progress(&self) -> f64 {
        if self.duration > 0.0 {
            (self.position / self.duration).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
    /// Worth resuming: watched a bit, but not (almost) finished.
    pub fn resumable(&self) -> bool {
        self.duration > 60.0 && self.position > 15.0 && self.position < self.duration - 20.0 && self.progress() < 0.96
    }
}

pub struct Store {
    dir: PathBuf,
    pub settings: Settings,
    pub history: Vec<HistoryEntry>,
    settings_dirty: bool,
    history_dirty: bool,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

const APP_DIR: &str = if cfg!(any(windows, target_os = "macos")) { "Vela" } else { "vela" };

pub fn data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().map(|d| d.join(APP_DIR)).unwrap_or_else(data_dir)
}

/// Paths are case-insensitive on Windows, case-sensitive elsewhere.
pub fn same_path(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

const MAX_HISTORY: usize = 200;

impl Store {
    pub fn load() -> Self {
        let dir = data_dir();
        let _ = fs::create_dir_all(&dir);
        let settings = read_json(&dir.join("settings.json")).unwrap_or_default();
        let history = read_json(&dir.join("history.json")).unwrap_or_default();
        Self { dir, settings, history, settings_dirty: false, history_dirty: false }
    }

    pub fn mark_settings(&mut self) {
        self.settings_dirty = true;
    }

    pub fn flush(&mut self) {
        if self.settings_dirty {
            write_json(&self.dir.join("settings.json"), &self.settings);
            self.settings_dirty = false;
        }
        if self.history_dirty {
            write_json(&self.dir.join("history.json"), &self.history);
            self.history_dirty = false;
        }
    }

    pub fn find(&self, path: &str) -> Option<&HistoryEntry> {
        self.history.iter().find(|h| same_path(&h.path, path))
    }

    pub fn record(&mut self, path: &str, title: &str, position: f64, duration: f64, video: bool) {
        if path.is_empty() || !position.is_finite() {
            return;
        }
        let idx = self.history.iter().position(|h| same_path(&h.path, path));
        let mut entry = match idx {
            Some(i) => self.history.remove(i),
            None => HistoryEntry { path: path.to_string(), title: String::new(), position: 0.0, duration: 0.0, updated: 0, video },
        };
        entry.title = title.to_string();
        entry.position = position;
        if duration > 0.0 {
            entry.duration = duration;
        }
        entry.updated = now_secs();
        entry.video = video;
        self.history.insert(0, entry);
        self.history.truncate(MAX_HISTORY);
        self.history_dirty = true;
    }

    pub fn forget(&mut self, path: &str) {
        self.history.retain(|h| !same_path(&h.path, path));
        self.history_dirty = true;
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_dirty = true;
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(p: &Path) -> Option<T> {
    let data = fs::read(p).ok()?;
    serde_json::from_slice(&data).ok()
}

fn write_json<T: Serialize>(p: &Path, v: &T) {
    if let Ok(data) = serde_json::to_vec_pretty(v) {
        let tmp = p.with_extension("tmp");
        if fs::write(&tmp, data).is_ok() {
            let _ = fs::rename(&tmp, p);
        }
    }
}
