//! Design tokens (colors, type, spacing, radii), the egui style built from them, and the form
//! helpers every dialog uses. Shared components (buttons, menus, badges, cards) are in `kit.rs`.
//!
//! The tokens are those of the design book in `design/` (`design/css/tokens.css`): two themes,
//! Graphite (dark) and Paper (light): neutral greys and one quiet blue, as in TernaryCode. Every screen draws with [`Palette`] and the type helpers
//! [`medium`], [`semibold`], [`mono`] and [`icon_font`].

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Shadow, Stroke, Theme,
    ThemePreference, Ui, vec2,
};

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::fonts;
use crate::bookmarks::LabelColor;
use crate::settings::{Accent, Density, ThemeMode};

/// Colors of one theme.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    /// Content: the canvas card around the terminal, file lists, the start page.
    pub canvas: Color32,
    /// Window frame: sidebar and the title row.
    pub chrome: Color32,
    /// Raised surfaces on the canvas: in-pane connection cards.
    pub surface: Color32,
    /// Dialogs.
    pub dialog: Color32,
    /// Cards inside dialogs and pages.
    pub card: Color32,
    /// Menus, popovers, toasts, tooltips.
    pub raised: Color32,
    /// The command palette.
    pub sheet: Color32,
    /// Inputs (inset in the dark theme).
    pub field: Color32,
    pub field_border: Color32,
    /// Hovered rows and quiet button fills (translucent, works on any surface).
    pub hover: Color32,
    /// Pressed state and quiet tracks (translucent).
    pub subtle: Color32,
    /// Hairlines.
    pub border: Color32,
    pub border_strong: Color32,
    /// The faintest dividers (inside lists).
    pub line_soft: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    /// Disabled text, placeholder icons.
    pub text_ghost: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_press: Color32,
    /// Text on an accent fill.
    pub on_accent: Color32,
    /// Background of selected rows and "on" toggles (a neutral grey, as in TernaryCode).
    pub accent_soft: Color32,
    /// The quietest accent wash (drop targets, the active editor line).
    pub accent_softer: Color32,
    /// A soft accent fill (accent badges and icon tiles, the update pill, "on" toolbar toggles).
    pub accent_tint: Color32,
    /// The halo around a focused field.
    pub accent_halo: Color32,
    /// Accent borders (focus rings, drop targets).
    pub accent_line: Color32,
    /// Accent-colored text (readable on the canvas and on `accent_soft`).
    pub accent_text: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
    /// Opaque filled danger button background (WCAG AA with white text).
    pub err_fill: Color32,
    pub err_hover: Color32,
    pub info: Color32,
    /// Browsing as root (sudo) in SFTP.
    pub root: Color32,
    /// Folder icons.
    pub folder: Color32,
    /// Behind dialogs.
    pub scrim: Color32,
    /// The ring around the canvas card and the active tab.
    pub card_ring: Color32,
    /// The active tab's fill.
    pub tab_on: Color32,
    /// A secondary button at rest.
    pub button: Color32,
    pub button_border: Color32,
    /// The raised segment of a segmented control.
    pub segment_on: Color32,
    /// The side navigation of two-pane dialogs.
    pub nav: Color32,
    /// The footer band of dialogs.
    pub footer: Color32,
    /// Wells: code, fingerprints, previews.
    pub well: Color32,
}

/// Unmultiplied RGBA, for the translucent tokens.
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, (a * 255.0) as u8)
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

