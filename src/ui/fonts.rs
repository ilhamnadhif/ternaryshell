//! Font setup. System font files are memory-mapped rather than read, so a 25 MB CJK fallback
//! costs only the pages actually used for glyphs that appear on screen.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use parking_lot::Mutex;

use crate::settings::FontChoice;

pub const TERM_REGULAR: &str = "term-regular";
pub const TERM_BOLD: &str = "term-bold";
pub const TERM_ITALIC: &str = "term-italic";
pub const TERM_BOLD_ITALIC: &str = "term-bold-italic";

/// A selectable terminal font: file and face indices (regular, bold, italic, bold italic). On
/// macOS files are absolute paths; on Windows and Linux file names, looked up in the system's
/// font folders (`locate`).
#[derive(Clone, Debug)]
pub struct FontPreset {
    pub name: &'static str,
    pub path: Option<&'static str>,
    pub faces: [Option<(Option<&'static str>, u32)>; 4],
}

#[cfg(not(target_os = "macos"))]
const fn four(
    r: &'static str,
    b: &'static str,
    i: &'static str,
    bi: &'static str,
) -> [Option<(Option<&'static str>, u32)>; 4] {
    [
        Some((Some(r), 0)),
        Some((Some(b), 0)),
        Some((Some(i), 0)),
        Some((Some(bi), 0)),
    ]
}

const BUILT_IN: FontPreset = FontPreset {
    name: "Hack (built-in)",
    path: None,
    faces: [None, None, None, None],
};

/// JetBrains Mono, bundled with the app: the family default monospace font on every system.
pub const JETBRAINS: FontPreset = FontPreset {
    name: "JetBrains Mono (built-in)",
    path: None,
    faces: [None, None, None, None],
};

#[cfg(target_os = "macos")]
pub const PRESETS: &[FontPreset] = &[
    JETBRAINS,
    FontPreset {
        name: "Menlo",
        path: Some("/System/Library/Fonts/Menlo.ttc"),
        faces: [
            Some((None, 0)),
            Some((None, 1)),
            Some((None, 2)),
            Some((None, 3)),
        ],
    },
    FontPreset {
        name: "SF Mono",
        path: Some("/System/Library/Fonts/SFNSMono.ttf"),
        faces: [
            Some((None, 0)),
            None,
            Some((Some("/System/Library/Fonts/SFNSMonoItalic.ttf"), 0)),
            None,
        ],
    },
    FontPreset {
        name: "Monaco",
        path: Some("/System/Library/Fonts/Monaco.ttf"),
        faces: [Some((None, 0)), None, None, None],
    },
    FontPreset {
        name: "Courier New",
        path: Some("/System/Library/Fonts/Supplemental/Courier New.ttf"),
        faces: [
            Some((None, 0)),
            Some((
                Some("/System/Library/Fonts/Supplemental/Courier New Bold.ttf"),
                0,
            )),
            Some((
                Some("/System/Library/Fonts/Supplemental/Courier New Italic.ttf"),
                0,
            )),
            Some((
                Some("/System/Library/Fonts/Supplemental/Courier New Bold Italic.ttf"),
                0,
            )),
        ],
    },
    FontPreset {
        name: "Andale Mono",
        path: Some("/System/Library/Fonts/Supplemental/Andale Mono.ttf"),
        faces: [Some((None, 0)), None, None, None],
    },
    BUILT_IN,
];

#[cfg(windows)]
pub const PRESETS: &[FontPreset] = &[
    JETBRAINS,
    FontPreset {
        name: "Consolas",
        path: Some("consola.ttf"),
        faces: four(
            "consola.ttf",
            "consolab.ttf",
            "consolai.ttf",
            "consolaz.ttf",
        ),
    },
    FontPreset {
        name: "Cascadia Mono",
        path: Some("CascadiaMono.ttf"),
        faces: [
            Some((None, 0)),
            None,
            Some((Some("CascadiaMonoItalic.ttf"), 0)),
            None,
        ],
    },
    FontPreset {
        name: "Lucida Console",
        path: Some("lucon.ttf"),
        faces: [Some((None, 0)), None, None, None],
    },
    FontPreset {
        name: "Courier New",
        path: Some("cour.ttf"),
        faces: four("cour.ttf", "courbd.ttf", "couri.ttf", "courbi.ttf"),
    },
    BUILT_IN,
];

#[cfg(not(any(target_os = "macos", windows)))]
pub const PRESETS: &[FontPreset] = &[
    JETBRAINS,
    BUILT_IN,
    FontPreset {
        name: "DejaVu Sans Mono",
        path: Some("DejaVuSansMono.ttf"),
        faces: four(
            "DejaVuSansMono.ttf",
            "DejaVuSansMono-Bold.ttf",
            "DejaVuSansMono-Oblique.ttf",
            "DejaVuSansMono-BoldOblique.ttf",
        ),
    },
    FontPreset {
        name: "Liberation Mono",
        path: Some("LiberationMono-Regular.ttf"),
        faces: four(
            "LiberationMono-Regular.ttf",
            "LiberationMono-Bold.ttf",
            "LiberationMono-Italic.ttf",
            "LiberationMono-BoldItalic.ttf",
        ),
    },
    FontPreset {
        name: "Ubuntu Mono",
        path: Some("UbuntuMono-R.ttf"),
        faces: four(
            "UbuntuMono-R.ttf",
            "UbuntuMono-B.ttf",
            "UbuntuMono-RI.ttf",
            "UbuntuMono-BI.ttf",
        ),
    },
    FontPreset {
        name: "Noto Sans Mono",
        path: Some("NotoSansMono-Regular.ttf"),
        faces: [
            Some((None, 0)),
            Some((Some("NotoSansMono-Bold.ttf"), 0)),
            None,
            None,
        ],
    },
];

/// Fallbacks for Chinese, Japanese and Korean text, by key and file.
#[cfg(target_os = "macos")]
const CJK_FALLBACKS: &[(&str, &str)] = &[
    ("cjk-ja", "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"),
    ("cjk-zh", "/System/Library/Fonts/Hiragino Sans GB.ttc"),
    ("cjk-ko", "/System/Library/Fonts/AppleSDGothicNeo.ttc"),
];
#[cfg(windows)]
const CJK_FALLBACKS: &[(&str, &str)] = &[
    ("cjk-ja", "YuGothM.ttc"),
    ("cjk-zh", "msyh.ttc"),
    ("cjk-ko", "malgun.ttf"),
];
/// Noto CJK covers all three; WenQuanYi where it is the one installed.
#[cfg(not(any(target_os = "macos", windows)))]
const CJK_FALLBACKS: &[(&str, &str)] = &[
    ("cjk", "NotoSansCJK-Regular.ttc"),
    ("cjk-wqy", "wqy-microhei.ttc"),
];

#[cfg(target_os = "macos")]
const SYMBOLS: (&str, &str) = ("symbols", "/System/Library/Fonts/Apple Symbols.ttf");
#[cfg(windows)]
const SYMBOLS: (&str, &str) = ("symbols", "seguisym.ttf");
#[cfg(not(any(target_os = "macos", windows)))]
const SYMBOLS: (&str, &str) = ("symbols", "DejaVuSans.ttf");

/// The terminal font when none was chosen: JetBrains Mono, bundled, the same on every system.
const DEFAULT_FONT: &str = JETBRAINS.name;

/// The system's font folders (Windows and Linux).
#[cfg(windows)]
fn font_dirs() -> Vec<PathBuf> {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    let mut dirs = vec![PathBuf::from(windir).join("Fonts")];
    // Fonts installed for this user only.
    if let Some(local) = dirs::data_local_dir() {
        dirs.push(local.join("Microsoft").join("Windows").join("Fonts"));
    }
    dirs
}

#[cfg(not(any(target_os = "macos", windows)))]
fn font_dirs() -> Vec<PathBuf> {
    let home = crate::paths::home_dir();
    let mut dirs = vec![
        PathBuf::from("/usr/share/fonts"),
        PathBuf::from("/usr/local/share/fonts"),
        home.join(".local/share/fonts"),
        home.join(".fonts"),
    ];
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data).join("fonts"));
    }
    dirs
}

