//! Immediate-mode UI (egui), painted on the GPU after mpv renders the frame.
//! Everything is drawn procedurally: vector icons, gradients and the seek bar
//! are a few dozen triangles each - no image assets, no icon fonts.

use crate::player::{State, Track, EQ_FREQS, EQ_PRESETS};
use crate::store::{HistoryEntry, Settings};
use egui::epaint::{CornerRadius, Mesh, Shadow};
use egui::{
    pos2, vec2, Align2, Color32, Context, FontFamily, FontId, Id, LayerId, Order, Painter, Pos2, Rect, Response, Sense, Shape,
    Stroke, TextureId, Ui, Vec2,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Theme

// Monochrome, mission-control palette: near-black, white, an ice-white accent
// and one deep-blue "earth limb" glow.
pub const BG: Color32 = Color32::from_rgb(6, 7, 10);
pub const TEXT: Color32 = Color32::from_rgb(240, 242, 246);
pub const DIM: Color32 = Color32::from_rgba_premultiplied(140, 144, 152, 200);
pub const ACCENT: Color32 = Color32::from_rgb(236, 242, 255);
/// Text drawn on top of ACCENT fills.
const INK: Color32 = Color32::from_rgb(6, 8, 12);
const PANEL: Color32 = Color32::from_rgba_premultiplied(11, 12, 16, 242);
const LINE: Color32 = Color32::from_rgba_premultiplied(40, 40, 44, 40);

/// Straight-alpha colour -> premultiplied, in gamma space.
fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    let m = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
    Color32::from_rgba_premultiplied(m(r), m(g), m(b), a)
}

/// Premultiplied white at `a` opacity (gamma space, like every other colour here).
fn white(a: f32) -> Color32 {
    let v = (a.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgba_premultiplied(v, v, v, v)
}
fn black(a: f32) -> Color32 {
    Color32::from_black_alpha((a.clamp(0.0, 1.0) * 255.0) as u8)
}
fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
fn font_sb(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn setup(ctx: &Context) {
    ctx.set_fonts(crate::fonts::definitions());
    let mut style = (*ctx.style()).clone();
    let mut v = egui::Visuals::dark();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = PANEL;
    v.extreme_bg_color = white(0.07);
    v.faint_bg_color = white(0.04);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.3);
    v.selection.stroke = Stroke::new(1.0_f32, TEXT);
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.popup_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: black(0.5) };
    v.window_shadow = v.popup_shadow;
    v.override_text_color = Some(TEXT);
    v.text_cursor.stroke = Stroke::new(1.5_f32, ACCENT);
    let w = &mut v.widgets;
    for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        wv.corner_radius = CornerRadius::same(8);
        wv.bg_stroke = Stroke::NONE;
        wv.fg_stroke = Stroke::new(1.5_f32, TEXT);
        wv.expansion = 0.0;
    }
    w.inactive.bg_fill = white(0.16);
    w.inactive.weak_bg_fill = white(0.08);
    w.hovered.bg_fill = TEXT;
    w.hovered.weak_bg_fill = white(0.14);
    w.active.bg_fill = TEXT;
    w.active.weak_bg_fill = white(0.2);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, DIM);
    style.visuals = v;
    style.spacing.item_spacing = vec2(8.0, 6.0);
    style.spacing.button_padding = vec2(14.0, 7.0);
    style.spacing.slider_width = 190.0;
    style.spacing.interact_size.y = 26.0;
    style.interaction.selectable_labels = false;
    style.interaction.tooltip_delay = 0.35;
    style.animation_time = 0.14;
    style.text_styles.insert(egui::TextStyle::Body, font(14.0));
    style.text_styles.insert(egui::TextStyle::Button, font(14.0));
    style.text_styles.insert(egui::TextStyle::Small, font(12.0));
    ctx.set_style(style);
    ctx.options_mut(|o| {
        o.zoom_with_keyboard = false;
    });
}

// ---------------------------------------------------------------------------
// Model