// Unified Ternary v4 ("Obsidian" dark & "Paper" light): calibrated micro-zinc neutrals,
// translucent hairlines, WCAG 2.0 AA verified text/status, and TernaryShell's Indigo signature
// accent (switchable in Settings).
const DARK: Palette = Palette {
    dark: true,
    // The background of the "TernaryShell Dark" terminal scheme, so the terminal blends in.
    canvas: rgb(0x151517),
    chrome: rgb(0x0d0d0f),
    surface: rgb(0x1d1d20),
    dialog: rgb(0x19191c),
    card: rgb(0x1d1d20),
    raised: rgb(0x232327),
    sheet: rgb(0x19191c),
    field: rgb(0x101012),
    field_border: rgba(255, 255, 255, 0.12),
    hover: rgba(255, 255, 255, 0.045),
    subtle: rgba(255, 255, 255, 0.08),
    border: rgba(255, 255, 255, 0.08),
    border_strong: rgba(255, 255, 255, 0.14),
    line_soft: rgba(255, 255, 255, 0.05),
    text: rgb(0xededf0),
    text_muted: rgb(0xa1a1aa),
    text_faint: rgb(0x75757e),
    text_ghost: rgb(0x4b4b53),
    accent: rgb(0x5b52dc),
    accent_hover: rgb(0x6b63e6),
    accent_press: rgb(0x4d44c9),
    on_accent: Color32::WHITE,
    accent_soft: rgba(255, 255, 255, 0.075),
    accent_softer: rgba(91, 82, 220, 0.09),
    accent_tint: rgba(91, 82, 220, 0.17),
    accent_halo: rgba(91, 82, 220, 0.22),
    accent_line: rgba(168, 162, 255, 0.45),
    accent_text: rgb(0xa8a2ff),
    ok: rgb(0x4dc48c),
    warn: rgb(0xf0ad42),
    err: rgb(0xf06560),
    err_fill: rgb(0xc93530),
    err_hover: rgb(0xb32c27),
    info: rgb(0x6aa5ff),
    root: rgb(0xf0894a),
    folder: rgb(0xd6a354),
    scrim: rgba(0, 0, 0, 0.58),
    card_ring: rgba(255, 255, 255, 0.08),
    tab_on: rgb(0x151517),
    button: rgba(255, 255, 255, 0.055),
    button_border: rgba(255, 255, 255, 0.10),
    segment_on: rgb(0x28282d),
    nav: rgb(0x111114),
    footer: rgba(255, 255, 255, 0.015),
    well: rgb(0x111114),
};

const LIGHT: Palette = Palette {
    dark: false,
    // The background of the "TernaryShell Light" terminal scheme.
    canvas: rgb(0xffffff),
    chrome: rgb(0xececef),
    surface: rgb(0xffffff),
    dialog: rgb(0xffffff),
    card: rgb(0xffffff),
    raised: rgb(0xffffff),
    sheet: rgb(0xffffff),
    field: rgb(0xffffff),
    field_border: rgba(0, 0, 0, 0.16),
    hover: rgba(0, 0, 0, 0.045),
    subtle: rgba(0, 0, 0, 0.08),
    border: rgba(0, 0, 0, 0.09),
    border_strong: rgba(0, 0, 0, 0.16),
    line_soft: rgba(0, 0, 0, 0.055),
    text: rgb(0x17171a),
    text_muted: rgb(0x55555f),
    text_faint: rgb(0x767680),
    text_ghost: rgb(0xababb4),
    accent: rgb(0x4b42c4),
    accent_hover: rgb(0x3f37b0),
    accent_press: rgb(0x352e9c),
    on_accent: Color32::WHITE,
    accent_soft: rgba(0, 0, 0, 0.065),
    accent_softer: rgba(75, 66, 196, 0.06),
    accent_tint: rgba(75, 66, 196, 0.10),
    accent_halo: rgba(75, 66, 196, 0.16),
    accent_line: rgba(75, 66, 196, 0.45),
    accent_text: rgb(0x433ab5),
    ok: rgb(0x117040),
    warn: rgb(0x8f5300),
    err: rgb(0xbe2722),
    err_fill: rgb(0xbe2722),
    err_hover: rgb(0xa31f1b),
    info: rgb(0x1f5fbf),
    root: rgb(0xa84705),
    folder: rgb(0x9c6414),
    scrim: rgba(18, 18, 22, 0.28),
    card_ring: rgba(0, 0, 0, 0.09),
    tab_on: rgb(0xffffff),
    button: rgb(0xffffff),
    button_border: rgba(0, 0, 0, 0.16),
    segment_on: rgb(0xffffff),
    nav: rgb(0xf5f5f7),
    footer: rgb(0xf5f5f7),
    well: rgb(0xf5f5f7),
};