/// Where a font file is: absolute paths as they are (macOS); file names in the system's font
/// folders. On Linux the folders are indexed once, on first use (fonts sit in distribution-
/// specific subfolders); nothing is read for the built-in font.
fn locate(file: &str) -> Option<PathBuf> {
    let path = Path::new(file);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    #[cfg(target_os = "macos")]
    {
        None
    }
    #[cfg(windows)]
    {
        font_dirs()
            .into_iter()
            .map(|d| d.join(file))
            .find(|p| p.is_file())
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
        let index = INDEX.get_or_init(|| {
            let mut index = HashMap::new();
            let mut visited = std::collections::HashSet::new();
            let mut stack: Vec<(PathBuf, u32)> = font_dirs().into_iter().map(|d| (d, 0)).collect();
            while let Some((dir, depth)) = stack.pop() {
                if let Ok(canon) = std::fs::canonicalize(&dir)
                    && !visited.insert(canon)
                {
                    continue;
                }
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_dir() && depth < 4 {
                        stack.push((p, depth + 1));
                    } else if p.is_file() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        index.entry(name).or_insert(p);
                    }
                }
            }
            index
        });
        index.get(file).cloned()
    }
}

/// Sealed read-only system fonts on macOS (immutable APFS system volume, safe to memory-map).
#[cfg(target_os = "macos")]
fn is_sealed_system_font(path: &Path) -> bool {
    path.canonicalize()
        .is_ok_and(|c| c.starts_with("/System/Library/"))
}