#[derive(Debug, Clone)]
pub enum Action {
    TogglePause,
    SeekTo(f64, bool),
    SeekBy(f64),
    Next,
    Prev,
    PlayIndex(i64),
    RemoveIndex(i64),
    SetVolume(f64),
    VolumeBy(f64),
    ToggleMute,
    SetSpeed(f64),
    ToggleFullscreen,
    Minimize,
    ToggleMaximize,
    Close,
    ToggleOnTop,
    StartDrag,
    SetSub(Option<i64>),
    LoadSubtitle,
    SubDelay(f64),
    SetAudio(i64),
    AudioDelay(f64),
    SetAspect(&'static str),
    SetZoom(f64),
    SetRotate(i64),
    SetVideoEq(&'static str, i64),
    ResetVideo,
    SetDeinterlace(bool),
    SetEqPreset(&'static str),
    SetEqBand(usize, i32),
    SetNormalize(bool),
    SetSubScale(f64),
    SetSubPos(i64),
    SetSubColor(&'static str),
    SetSubBack(f64),
    SetLoopFile(bool),
    SetLoopPlaylist(bool),
    AbLoop,
    Shuffle,
    SetSleep(Option<u32>),
    SetAutoplay(bool),
    SetHwdec(bool),
    SetHighQuality(bool),
    SetRemember(bool),
    SetFolderPlaylist(bool),
    Screenshot,
    OpenFiles,
    OpenFolder,
    OpenUrl(String),
    OpenPaths(Vec<String>),
    ForgetHistory(String),
    ClearHistory,
    Stop,
    ShowInFolder,
    ChapterBy(i64),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Menu {
    None,
    Settings,
    Subs,
    Audio,
    Speed,
    Playlist,
    Context,
    Url,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Main,
    Speed,
    Subs,
    SubStyle,
    Audio,
    Video,
    Eq,
    Repeat,
    Sleep,
    Prefs,
}

pub struct UiState {
    pub menu: Menu,
    pub page: Page,
    pub ctx_pos: Pos2,
    pub seek_drag: Option<f64>,
    pub remaining: bool,
    pub url_text: String,
    pub url_focus: bool,
    pub help: bool,
    pub info: bool,
    menu_changed: bool,
    vol_drag: bool,
    last_page: Option<(u8, u8)>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            menu: Menu::None,
            page: Page::Main,
            ctx_pos: Pos2::ZERO,
            seek_drag: None,
            remaining: false,
            url_text: String::new(),
            url_focus: false,
            help: false,
            info: false,
            menu_changed: false,
            vol_drag: false,
            last_page: None,
        }
    }
}

impl UiState {
    pub fn open(&mut self, m: Menu) {
        self.menu = if self.menu == m { Menu::None } else { m };
        self.menu_changed = true;
        self.page = match m {
            Menu::Subs => Page::Subs,
            Menu::Audio => Page::Audio,
            Menu::Speed => Page::Speed,
            _ => Page::Main,
        };
        if m == Menu::Url {
            self.url_focus = true;
        }
    }
    pub fn close_all(&mut self) -> bool {
        let had = self.menu != Menu::None || self.help;
        self.menu = Menu::None;
        self.help = false;
        had
    }
}

pub struct View<'a> {
    pub st: &'a State,
    pub settings: &'a Settings,
    pub history: &'a [HistoryEntry],
    pub home: bool,
    pub time: f64,
    pub cache: Option<(f64, f64)>,
    pub fullscreen: bool,
    pub maximized: bool,
    pub on_top: bool,
    pub controls: f32,
    pub osd: Option<(&'a str, f32)>,
    pub hover_thumb: Option<(TextureId, Vec2, f64)>,
    pub posters: &'a HashMap<String, TextureId>,
    pub info: Option<Vec<(String, String)>>,
    pub sleep_left: Option<f64>,
    pub error: Option<&'a str>,
    pub up_next: Option<(String, f64)>,
    pub artist: String,
    pub busy: bool,
    pub cover: bool,
}

#[derive(Default)]
pub struct UiOut {
    pub actions: Vec<Action>,
    pub interacting: bool,
    pub hover_time: Option<f64>,
    pub animating: bool,
}

// ---------------------------------------------------------------------------
// Formatting helpers

pub fn fmt_time(t: f64) -> String {
    let t = if t.is_finite() && t > 0.0 { t } else { 0.0 } as u64;
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn fmt_left(t: f64) -> String {
    let m = (t / 60.0).round() as u64;
    if m >= 60 {
        format!("{}h {}m left", m / 60, m % 60)
    } else {
        format!("{} min left", m.max(1))
    }
}

pub fn lang_name(code: &str) -> String {
    let c = code.to_ascii_lowercase();
    let n = match c.as_str() {
        "" | "und" => "",
        "en" | "eng" => "English",
        "hi" | "hin" => "Hindi",
        "es" | "spa" => "Spanish",
        "fr" | "fre" | "fra" => "French",
        "de" | "ger" | "deu" => "German",
        "it" | "ita" => "Italian",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "zh" | "chi" | "zho" => "Chinese",
        "ru" | "rus" => "Russian",
        "pt" | "por" => "Portuguese",
        "ar" | "ara" => "Arabic",
        "ta" | "tam" => "Tamil",
        "te" | "tel" => "Telugu",
        "ml" | "mal" => "Malayalam",
        "kn" | "kan" => "Kannada",
        "bn" | "ben" => "Bengali",
        "mr" | "mar" => "Marathi",
        "gu" | "guj" => "Gujarati",
        "pa" | "pan" => "Punjabi",
        "ur" | "urd" => "Urdu",
        "tr" | "tur" => "Turkish",
        "nl" | "dut" | "nld" => "Dutch",
        "pl" | "pol" => "Polish",
        "sv" | "swe" => "Swedish",
        "no" | "nor" => "Norwegian",
        "da" | "dan" => "Danish",
        "fi" | "fin" => "Finnish",
        "el" | "gre" | "ell" => "Greek",
        "he" | "heb" => "Hebrew",
        "th" | "tha" => "Thai",
        "vi" | "vie" => "Vietnamese",
        "id" | "ind" => "Indonesian",
        "ms" | "may" | "msa" => "Malay",
        "uk" | "ukr" => "Ukrainian",
        "cs" | "cze" | "ces" => "Czech",
        "hu" | "hun" => "Hungarian",
        "ro" | "rum" | "ron" => "Romanian",
        _ => return code.to_uppercase(),
    };
    n.to_string()
}

pub fn codec_name(c: &str) -> String {
    match c {
        "h264" => "H.264",
        "hevc" => "HEVC",
        "av1" => "AV1",
        "vp9" => "VP9",
        "vp8" => "VP8",
        "mpeg4" => "MPEG-4",
        "mpeg2video" => "MPEG-2",
        "vc1" => "VC-1",
        "wmv3" => "WMV9",
        "aac" => "AAC",
        "mp3" => "MP3",
        "ac3" => "Dolby Digital",
        "eac3" => "Dolby Digital+",
        "dts" => "DTS",
        "truehd" => "TrueHD",
        "flac" => "FLAC",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "subrip" => "SRT",
        "ass" => "ASS",
        "ssa" => "SSA",
        "webvtt" => "WebVTT",
        "mov_text" => "Text",
        "hdmv_pgs_subtitle" => "PGS",
        "dvd_subtitle" => "VobSub",
        "" => "",
        other => return other.to_uppercase(),
    }
    .to_string()
}

fn speed_label(s: f64) -> String {
    let t = format!("{:.2}", s);
    let t = t.trim_end_matches('0').trim_end_matches('.');
    format!("{t}×")
}

// ---------------------------------------------------------------------------
// Icons (24x24 design grid)

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Back10,
    Fwd10,
    Volume(u8),
    Mute,
    Full,
    Unfull,
    Subs,
    Gear,
    List,
    Close,
    Min,
    Max,
    Restore,
    Back,
    Pin,
    Info,
    Check,
    Chevron,
    Audio,
    Note,
}

pub fn paint_icon(p: &Painter, icon: Icon, c: Pos2, size: f32, col: Color32) {
    let k = size / 24.0;
    let pt = |x: f32, y: f32| c + vec2((x - 12.0) * k, (y - 12.0) * k);
    let st = Stroke::new(1.9 * k, col);
    let line = |pts: &[(f32, f32)]| p.add(Shape::line(pts.iter().map(|(x, y)| pt(*x, *y)).collect(), st));
    let fill = |pts: &[(f32, f32)]| p.add(Shape::convex_polygon(pts.iter().map(|(x, y)| pt(*x, *y)).collect(), col, Stroke::NONE));
    let rrect = |x0: f32, y0: f32, x1: f32, y1: f32, r: f32| p.rect_filled(Rect::from_min_max(pt(x0, y0), pt(x1, y1)), r * k, col);
    let arc = |cx: f32, cy: f32, r: f32, a0: f32, a1: f32| {
        let n = 20;
        let pts: Vec<Pos2> = (0..=n)
            .map(|i| {
                let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                pt(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        p.add(Shape::line(pts, st));
    };
    match icon {
        Icon::Play => {
            fill(&[(7.5, 4.5), (7.5, 19.5), (19.5, 12.0)]);
        }
        Icon::Pause => {
            rrect(6.0, 4.5, 10.0, 19.5, 1.2);
            rrect(14.0, 4.5, 18.0, 19.5, 1.2);
        }
        Icon::Prev => {
            fill(&[(18.5, 5.5), (18.5, 18.5), (8.5, 12.0)]);
            rrect(5.0, 5.5, 7.4, 18.5, 1.0);
        }
        Icon::Next => {
            fill(&[(5.5, 5.5), (5.5, 18.5), (15.5, 12.0)]);
            rrect(16.6, 5.5, 19.0, 18.5, 1.0);
        }
        Icon::Back10 | Icon::Fwd10 => {
            let back = icon == Icon::Back10;
            if back {
                arc(12.0, 12.5, 8.5, -100.0, 200.0);
                fill(&[(2.6, 6.2), (8.2, 4.4), (6.6, 9.8)]);
            } else {
                arc(12.0, 12.5, 8.5, -20.0, 280.0);
                fill(&[(21.4, 6.2), (15.8, 4.4), (17.4, 9.8)]);
            }
            p.text(pt(12.0, 12.8), Align2::CENTER_CENTER, "10", font_sb(7.6 * k), col);
        }
        Icon::Volume(_) | Icon::Mute => {
            let level = if let Icon::Volume(l) = icon { l } else { 0 };
            fill(&[(3.5, 9.0), (7.5, 9.0), (12.0, 4.8), (12.0, 19.2), (7.5, 15.0), (3.5, 15.0)]);
            if icon == Icon::Mute {
                line(&[(15.5, 9.0), (21.0, 14.5)]);
                line(&[(21.0, 9.0), (15.5, 14.5)]);
            } else {
                if level >= 1 {
                    arc(12.0, 12.0, 4.3, -45.0, 45.0);
                }
                if level >= 2 {
                    arc(12.0, 12.0, 8.0, -50.0, 50.0);
                }
            }
        }
        Icon::Full => {
            line(&[(4.0, 9.0), (4.0, 4.0), (9.0, 4.0)]);
            line(&[(15.0, 4.0), (20.0, 4.0), (20.0, 9.0)]);
            line(&[(20.0, 15.0), (20.0, 20.0), (15.0, 20.0)]);
            line(&[(9.0, 20.0), (4.0, 20.0), (4.0, 15.0)]);
        }
        Icon::Unfull => {
            line(&[(9.0, 4.0), (9.0, 9.0), (4.0, 9.0)]);
            line(&[(20.0, 9.0), (15.0, 9.0), (15.0, 4.0)]);
            line(&[(15.0, 20.0), (15.0, 15.0), (20.0, 15.0)]);
            line(&[(4.0, 15.0), (9.0, 15.0), (9.0, 20.0)]);
        }
        Icon::Subs => {
            p.rect_stroke(Rect::from_min_max(pt(3.0, 5.0), pt(21.0, 19.0)), 3.0 * k, st, egui::StrokeKind::Middle);
            line(&[(6.5, 12.5), (10.5, 12.5)]);
            line(&[(13.0, 12.5), (17.5, 12.5)]);
            line(&[(6.5, 15.5), (14.5, 15.5)]);
        }
        Icon::Gear => {
            let mut pts = Vec::new();
            for i in 0..32 {
                let a = (i as f32 / 32.0) * std::f32::consts::TAU;
                let r = if (i / 2) % 2 == 0 { 9.2 } else { 7.0 };
                pts.push(pt(12.0 + r * a.cos(), 12.0 + r * a.sin()));
            }
            p.add(Shape::closed_line(pts, st));
            p.circle_stroke(pt(12.0, 12.0), 3.0 * k, st);
        }
        Icon::List => {
            line(&[(3.5, 6.0), (16.5, 6.0)]);
            line(&[(3.5, 12.0), (16.5, 12.0)]);
            line(&[(3.5, 18.0), (11.0, 18.0)]);
            fill(&[(15.0, 15.0), (15.0, 21.0), (20.5, 18.0)]);
        }
        Icon::Close => {
            line(&[(6.0, 6.0), (18.0, 18.0)]);
            line(&[(18.0, 6.0), (6.0, 18.0)]);
        }
        Icon::Min => {
            line(&[(6.0, 12.0), (18.0, 12.0)]);
        }
        Icon::Max => {
            p.rect_stroke(Rect::from_min_max(pt(6.0, 6.0), pt(18.0, 18.0)), 1.5 * k, st, egui::StrokeKind::Middle);
        }
        Icon::Restore => {
            p.rect_stroke(Rect::from_min_max(pt(5.0, 9.0), pt(15.0, 19.0)), 1.5 * k, st, egui::StrokeKind::Middle);
            line(&[(9.0, 9.0), (9.0, 5.0), (19.0, 5.0), (19.0, 15.0), (15.0, 15.0)]);
        }
        Icon::Back => {
            line(&[(15.0, 5.5), (8.5, 12.0), (15.0, 18.5)]);
        }
        Icon::Chevron => {
            line(&[(9.0, 5.5), (15.5, 12.0), (9.0, 18.5)]);
        }
        Icon::Pin => {
            p.circle_stroke(pt(12.0, 9.0), 5.0 * k, st);
            line(&[(12.0, 14.0), (12.0, 21.0)]);
        }
        Icon::Info => {
            p.circle_stroke(pt(12.0, 12.0), 9.0 * k, st);
            line(&[(12.0, 11.0), (12.0, 16.5)]);
            p.circle_filled(pt(12.0, 7.8), 1.2 * k, col);
        }
        Icon::Check => {
            line(&[(5.0, 12.5), (10.0, 17.5), (19.5, 6.5)]);
        }
        Icon::Audio => {
            line(&[(3.0, 10.0), (3.0, 14.0)]);
            line(&[(7.5, 7.0), (7.5, 17.0)]);
            line(&[(12.0, 4.0), (12.0, 20.0)]);
            line(&[(16.5, 7.5), (16.5, 16.5)]);
            line(&[(21.0, 10.0), (21.0, 14.0)]);
        }
        Icon::Note => {
            line(&[(9.0, 18.0), (9.0, 5.0), (19.0, 3.5), (19.0, 16.0)]);
            p.circle_filled(pt(6.5, 18.0), 2.8 * k, col);
            p.circle_filled(pt(16.5, 16.0), 2.8 * k, col);
        }
    }
}

/// Round icon button with hover halo. Returns the response.
fn icon_btn(ui: &mut Ui, center: Pos2, size: f32, icon: Icon, active: bool, tip: &str) -> Response {
    let rect = Rect::from_center_size(center, Vec2::splat(size + 14.0));
    let id = Id::new(("ib", tip, center.x as i32, center.y as i32));
    let r = ui.interact(rect, id, Sense::click());
    let p = ui.painter();
    let hover = ui.ctx().animate_bool_with_time(id, r.hovered(), 0.12);
    if hover > 0.0 {
        p.circle_filled(center, (size + 14.0) / 2.0, white(0.12 * hover));
    }
    if active {
        p.circle_filled(center, (size + 10.0) / 2.0, white(0.14));
    }
    let col = if active { ACCENT } else { TEXT };
    paint_icon(p, icon, center, size, col);
    if !tip.is_empty() {
        r.on_hover_text(tip)
    } else {
        r
    }
}

/// Vertical gradient quad (used for the top/bottom scrims).
fn vgrad(p: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), top);
    m.colored_vertex(rect.right_top(), top);
    m.colored_vertex(rect.left_bottom(), bottom);
    m.colored_vertex(rect.right_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 3, 2);
    p.add(Shape::mesh(m));
}

// ---------------------------------------------------------------------------
// Entry point

pub fn draw(ctx: &Context, v: &View, s: &mut UiState) -> UiOut {
    let mut out = UiOut::default();
    s.menu_changed = false;
    let screen = ctx.screen_rect();

    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
        if v.home {
            home(ui, v, s, &mut out);
        } else {
            player(ui, v, s, &mut out, screen);
        }
    });

    if s.menu != Menu::None {
        out.interacting = true;
        menus(ctx, v, s, &mut out, screen);
    }
    if s.help {
        out.interacting = true;
        help(ctx, s);
    }
    if let Some(rows) = &v.info {
        info_panel(ctx, rows, s);
    }
    if let Some((text, a)) = v.osd {
        let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("osd")));
        let galley = p.layout_no_wrap(text.to_string(), font_sb(15.0), white(a));
        let size = galley.size() + vec2(28.0, 16.0);
        let top = if v.home { 64.0 } else { 56.0 };
        let r = Rect::from_center_size(pos2(screen.center().x, top + size.y / 2.0), size);
        p.rect_filled(r, 10.0, black(0.62 * a));
        p.galley(r.center() - galley.size() / 2.0, galley, white(a));
        out.animating = true;
    }
    out
}