/// The accent colours of one option: fill, hover, press, text, tint (badges, halos), wash
/// (drop targets), line (rings).
struct AccentSet {
    accent: Color32,
    hover: Color32,
    press: Color32,
    text: Color32,
    tint: Color32,
    wash: Color32,
    line: Color32,
}

#[allow(clippy::too_many_arguments)]
const fn accent_set(
    accent: u32,
    hover: u32,
    press: u32,
    text: u32,
    (r, g, b): (u8, u8, u8),
    tint: f32,
    wash: f32,
    line: f32,
) -> AccentSet {
    AccentSet {
        accent: rgb(accent),
        hover: rgb(hover),
        press: rgb(press),
        text: rgb(text),
        tint: rgba(r, g, b, tint),
        wash: rgba(r, g, b, wash),
        line: rgba(r, g, b, line),
    }
}

/// Accents other than Indigo (Indigo is what `DARK` and `LIGHT` hold): `[dark, light]`.
fn accent_sets(accent: Accent) -> Option<[AccentSet; 2]> {
    Some(match accent {
        Accent::Indigo => return None,
        Accent::Blue => [
            accent_set(
                0x2e6be6,
                0x3a78f0,
                0x255cc9,
                0x85afff,
                (46, 107, 230),
                0.16,
                0.08,
                0.45,
            ),
            accent_set(
                0x1f5fbf,
                0x1a52a8,
                0x164691,
                0x1a56b3,
                (31, 95, 191),
                0.10,
                0.06,
                0.45,
            ),
        ],
        Accent::Green => [
            accent_set(
                0x0f8572,
                0x139983,
                0x0c7362,
                0x52d1ba,
                (15, 133, 114),
                0.18,
                0.08,
                0.45,
            ),
            accent_set(
                0x0f7a68,
                0x0c6657,
                0x095448,
                0x0b6a5a,
                (15, 122, 104),
                0.10,
                0.06,
                0.45,
            ),
        ],
        Accent::Orange => [
            accent_set(
                0xcf4422,
                0xe05330,
                0xba3a1b,
                0xff967d,
                (207, 68, 34),
                0.17,
                0.08,
                0.45,
            ),
            accent_set(
                0xc73d1d,
                0xb03417,
                0x992c12,
                0xb33417,
                (199, 61, 29),
                0.10,
                0.06,
                0.45,
            ),
        ],
        Accent::Pink => [
            accent_set(
                0xc43c82,
                0xd64c93,
                0xae3171,
                0xf49cc8,
                (215, 85, 150),
                0.16,
                0.08,
                0.45,
            ),
            accent_set(
                0xb0326a,
                0x9c2a5d,
                0x872350,
                0x9e2b5e,
                (176, 50, 106),
                0.10,
                0.05,
                0.45,
            ),
        ],
        Accent::Graphite => [
            accent_set(
                0x5a5a64,
                0x6b6b76,
                0x4b4b54,
                0xd4d4d8,
                (255, 255, 255),
                0.10,
                0.05,
                0.35,
            ),
            accent_set(
                0x27272a,
                0x18181b,
                0x09090b,
                0x27272a,
                (0, 0, 0),
                0.08,
                0.04,
                0.35,
            ),
        ],
    })
}

static ACCENT: AtomicU8 = AtomicU8::new(0);
static COMPACT: AtomicBool = AtomicBool::new(false);

/// Applies the accent and density chosen in Settings to every palette and row from now on.
pub fn set_preferences(accent: Accent, density: Density) {
    let i = Accent::ALL.iter().position(|a| *a == accent).unwrap_or(0);
    ACCENT.store(i as u8, Ordering::Relaxed);
    COMPACT.store(density == Density::Compact, Ordering::Relaxed);
}

fn current_accent() -> Accent {
    Accent::ALL
        .get(ACCENT.load(Ordering::Relaxed) as usize)
        .copied()
        .unwrap_or_default()
}

/// Height of a row in the sidebar and the file lists (Settings → Appearance → Density).
pub fn row_h() -> f32 {
    if COMPACT.load(Ordering::Relaxed) {
        24.0
    } else {
        28.0
    }
}

/// Whether rows are compact.
pub fn compact() -> bool {
    COMPACT.load(Ordering::Relaxed)
}

