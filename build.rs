//! Build script:
//!  * links against libmpv: on Windows the import library in .tools/mpv (or
//!    $VELA_MPV_DIR), elsewhere the system/Homebrew libmpv
//!  * Windows: copies libmpv-2.dll next to the exe so `cargo run` just works,
//!    and embeds icon + version info + manifest as a Win32 resource section.
//!    The GNU toolchain ships no resource compiler, so we emit the COFF
//!    `.rsrc` object ourselves (the format is small and stable).

use object::write::{Object, Relocation};
use object::{pe, Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const VERSION: [u16; 4] = [1, 0, 0, 0];

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/icon.ico");
    println!("cargo:rerun-if-env-changed=VELA_MPV_DIR");

    let custom = env::var("VELA_MPV_DIR").map(PathBuf::from).ok();
    if target_os == "windows" {
        let mpv_dir = custom.unwrap_or_else(|| root.join(".tools").join("mpv"));
        println!("cargo:rustc-link-search=native={}", mpv_dir.display());
        // target/<profile>/ is three levels above OUT_DIR
        if let Some(profile_dir) = out_dir.ancestors().nth(3) {
            let dll = mpv_dir.join("libmpv-2.dll");
            let dest = profile_dir.join("libmpv-2.dll");
            if dll.exists() && !same_size(&dll, &dest) {
                let _ = fs::copy(&dll, &dest);
            }
        }
        let obj = build_resources(&root.join("build").join("icon.ico"));
        let path = out_dir.join("vela-resources.o");
        fs::write(&path, obj).expect("write resource object");
        println!("cargo:rustc-link-arg-bins={}", path.display());
    } else {
        // System libmpv (apt: libmpv-dev, dnf: mpv-libs-devel, brew: mpv)
        let mut dirs: Vec<PathBuf> = custom.into_iter().collect();
        if target_os == "macos" {
            dirs.extend(["/opt/homebrew/lib", "/usr/local/lib"].map(PathBuf::from));
        }
        for d in dirs.into_iter().filter(|d| d.exists()) {
            println!("cargo:rustc-link-search=native={}", d.display());
        }
    }
}

