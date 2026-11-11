//! Design tokens (colors, type, spacing, radii), the egui style built from them, and the form
//! helpers every dialog uses. Shared components (buttons, menus, badges, cards) are in `kit.rs`.
//!
//! The tokens are those of the design book in `design/` (`design/css/tokens.css`): two themes,
//! Graphite (dark) and Paper (light). Every screen draws with [`Palette`] and the type helpers
//! [`medium`], [`semibold`], [`mono`] and [`icon_font`].

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, Margin, Shadow, Stroke, Theme,
    ThemePreference, Ui, vec2,
};

use super::fonts;
use crate::settings::ThemeMode;

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
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_press: Color32,
    /// Text on an accent fill.
    pub on_accent: Color32,
    /// Background of selected rows and "on" toggles.
    pub accent_soft: Color32,
    /// Even quieter accent wash (drop targets, the active editor line).
    pub accent_softer: Color32,
    /// Accent borders (focus rings, drop targets).
    pub accent_line: Color32,
    /// Accent-colored text (readable on the canvas and on `accent_soft`).
    pub accent_text: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
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
}

/// Unmultiplied RGBA, for the translucent tokens.
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, (a * 255.0) as u8)
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const DARK: Palette = Palette {
    dark: true,
    canvas: rgb(0x16171c),
    chrome: rgb(0x0f1014),
    surface: rgb(0x1c1d23),
    dialog: rgb(0x1b1c22),
    card: rgb(0x1a1b21),
    raised: rgb(0x23242b),
    sheet: rgb(0x1f2026),
    field: rgb(0x121318),
    field_border: rgb(0x2c2d35),
    hover: rgba(255, 255, 255, 0.045),
    subtle: rgba(255, 255, 255, 0.085),
    border: rgb(0x25262d),
    border_strong: rgb(0x34353e),
    line_soft: rgba(255, 255, 255, 0.06),
    text: rgb(0xececf1),
    text_muted: rgb(0xa2a3ad),
    text_faint: rgb(0x6d6e78),
    accent: rgb(0x7b6ef6),
    accent_hover: rgb(0x8d82f8),
    accent_press: rgb(0x6a5cee),
    on_accent: Color32::WHITE,
    accent_soft: rgba(123, 110, 246, 0.16),
    accent_softer: rgba(123, 110, 246, 0.09),
    accent_line: rgba(123, 110, 246, 0.50),
    accent_text: rgb(0xb4abff),
    ok: rgb(0x3ccf8e),
    warn: rgb(0xf2b84b),
    err: rgb(0xf26d6d),
    info: rgb(0x64a8ff),
    root: rgb(0xf59e4c),
    folder: rgb(0x8d9df9),
    scrim: rgba(6, 7, 10, 0.58),
    card_ring: rgb(0x24252c),
    tab_on: rgb(0x1d1e24),
    button: rgb(0x24252c),
    button_border: rgb(0x33343c),
    segment_on: rgb(0x2a2b33),
    nav: rgb(0x17181d),
    footer: rgba(255, 255, 255, 0.015),
};

const LIGHT: Palette = Palette {
    dark: false,
    canvas: rgb(0xffffff),
    chrome: rgb(0xf1f1f4),
    surface: rgb(0xffffff),
    dialog: rgb(0xffffff),
    card: rgb(0xffffff),
    raised: rgb(0xffffff),
    sheet: rgb(0xffffff),
    field: rgb(0xffffff),
    field_border: rgb(0xd1d1da),
    hover: rgba(22, 22, 48, 0.045),
    subtle: rgba(22, 22, 48, 0.085),
    border: rgb(0xe4e4ea),
    border_strong: rgb(0xd1d1da),
    line_soft: rgba(22, 22, 48, 0.06),
    text: rgb(0x17171c),
    text_muted: rgb(0x5e5e6a),
    text_faint: rgb(0x9898a4),
    accent: rgb(0x5b4ee6),
    accent_hover: rgb(0x4c3fd7),
    accent_press: rgb(0x4234c7),
    on_accent: Color32::WHITE,
    accent_soft: rgba(91, 78, 230, 0.10),
    accent_softer: rgba(91, 78, 230, 0.06),
    accent_line: rgba(91, 78, 230, 0.42),
    accent_text: rgb(0x4a3dcc),
    ok: rgb(0x12895a),
    warn: rgb(0xad6a00),
    err: rgb(0xd33b3b),
    info: rgb(0x2463d9),
    root: rgb(0xc25e0a),
    folder: rgb(0x5b6ee8),
    scrim: rgba(24, 24, 36, 0.28),
    card_ring: rgb(0xe4e4ea),
    tab_on: rgb(0xffffff),
    button: rgb(0xffffff),
    button_border: rgb(0xd1d1da),
    segment_on: rgb(0xffffff),
    nav: rgb(0xf1f1f4),
    footer: rgba(91, 78, 230, 0.06),
};

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
            Tone::Accent => self.accent_soft,
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
    if dark { DARK } else { LIGHT }
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
pub const MONO: f32 = 12.5;

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
pub fn section_label(text: &str, color: Color32) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .font(semibold(XS))
        .color(color)
        .extra_letter_spacing(0.66)
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
            v.menu_corner_radius = CornerRadius::same(RADIUS_MD);
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
        Stroke::new(3.0, p.accent_soft),
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
    ui.label(section_label(title, pal(ui).text_faint));
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