/// The UI & monospace fonts, bundled: Inter in three weights, JetBrains Mono, and Phosphor icons.
const INTER: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");
const JB_REGULAR: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");
const JB_BOLD: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf");
const JB_ITALIC: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Italic.ttf");
const JB_BOLD_ITALIC: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-BoldItalic.ttf");
const PHOSPHOR: &[u8] = include_bytes!("../../assets/fonts/Phosphor.ttf");

/// Font families of the UI beyond egui's two: Inter Medium and SemiBold, and the icons alone.
pub const MEDIUM: &str = "ui-medium";
pub const SEMIBOLD: &str = "ui-semibold";
pub const ICONS: &str = "ui-icons";

/// Largest font file loaded.
const MAX_FONT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CACHED_OWNED_FONTS: usize = 16;

static FONT_GEN: AtomicU64 = AtomicU64::new(1);
/// Bitmask of actually loaded terminal faces (bit 0 = regular, 1 = bold, 2 = italic, 3 = bold+italic).
/// `0` means `apply()` has not run yet.
static LOADED_FACES: AtomicU8 = AtomicU8::new(0);

/// Increments whenever `apply()` installs new fonts, so cached galleys can invalidate.
pub fn generation() -> u64 {
    FONT_GEN.load(Ordering::Relaxed)
}

/// Loads and validates a font face at `index`. Sealed macOS `/System/Library/` fonts are
/// memory-mapped (so large CJK collections page in lazily); all other fonts (Linux, Windows,
/// user-picked files) are read into bounded owned memory and cached as `Arc<FontData>` without
/// leaking invalid or evicted files.
fn load_font_data(path: &Path, index: u32) -> Option<Arc<FontData>> {
    #[cfg(target_os = "macos")]
    if is_sealed_system_font(path) {
        static MMAP_CACHE: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
        let cache = MMAP_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let bytes = {
            let mut guard = cache.lock();
            if let Some(&b) = guard.get(path) {
                b
            } else {
                let file = std::fs::File::open(path).ok()?;
                let meta = file.metadata().ok()?;
                if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
                    return None;
                }
                // SAFETY: `is_sealed_system_font` verified the canonical path sits on macOS's
                // sealed read-only `/System/Library/` volume, and the mapping lives for the
                // rest of the process only after validation succeeds.
                let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
                if !is_usable_font(&map, index) {
                    return None;
                }
                let leaked: &'static [u8] = Box::leak(Box::new(map));
                guard.insert(path.to_path_buf(), leaked);
                leaked
            }
        };
        if !is_usable_font(bytes, index) {
            return None;
        }
        let mut data = FontData::from_static(bytes);
        data.index = index;
        return Some(Arc::new(data));
    }

    type OwnedFontCache = Mutex<HashMap<(PathBuf, u32), Arc<FontData>>>;
    static OWNED_CACHE: OnceLock<OwnedFontCache> = OnceLock::new();
    let cache = OWNED_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (path.to_path_buf(), index);
    if let Some(existing) = cache.lock().get(&key) {
        return Some(Arc::clone(existing));
    }
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FONT_BYTES {
        return None;
    }
    let mut data = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(&file, MAX_FONT_BYTES + 1),
        &mut data,
    )
    .ok()?;
    if data.len() as u64 > MAX_FONT_BYTES || !is_usable_font(&data, index) {
        return None;
    }
    let mut font = FontData::from_owned(data);
    font.index = index;
    let arc = Arc::new(font);
    let mut guard = cache.lock();
    if guard.len() >= MAX_CACHED_OWNED_FONTS {
        guard.clear();
    }
    guard.insert(key, Arc::clone(&arc));
    Some(arc)
}