/// The colour of an environment label (`PROD`, `STG`…) in a theme.
pub fn label_color(color: LabelColor, dark: bool) -> Color32 {
    let (d, l) = match color {
        LabelColor::Red => (0xf06b63, 0xd9443b),
        LabelColor::Orange => (0xf0894a, 0xd66a1f),
        LabelColor::Yellow => (0xe2b445, 0xb88a12),
        LabelColor::Green => (0x4fbf85, 0x1f8a52),
        LabelColor::Teal => (0x3fb5c2, 0x12879a),
        LabelColor::Blue => (0x5c9cf5, 0x2a6fd6),
        LabelColor::Violet => (0xa48bf0, 0x7a5ad8),
        LabelColor::Pink => (0xe27bb2, 0xc4448b),
        LabelColor::Grey => (0x8b8b8b, 0x7d7d7d),
    };
    rgb(if dark { d } else { l })
}

/// The tint behind a status color (badges, callouts, icon tiles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Ok,
    Warn,
    Err,
    Info,
    Root,
}

impl Palette {
    /// Barely visible fill for hovered list rows.
    pub fn faint_row(&self) -> Color32 {
        self.hover
    }

    /// The foreground color of a tone.
    pub fn tone(&self, tone: Tone) -> Color32 {
        match tone {
            Tone::Neutral => self.text_muted,
            Tone::Accent => self.accent_text,
            Tone::Ok => self.ok,
            Tone::Warn => self.warn,
            Tone::Err => self.err,
            Tone::Info => self.info,
            Tone::Root => self.root,
        }
    }

    /// The soft fill behind a tone.
    pub fn tone_soft(&self, tone: Tone) -> Color32 {
        let d = self.dark;
        match tone {
            Tone::Neutral => self.hover,
            Tone::Accent => self.accent_tint,
            Tone::Ok => soft(self.ok, if d { 0.14 } else { 0.10 }),
            Tone::Warn => {
                if d {
                    soft(self.warn, 0.14)
                } else {
                    rgba(214, 140, 0, 0.12)
                }
            }
            Tone::Err => soft(self.err, if d { 0.14 } else { 0.09 }),
            Tone::Info => soft(self.info, if d { 0.14 } else { 0.09 }),
            Tone::Root => {
                if d {
                    soft(self.root, 0.13)
                } else {
                    rgba(230, 120, 20, 0.10)
                }
            }
        }
    }
}

pub fn palette(dark: bool) -> Palette {
    let mut p = if dark { DARK } else { LIGHT };
    if let Some(sets) = accent_sets(current_accent()) {
        let a = &sets[if dark { 0 } else { 1 }];
        p.accent = a.accent;
        p.accent_hover = a.hover;
        p.accent_press = a.press;
        p.accent_text = a.text;
        p.accent_tint = a.tint;
        p.accent_halo = a.tint;
        p.accent_softer = a.wash;
        p.accent_line = a.line;
        if !dark && matches!(current_accent(), Accent::Graphite) {
            p.info = a.accent;
        }
    }
    p
}

/// The palette of the theme `ui` is drawn with.
pub fn pal(ui: &Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

/// A color at `alpha` (0–1) opacity, for soft fills behind colored text and icons.
pub fn soft(c: Color32, alpha: f32) -> Color32 {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, ((alpha.clamp(0.0, 1.0) * a as f32).round()) as u8)
}

// ---------------------------------------------------------------------------------------------
// Type
// ---------------------------------------------------------------------------------------------

/// Body text: rows, buttons, inputs.
pub const BODY: f32 = 13.0;
/// Small text: hints, secondary lines, table details.
pub const SMALL: f32 = 12.0;
/// Extra small: section labels, the status strip.
pub const XS: f32 = 11.0;
/// Micro: count pills.
pub const MICRO: f32 = 10.5;
/// Dialog titles.
pub const TITLE: f32 = 15.0;
/// UI monospace: addresses, paths, fingerprints.
pub const MONO: f32 = 12.0;

/// Inter Medium: labels that need a little weight (tabs, buttons, row titles).
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(fonts::MEDIUM.into()))
}

/// Inter SemiBold: titles and headings.
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(fonts::SEMIBOLD.into()))
}

