//! UI fonts come from the operating system and are memory-mapped, not
//! embedded or read into the heap: the binary stays small and only the glyph
//! pages that are actually touched ever become resident.

use egui::{FontData, FontDefinitions, FontFamily};
use std::path::PathBuf;
use std::sync::Arc;

/// (key, candidate files, face index). The first existing file wins.
type Face = (&'static str, &'static [&'static str], u32);

#[cfg(windows)]
const FACES: &[Face] = &[
    ("ui", &["segoeui.ttf", "SegUIVar.ttf", "arial.ttf"], 0),
    ("ui-semibold", &["seguisb.ttf", "segoeuib.ttf", "arialbd.ttf"], 0),
    ("mono", &["consola.ttf", "cour.ttf"], 0),
    ("sym", &["seguisym.ttf"], 0),
    ("indic", &["Nirmala.ttc", "Nirmala.ttf"], 0),
    ("cjk-sc", &["msyh.ttc", "simsun.ttc"], 0),
    ("cjk-jp", &["YuGothM.ttc", "meiryo.ttc", "msgothic.ttc"], 0),
    ("korean", &["malgun.ttf"], 0),
    ("emoji", &["seguiemj.ttf"], 0),
];

#[cfg(target_os = "macos")]
const FACES: &[Face] = &[
    ("ui", &["/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Helvetica.ttc"], 0),
    ("ui-semibold", &["/System/Library/Fonts/SFNS.ttf", "/System/Library/Fonts/Helvetica.ttc"], 0),
    ("mono", &["/System/Library/Fonts/SFNSMono.ttf", "/System/Library/Fonts/Menlo.ttc"], 0),
    ("indic", &["/System/Library/Fonts/Kohinoor.ttc", "/System/Library/Fonts/Supplemental/DevanagariMT.ttc"], 0),
    ("cjk", &["/System/Library/Fonts/PingFang.ttc", "/System/Library/Fonts/Hiragino Sans GB.ttc"], 0),
    ("korean", &["/System/Library/Fonts/AppleSDGothicNeo.ttc"], 0),
    ("sym", &["/System/Library/Fonts/Apple Symbols.ttf", "/Library/Fonts/Arial Unicode.ttf"], 0),
];

#[cfg(all(unix, not(target_os = "macos")))]
const FACES: &[Face] = &[
    (
        "ui",
        &[
            "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        ],
        0,
    ),
    (
        "ui-semibold",
        &[
            "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/google-noto/NotoSans-SemiBold.ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-M.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        ],
        0,
    ),
    ("mono", &["/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf", "/usr/share/fonts/TTF/DejaVuSansMono.ttf"], 0),
    ("indic", &["/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf", "/usr/share/fonts/noto/NotoSansDevanagari-Regular.ttf"], 0),
    (
        "cjk",
        &[
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        ],
        0,
    ),
];

fn resolve(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() {
        return p;
    }
    let win = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    win.join("Fonts").join(name)
}

fn map(name: &str) -> Option<&'static [u8]> {
    let file = std::fs::File::open(resolve(name)).ok()?;
    // SAFETY: system font files are not modified while we run.
    let mmap = unsafe { memmap2::Mmap::map(&file).ok()? };
    Some(&Box::leak(Box::new(mmap))[..])
}

pub fn definitions() -> FontDefinitions {
    // Start from egui's bundled fonts where that feature is enabled (Linux/macOS),
    // so text always renders even on a bare system.
    let mut defs = FontDefinitions::default();
    let mut loaded = Vec::new();
    for (key, files, index) in FACES {
        if let Some(bytes) = files.iter().find_map(|f| map(f)) {
            let mut fd = FontData::from_static(bytes);
            fd.index = *index;
            defs.font_data.insert(key.to_string(), Arc::new(fd));
            loaded.push(*key);
        }
    }
    let has = |k: &str| loaded.contains(&k);
    let fallbacks: Vec<String> = loaded.iter().filter(|k| !matches!(**k, "ui" | "ui-semibold" | "mono")).map(|k| k.to_string()).collect();
    let builtin = |fam: FontFamily| defs.families.get(&fam).cloned().unwrap_or_default();
    let chain = |primary: Option<&str>, fam: FontFamily| {
        let mut v: Vec<String> = primary.into_iter().map(String::from).collect();
        v.extend(fallbacks.iter().cloned());
        v.extend(builtin(fam));
        v
    };
    let ui = has("ui").then_some("ui");
    let prop = chain(ui, FontFamily::Proportional);
    let semi = chain(if has("ui-semibold") { Some("ui-semibold") } else { ui }, FontFamily::Proportional);
    let mono = chain(if has("mono") { Some("mono") } else { ui }, FontFamily::Monospace);
    defs.families.insert(FontFamily::Proportional, prop);
    defs.families.insert(FontFamily::Name("semibold".into()), semi);
    defs.families.insert(FontFamily::Monospace, mono);
    defs
}