/// Whether `bytes` holds a usable font at `index` (egui panics on a font it can't parse, so
/// every face is checked before it is handed over).
fn is_usable_font(bytes: &[u8], index: u32) -> bool {
    use skrifa::MetadataProvider;
    skrifa::FontRef::from_index(bytes, index).is_ok_and(|f| f.charmap().map('M').is_some())
}

fn add_face(defs: &mut FontDefinitions, key: &str, file: &str, index: u32) -> bool {
    let Some(path) = locate(file) else {
        return false;
    };
    let Some(data) = load_font_data(&path, index) else {
        return false;
    };
    defs.font_data.insert(key.to_string(), data);
    true
}

/// Whether a preset's font file is on this computer, so it can be offered: the built-in font
/// always is; the others are looked up where the system keeps fonts (on Windows and Linux the
/// preset names a file, not a path).
pub fn is_available(preset: &FontPreset) -> bool {
    preset
        .path
        .is_none_or(|file| locate(file).is_some_and(|p| p.is_file()))
}

/// Finds the preset matching a saved choice (by name), if it is not a custom file path.
pub fn preset(choice: &FontChoice) -> Option<&'static FontPreset> {
    if choice.path.is_some() {
        return None;
    }
    PRESETS.iter().find(|p| p.name == choice.name)
}

/// Builds the font set: the terminal font (four faces where available), the UI font, and
/// fallbacks for symbols, CJK and emoji.
pub fn definitions(choice: &FontChoice, load_cjk: bool) -> FontDefinitions {
    definitions_with_faces(choice, load_cjk).0
}

fn definitions_with_faces(choice: &FontChoice, load_cjk: bool) -> (FontDefinitions, u8) {
    let mut defs = FontDefinitions::default();
    // egui's bundled fonts: Hack, Ubuntu-Light, NotoEmoji-Regular, emoji-icon-font.
    let emoji: Vec<String> = ["NotoEmoji-Regular", "emoji-icon-font"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut fallbacks: Vec<String> = Vec::new();
    if add_face(&mut defs, SYMBOLS.0, SYMBOLS.1, 0) {
        fallbacks.push(SYMBOLS.0.into());
    }
    if load_cjk {
        for (key, path) in CJK_FALLBACKS {
            if add_face(&mut defs, key, path, 0) {
                fallbacks.push(key.to_string());
            }
        }
    }
    fallbacks.extend(emoji.iter().cloned());

    // Terminal faces. Missing faces fall back to the regular one (egui then synthesizes italics
    // and the renderer fakes bold).
    let mut faces: [Option<String>; 4] = [None, None, None, None];
    let keys = [TERM_REGULAR, TERM_BOLD, TERM_ITALIC, TERM_BOLD_ITALIC];
    let font = |bytes: &'static [u8]| Arc::new(FontData::from_static(bytes));
    for (key, bytes) in [
        ("JetBrainsMono", JB_REGULAR),
        ("JetBrainsMono-Bold", JB_BOLD),
        ("JetBrainsMono-Italic", JB_ITALIC),
        ("JetBrainsMono-BoldItalic", JB_BOLD_ITALIC),
    ] {
        defs.font_data.insert(key.into(), font(bytes));
    }
    match (preset(choice), &choice.path) {
        (Some(p), _) if p.name == JETBRAINS.name => {
            for (i, key) in [
                "JetBrainsMono",
                "JetBrainsMono-Bold",
                "JetBrainsMono-Italic",
                "JetBrainsMono-BoldItalic",
            ]
            .iter()
            .enumerate()
            {
                faces[i] = Some((*key).to_string());
            }
        }
        (Some(p), _) => {
            if let Some(base) = p.path {
                for (i, face) in p.faces.iter().enumerate() {
                    if let Some((file, index)) = face
                        && add_face(&mut defs, keys[i], file.unwrap_or(base), *index)
                    {
                        faces[i] = Some(keys[i].to_string());
                    }
                }
            }
        }
        (None, Some(path))
            if add_face(
                &mut defs,
                TERM_REGULAR,
                &crate::paths::expand_tilde(path).to_string_lossy(),
                choice.index,
            ) =>
        {
            faces[0] = Some(TERM_REGULAR.to_string());
        }
        _ => {}
    }
    let regular = faces[0].clone().unwrap_or_else(|| "Hack".to_string());
    for (i, key) in keys.iter().enumerate() {
        let primary = faces[i].clone().unwrap_or_else(|| regular.clone());
        let mut family = vec![primary];
        family.push("Hack".into());
        family.extend(fallbacks.iter().cloned());
        family.dedup();
        defs.families
            .insert(FontFamily::Name((*key).into()), family);
    }
    let mut mono = vec![regular, "JetBrainsMono".to_string(), "Hack".to_string()];
    mono.extend(fallbacks.iter().cloned());
    mono.dedup();
    defs.families.insert(FontFamily::Monospace, mono);

    // UI: Inter, with the icon font right behind it so icons can sit inside ordinary text.
    // The icons live in the Private Use Area, like the Powerline and Nerd Font glyphs that
    // remote prompts print, so they stay out of the terminal and monospace families above.
    defs.font_data.insert("Inter".into(), font(INTER));
    defs.font_data
        .insert("Inter-Medium".into(), font(INTER_MEDIUM));
    defs.font_data
        .insert("Inter-SemiBold".into(), font(INTER_SEMIBOLD));
    let mut phosphor = FontData::from_static(PHOSPHOR);
    // Icons sit a little high next to Inter's x-height.
    phosphor.tweak.y_offset_factor = 0.08;
    defs.font_data.insert("Phosphor".into(), Arc::new(phosphor));
    let ui_family = |first: &str| {
        let mut family = vec![first.to_string(), "Phosphor".to_string()];
        family.extend(fallbacks.iter().cloned());
        family
    };
    defs.families
        .insert(FontFamily::Proportional, ui_family("Inter"));
    defs.families
        .insert(FontFamily::Name(MEDIUM.into()), ui_family("Inter-Medium"));
    defs.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        ui_family("Inter-SemiBold"),
    );
    defs.families.insert(
        FontFamily::Name(ICONS.into()),
        vec!["Phosphor".into(), "Inter".into()],
    );
    let mut face_mask = 1u8; // regular always resolves (chosen or Hack fallback)
    for (i, f) in faces.iter().enumerate() {
        if f.is_some() {
            face_mask |= 1u8 << i;
        }
    }
    (defs, face_mask)
}