/// Inter Regular.
pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

/// The monospace font (the terminal's) for addresses, paths and commands.
pub fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

/// The icon font alone (icons also render inside ordinary text).
pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(fonts::ICONS.into()))
}

/// An uppercase section label (`CONNECTIONS`): 11 px semibold, faint, tracked out.
/// A section label (`Theme`): sentence case, 12 px semibold (Redesign 3.1: no tracked-out
/// capitals). Callers pass `text_muted`.
pub fn section_label(text: &str, color: Color32) -> egui::RichText {
    egui::RichText::new(text).font(semibold(SMALL)).color(color)
}

// ---------------------------------------------------------------------------------------------
// Sizes
// ---------------------------------------------------------------------------------------------

/// Buttons, inputs, rows.
pub const RADIUS: u8 = 6;
/// Menus, popovers, tabs.
pub const RADIUS_MD: u8 = 8;
/// The canvas card, cards.
pub const RADIUS_LG: u8 = 12;
/// Dialogs.
pub const RADIUS_XL: u8 = 14;

/// Height of a form control (text field, button, select).
pub const CONTROL_H: f32 = 30.0;
/// Height of a toolbar control (small buttons, icon buttons).
pub const TOOLBAR_H: f32 = 26.0;
/// Height of a large input (dialogs, the start page search).
pub const CONTROL_LG: f32 = 36.0;
/// Width of the label column in forms.
pub const LABEL_W: f32 = 140.0;

pub fn apply(ctx: &egui::Context, mode: ThemeMode) {
    ctx.set_theme(match mode {
        ThemeMode::System => ThemePreference::System,
        ThemeMode::Light => ThemePreference::Light,
        ThemeMode::Dark => ThemePreference::Dark,
    });
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            let dark = theme == Theme::Dark;
            let p = palette(dark);
            let s = &mut style.spacing;
            s.item_spacing = vec2(8.0, 6.0);
            s.button_padding = vec2(12.0, 4.0);
            s.interact_size = vec2(24.0, CONTROL_H);
            s.window_margin = Margin::same(20);
            s.menu_margin = Margin::same(5);
            s.indent = 16.0;
            s.icon_width = 16.0;
            s.icon_width_inner = 10.0;
            s.icon_spacing = 8.0;
            s.combo_height = 320.0;
            s.tooltip_width = 360.0;
            s.menu_width = 220.0;
            s.menu_spacing = 2.0;
            s.scroll = egui::style::ScrollStyle::floating();
            s.scroll.bar_width = 8.0;
            s.scroll.floating_width = 3.0;
            s.scroll.floating_allocated_width = 0.0;
            // Selectable labels sense clicks and drags and would steal them from the rows,
            // buttons and tables they sit in; text that should be copyable opts in explicitly.
            style.interaction.selectable_labels = false;

            let v = &mut style.visuals;
            v.text_options.subpixel_binning = false;
            v.text_cursor.blink = false; // a blinking caret would repaint twice a second
            v.text_cursor.stroke = Stroke::new(1.5, p.accent);
            v.panel_fill = p.chrome;
            v.window_fill = p.raised;
            v.extreme_bg_color = p.field;
            v.text_edit_bg_color = Some(p.field);
            v.faint_bg_color = p.hover;
            v.code_bg_color = p.hover;
            v.hyperlink_color = p.accent_text;
            v.warn_fg_color = p.warn;
            v.error_fg_color = p.err;
            v.override_text_color = None;
            v.weak_text_color = Some(p.text_muted);
            v.selection.bg_fill = soft(p.accent, if dark { 0.32 } else { 0.18 });
            // egui's ring around a focused text field (`Field` adds the soft halo).
            v.selection.stroke = Stroke::new(1.0, p.accent);
            v.window_corner_radius = CornerRadius::same(RADIUS_XL);
            v.menu_corner_radius = CornerRadius::same(10);
            v.window_stroke = Stroke::new(
                1.0,
                if dark {
                    rgba(255, 255, 255, 0.05)
                } else {
                    rgba(20, 20, 40, 0.07)
                },
            );
            v.window_shadow = Shadow {
                offset: [0, 28],
                blur: 70,
                spread: 0,
                color: Color32::from_black_alpha(if dark { 153 } else { 56 }),
            };
            v.popup_shadow = Shadow {
                offset: [0, 10],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(if dark { 115 } else { 36 }),
            };
            v.window_highlight_topmost = false;
            v.collapsing_header_frame = false;
            v.indent_has_left_vline = true;
            v.striped = false;
            v.slider_trailing_fill = true;
            v.handle_shape = egui::style::HandleShape::Circle;
            v.disabled_alpha = 0.45;

            let radius = CornerRadius::same(RADIUS);
            let w = &mut v.widgets;
            // Labels, separators, frames.
            w.noninteractive.bg_fill = p.surface;
            w.noninteractive.weak_bg_fill = p.surface;
            w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
            w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
            w.noninteractive.corner_radius = radius;
            // Inputs and selects at rest: the field look.
            w.inactive.bg_fill = p.subtle;
            w.inactive.weak_bg_fill = p.field;
            w.inactive.bg_stroke = Stroke::new(1.0, p.field_border);
            w.inactive.fg_stroke = Stroke::new(1.0, p.text);
            w.inactive.corner_radius = radius;
            w.inactive.expansion = 0.0;
            w.hovered.bg_fill = p.subtle;
            w.hovered.weak_bg_fill = p.field;
            w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
            w.hovered.fg_stroke = Stroke::new(1.0, p.text);
            w.hovered.corner_radius = radius;
            w.hovered.expansion = 0.0;
            w.active.bg_fill = p.subtle;
            w.active.weak_bg_fill = p.field;
            w.active.bg_stroke = Stroke::new(1.0, p.accent);
            w.active.fg_stroke = Stroke::new(1.0, p.text);
            w.active.corner_radius = radius;
            w.active.expansion = 0.0;
            w.open.bg_fill = p.subtle;
            w.open.weak_bg_fill = p.field;
            w.open.bg_stroke = Stroke::new(1.0, p.accent);
            w.open.fg_stroke = Stroke::new(1.0, p.text);
            w.open.corner_radius = radius;

            for (ts, font) in style.text_styles.iter_mut() {
                *font = match ts {
                    egui::TextStyle::Heading => semibold(TITLE),
                    egui::TextStyle::Small => FontId::proportional(SMALL),
                    egui::TextStyle::Monospace => FontId::monospace(MONO),
                    _ => FontId::proportional(BODY),
                };
            }
        });
    }
}