// ---------------------------------------------------------------------------
// Window controls (shared by home + player)

fn window_buttons(ui: &mut Ui, v: &View, s: &mut UiState, out: &mut UiOut, screen: Rect, alpha: f32) {
    if v.fullscreen || crate::app::NATIVE_CHROME {
        return;
    }
    let w = 46.0;
    let h = 34.0;
    let items = [
        (Icon::Min, Action::Minimize),
        (if v.maximized { Icon::Restore } else { Icon::Max }, Action::ToggleMaximize),
        (Icon::Close, Action::Close),
    ];
    for (i, (icon, act)) in items.into_iter().enumerate() {
        let rect = Rect::from_min_size(pos2(screen.right() - w * (3 - i) as f32, screen.top()), vec2(w, h));
        let r = ui.interact(rect, Id::new(("win", i)), Sense::click());
        if r.hovered() {
            let c = if icon == Icon::Close { Color32::from_rgb(196, 43, 28) } else { white(0.1) };
            ui.painter().rect_filled(rect, 0.0, c.gamma_multiply(alpha));
        }
        paint_icon(ui.painter(), icon, rect.center(), 15.0, white(alpha * 0.92));
        if r.hovered() {
            out.interacting = true;
        }
        if r.clicked() {
            s.close_all();
            out.actions.push(act);
        }
    }
}

// ---------------------------------------------------------------------------
// Home screen

/// Left inset of the top bar: room for the macOS traffic lights.
const CHROME_LEFT: f32 = if crate::app::NATIVE_CHROME { 76.0 } else { 0.0 };

fn cubic(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, n: usize) -> Vec<Pos2> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            (p0.to_vec2() * u * u * u + p1.to_vec2() * 3.0 * u * u * t + p2.to_vec2() * 3.0 * u * t * t + p3.to_vec2() * t * t * t).to_pos2()
        })
        .collect()
}

/// The trajectory swoosh: swells from nothing to `max_w` and back, fading in.
/// Built as one triangle strip with feathered (transparent) outer edges, so
/// it is anti-aliased without the seams overlapping stroke segments leave.
fn swoosh(p: &Painter, pts: &[Pos2], max_w: f32, color: Color32, fade_in: f32) {
    if pts.len() < 2 {
        return;
    }
    let n = (pts.len() - 1) as f32;
    let feather = 0.8;
    let mut m = Mesh::default();
    for (i, &pt) in pts.iter().enumerate() {
        let t = i as f32 / n;
        let w = 0.5 * max_w * (std::f32::consts::PI * (t * 1.15).min(1.0)).sin().max(0.0).powf(0.8);
        let dir = pts[(i + 1).min(pts.len() - 1)] - pts[i.saturating_sub(1)];
        let normal = vec2(-dir.y, dir.x).normalized();
        let c = color.gamma_multiply((t / fade_in).min(1.0) * (w / 0.6).min(1.0));
        let base = m.vertices.len() as u32;
        m.colored_vertex(pt + normal * (w + feather), Color32::TRANSPARENT);
        m.colored_vertex(pt + normal * w, c);
        m.colored_vertex(pt - normal * w, c);
        m.colored_vertex(pt - normal * (w + feather), Color32::TRANSPARENT);
        if i > 0 {
            let prev = base - 4;
            for k in 0..3 {
                m.add_triangle(prev + k, prev + k + 1, base + k);
                m.add_triangle(prev + k + 1, base + k + 1, base + k);
            }
        }
    }
    p.add(Shape::mesh(m));
}

/// App mark: black tile, earth-limb glow, play triangle, orbital swoosh.
/// Same geometry as scripts/brand.py (which renders the .ico/.png files).
pub fn logo_mark(p: &Painter, rect: Rect) {
    let s = rect.width();
    let at = |x: f32, y: f32| rect.min + vec2(x * s, y * s);
    let tile = rect.shrink(s * 0.04);
    p.rect_filled(tile, s * 0.2, Color32::from_rgb(9, 12, 18));
    // glow: a triangle fan, coloured centre fading to transparent
    let clip = p.with_clip_rect(tile.shrink(s * 0.02));
    let mut m = Mesh::default();
    let c = at(0.9, 1.05);
    m.colored_vertex(c, rgba(40, 110, 220, 110));
    for i in 0..=24u32 {
        let a = std::f32::consts::PI * (1.0 + i as f32 / 24.0);
        m.colored_vertex(c + vec2(a.cos() * s * 0.62, a.sin() * s * 0.5), Color32::TRANSPARENT);
        if i > 0 {
            m.add_triangle(0, i, i + 1);
        }
    }
    clip.add(Shape::mesh(m));
    p.rect_stroke(tile, s * 0.2, Stroke::new(1.0_f32, white(0.1)), egui::StrokeKind::Inside);
    let pts = cubic(at(0.14, 0.80), at(0.40, 0.92), at(0.80, 0.80), at(0.90, 0.22), 48);
    swoosh(p, &pts, s * 0.045, ACCENT, 0.35);
    p.add(Shape::convex_polygon(vec![at(0.385, 0.315), at(0.385, 0.685), at(0.70, 0.5)], Color32::WHITE, Stroke::NONE));
}

/// "VELΛ" wordmark: extended geometric letters, a crossbar-less Λ and the
/// trajectory swoosh launching from its foot. `origin` is the top-left of the
/// letters; the swoosh rises about 0.75 h above them.
pub fn wordmark(p: &Painter, origin: Pos2, h: f32, color: Color32, with_swoosh: bool) {
    let t = h * 0.15;
    let w = h * 1.12;
    let gap = h * 0.42;
    let at = |x: f32, y: f32| origin + vec2(x, y);
    let quad = |pts: [Pos2; 4]| {
        p.add(Shape::convex_polygon(pts.to_vec(), color, Stroke::NONE));
    };
    let bar = |x0: f32, y0: f32, x1: f32, y1: f32| {
        p.rect_filled(Rect::from_min_max(at(x0, y0), at(x1, y1)), 0.0, color);
    };
    let mut x = 0.0;
    // V
    quad([at(x, 0.0), at(x + t * 1.15, 0.0), at(x + w / 2.0 + t * 0.58, h), at(x + w / 2.0 - t * 0.58, h)]);
    quad([at(x + w - t * 1.15, 0.0), at(x + w, 0.0), at(x + w / 2.0 + t * 0.58, h), at(x + w / 2.0 - t * 0.58, h)]);
    x += w + gap;
    // E
    bar(x, 0.0, x + t, h);
    bar(x, 0.0, x + w * 0.92, t);
    bar(x, h / 2.0 - t / 2.0, x + w * 0.78, h / 2.0 + t / 2.0);
    bar(x, h - t, x + w * 0.92, h);
    x += w * 0.92 + gap;
    // L
    bar(x, 0.0, x + t, h);
    bar(x, h - t, x + w * 0.88, h);
    x += w * 0.88 + gap;
    // Λ
    quad([at(x, h), at(x + t * 1.15, h), at(x + w / 2.0 + t * 0.58, 0.0), at(x + w / 2.0 - t * 0.58, 0.0)]);
    quad([at(x + w - t * 1.15, h), at(x + w, h), at(x + w / 2.0 + t * 0.58, 0.0), at(x + w / 2.0 - t * 0.58, 0.0)]);
    if with_swoosh {
        let mut pts = cubic(at(x + w - t * 0.3, h), at(x + w + h * 0.95, h * 0.2), at(x + w + h * 0.4, -h * 0.75), at(h * 0.7, -h * 0.42), 96);
        pts.reverse();
        swoosh(p, &pts, t * 0.62, color, 0.6);
    }
}

/// Width of the letters (the swoosh overhangs ~0.95 h to the right).
pub fn wordmark_width(h: f32) -> f32 {
    h * (1.12 * 2.0 + 0.92 + 0.88 + 0.42 * 3.0)
}

/// SpaceX-style tracked capitals.
fn tracked(p: &Painter, pos: Pos2, align: Align2, text: &str, font: FontId, color: Color32, spacing: f32) -> Rect {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    let galley = p.layout_job(job);
    let rect = align.anchor_size(pos, galley.size());
    p.galley(rect.min, galley, color);
    rect
}

/// Hairline rectangular button with tracked caps; fills white on hover.
fn launch_button(ui: &mut Ui, rect: Rect, label: &str, primary: bool) -> Response {
    let r = ui.interact(rect, Id::new(("btn", label)), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.14);
    let fill = (if primary { 0.1 } else { 0.0 }) + hover * 0.9;
    let p = ui.painter();
    p.rect_filled(rect, 2.0, white(fill));
    p.rect_stroke(rect, 2.0, Stroke::new(1.0_f32, white(if primary { 0.9 } else { 0.42 })), egui::StrokeKind::Inside);
    let col = if hover > 0.5 { INK } else { TEXT };
    tracked(p, rect.center(), Align2::CENTER_CENTER, label, font_sb(12.0), col, 2.2);
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r
}

fn section_label(p: &Painter, pos: Pos2, text: &str) {
    tracked(p, pos, Align2::LEFT_CENTER, text, font_sb(11.5), DIM, 2.6);
}

/// Shortcut hints use the platform's primary modifier name.
const MOD: &str = if cfg!(target_os = "macos") { "⌘" } else { "CTRL+" };