/// Whether a terminal style has a real face (otherwise the renderer emulates it).
pub fn has_real_face(choice: &FontChoice, bold: bool, italic: bool) -> bool {
    let idx = match (bold, italic) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    };
    let mask = LOADED_FACES.load(Ordering::Relaxed);
    if mask != 0 {
        return (mask & (1u8 << idx)) != 0;
    }
    preset(choice)
        .is_some_and(|p| p.name == JETBRAINS.name || (p.path.is_some() && p.faces[idx].is_some()))
}

/// Installs the fonts. Returns false when a font file the user picked (or a non-builtin preset)
/// could not be used (the built-in Hack font is used instead).
pub fn apply(ctx: &egui::Context, choice: &FontChoice, load_cjk: bool) -> bool {
    let (defs, face_mask) = definitions_with_faces(choice, load_cjk);
    let is_builtin = preset(choice).is_some_and(|p| p.path.is_none());
    let ok = is_builtin || defs.font_data.contains_key(TERM_REGULAR);
    LOADED_FACES.store(face_mask, Ordering::Relaxed);
    FONT_GEN.fetch_add(1, Ordering::Relaxed);
    ctx.set_fonts(defs);
    ok
}

/// The saved font, or the platform's usual one when nothing is chosen yet.
pub fn effective_choice(choice: &FontChoice) -> FontChoice {
    if choice.name.is_empty() && choice.path.is_none() {
        FontChoice {
            name: DEFAULT_FONT.into(),
            path: None,
            index: 0,
        }
    } else {
        choice.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings offers the presets whose files are on this computer: the platform's usual font
    /// (Menlo, Consolas; the built-in Hack on Linux), never one whose file is missing.
    #[test]
    fn presets_are_offered_when_their_file_exists() {
        let default = PRESETS.iter().find(|p| p.name == DEFAULT_FONT).unwrap();
        assert!(is_available(default), "{}", default.name);
        assert!(is_available(&BUILT_IN));
        for file in ["ternaryshell-no-such-font.ttf", "/no/such/dir/font.ttf"] {
            let missing = FontPreset {
                name: "Missing",
                path: Some(file),
                faces: [None, None, None, None],
            };
            assert!(!is_available(&missing), "{file}");
        }
    }
}