/// Warning text: amber, dark enough to read on a light background.
pub fn warn_text(v: &egui::Visuals) -> Color32 {
    palette(v.dark_mode).warn
}

/// The accent ring around a focused control.
pub fn focus_ring(v: &egui::Visuals) -> Stroke {
    Stroke::new(1.5, palette(v.dark_mode).accent)
}

/// Focus of an input: a 1 px accent border and a 3 px soft accent halo just outside it.
pub fn focus_halo(ui: &Ui, rect: egui::Rect, radius: u8) {
    let p = pal(ui);
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(3.0, p.accent_halo),
        egui::StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0, p.accent),
        egui::StrokeKind::Inside,
    );
}

/// An input in the error state: a red border and a soft red halo.
pub fn error_halo(ui: &Ui, rect: egui::Rect, radius: u8) {
    let p = pal(ui);
    let painter = ui.painter();
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(3.0, p.tone_soft(Tone::Err)),
        egui::StrokeKind::Outside,
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(radius),
        Stroke::new(1.0, p.err),
        egui::StrokeKind::Inside,
    );
}

/// The frame of a dialog: the dialog fill, 14 px corners, the dialog shadow.
pub fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    let dark = ctx.global_style().visuals.dark_mode;
    let p = palette(dark);
    let v = &ctx.global_style().visuals;
    egui::Frame::NONE
        .fill(p.dialog)
        .corner_radius(CornerRadius::same(RADIUS_XL))
        .shadow(v.window_shadow)
        .stroke(Stroke::new(
            1.0,
            if dark {
                rgba(255, 255, 255, 0.06)
            } else {
                rgba(20, 20, 40, 0.08)
            },
        ))
        .inner_margin(Margin::ZERO)
}