fn home(ui: &mut Ui, v: &View, s: &mut UiState, out: &mut UiOut) {
    let screen = ui.max_rect();
    let p = ui.painter().clone();
    p.rect_filled(screen, 0.0, BG);
    // earth-limb glow rising from the bottom edge
    vgrad(&p, Rect::from_min_max(pos2(screen.left(), screen.bottom() - 280.0), screen.right_bottom()), Color32::TRANSPARENT, rgba(30, 90, 190, 36));

    // whole-window background: drag to move, double-click the top strip to maximise
    let bg = ui.interact(screen, Id::new("home-bg"), Sense::click_and_drag());
    if bg.drag_started() && !v.maximized && !v.fullscreen {
        out.actions.push(Action::StartDrag);
    }
    if bg.double_clicked() && bg.interact_pointer_pos().map_or(false, |p| p.y < screen.top() + 48.0) {
        out.actions.push(Action::ToggleMaximize);
    }

    // title bar: mark + small wordmark
    let left = screen.left() + 18.0 + CHROME_LEFT;
    logo_mark(&p, Rect::from_min_size(pos2(left, screen.top() + 9.0), Vec2::splat(24.0)));
    wordmark(&p, pos2(left + 36.0, screen.top() + 16.0), 10.0, TEXT, false);
    window_buttons(ui, v, s, out, screen, 1.0);

    let continue_list: Vec<&HistoryEntry> = v.history.iter().filter(|h| h.resumable() && h.video).take(8).collect();
    let recent: Vec<&HistoryEntry> = v.history.iter().filter(|h| !continue_list.iter().any(|c| c.path == h.path)).take(10).collect();
    let open = [("OPEN FILE", true, Action::OpenFiles), ("OPEN FOLDER", false, Action::OpenFolder), ("OPEN URL", false, Action::OpenUrl(String::new()))];
    let launch = |ui: &mut Ui, rect: Rect, label: &str, primary: bool, act: Action, s: &mut UiState, out: &mut UiOut| {
        if launch_button(ui, rect, label, primary).clicked() {
            match act {
                Action::OpenUrl(_) => s.open(Menu::Url),
                a => out.actions.push(a),
            }
        }
    };

    let cx = screen.center().x;
    if continue_list.is_empty() && recent.is_empty() {
        // Hero: the wordmark, a tagline and three launch buttons.
        let h = (screen.width() / 12.0).clamp(30.0, 78.0);
        let top = screen.center().y - h * 1.25;
        wordmark(&p, pos2(cx - wordmark_width(h) / 2.0, top), h, TEXT, true);
        tracked(&p, pos2(cx, top + h + 30.0), Align2::CENTER_TOP, "PLAY ANYTHING. BEAUTIFULLY.", font(12.5), DIM, 4.5);
        let (bw, bh, gap) = (176.0_f32.min((screen.width() - 80.0) / 3.0), 44.0, 14.0);
        let total = bw * 3.0 + gap * 2.0;
        let y = top + h + 84.0;
        for (i, (label, primary, act)) in open.into_iter().enumerate() {
            let rect = Rect::from_min_size(pos2(cx - total / 2.0 + i as f32 * (bw + gap), y), vec2(bw, bh));
            launch(ui, rect, label, primary, act, s, out);
        }
        let hint = format!("DROP A FILE ANYWHERE  ·  {MOD}O OPEN  ·  {MOD}V PASTE A LINK  ·  ? SHORTCUTS");
        tracked(&p, pos2(cx, screen.bottom() - 34.0), Align2::CENTER_CENTER, &hint, font(10.5), white(0.32), 1.8);
        return;
    }

    // ---- Library: continue watching + recent ----
    let margin = 40.0;
    let mut y = screen.top() + 72.0;
    tracked(&p, pos2(screen.left() + margin, y + 18.0), Align2::LEFT_CENTER, "WELCOME BACK", font_sb(20.0), TEXT, 3.5);
    let mut x = screen.right() - margin;
    for (label, primary, act) in open.into_iter().rev() {
        let w = 136.0;
        x -= w;
        launch(ui, Rect::from_min_size(pos2(x, y), vec2(w, 36.0)), label, primary, act, s, out);
        x -= 10.0;
    }
    y += 76.0;

    if !continue_list.is_empty() {
        section_label(&p, pos2(screen.left() + margin, y), "CONTINUE WATCHING");
        y += 18.0;
        let avail = screen.width() - margin * 2.0;
        let cols = ((avail + 16.0) / (232.0 + 16.0)).floor().max(1.0) as usize;
        let cw = ((avail + 16.0) / cols as f32 - 16.0).min(300.0);
        let ch = cw * 9.0 / 16.0;
        for (i, h) in continue_list.iter().take(cols).enumerate() {
            let rect = Rect::from_min_size(pos2(screen.left() + margin + i as f32 * (cw + 16.0), y), vec2(cw, ch));
            let r = ui.interact(rect.expand2(vec2(0.0, 40.0)), Id::new(("card", &h.path)), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.15);
            let grow = rect.expand(hover * 3.0);
            p.rect_filled(grow, 6.0, white(0.06));
            if let Some(tex) = v.posters.get(&h.path) {
                p.add(Shape::Rect(
                    egui::epaint::RectShape::filled(grow, 6.0, Color32::WHITE).with_texture(*tex, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                ));
            } else {
                paint_icon(&p, Icon::Play, grow.center(), 26.0, white(0.25));
            }
            vgrad(&p, Rect::from_min_max(pos2(grow.left(), grow.bottom() - 40.0), grow.right_bottom()), Color32::TRANSPARENT, black(0.55));
            // progress
            let bar = Rect::from_min_size(pos2(grow.left() + 10.0, grow.bottom() - 10.0), vec2(grow.width() - 20.0, 3.0));
            p.rect_filled(bar, 2.0, white(0.25));
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * h.progress() as f32, 3.0)), 2.0, ACCENT);
            if hover > 0.0 {
                p.circle_filled(grow.center(), 24.0, black(0.5 * hover));
                paint_icon(&p, Icon::Play, grow.center() + vec2(1.5, 0.0), 22.0, white(hover));
            }
            let title = crate::player::display_name(&h.path);
            let title = if h.title.is_empty() || h.title.len() > 90 { title } else { h.title.clone() };
            p.with_clip_rect(Rect::from_min_size(pos2(rect.left(), rect.bottom()), vec2(cw, 40.0))).text(
                pos2(rect.left() + 2.0, rect.bottom() + 15.0),
                Align2::LEFT_CENTER,
                &title,
                font_sb(13.5),
                TEXT,
            );
            p.text(pos2(rect.left() + 2.0, rect.bottom() + 33.0), Align2::LEFT_CENTER, fmt_left(h.duration - h.position), font(12.0), DIM);
            if r.clicked() {
                out.actions.push(Action::OpenPaths(vec![h.path.clone()]));
            }
            if r.secondary_clicked() {
                out.actions.push(Action::ForgetHistory(h.path.clone()));
            }
            if r.hovered() {
                out.interacting = true;
            }
        }
        y += ch + 64.0;
    }

    if !recent.is_empty() {
        section_label(&p, pos2(screen.left() + margin, y), "RECENTLY PLAYED");
        let clear = Rect::from_min_size(pos2(screen.right() - margin - 70.0, y - 12.0), vec2(70.0, 24.0));
        let cr = ui.interact(clear, Id::new("clear-hist"), Sense::click());
        tracked(&p, clear.right_center(), Align2::RIGHT_CENTER, "CLEAR", font_sb(11.0), if cr.hovered() { TEXT } else { DIM }, 2.0);
        if cr.clicked() {
            out.actions.push(Action::ClearHistory);
        }
        y += 16.0;
        let row_h = 44.0;
        for (i, h) in recent.iter().enumerate() {
            if y + row_h > screen.bottom() - 12.0 {
                break;
            }
            let rect = Rect::from_min_size(pos2(screen.left() + margin - 10.0, y), vec2(screen.width() - margin * 2.0 + 20.0, row_h));
            let r = ui.interact(rect, Id::new(("recent", i)), Sense::click());
            if r.hovered() {
                p.rect_filled(rect, 4.0, white(0.05));
                out.interacting = true;
            }
            paint_icon(&p, if h.video { Icon::Play } else { Icon::Note }, pos2(rect.left() + 26.0, rect.center().y), 16.0, white(0.6));
            let name = crate::player::display_name(&h.path);
            p.with_clip_rect(rect.shrink2(vec2(140.0, 0.0)).translate(vec2(-100.0, 0.0)).intersect(rect)).text(
                pos2(rect.left() + 50.0, rect.center().y - 8.0),
                Align2::LEFT_CENTER,
                &name,
                font(14.0),
                TEXT,
            );
            let folder = std::path::Path::new(&h.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            p.with_clip_rect(rect.shrink2(vec2(140.0, 0.0)).translate(vec2(-100.0, 0.0)).intersect(rect)).text(
                pos2(rect.left() + 50.0, rect.center().y + 10.0),
                Align2::LEFT_CENTER,
                &folder,
                font(11.5),
                white(0.4),
            );
            let right = if h.progress() > 0.95 { "Watched".to_string() } else { fmt_time(h.duration) };
            p.text(pos2(rect.right() - 16.0, rect.center().y), Align2::RIGHT_CENTER, right, font(12.5), DIM);
            if r.clicked() {
                out.actions.push(Action::OpenPaths(vec![h.path.clone()]));
            }
            if r.secondary_clicked() {
                out.actions.push(Action::ForgetHistory(h.path.clone()));
            }
            y += row_h;
        }
    }
}

// ---------------------------------------------------------------------------
// Player overlay

