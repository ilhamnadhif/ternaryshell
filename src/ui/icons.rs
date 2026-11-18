//! Icons: the bundled Phosphor glyphs (`icon_codes.rs`, used inside ordinary text such as
//! `format!("{} Refresh", icons::ARROWS_CLOCKWISE)`), which file gets which icon, and the app
//! logo drawn with the painter.

use eframe::egui::{self, Color32, Painter, Rect, Stroke, Ui, vec2};

pub use super::icon_codes::*;
use super::style::{self, Palette};

/// What kind of file an entry is, for its icon and colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Folder,
    Link,
    /// Configuration: YAML, TOML, INI, nginx/systemd files.
    Config,
    /// Source code and JSON.
    Code,
    /// Scripts and executables.
    Script,
    Text,
    Image,
    Archive,
    Pdf,
    /// Keys, certificates and `.env` files.
    Secret,
    Other,
}

impl FileKind {
    /// The kind of `name` (`exec`: the file has an executable bit).
    pub fn of(name: &str, is_dir: bool, is_link: bool, exec: bool) -> FileKind {
        if is_dir {
            return FileKind::Folder;
        }
        if is_link {
            return FileKind::Link;
        }
        let lower = name.to_ascii_lowercase();
        let ext = lower
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map(|(_, e)| e.to_string())
            .unwrap_or_default();
        if lower == ".env"
            || lower.starts_with(".env.")
            || lower.starts_with("id_")
            || matches!(
                ext.as_str(),
                "pem" | "key" | "crt" | "cer" | "p12" | "pfx" | "gpg" | "asc"
            )
        {
            return FileKind::Secret;
        }
        match ext.as_str() {
            "yml" | "yaml" | "toml" | "ini" | "conf" | "cfg" | "properties" | "service"
            | "timer" | "socket" | "xml" | "plist" | "env" => FileKind::Config,
            "rs" | "go" | "py" | "js" | "mjs" | "cjs" | "ts" | "jsx" | "tsx" | "c" | "h" | "cc"
            | "cpp" | "hpp" | "java" | "kt" | "rb" | "php" | "lua" | "swift" | "json" | "html"
            | "css" | "scss" | "sql" | "vue" | "svelte" => FileKind::Code,
            "sh" | "bash" | "zsh" | "fish" | "command" | "exe" | "bin" | "run" => FileKind::Script,
            "zip" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "tar" | "7z" | "rar" | "deb" | "rpm"
            | "jar" | "war" | "dmg" | "iso" => FileKind::Archive,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "ico" | "bmp" | "tiff" | "heic" => {
                FileKind::Image
            }
            "pdf" => FileKind::Pdf,
            "txt" | "md" | "log" | "csv" | "rst" | "out" | "lock" | "types" => FileKind::Text,
            _ if exec => FileKind::Script,
            _ if matches!(lower.as_str(), "dockerfile" | "makefile" | "caddyfile") => {
                FileKind::Config
            }
            _ => FileKind::Text,
        }
    }

    /// The icon and its colour (the label colours, so kinds read at a glance).
    pub fn icon(self, p: &Palette) -> (&'static str, Color32) {
        use crate::bookmarks::LabelColor as L;
        let c = |l: L| style::label_color(l, p.dark);
        match self {
            FileKind::Folder => (FOLDER_SIMPLE, p.folder),
            FileKind::Link => (LINK_SIMPLE, c(L::Teal)),
            FileKind::Config => (FILE_CODE, c(L::Violet)),
            FileKind::Code => (FILE_CODE, c(L::Blue)),
            FileKind::Script => (FILE_CODE, c(L::Green)),
            FileKind::Text => (FILE_TEXT, p.text_faint),
            FileKind::Image => (FILE_IMAGE, c(L::Pink)),
            FileKind::Archive => (FILE_ZIP, c(L::Yellow)),
            FileKind::Pdf => (FILE_PDF, c(L::Red)),
            FileKind::Secret => (FILE_LOCK, p.text_faint),
            FileKind::Other => (FILE, p.text_faint),
        }
    }
}