/// A modal dialog with the design's scrim and frame (content draws its own padding).
pub fn modal(ctx: &egui::Context, id: egui::Id) -> egui::Modal {
    let dark = ctx.global_style().visuals.dark_mode;
    egui::Modal::new(id)
        .backdrop_color(palette(dark).scrim)
        .frame(dialog_frame(ctx))
}

/// Roomier spacing for the content of a dialog or form.
pub fn form_spacing(ui: &mut Ui) {
    let s = ui.spacing_mut();
    s.interact_size.y = CONTROL_H;
    s.item_spacing = vec2(8.0, 8.0);
    s.button_padding = vec2(12.0, 5.0);
}

/// A one-line text field (30 px), as wide as there is room.
pub fn input(text: &mut dyn egui::TextBuffer) -> Field<'_> {
    Field {
        edit: egui::TextEdit::singleline(text)
            .margin(Margin::symmetric(10, 7))
            .desired_width(f32::INFINITY),
        error: false,
    }
}

/// A multi-line text field with the same padding.
pub fn input_multiline(text: &mut dyn egui::TextBuffer) -> Field<'_> {
    Field {
        edit: egui::TextEdit::multiline(text)
            .margin(Margin::symmetric(10, 8))
            .desired_width(f32::INFINITY),
        error: false,
    }
}

/// A text field with the soft focus halo (or the error look).
pub struct Field<'t> {
    pub edit: egui::TextEdit<'t>,
    pub error: bool,
}

impl Field<'_> {
    pub fn hint_text(self, text: impl Into<egui::WidgetText>) -> Self {
        Self {
            edit: self.edit.hint_text(text),
            ..self
        }
    }

    pub fn password(self, password: bool) -> Self {
        Self {
            edit: self.edit.password(password),
            ..self
        }
    }

    pub fn desired_width(self, width: f32) -> Self {
        Self {
            edit: self.edit.desired_width(width),
            ..self
        }
    }

    pub fn desired_rows(self, rows: usize) -> Self {
        Self {
            edit: self.edit.desired_rows(rows),
            ..self
        }
    }

    pub fn font(self, font: impl Into<egui::FontSelection>) -> Self {
        Self {
            edit: self.edit.font(font),
            ..self
        }
    }

    pub fn id(self, id: egui::Id) -> Self {
        Self {
            edit: self.edit.id(id),
            ..self
        }
    }

    pub fn margin(self, margin: impl Into<Margin>) -> Self {
        Self {
            edit: self.edit.margin(margin),
            ..self
        }
    }

    /// Draws the field in the error state (red border and halo).
    pub fn error(self, error: bool) -> Self {
        Self { error, ..self }
    }

    pub fn mono(self) -> Self {
        Self {
            edit: self.edit.font(mono(MONO)),
            ..self
        }
    }
}

impl egui::Widget for Field<'_> {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let response = ui.add(self.edit);
        if self.error {
            error_halo(ui, response.rect, RADIUS);
        } else if response.has_focus() {
            focus_halo(ui, response.rect, RADIUS);
        }
        response
    }
}

/// The main button of a dialog (filled accent).
pub fn primary(text: impl Into<String>) -> Primary {
    Primary(text.into())
}

/// See [`primary`]: drawn by `kit::button_ex` once the theme is known.
pub struct Primary(String);

impl egui::Widget for Primary {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        super::kit::button_ex(ui, super::kit::Kind::Primary, None, &self.0, false)
    }
}

/// A section label in a form (`SERVER`), with room above.
pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(section_label(title, pal(ui).text_muted));
    ui.add_space(2.0);
}

/// A form row: the label on the left (140 px, aligned with the first control), the controls on
/// the right with 12 px between.
pub fn row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.allocate_ui_with_layout(
            vec2(LABEL_W, CONTROL_H),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.set_min_size(vec2(LABEL_W, CONTROL_H));
                ui.label(
                    egui::RichText::new(label)
                        .font(medium(SMALL))
                        .color(pal(ui).text_muted),
                );
            },
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
    })
    .inner
}