fn player(ui: &mut Ui, v: &View, s: &mut UiState, out: &mut UiOut, screen: Rect) {
    let st = v.st;
    let a = v.controls;
    let p = ui.painter().clone();

    // Audio-only stage
    if !st.has_video() && !v.cover {
        p.rect_filled(screen, 0.0, BG);
        vgrad(&p, screen, rgba(233, 54, 92, 18), Color32::TRANSPARENT);
        let c = screen.center() - vec2(0.0, 30.0);
        let sz = (screen.height() * 0.28).clamp(90.0, 220.0);
        let r = Rect::from_center_size(c, Vec2::splat(sz));
        p.rect_filled(r, 24.0, white(0.05));
        paint_icon(&p, Icon::Note, r.center(), sz * 0.42, white(0.7));
        p.text(pos2(c.x, r.bottom() + 30.0), Align2::CENTER_CENTER, st.display_title(), font_sb(20.0), TEXT);
        if !v.artist.is_empty() {
            p.text(pos2(c.x, r.bottom() + 56.0), Align2::CENTER_CENTER, &v.artist, font(14.0), DIM);
        }
    }

    // Background interaction: click = play/pause, double-click = fullscreen, drag = move window
    let bg = ui.interact(screen, Id::new("video-bg"), Sense::click_and_drag());
    if bg.clicked() && !s.close_all() {
        out.actions.push(Action::TogglePause);
    }
    if bg.double_clicked() {
        out.actions.push(Action::ToggleFullscreen);
    }
    if bg.drag_started() && !v.fullscreen && !v.maximized {
        out.actions.push(Action::StartDrag);
    }
    if bg.secondary_clicked() {
        s.ctx_pos = bg.interact_pointer_pos().unwrap_or(screen.center());
        s.menu = Menu::Context;
        s.menu_changed = true;
    }
    if bg.hovered() && s.menu == Menu::None {
        let dy = ui.ctx().input(|i| i.raw_scroll_delta.y);
        if dy != 0.0 {
            out.actions.push(Action::VolumeBy(if dy > 0.0 { 5.0 } else { -5.0 }));
        }
    }

    // Center spinner / error / up-next
    if v.busy {
        let t = ui.ctx().input(|i| i.time) as f32;
        let c = screen.center();
        let pts: Vec<Pos2> = (0..=24)
            .map(|i| {
                let ang = t * 5.0 + i as f32 / 24.0 * 4.4;
                c + vec2(ang.cos(), ang.sin()) * 22.0
            })
            .collect();
        p.add(Shape::line(pts, Stroke::new(3.0_f32, white(0.85))));
        out.animating = true;
    }
    if let Some(err) = v.error {
        let r = Rect::from_center_size(screen.center(), vec2(440.0_f32.min(screen.width() - 40.0), 110.0));
        p.rect_filled(r, 14.0, PANEL);
        p.text(r.center() - vec2(0.0, 20.0), Align2::CENTER_CENTER, "Can't play this file", font_sb(16.0), TEXT);
        let g = p.layout(err.to_string(), font(12.5), DIM, r.width() - 40.0);
        p.galley(pos2(r.center().x - g.size().x / 2.0, r.center().y + 2.0), g, DIM);
    }
    if let Some((name, left)) = &v.up_next {
        let r = Rect::from_min_size(pos2(screen.right() - 340.0, screen.bottom() - 100.0 - 92.0 * a), vec2(316.0, 76.0));
        p.rect_filled(r, 14.0, PANEL);
        p.text(r.left_top() + vec2(16.0, 18.0), Align2::LEFT_CENTER, format!("Up next in {}", left.ceil() as i64), font(12.0), DIM);
        p.with_clip_rect(r.shrink(8.0)).text(r.left_top() + vec2(16.0, 38.0), Align2::LEFT_CENTER, name, font_sb(14.0), TEXT);
        let br = Rect::from_min_size(r.left_bottom() + vec2(16.0, -26.0), vec2(90.0, 20.0));
        let resp = ui.interact(br.expand(4.0), Id::new("upnext-play"), Sense::click());
        p.text(br.left_center(), Align2::LEFT_CENTER, "Play now  ›", font_sb(12.5), if resp.hovered() { ACCENT } else { TEXT });
        if resp.clicked() {
            out.actions.push(Action::Next);
        }
    }

    if a <= 0.01 {
        return;
    }

    // ---- Top bar ----
    vgrad(&p, Rect::from_min_max(screen.left_top(), pos2(screen.right(), screen.top() + 110.0)), black(0.62 * a), Color32::TRANSPARENT);
    let top = Rect::from_min_max(screen.left_top(), pos2(screen.right(), screen.top() + 48.0));
    let top_r = ui.interact(top, Id::new("topbar"), Sense::click_and_drag());
    if top_r.double_clicked() && !v.fullscreen {
        out.actions.push(Action::ToggleMaximize);
    }
    if top_r.drag_started() && !v.fullscreen && !v.maximized {
        out.actions.push(Action::StartDrag);
    }
    if top_r.hovered() {
        out.interacting = true;
    }
    let mut pp = p.clone();
    pp.set_opacity(a);
    if icon_btn(ui, pos2(screen.left() + 26.0 + CHROME_LEFT, screen.top() + 24.0), 20.0, Icon::Back, false, "Back to home").clicked() {
        out.actions.push(Action::Stop);
    }
    let title_clip = Rect::from_min_max(pos2(screen.left() + 50.0 + CHROME_LEFT, screen.top()), pos2(screen.right() - 240.0, screen.top() + 48.0));
    pp.with_clip_rect(title_clip).text(pos2(screen.left() + 52.0 + CHROME_LEFT, screen.top() + 24.0), Align2::LEFT_CENTER, st.display_title(), font_sb(15.0), TEXT);
    let right_x = screen.right() - if v.fullscreen || crate::app::NATIVE_CHROME { 30.0 } else { 168.0 };
    if icon_btn(ui, pos2(right_x, screen.top() + 24.0), 18.0, Icon::Info, s.info, "Stats (I)").clicked() {
        s.info = !s.info;
    }
    if icon_btn(ui, pos2(right_x - 40.0, screen.top() + 24.0), 18.0, Icon::Pin, v.on_top, "Always on top (T)").clicked() {
        out.actions.push(Action::ToggleOnTop);
    }
    window_buttons(ui, v, s, out, screen, a);

    // ---- Bottom bar ----
    vgrad(&p, Rect::from_min_max(pos2(screen.left(), screen.bottom() - 150.0), screen.right_bottom()), Color32::TRANSPARENT, black(0.75 * a));
    let bottom_zone = Rect::from_min_max(pos2(screen.left(), screen.bottom() - 96.0), screen.right_bottom());
    if ui.rect_contains_pointer(bottom_zone) {
        out.interacting = true;
    }
    seekbar(ui, v, s, out, Rect::from_min_max(pos2(screen.left() + 22.0, screen.bottom() - 82.0), pos2(screen.right() - 22.0, screen.bottom() - 62.0)), a);

    let cy = screen.bottom() - 30.0;
    let mut x = screen.left() + 34.0;
    let playing = !st.pause && !st.eof;
    if icon_btn(ui, pos2(x, cy), 24.0, if playing { Icon::Pause } else { Icon::Play }, false, if playing { "Pause (Space)" } else { "Play (Space)" }).clicked() {
        out.actions.push(Action::TogglePause);
    }
    x += 46.0;
    let multi = st.playlist.len() > 1;
    if multi {
        if icon_btn(ui, pos2(x, cy), 20.0, Icon::Prev, false, "Previous (P)").clicked() {
            out.actions.push(Action::Prev);
        }
        x += 40.0;
        if icon_btn(ui, pos2(x, cy), 20.0, Icon::Next, false, "Next (N)").clicked() {
            out.actions.push(Action::Next);
        }
        x += 40.0;
    }
    if icon_btn(ui, pos2(x, cy), 21.0, Icon::Back10, false, "Back 10s (J)").clicked() {
        out.actions.push(Action::SeekBy(-10.0));
    }
    x += 40.0;
    if icon_btn(ui, pos2(x, cy), 21.0, Icon::Fwd10, false, "Forward 10s (L)").clicked() {
        out.actions.push(Action::SeekBy(10.0));
    }
    x += 42.0;

    // volume
    let vol = if st.mute { 0.0 } else { st.volume };
    let vicon = if vol <= 0.0 { Icon::Mute } else if vol < 50.0 { Icon::Volume(1) } else { Icon::Volume(2) };
    if icon_btn(ui, pos2(x, cy), 20.0, vicon, false, "Mute (M)").clicked() {
        out.actions.push(Action::ToggleMute);
    }
    x += 22.0;
    let vrect = Rect::from_min_size(pos2(x, cy - 10.0), vec2(96.0, 20.0));
    let vr = ui.interact(vrect, Id::new("vol"), Sense::click_and_drag());
    if vr.dragged() || vr.clicked() || vr.drag_started() {
        if let Some(pos) = vr.interact_pointer_pos() {
            let nv = ((pos.x - vrect.left()) / vrect.width()).clamp(0.0, 1.0) as f64 * 150.0;
            out.actions.push(Action::SetVolume((nv / 5.0).round() * 5.0));
        }
        s.vol_drag = true;
    }
    if vr.drag_stopped() {
        s.vol_drag = false;
    }
    if vr.hovered() {
        let dy = ui.ctx().input(|i| i.raw_scroll_delta.y);
        if dy != 0.0 {
            out.actions.push(Action::VolumeBy(if dy > 0.0 { 5.0 } else { -5.0 }));
        }
    }
    let track = Rect::from_center_size(vrect.center(), vec2(vrect.width(), 4.0));
    pp.rect_filled(track, 2.0, white(0.25));
    let frac = (vol / 150.0).clamp(0.0, 1.0) as f32;
    let fill_col = if vol > 100.0 { Color32::from_rgb(255, 170, 60) } else { TEXT };
    pp.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * frac, 4.0)), 2.0, fill_col);
    pp.rect_filled(Rect::from_center_size(pos2(track.left() + track.width() * (100.0 / 150.0), track.center().y), vec2(1.5, 8.0)), 0.0, white(0.35));
    if vr.hovered() || s.vol_drag {
        pp.circle_filled(pos2(track.left() + track.width() * frac, track.center().y), 6.0, fill_col);
        out.interacting = true;
    }
    x += 108.0;
    pp.text(pos2(x, cy), Align2::LEFT_CENTER, format!("{}%", vol.round() as i64), font(12.0), DIM);
    x += 44.0;

    // time
    let t = s.seek_drag.unwrap_or(v.time);
    let tl = if s.remaining && st.duration > 0.0 { format!("−{}", fmt_time(st.duration - t)) } else { fmt_time(t) };
    let label = if st.duration > 0.0 { format!("{tl}  /  {}", fmt_time(st.duration)) } else { tl };
    let tr = pp.text(pos2(x, cy), Align2::LEFT_CENTER, &label, font(13.5), TEXT);
    if ui.interact(tr.expand(4.0), Id::new("time"), Sense::click()).clicked() {
        s.remaining = !s.remaining;
    }
    let mut left_end = tr.right() + 16.0;
    if st.chapters.len() > 1 && st.chapter >= 0 {
        if let Some(ch) = st.chapters.get(st.chapter as usize) {
            let clip = Rect::from_min_max(pos2(left_end, cy - 10.0), pos2(screen.right() - 290.0, cy + 10.0));
            if clip.width() > 40.0 {
                pp.with_clip_rect(clip).text(pos2(left_end, cy), Align2::LEFT_CENTER, format!("·  {}", ch.title), font(13.0), DIM);
            }
            left_end += 0.0;
        }
    }

    // right cluster
    let mut rx = screen.right() - 34.0;
    if icon_btn(ui, pos2(rx, cy), 21.0, if v.fullscreen { Icon::Unfull } else { Icon::Full }, false, "Fullscreen (F)").clicked() {
        out.actions.push(Action::ToggleFullscreen);
    }
    rx -= 44.0;
    if icon_btn(ui, pos2(rx, cy), 21.0, Icon::Gear, s.menu == Menu::Settings, "Settings").clicked() {
        s.open(Menu::Settings);
    }
    rx -= 44.0;
    if icon_btn(ui, pos2(rx, cy), 22.0, Icon::Subs, st.sid.is_some() && st.sub_visible, "Subtitles (C)").clicked() {
        s.open(Menu::Subs);
    }
    rx -= 44.0;
    if st.tracks_of("audio").count() > 1 {
        if icon_btn(ui, pos2(rx, cy), 20.0, Icon::Audio, s.menu == Menu::Audio, "Audio track (B)").clicked() {
            s.open(Menu::Audio);
        }
        rx -= 44.0;
    }
    if multi {
        if icon_btn(ui, pos2(rx, cy), 21.0, Icon::List, s.menu == Menu::Playlist, "Playlist").clicked() {
            s.open(Menu::Playlist);
        }
        rx -= 44.0;
    }
    // speed chip
    let sp = speed_label(st.speed);
    let sw = 46.0;
    let srect = Rect::from_center_size(pos2(rx - 2.0, cy), vec2(sw, 26.0));
    let sr = ui.interact(srect, Id::new("speed"), Sense::click());
    if sr.hovered() || (st.speed - 1.0).abs() > 0.001 {
        pp.rect_filled(srect, 13.0, if (st.speed - 1.0).abs() > 0.001 { ACCENT } else { white(0.12) });
    }
    pp.text(srect.center(), Align2::CENTER_CENTER, sp, font_sb(12.5), if (st.speed - 1.0).abs() > 0.001 { INK } else { TEXT });
    if sr.clicked() {
        s.open(Menu::Speed);
    }
    let _ = left_end;
}