/// The icon and its color for an entry of a file list.
pub fn file_icon(name: &str, is_dir: bool, is_link: bool, p: &Palette) -> (&'static str, Color32) {
    FileKind::of(name, is_dir, is_link, false).icon(p)
}

/// Paints a file kind's icon centred on `center` (`size` px). Folders are drawn filled, as
/// Phosphor's filled folder (the bundled font has the outlined icons only).
pub fn paint_file_icon(
    painter: &Painter,
    center: egui::Pos2,
    size: f32,
    kind: FileKind,
    p: &Palette,
) {
    let (icon, color) = kind.icon(p);
    if kind == FileKind::Folder {
        paint_folder(painter, center, size, color);
        return;
    }
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        icon,
        style::icon_font(size),
        color,
    );
}

/// A filled folder (Phosphor's `folder-simple` fill) in a `size` px box.
pub fn paint_folder(painter: &Painter, center: egui::Pos2, size: f32, color: Color32) {
    let u = size / 16.0;
    let o = center - vec2(8.0 * u, 8.0 * u);
    let pt = |x: f32, y: f32| o + vec2(x * u, y * u);
    // The tab on the top left, then the body.
    painter.add(egui::Shape::convex_polygon(
        vec![
            pt(1.6, 3.6),
            pt(5.6, 3.6),
            pt(7.4, 5.2),
            pt(7.4, 6.0),
            pt(1.6, 6.0),
        ],
        color,
        Stroke::NONE,
    ));
    painter.rect_filled(
        Rect::from_min_max(pt(1.6, 4.4), pt(14.4, 13.0)),
        egui::CornerRadius::same((1.4 * u).round() as u8),
        color,
    );
}

/// A filled status circle with a glyph knocked out of it (`check-circle` / `warning-circle`
/// fill): `glyph` is drawn in `knock` (the surface colour) on a `color` disc.
pub fn paint_badge_circle(
    painter: &Painter,
    center: egui::Pos2,
    size: f32,
    glyph: &str,
    color: Color32,
    knock: Color32,
) {
    painter.circle_filled(center, size * 0.42, color);
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        glyph,
        style::icon_font(size * 0.62),
        knock,
    );
}

/// An icon followed by a (truncated) label, as one row element.
pub fn icon_label(ui: &mut Ui, icon: &str, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        ui.label(
            egui::RichText::new(icon)
                .font(style::icon_font(15.0))
                .color(color),
        );
        ui.add(egui::Label::new(text).truncate().selectable(false));
    });
}

// ---------------------------------------------------------------------------------------------
// The logo: the app icon (assets/icon.svg) without its margin, for the welcome screen.
// Coordinates are those of the 1024 px icon; its tile spans 72..952.
// ---------------------------------------------------------------------------------------------

/// The tile, and the ink of the lattice and the prompt.
pub const LOGO_TILE: Color32 = Color32::from_rgb(0x4b, 0x42, 0xc4);
const LOGO_INK: Color32 = Color32::from_rgb(0xff, 0xf6, 0xea);

/// Draws the logo filling `rect` (a square): the 3 x 3 lattice of the Ternary family, with
/// the prompt `>_` running through it.
pub fn paint_logo(painter: &Painter, rect: Rect) {
    let s = rect.width() / 880.0;
    let point = |c: usize, r: usize| {
        rect.min
            + vec2(
                (240.0 + 200.0 * c as f32) * s,
                (240.0 + 200.0 * r as f32) * s,
            )
    };
    painter.rect_filled(rect, 196.0 * s, LOGO_TILE);
    let stroke = Stroke::new(72.0 * s, LOGO_INK);
    painter.line_segment([point(0, 0), point(1, 1)], stroke);
    painter.line_segment([point(1, 1), point(0, 2)], stroke);
    painter.line_segment([point(1, 2), point(2, 2)], stroke);
    let on = [(0, 0), (1, 1), (0, 2), (1, 2), (2, 2)];
    for r in 0..3 {
        for c in 0..3 {
            if on.contains(&(c, r)) {
                painter.circle_filled(point(c, r), 52.0 * s, LOGO_INK);
            } else {
                painter.circle_filled(point(c, r), 30.0 * s, LOGO_INK.gamma_multiply(0.32));
            }
        }
    }
}