/// A segmented control: one of `options`, the chosen one raised on a quiet track. Returns each
/// segment's response.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
) -> Vec<egui::Response> {
    super::kit::segmented(ui, value, options, false)
}

/// Lays widgets out from the right, in a band one control high (a plain right-to-left layout
/// in a vertical one would take all the height there is). The last widget added can fill
/// what is left with `ui.available_width()`.
pub fn right_to_left<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), CONTROL_H),
        Layout::right_to_left(Align::Center),
        add,
    )
    .inner
}

/// A checkbox: a 16 px box, accent with a white check when on.
pub fn checkbox(
    ui: &mut Ui,
    checked: &mut bool,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    const BOX: f32 = 16.0;
    const GAP: f32 = 8.0;
    let text: egui::WidgetText = text.into();
    let empty = text.is_empty();
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        (ui.available_width() - BOX - GAP).max(40.0),
        egui::TextStyle::Body,
    );
    // As tall as the other controls in the row, so labels beside it line up.
    let line = ui.spacing().interact_size.y.clamp(20.0, CONTROL_H);
    let size = if empty {
        vec2(BOX, line)
    } else {
        vec2(
            BOX + GAP + galley.size().x,
            galley.size().y.max(BOX).max(line),
        )
    };
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    let on = *checked;
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            on,
            galley.text(),
        )
    });
    if ui.is_rect_visible(rect) {
        let p = pal(ui);
        let box_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + BOX / 2.0, rect.top() + line / 2.0),
            vec2(BOX, BOX),
        );
        let painter = ui.painter();
        if on {
            let fill = if response.hovered() {
                p.accent_hover
            } else {
                p.accent
            };
            painter.rect_filled(box_rect, 4.0, fill);
            painter.text(
                box_rect.center(),
                egui::Align2::CENTER_CENTER,
                super::icons::CHECK,
                icon_font(11.0),
                p.on_accent,
            );
        } else {
            painter.rect(
                box_rect,
                4.0,
                p.field,
                Stroke::new(
                    1.0,
                    if response.hovered() {
                        p.text_faint
                    } else {
                        p.border_strong
                    },
                ),
                egui::StrokeKind::Inside,
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                box_rect.expand(2.0),
                5.0,
                focus_ring(ui.visuals()),
                egui::StrokeKind::Outside,
            );
        }
        if !empty {
            let text_pos = egui::pos2(
                box_rect.right() + GAP,
                box_rect.center().y
                    - galley.rows.first().map_or(galley.size().y, |r| r.height()) / 2.0,
            );
            let text_color = if ui.is_enabled() {
                p.text
            } else {
                p.text_faint
            };
            painter.galley(text_pos, galley, text_color);
        }
    }
    response
}

/// How wide `text` is in the body font.
pub fn text_width(ui: &Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE)
        .size()
        .x
}

/// A small explanation under a control (12 px, faint).
pub fn hint(ui: &mut Ui, text: impl Into<String>) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.into())
                .font(regular(SMALL))
                .color(pal(ui).text_faint),
        )
        .wrap(),
    );
}

/// A small explanation in a tone (errors under fields).
pub fn hint_tone(ui: &mut Ui, text: impl Into<String>, tone: Tone) {
    let color = pal(ui).tone(tone);
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.into())
                .font(regular(SMALL))
                .color(color),
        )
        .wrap(),
    );
}

/// A ring that fills clockwise with `fraction`: the 14 px progress ring of tabs and strips.
pub fn progress_ring(
    painter: &egui::Painter,
    center: egui::Pos2,
    radius: f32,
    fraction: f32,
    dark: bool,
) {
    let p = palette(dark);
    let width = (radius * 0.36).clamp(2.0, 3.0);
    let r = radius - width / 2.0;
    painter.circle_stroke(center, r, Stroke::new(width, p.border_strong));
    let fraction = fraction.clamp(0.0, 1.0);
    let steps = (fraction * 40.0).ceil() as usize;
    if steps > 0 {
        let points: Vec<egui::Pos2> = (0..=steps)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2
                    + std::f32::consts::TAU * fraction * i as f32 / steps as f32;
                center + vec2(a.cos(), a.sin()) * r
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(width, p.accent)));
    }
}