fn seekbar(ui: &mut Ui, v: &View, s: &mut UiState, out: &mut UiOut, rect: Rect, a: f32) {
    let st = v.st;
    let dur = st.duration;
    let r = ui.interact(rect, Id::new("seek"), Sense::click_and_drag());
    let hover_amt = ui.ctx().animate_bool_with_time(r.id, r.hovered() || s.seek_drag.is_some(), 0.12);
    let p = ui.painter();
    let thick = 3.5 + 2.5 * hover_amt;
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), thick));
    let x_of = |t: f64| track.left() + track.width() * if dur > 0.0 { (t / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let t_of = |x: f32| ((x - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * dur;

    if dur > 0.0 && (r.drag_started() || r.dragged() || r.clicked()) {
        if let Some(pos) = r.interact_pointer_pos() {
            let t = t_of(pos.x);
            if s.seek_drag.map_or(true, |old| (old - t).abs() > 0.05) {
                out.actions.push(Action::SeekTo(t, false));
            }
            s.seek_drag = Some(t);
        }
    }
    if r.drag_stopped() || (r.clicked() && !r.dragged()) {
        if let Some(t) = s.seek_drag.take() {
            out.actions.push(Action::SeekTo(t, true));
        }
    }

    p.rect_filled(track, thick / 2.0, white(0.22 * a));
    if let Some((c0, c1)) = v.cache {
        let (x0, x1) = (x_of(c0), x_of(c1));
        if x1 > x0 {
            p.rect_filled(Rect::from_min_max(pos2(x0, track.top()), pos2(x1, track.bottom())), thick / 2.0, white(0.25 * a));
        }
    }
    let shown = s.seek_drag.unwrap_or(v.time);
    p.rect_filled(Rect::from_min_max(track.min, pos2(x_of(shown), track.bottom())), thick / 2.0, ACCENT.gamma_multiply(a));
    // A-B loop region
    if let Some(av) = st.ab_a {
        let xa = x_of(av);
        let xb = st.ab_b.map(x_of).unwrap_or(xa + 2.0);
        p.rect_filled(Rect::from_min_max(pos2(xa, track.top() - 3.0), pos2(xb, track.bottom() + 3.0)), 2.0, rgba(255, 200, 80, (90.0 * a) as u8));
    }
    // chapter gaps
    if dur > 0.0 && st.chapters.len() > 1 {
        for c in st.chapters.iter().skip(1) {
            let x = x_of(c.time);
            p.rect_filled(Rect::from_center_size(pos2(x, track.center().y), vec2(2.0, thick + 1.0)), 0.0, black(0.75 * a));
        }
    }
    if hover_amt > 0.0 || s.seek_drag.is_some() {
        p.circle_filled(pos2(x_of(shown), track.center().y), 4.0 + 3.0 * hover_amt, ACCENT.gamma_multiply(a));
    }

    // hover preview: thumbnail + time + chapter
    let hover_pos = if s.seek_drag.is_some() { Some(pos2(x_of(shown), rect.center().y)) } else { r.hover_pos() };
    if let (Some(hp), true) = (hover_pos, dur > 0.0) {
        out.interacting = true;
        let t = t_of(hp.x);
        out.hover_time = Some(t);
        let chap = st.chapters.iter().rev().find(|c| c.time <= t).filter(|_| st.chapters.len() > 1).map(|c| c.title.clone());
        let tp = ui.ctx().layer_painter(LayerId::new(Order::Tooltip, Id::new("seek-tip")));
        let mut y = rect.top() - 10.0;
        let (tw, th) = match &v.hover_thumb {
            Some((_, size, _)) => (size.x, size.y),
            None if st.has_video() => (192.0, 108.0),
            None => (0.0, 0.0),
        };
        let cx = hp.x.clamp(rect.left() + tw / 2.0 + 2.0, rect.right() - tw / 2.0 - 2.0);
        // time label
        let label = fmt_time(t);
        let g = tp.layout_no_wrap(label, font_sb(13.0), TEXT);
        let lr = Rect::from_center_size(pos2(cx, y - 11.0), g.size() + vec2(14.0, 6.0));
        tp.rect_filled(lr, 6.0, black(0.75));
        tp.galley(lr.center() - g.size() / 2.0, g, TEXT);
        y = lr.top() - 6.0;
        if let Some(ch) = chap {
            let g = tp.layout_no_wrap(ch, font(12.0), TEXT);
            let cr = Rect::from_center_size(pos2(cx, y - 9.0), g.size() + vec2(12.0, 4.0));
            tp.galley(cr.center() - g.size() / 2.0, g, TEXT);
            y = cr.top() - 4.0;
        }
        if tw > 0.0 {
            let trect = Rect::from_min_max(pos2(cx - tw / 2.0, y - th), pos2(cx + tw / 2.0, y));
            tp.rect_filled(trect.expand(2.0), 8.0, black(0.8));
            if let Some((tex, _, _)) = v.hover_thumb {
                tp.add(Shape::Rect(
                    egui::epaint::RectShape::filled(trect, 6.0, Color32::WHITE).with_texture(tex, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Menus

fn row(ui: &mut Ui, label: &str, value: Option<&str>, checked: Option<bool>, chevron: bool) -> Response {
    let w = ui.available_width().max(220.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let p = ui.painter();
    if r.hovered() {
        p.rect_filled(rect, 8.0, white(0.08));
    }
    let mut x = rect.left() + 12.0;
    if let Some(c) = checked {
        if c {
            paint_icon(p, Icon::Check, pos2(x + 7.0, rect.center().y), 15.0, ACCENT);
        }
        x += 24.0;
    }
    let right_pad = if chevron { 30.0 } else { 12.0 };
    let value_w = value.map(|v| p.layout_no_wrap(v.to_string(), font(13.0), DIM).size().x).unwrap_or(0.0);
    let label_clip = Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right() - right_pad - value_w - 8.0, rect.bottom()));
    p.with_clip_rect(label_clip).text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font(14.0), TEXT);
    if let Some(vv) = value {
        p.text(pos2(rect.right() - right_pad, rect.center().y), Align2::RIGHT_CENTER, vv, font(13.0), DIM);
    }
    if chevron {
        paint_icon(p, Icon::Chevron, pos2(rect.right() - 16.0, rect.center().y), 14.0, DIM);
    }
    r
}

fn row2(ui: &mut Ui, label: &str, detail: &str, checked: bool) -> Response {
    let w = ui.available_width().max(220.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, if detail.is_empty() { 34.0 } else { 44.0 }), Sense::click());
    let p = ui.painter();
    if r.hovered() {
        p.rect_filled(rect, 8.0, white(0.08));
    }
    if checked {
        paint_icon(p, Icon::Check, pos2(rect.left() + 19.0, rect.center().y), 15.0, ACCENT);
    }
    let clip = Rect::from_min_max(pos2(rect.left() + 36.0, rect.top()), pos2(rect.right() - 8.0, rect.bottom()));
    if detail.is_empty() {
        p.with_clip_rect(clip).text(pos2(rect.left() + 36.0, rect.center().y), Align2::LEFT_CENTER, label, font(14.0), TEXT);
    } else {
        p.with_clip_rect(clip).text(pos2(rect.left() + 36.0, rect.center().y - 8.0), Align2::LEFT_CENTER, label, font(14.0), TEXT);
        p.with_clip_rect(clip).text(pos2(rect.left() + 36.0, rect.center().y + 10.0), Align2::LEFT_CENTER, detail, font(11.5), DIM);
    }
    r
}

fn toggle(ui: &mut Ui, label: &str, on: bool) -> Response {
    let w = ui.available_width().max(220.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
    let p = ui.painter();
    if r.hovered() {
        p.rect_filled(rect, 8.0, white(0.08));
    }
    p.text(pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, label, font(14.0), TEXT);
    let t = ui.ctx().animate_bool_with_time(r.id, on, 0.15);
    let sw = Rect::from_center_size(pos2(rect.right() - 30.0, rect.center().y), vec2(34.0, 20.0));
    p.rect_filled(sw, 10.0, if on { ACCENT } else { white(0.2) });
    p.circle_filled(pos2(sw.left() + 10.0 + 14.0 * t, sw.center().y), 7.0, if on { INK } else { Color32::WHITE });
    r
}

fn header(ui: &mut Ui, title: &str, s: &mut UiState, back_to: Option<Page>) {
    let w = ui.available_width().max(220.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
    let p = ui.painter();
    if back_to.is_some() {
        if r.hovered() {
            p.rect_filled(rect, 8.0, white(0.06));
        }
        paint_icon(p, Icon::Back, pos2(rect.left() + 16.0, rect.center().y), 15.0, TEXT);
    }
    p.text(pos2(rect.left() + if back_to.is_some() { 32.0 } else { 12.0 }, rect.center().y), Align2::LEFT_CENTER, title, font_sb(14.5), TEXT);
    if let Some(pg) = back_to {
        if r.clicked() {
            s.page = pg;
        }
    }
    ui.add_space(2.0);
    let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().rect_filled(sep, 0.0, LINE);
    ui.add_space(4.0);
}

fn slider_row(ui: &mut Ui, label: &str, value: f64, range: std::ops::RangeInclusive<f64>, suffix: &str, step: f64) -> Option<f64> {
    let mut v = value;
    let mut changed = None;
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        ui.allocate_ui(vec2(92.0, 26.0), |ui| ui.label(egui::RichText::new(label).size(13.5)));
        let resp = ui.add(egui::Slider::new(&mut v, range).step_by(step).show_value(false).trailing_fill(true));
        let txt = if suffix == "%" { format!("{:.0}%", v) } else if step < 1.0 { format!("{v:+.1}{suffix}") } else { format!("{v:.0}{suffix}") };
        ui.label(egui::RichText::new(txt).size(12.5).color(DIM));
        if resp.changed() {
            changed = Some(v);
        }
        if resp.double_clicked() {
            changed = Some(f64::NAN);
        }
    });
    changed
}

fn menus(ctx: &Context, v: &View, s: &mut UiState, out: &mut UiOut, screen: Rect) {
    let st = v.st;
    let frame = egui::Frame::new().fill(PANEL).corner_radius(14).inner_margin(8).shadow(Shadow { offset: [0, 10], blur: 30, spread: 0, color: black(0.55) });

    if s.menu == Menu::Url {
        let resp = egui::Area::new(Id::new("url-dlg"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                frame.inner_margin(18).show(ui, |ui| {
                    ui.set_width(440.0_f32.min(screen.width() - 60.0));
                    ui.label(egui::RichText::new("Open a network stream").font(font_sb(16.0)));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Direct video links, HLS/DASH streams, or YouTube (with yt-dlp installed)").size(12.5).color(DIM));
                    ui.add_space(10.0);
                    let te = ui.add(
                        egui::TextEdit::singleline(&mut s.url_text)
                            .hint_text("https://…")
                            .desired_width(f32::INFINITY)
                            .margin(vec2(10.0, 8.0))
                            .font(font(14.0)),
                    );
                    if s.url_focus {
                        te.request_focus();
                        s.url_focus = false;
                    }
                    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        let play = ui.add(egui::Button::new(egui::RichText::new("PLAY").font(font_sb(13.0)).color(INK)).fill(ACCENT).corner_radius(2));
                        let cancel = ui.add(egui::Button::new(egui::RichText::new("CANCEL").font(font_sb(13.0))).corner_radius(2));
                        if (play.clicked() || enter) && !s.url_text.trim().is_empty() {
                            out.actions.push(Action::OpenUrl(s.url_text.trim().to_string()));
                            s.url_text.clear();
                            s.menu = Menu::None;
                        }
                        if cancel.clicked() {
                            s.menu = Menu::None;
                        }
                    });
                });
            });
        if resp.response.clicked_elsewhere() && !s.menu_changed {
            s.menu = Menu::None;
        }
        return;
    }

    if s.menu == Menu::Context {
        let resp = egui::Area::new(Id::new("ctx-menu")).order(Order::Foreground).fixed_pos(s.ctx_pos).constrain(true).show(ctx, |ui| {
            frame.inner_margin(6).show(ui, |ui| {
                ui.set_width(230.0);
                let act = |ui: &mut Ui, label: &str, hint: &str, a: Action, out: &mut UiOut| {
                    if row(ui, label, Some(hint), None, false).clicked() {
                        out.actions.push(a);
                        true
                    } else {
                        false
                    }
                };
                let mut close = false;
                close |= act(ui, if st.pause { "Play" } else { "Pause" }, "Space", Action::TogglePause, out);
                close |= act(ui, if v.fullscreen { "Exit fullscreen" } else { "Fullscreen" }, "F", Action::ToggleFullscreen, out);
                close |= act(ui, "Take screenshot", "S", Action::Screenshot, out);
                close |= act(ui, if v.on_top { "Unpin from top" } else { "Always on top" }, "T", Action::ToggleOnTop, out);
                close |= act(ui, "Load subtitle file…", "", Action::LoadSubtitle, out);
                close |= act(ui, "Open file…", if cfg!(target_os = "macos") { "⌘O" } else { "Ctrl+O" }, Action::OpenFiles, out);
                close |= act(ui, "Show in folder", "", Action::ShowInFolder, out);
                if row(ui, "Settings", None, None, true).clicked() {
                    s.menu = Menu::Settings;
                    s.page = Page::Main;
                    s.menu_changed = true;
                }
                if row(ui, if s.info { "Hide stats" } else { "Stats for nerds" }, Some("I"), None, false).clicked() {
                    s.info = !s.info;
                    close = true;
                }
                if row(ui, "Keyboard shortcuts", Some("?"), None, false).clicked() {
                    s.help = true;
                    close = true;
                }
                close |= act(ui, "Back to home", "Esc", Action::Stop, out);
                if close {
                    s.menu = Menu::None;
                }
            });
        });
        if resp.response.clicked_elsewhere() && !s.menu_changed {
            s.menu = Menu::None;
        }
        return;
    }

    // One Area/ScrollArea id per page: egui sizes a new area in an invisible
    // first pass, so every page opens at its natural height.
    let page_key = (s.menu as u8, s.page as u8);
    let resp = egui::Area::new(Id::new(("menu", page_key))).order(Order::Foreground).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -70.0)).show(ctx, |ui| {
        frame.show(ui, |ui| {
            let max_h = (screen.height() - 140.0).max(200.0);
            match s.menu {
                Menu::Playlist => {
                    ui.set_width(380.0_f32.min(screen.width() - 40.0));
                    header(ui, &format!("Playlist · {}", st.playlist.len()), s, None);
                    egui::ScrollArea::vertical().id_salt(("pl", page_key)).max_height(max_h.min(420.0)).show(ui, |ui| {
                        for (i, e) in st.playlist.iter().enumerate() {
                            let r = row2(ui, &format!("{}. {}", i + 1, e.name()), "", e.current);
                            if r.clicked() {
                                out.actions.push(Action::PlayIndex(i as i64));
                            }
                            if r.secondary_clicked() && !e.current {
                                out.actions.push(Action::RemoveIndex(i as i64));
                            }
                            if e.current && s.menu_changed {
                                r.scroll_to_me(Some(egui::Align::Center));
                            }
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Right-click an item to remove it").size(11.5).color(DIM));
                    });
                }
                _ => {
                    ui.set_width(if matches!(s.page, Page::Eq | Page::Video) { 340.0 } else { 300.0 });
                    egui::ScrollArea::vertical().id_salt(("pg", page_key)).max_height(max_h.min(560.0)).show(ui, |ui| settings_pages(ui, v, s, out));
                }
            }
        });
    });
    if resp.response.clicked_elsewhere() && !s.menu_changed {
        s.menu = Menu::None;
    }
    if s.last_page != Some(page_key) {
        s.last_page = Some(page_key);
        ctx.request_repaint();
    }
}

fn track_label(t: &Track) -> (String, String) {
    (t.label(), t.detail())
}

fn settings_pages(ui: &mut Ui, v: &View, s: &mut UiState, out: &mut UiOut) {
    let st = v.st;
    let cfg = v.settings;
    let back = if s.menu == Menu::Settings { Some(Page::Main) } else { None };
    match s.page {
        Page::Main => {
            let sub = st.sid.and_then(|id| st.tracks.iter().find(|t| t.kind == "sub" && t.id == id)).map(|t| t.label()).unwrap_or_else(|| "Off".into());
            let aud = st.aid.and_then(|id| st.tracks.iter().find(|t| t.kind == "audio" && t.id == id)).map(|t| t.label()).unwrap_or_else(|| "—".into());
            if row(ui, "Playback speed", Some(&speed_label(st.speed)), None, true).clicked() {
                s.page = Page::Speed;
            }
            if row(ui, "Subtitles", Some(&sub), None, true).clicked() {
                s.page = Page::Subs;
            }
            if row(ui, "Audio", Some(&aud), None, true).clicked() {
                s.page = Page::Audio;
            }
            let eq = EQ_PRESETS.iter().find(|p| p.0 == cfg.eq_preset).map(|p| p.1).unwrap_or("Custom");
            if row(ui, "Equalizer", Some(eq), None, true).clicked() {
                s.page = Page::Eq;
            }
            if st.has_video() && row(ui, "Video & picture", None, None, true).clicked() {
                s.page = Page::Video;
            }
            let rep = if st.ab_b.is_some() { "A-B" } else if st.loop_file { "This file" } else if st.loop_playlist { "Playlist" } else { "Off" };
            if row(ui, "Repeat", Some(rep), None, true).clicked() {
                s.page = Page::Repeat;
            }
            let sl = v.sleep_left.map(|x| format!("{} min", (x / 60.0).ceil() as i64)).unwrap_or_else(|| "Off".into());
            if row(ui, "Sleep timer", Some(&sl), None, true).clicked() {
                s.page = Page::Sleep;
            }
            if row(ui, "Preferences", None, None, true).clicked() {
                s.page = Page::Prefs;
            }
        }
        Page::Speed => {
            header(ui, "Playback speed", s, back);
            if let Some(x) = slider_row(ui, "Custom", st.speed, 0.25..=4.0, "×", 0.05) {
                out.actions.push(Action::SetSpeed(if x.is_nan() { 1.0 } else { x }));
            }
            for sp in [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0] {
                let label = if sp == 1.0 { "Normal".to_string() } else { speed_label(sp) };
                if row(ui, &label, None, Some((st.speed - sp).abs() < 0.001), false).clicked() {
                    out.actions.push(Action::SetSpeed(sp));
                }
            }
        }
        Page::Subs => {
            header(ui, "Subtitles", s, back);
            if row2(ui, "Off", "", st.sid.is_none()).clicked() {
                out.actions.push(Action::SetSub(None));
            }
            for t in st.tracks_of("sub") {
                let (l, d) = track_label(t);
                if row2(ui, &l, &d, st.sid == Some(t.id)).clicked() {
                    out.actions.push(Action::SetSub(Some(t.id)));
                }
            }
            if row(ui, "Load from file…", None, None, false).clicked() {
                out.actions.push(Action::LoadSubtitle);
            }
            ui.add_space(4.0);
            if let Some(x) = slider_row(ui, "Delay", st.sub_delay, -10.0..=10.0, "s", 0.1) {
                out.actions.push(Action::SubDelay(if x.is_nan() { 0.0 } else { x }));
            }
            if row(ui, "Style", None, None, true).clicked() {
                s.page = Page::SubStyle;
            }
        }
        Page::SubStyle => {
            header(ui, "Subtitle style", s, Some(Page::Subs));
            if let Some(x) = slider_row(ui, "Size", cfg.sub_scale * 100.0, 50.0..=250.0, "%", 5.0) {
                out.actions.push(Action::SetSubScale(if x.is_nan() { 1.0 } else { x / 100.0 }));
            }
            if let Some(x) = slider_row(ui, "Position", cfg.sub_pos as f64, 50.0..=100.0, "%", 1.0) {
                out.actions.push(Action::SetSubPos(if x.is_nan() { 100 } else { x as i64 }));
            }
            if let Some(x) = slider_row(ui, "Background", cfg.sub_back * 100.0, 0.0..=100.0, "%", 5.0) {
                out.actions.push(Action::SetSubBack(if x.is_nan() { 0.0 } else { x / 100.0 }));
            }
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.allocate_ui(vec2(92.0, 26.0), |ui| ui.label(egui::RichText::new("Color").size(13.5)));
                for c in ["#FFFFFF", "#FFE066", "#7CF0C0", "#8ECBFF", "#FFB0D0"] {
                    let col = Color32::from_hex(c).unwrap_or(Color32::WHITE);
                    let (r, resp) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
                    ui.painter().circle_filled(r.center(), 10.0, col);
                    if cfg.sub_color.eq_ignore_ascii_case(c) {
                        ui.painter().circle_stroke(r.center(), 12.0, Stroke::new(2.0_f32, ACCENT));
                    }
                    if resp.clicked() {
                        out.actions.push(Action::SetSubColor(c));
                    }
                }
            });
        }
        Page::Audio => {
            header(ui, "Audio", s, back);
            for t in st.tracks_of("audio") {
                let (l, d) = track_label(t);
                if row2(ui, &l, &d, st.aid == Some(t.id)).clicked() {
                    out.actions.push(Action::SetAudio(t.id));
                }
            }
            ui.add_space(4.0);
            if let Some(x) = slider_row(ui, "Sync", st.audio_delay, -5.0..=5.0, "s", 0.05) {
                out.actions.push(Action::AudioDelay(if x.is_nan() { 0.0 } else { x }));
            }
            if let Some(x) = slider_row(ui, "Volume", st.volume, 0.0..=200.0, "%", 5.0) {
                out.actions.push(Action::SetVolume(if x.is_nan() { 100.0 } else { x }));
            }
        }
        Page::Eq => {
            header(ui, "Equalizer", s, back);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(6.0);
                for (key, label, _) in EQ_PRESETS {
                    let on = cfg.eq_preset == *key;
                    let b = egui::Button::new(egui::RichText::new(*label).size(12.5).color(if on { INK } else { TEXT })).corner_radius(14).fill(if on { ACCENT } else { white(0.08) });
                    if ui.add(b).clicked() {
                        out.actions.push(Action::SetEqPreset(key));
                    }
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, f) in EQ_FREQS.iter().enumerate() {
                    ui.vertical(|ui| {
                        ui.set_width(28.0);
                        let mut g = cfg.eq_gains[i] as f64;
                        ui.label(egui::RichText::new(format!("{:+}", cfg.eq_gains[i])).size(10.5).color(DIM));
                        ui.spacing_mut().slider_width = 110.0;
                        let r = ui.add(egui::Slider::new(&mut g, -12.0..=12.0).vertical().step_by(1.0).show_value(false));
                        if r.changed() {
                            out.actions.push(Action::SetEqBand(i, g as i32));
                        }
                        let lbl = if *f >= 1000 { format!("{}k", f / 1000) } else { f.to_string() };
                        ui.label(egui::RichText::new(lbl).size(10.5).color(DIM));
                    });
                }
            });
            ui.add_space(6.0);
            if toggle(ui, "Normalize loudness", cfg.normalize).clicked() {
                out.actions.push(Action::SetNormalize(!cfg.normalize));
            }
        }
        Page::Video => {
            header(ui, "Video & picture", s, back);
            ui.horizontal_wrapped(|ui| {
                ui.add_space(6.0);
                let cur = if !st.keepaspect {
                    "stretch"
                } else if st.panscan > 0.5 {
                    "fill"
                } else if st.aspect <= 0.0 {
                    "auto"
                } else if (st.aspect - 16.0 / 9.0).abs() < 0.01 {
                    "16:9"
                } else if (st.aspect - 4.0 / 3.0).abs() < 0.01 {
                    "4:3"
                } else if (st.aspect - 2.35).abs() < 0.01 {
                    "2.35:1"
                } else if (st.aspect - 21.0 / 9.0).abs() < 0.01 {
                    "21:9"
                } else if (st.aspect - 1.0).abs() < 0.01 {
                    "1:1"
                } else {
                    ""
                };
                for (key, label) in [("auto", "Original"), ("16:9", "16:9"), ("4:3", "4:3"), ("21:9", "21:9"), ("2.35:1", "2.35:1"), ("1:1", "1:1"), ("fill", "Fill"), ("stretch", "Stretch")] {
                    let b = egui::Button::new(egui::RichText::new(label).size(12.5).color(if cur == key { INK } else { TEXT })).corner_radius(14).fill(if cur == key { ACCENT } else { white(0.08) });
                    if ui.add(b).clicked() {
                        out.actions.push(Action::SetAspect(key));
                    }
                }
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                for r in [0, 90, 180, 270] {
                    let b = egui::Button::new(egui::RichText::new(format!("{r}°")).size(12.5).color(if st.rotate == r { INK } else { TEXT })).corner_radius(14).fill(if st.rotate == r { ACCENT } else { white(0.08) });
                    if ui.add(b).clicked() {
                        out.actions.push(Action::SetRotate(r));
                    }
                }
            });
            ui.add_space(4.0);
            if let Some(x) = slider_row(ui, "Zoom", (2f64.powf(st.zoom) * 100.0).round(), 50.0..=300.0, "%", 5.0) {
                out.actions.push(Action::SetZoom(if x.is_nan() { 0.0 } else { (x / 100.0).log2() }));
            }
            for (name, label, val) in [
                ("brightness", "Brightness", st.brightness),
                ("contrast", "Contrast", st.contrast),
                ("saturation", "Saturation", st.saturation),
                ("gamma", "Gamma", st.gamma),
                ("hue", "Hue", st.hue),
            ] {
                if let Some(x) = slider_row(ui, label, val as f64, -100.0..=100.0, "", 1.0) {
                    out.actions.push(Action::SetVideoEq(name, if x.is_nan() { 0 } else { x as i64 }));
                }
            }
            let deint = st.deinterlace == "yes" || st.deinterlace == "true";
            if toggle(ui, "Deinterlace", deint).clicked() {
                out.actions.push(Action::SetDeinterlace(!deint));
            }
            if row(ui, "Reset picture", None, None, false).clicked() {
                out.actions.push(Action::ResetVideo);
            }
        }
        Page::Repeat => {
            header(ui, "Repeat", s, back);
            let off = !st.loop_file && !st.loop_playlist;
            if row(ui, "Off", None, Some(off), false).clicked() {
                out.actions.push(Action::SetLoopFile(false));
                out.actions.push(Action::SetLoopPlaylist(false));
            }
            if row(ui, "Repeat this file", None, Some(st.loop_file), false).clicked() {
                out.actions.push(Action::SetLoopFile(true));
            }
            if row(ui, "Repeat playlist", None, Some(st.loop_playlist && !st.loop_file), false).clicked() {
                out.actions.push(Action::SetLoopFile(false));
                out.actions.push(Action::SetLoopPlaylist(true));
            }
            let ab = match (st.ab_a, st.ab_b) {
                (None, _) => "Set point A".to_string(),
                (Some(a), None) => format!("A = {} · set point B", fmt_time(a)),
                (Some(a), Some(b)) => format!("{} → {} · clear", fmt_time(a), fmt_time(b)),
            };
            if row(ui, "A-B repeat", Some(&ab), None, false).clicked() {
                out.actions.push(Action::AbLoop);
            }
            if row(ui, "Shuffle playlist", None, None, false).clicked() {
                out.actions.push(Action::Shuffle);
            }
        }
        Page::Sleep => {
            header(ui, "Sleep timer", s, back);
            if row(ui, "Off", None, Some(v.sleep_left.is_none()), false).clicked() {
                out.actions.push(Action::SetSleep(None));
            }
            for m in [15u32, 30, 45, 60, 90, 120] {
                if row(ui, &format!("{m} minutes"), None, None, false).clicked() {
                    out.actions.push(Action::SetSleep(Some(m)));
                }
            }
        }
        Page::Prefs => {
            header(ui, "Preferences", s, back);
            if toggle(ui, "Autoplay next file", cfg.autoplay_next).clicked() {
                out.actions.push(Action::SetAutoplay(!cfg.autoplay_next));
            }
            if toggle(ui, "Resume where I left off", cfg.remember_position).clicked() {
                out.actions.push(Action::SetRemember(!cfg.remember_position));
            }
            if toggle(ui, "Queue other files in folder", cfg.folder_playlist).clicked() {
                out.actions.push(Action::SetFolderPlaylist(!cfg.folder_playlist));
            }
            if toggle(ui, "Hardware decoding", cfg.hwdec).clicked() {
                out.actions.push(Action::SetHwdec(!cfg.hwdec));
            }
            if toggle(ui, "High-quality scaling", cfg.high_quality).clicked() {
                out.actions.push(Action::SetHighQuality(!cfg.high_quality));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Help + info

const SHORTCUTS: &[(&str, &str)] = &[
    ("Space / K", "Play / pause"),
    ("← / →", "Seek 5 s"),
    ("J / L", "Seek 10 s"),
    ("Shift + ← / →", "Seek 1 min"),
    (if cfg!(target_os = "macos") { "⌘ ← / →" } else { "Ctrl + ← / →" }, "Previous / next chapter"),
    ("0 – 9", "Jump to 0–90 %"),
    (", / .", "Frame back / forward"),
    ("↑ / ↓ / wheel", "Volume"),
    ("M", "Mute"),
    ("[ / ]", "Slower / faster"),
    ("Backspace", "Normal speed"),
    ("F / Enter / dbl-click", "Fullscreen"),
    ("C", "Subtitles on / off"),
    ("V", "Next subtitle track"),
    ("G / H", "Subtitle delay −/+"),
    ("B", "Next audio track"),
    ("A", "Cycle aspect ratio"),
    ("N / P", "Next / previous file"),
    ("R", "Cycle repeat"),
    ("S", "Screenshot"),
    ("T", "Always on top"),
    ("I", "Stats"),
    (if cfg!(target_os = "macos") { "⌘ O" } else { "Ctrl + O" }, "Open file"),
    (if cfg!(target_os = "macos") { "⌘ V" } else { "Ctrl + V" }, "Paste link / path"),
    ("Esc", "Exit fullscreen / back"),
];

fn help(ctx: &Context, s: &mut UiState) {
    let screen = ctx.screen_rect();
    ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("help-dim"))).rect_filled(screen, 0.0, black(0.45));
    let resp = egui::Area::new(Id::new("help")).order(Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
        egui::Frame::new().fill(PANEL).corner_radius(16).inner_margin(20).show(ui, |ui| {
            ui.label(egui::RichText::new("Keyboard shortcuts").font(font_sb(17.0)));
            ui.add_space(10.0);
            let cols = if screen.width() > 760.0 { 2 } else { 1 };
            let per = SHORTCUTS.len().div_ceil(cols);
            ui.horizontal_top(|ui| {
                for c in 0..cols {
                    ui.vertical(|ui| {
                        ui.set_width(300.0);
                        for (k, d) in SHORTCUTS.iter().skip(c * per).take(per) {
                            ui.horizontal(|ui| {
                                ui.allocate_ui(vec2(150.0, 22.0), |ui| ui.label(egui::RichText::new(*k).font(font_sb(13.0)).color(ACCENT)));
                                ui.label(egui::RichText::new(*d).size(13.0));
                            });
                        }
                    });
                }
            });
        });
    });
    if resp.response.clicked_elsewhere() || resp.response.clicked() {
        s.help = false;
    }
}

fn info_panel(ctx: &Context, rows: &[(String, String)], s: &mut UiState) {
    egui::Area::new(Id::new("info")).order(Order::Foreground).fixed_pos(pos2(16.0, 56.0)).interactable(false).show(ctx, |ui| {
        egui::Frame::new().fill(Color32::from_black_alpha(185)).corner_radius(10).inner_margin(12).show(ui, |ui| {
            ui.set_max_width(420.0);
            for (k, val) in rows {
                ui.horizontal(|ui| {
                    ui.allocate_ui(vec2(96.0, 18.0), |ui| ui.label(egui::RichText::new(k).size(12.0).color(DIM)));
                    ui.label(egui::RichText::new(val).size(12.0).family(FontFamily::Monospace));
                });
            }
        });
    });
    let _ = s;
}