fn same_size(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.len() == y.len(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Resource tree

struct Res {
    kind: u16,
    id: u16,
    data: Vec<u8>,
}

fn build_resources(icon_path: &Path) -> Vec<u8> {
    let mut res = Vec::new();

    // Icons: ICO file -> one RT_ICON per image + one RT_GROUP_ICON directory
    if let Ok(ico) = fs::read(icon_path) {
        let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
        let mut group = Vec::new();
        group.extend_from_slice(&[0, 0, 1, 0]);
        group.extend_from_slice(&(count as u16).to_le_bytes());
        for i in 0..count {
            let e = &ico[6 + i * 16..6 + (i + 1) * 16];
            let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
            let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
            let id = (i + 1) as u16;
            res.push(Res { kind: 3, id, data: ico[offset..offset + size].to_vec() });
            group.extend_from_slice(&e[..12]); // w, h, colors, reserved, planes, bitcount, bytes
            group.extend_from_slice(&id.to_le_bytes());
        }
        res.push(Res { kind: 14, id: 1, data: group });
    }

    res.push(Res { kind: 16, id: 1, data: version_info() });
    res.push(Res { kind: 24, id: 1, data: MANIFEST.as_bytes().to_vec() });
    res.sort_by_key(|r| (r.kind, r.id));

    coff_rsrc(&res)
}

fn coff_rsrc(res: &[Res]) -> Vec<u8> {
    // Layout: [root dir][type dirs][name dirs][lang dirs][data entries][data]
    let mut kinds: Vec<u16> = res.iter().map(|r| r.kind).collect();
    kinds.dedup();
    let dir_size = |entries: usize| 16 + entries * 8;

    let root_size = dir_size(kinds.len());
    let type_dirs_size: usize = kinds.iter().map(|k| dir_size(res.iter().filter(|r| r.kind == *k).count())).sum();
    let lang_dirs_size = res.len() * dir_size(1);
    let entries_off = root_size + type_dirs_size + lang_dirs_size;
    let mut data_off = entries_off + res.len() * 16;
    data_off = (data_off + 7) & !7;

    let mut buf = vec![0u8; data_off];
    let mut data_offsets = Vec::new();
    for r in res {
        let off = buf.len();
        data_offsets.push(off);
        buf.extend_from_slice(&r.data);
        while buf.len() % 8 != 0 {
            buf.push(0);
        }
    }

    let put_dir = |buf: &mut Vec<u8>, at: usize, n: usize| {
        buf[at..at + 16].fill(0);
        buf[at + 14..at + 16].copy_from_slice(&(n as u16).to_le_bytes());
    };
    let put_entry = |buf: &mut Vec<u8>, at: usize, id: u32, target: u32| {
        buf[at..at + 4].copy_from_slice(&id.to_le_bytes());
        buf[at + 4..at + 8].copy_from_slice(&target.to_le_bytes());
    };

    put_dir(&mut buf, 0, kinds.len());
    let mut type_cursor = root_size;
    let mut lang_cursor = root_size + type_dirs_size;
    let mut idx = 0;
    let mut relocs = Vec::new();
    for (ki, kind) in kinds.iter().enumerate() {
        let items: Vec<usize> = (0..res.len()).filter(|i| res[*i].kind == *kind).collect();
        put_entry(&mut buf, 16 + ki * 8, *kind as u32, 0x8000_0000 | type_cursor as u32);
        put_dir(&mut buf, type_cursor, items.len());
        for (ni, ri) in items.iter().enumerate() {
            put_entry(&mut buf, type_cursor + 16 + ni * 8, res[*ri].id as u32, 0x8000_0000 | lang_cursor as u32);
            put_dir(&mut buf, lang_cursor, 1);
            let entry_at = entries_off + idx * 16;
            put_entry(&mut buf, lang_cursor + 16, 0x0409, entry_at as u32);
            // IMAGE_RESOURCE_DATA_ENTRY: RVA (relocated), size, codepage, reserved
            buf[entry_at + 4..entry_at + 8].copy_from_slice(&(res[*ri].data.len() as u32).to_le_bytes());
            relocs.push((entry_at as u64, data_offsets[*ri] as i64));
            lang_cursor += dir_size(1);
            idx += 1;
        }
        type_cursor += dir_size(items.len());
    }

    let mut obj = Object::new(BinaryFormat::Coff, Architecture::X86_64, Endianness::Little);
    let section = obj.add_section(Vec::new(), b".rsrc".to_vec(), SectionKind::ReadOnlyData);
    obj.set_section_data(section, buf, 8);
    let sym = obj.section_symbol(section);
    for (offset, addend) in relocs {
        obj.add_relocation(
            section,
            Relocation { offset, symbol: sym, addend, flags: RelocationFlags::Coff { typ: pe::IMAGE_REL_AMD64_ADDR32NB } },
        )
        .expect("relocation");
    }
    obj.write().expect("write coff")
}

// ---------------------------------------------------------------------------
// VS_VERSIONINFO

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect()
}

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// One version-resource block: header, key, value, children.
fn block(key: &str, value: &[u8], value_len_field: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0u8; 6];
    v.extend(utf16z(key));
    pad4(&mut v);
    v.extend_from_slice(value);
    for c in children {
        pad4(&mut v);
        v.extend_from_slice(c);
    }
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v[2..4].copy_from_slice(&value_len_field.to_le_bytes());
    v[4..6].copy_from_slice(&(text as u16).to_le_bytes());
    v
}

fn version_info() -> Vec<u8> {
    let ms = ((VERSION[0] as u32) << 16) | VERSION[1] as u32;
    let ls = ((VERSION[2] as u32) << 16) | VERSION[3] as u32;
    let fixed: Vec<u8> = [0xFEEF04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0]
        .iter()
        .flat_map(|x| x.to_le_bytes())
        .collect();
    let ver = format!("{}.{}.{}.{}", VERSION[0], VERSION[1], VERSION[2], VERSION[3]);
    let strings = [
        ("CompanyName", "VELA"),
        ("FileDescription", "VELA"),
        ("FileVersion", ver.as_str()),
        ("InternalName", "vela"),
        ("LegalCopyright", "MIT License"),
        ("OriginalFilename", "Vela.exe"),
        ("ProductName", "VELA"),
        ("ProductVersion", ver.as_str()),
    ];
    let string_blocks: Vec<Vec<u8>> = strings
        .iter()
        .map(|(k, v)| {
            let val = utf16z(v);
            block(k, &val, (val.len() / 2) as u16, true, &[])
        })
        .collect();
    let table = block("040904B0", &[], 0, true, &string_blocks);
    let sfi = block("StringFileInfo", &[], 0, true, &[table]);
    let translation: Vec<u8> = [0x0409u16, 0x04B0].iter().flat_map(|x| x.to_le_bytes()).collect();
    let var = block("Translation", &translation, 4, false, &[]);
    let vfi = block("VarFileInfo", &[], 0, true, &[var]);
    block("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[sfi, vfi])
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Vela.Player" version="1.0.0.0" processorArchitecture="amd64"/>
  <description>VELA media player</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v2">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#;
