//! Shared components drawn with the design tokens of `style.rs` (the "Components" page of the
//! design book): buttons, icon buttons, segmented controls, switches, badges, chips, key caps,
//! menus, dialog headers and footers, cards, callouts, empty states and search fields. Screens
//! use these instead of styling egui widgets one by one, so every view looks the same.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2,
};

use super::icons;
use super::style::{self, Palette, RADIUS, Tone};

/// Icon followed by a label, for egui's own buttons.
pub fn with_icon(icon: &str, text: &str) -> String {
    format!("{icon}  {text}")
}

// ---------------------------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Filled accent: the one main action of a view.
    Primary,
    /// Outlined surface: ordinary actions.
    Secondary,
    /// No frame until hovered: toolbars and quiet actions.
    Ghost,
    /// Filled red: the confirming button of a destructive dialog.
    Danger,
    /// Red text on a soft red fill: destructive actions inside forms.
    DangerQuiet,
}

/// A button with an optional leading icon. `small` makes it 26 px high (toolbar) instead of 30.
pub fn button_ex(ui: &mut Ui, kind: Kind, icon: Option<&str>, text: &str, small: bool) -> Response {
    button_full(ui, kind, icon, text, small, false)
}

/// [`button_ex`] that can be shown "on" (an accent-tinted toggle, e.g. SFTP).
pub fn button_full(
    ui: &mut Ui,
    kind: Kind,
    icon: Option<&str>,
    text: &str,
    small: bool,
    on: bool,
) -> Response {
    button_kbd(ui, kind, icon, text, None, small, on)
}

/// A button with a trailing key cap (`Save ⌘S`).
pub fn button_kbd(
    ui: &mut Ui,
    kind: Kind,
    icon: Option<&str>,
    text: &str,
    keys: Option<&str>,
    small: bool,
    on: bool,
) -> Response {
    let p = style::pal(ui);
    let size = if small { style::SMALL } else { style::BODY };
    let height = if small {
        style::TOOLBAR_H
    } else {
        style::CONTROL_H
    };
    let pad = if small { 9.0 } else { 12.0 };
    let gap = if small { 5.0 } else { 6.0 };
    let enabled = ui.is_enabled();
    let text_color = match kind {
        Kind::Primary | Kind::Danger => p.on_accent,
        Kind::DangerQuiet => p.err,
        _ if on => p.accent_text,
        Kind::Ghost => p.text_muted,
        Kind::Secondary => p.text,
    };
    let galley = (!text.is_empty()).then(|| {
        ui.painter()
            .layout_no_wrap(text.to_string(), style::medium(size), Color32::PLACEHOLDER)
    });
    let icon_galley = icon.map(|i| {
        ui.painter().layout_no_wrap(
            i.to_string(),
            style::icon_font(if small { 14.0 } else { 15.0 }),
            Color32::PLACEHOLDER,
        )
    });
    let kbd_galley = keys.map(|k| {
        ui.painter().layout_no_wrap(
            k.to_string(),
            style::regular(style::XS),
            Color32::PLACEHOLDER,
        )
    });
    let kbd_w = kbd_galley
        .as_ref()
        .map_or(0.0, |g| (g.size().x + 10.0).max(20.0));
    let parts = [
        icon_galley.as_ref().map(|g| g.size().x),
        galley.as_ref().map(|g| g.size().x),
        kbd_galley.as_ref().map(|_| kbd_w),
    ];
    let content_w = parts.iter().flatten().sum::<f32>()
        + gap * (parts.iter().flatten().count().saturating_sub(1)) as f32;
    let width = if galley.is_none() && kbd_galley.is_none() {
        height
    } else {
        content_w + pad * 2.0
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(height), height), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, on, text));
    if ui.is_rect_visible(rect) {
        let hovered = enabled && resp.hovered();
        let pressed = enabled && resp.is_pointer_button_down_on();
        let (fill, stroke) = match kind {
            Kind::Primary => (
                if pressed {
                    p.accent_press
                } else if hovered {
                    p.accent_hover
                } else {
                    p.accent
                },
                Stroke::NONE,
            ),
            Kind::Danger => (
                if hovered {
                    p.err.gamma_multiply(0.9)
                } else {
                    p.err
                },
                Stroke::NONE,
            ),
            Kind::DangerQuiet => (
                if hovered {
                    p.tone_soft(Tone::Err).gamma_multiply(1.4)
                } else {
                    p.tone_soft(Tone::Err)
                },
                Stroke::NONE,
            ),
            Kind::Secondary if on => (p.accent_soft, Stroke::NONE),
            Kind::Secondary => (
                if pressed {
                    p.subtle
                } else if hovered {
                    blend(p.button, p.hover)
                } else {
                    p.button
                },
                Stroke::new(
                    1.0,
                    if hovered {
                        p.border_strong
                    } else {
                        p.button_border
                    },
                ),
            ),
            Kind::Ghost => (
                if on {
                    p.accent_soft
                } else if pressed {
                    p.subtle
                } else if hovered {
                    p.hover
                } else {
                    Color32::TRANSPARENT
                },
                Stroke::NONE,
            ),
        };
        let painter = ui.painter();
        let radius = CornerRadius::same(RADIUS);
        if matches!(kind, Kind::Primary | Kind::Secondary) && !on && !p.dark {
            // shadow-1: a 1 px soft shadow under raised buttons.
            painter.rect_filled(
                rect.translate(vec2(0.0, 1.0)),
                radius,
                Color32::from_black_alpha(10),
            );
        }
        painter.rect(rect, radius, fill, stroke, StrokeKind::Inside);
        if kind == Kind::Primary {
            // The 1 px light edge along the top of a filled button.
            painter.hline(
                rect.x_range().shrink(3.0),
                rect.top() + 0.5,
                Stroke::new(1.0, Color32::from_white_alpha(36)),
            );
        }
        if resp.has_focus() {
            painter.rect_stroke(
                rect.expand(2.0),
                CornerRadius::same(RADIUS + 2),
                Stroke::new(2.0, p.accent_line),
                StrokeKind::Outside,
            );
        }
        let fade = |c: Color32| {
            if enabled { c } else { c.gamma_multiply(0.45) }
        };
        let icon_color = if hovered && kind == Kind::Ghost && !on {
            p.text
        } else if galley.is_some() && kind == Kind::Secondary && !on {
            p.text_muted
        } else {
            text_color
        };
        let label_color = if hovered && kind == Kind::Ghost && !on {
            p.text
        } else {
            text_color
        };
        let mut x = rect.center().x - content_w / 2.0;
        if let Some(g) = icon_galley {
            let pos = pos2(x, rect.center().y - g.size().y / 2.0);
            x += g.size().x + gap;
            painter.galley(pos, g, fade(icon_color));
        }
        if let Some(g) = galley {
            let pos = pos2(x, rect.center().y - g.size().y / 2.0);
            x += g.size().x + gap;
            painter.galley(pos, g, fade(label_color));
        }
        if let Some(g) = kbd_galley {
            let cap = Rect::from_min_size(pos2(x, rect.center().y - 9.0), vec2(kbd_w, 18.0));
            let cap_fill = match kind {
                Kind::Primary | Kind::Danger => Color32::from_white_alpha(36),
                _ => p.hover,
            };
            painter.rect_filled(cap, CornerRadius::same(4), cap_fill);
            painter.galley(
                cap.center() - g.size() / 2.0,
                g,
                fade(label_color.gamma_multiply(0.85)),
            );
        }
    }
    resp
}

/// `top` over `base` (both may be translucent), for hover fills on opaque buttons.
fn blend(base: Color32, top: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    let [tr, tg, tb, _] = top.to_srgba_unmultiplied();
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(base.r(), tr),
        mix(base.g(), tg),
        mix(base.b(), tb),
        base.a(),
    )
}

pub fn primary(ui: &mut Ui, text: &str) -> Response {
    button_ex(ui, Kind::Primary, None, text, false)
}

pub fn primary_icon(ui: &mut Ui, icon: &str, text: &str) -> Response {
    button_ex(ui, Kind::Primary, Some(icon), text, false)
}

pub fn secondary(ui: &mut Ui, icon: Option<&str>, text: &str) -> Response {
    button_ex(ui, Kind::Secondary, icon, text, false)
}

pub fn secondary_sm(ui: &mut Ui, icon: Option<&str>, text: &str) -> Response {
    button_ex(ui, Kind::Secondary, icon, text, true)
}

pub fn ghost(ui: &mut Ui, icon: Option<&str>, text: &str) -> Response {
    button_ex(ui, Kind::Ghost, icon, text, true)
}

pub fn danger(ui: &mut Ui, text: &str) -> Response {
    button_ex(ui, Kind::Danger, None, text, false)
}

pub fn danger_quiet(ui: &mut Ui, text: &str) -> Response {
    button_ex(ui, Kind::DangerQuiet, None, text, false)
}

/// A square, frameless 26 px icon button with a tooltip (also its accessible name).
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    icon_button_ex(ui, icon, tooltip, false, style::TOOLBAR_H)
}

/// An icon button showing an on/off state.
pub fn icon_toggle(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> Response {
    icon_button_ex(ui, icon, tooltip, on, style::TOOLBAR_H)
}

/// An icon button of side `side`: 26 (toolbar, 16 px icon), 22 (small, 14 px), 18 (row actions,
/// 12 px).
pub fn icon_button_ex(ui: &mut Ui, icon: &str, tooltip: &str, on: bool, side: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    let resp = if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    };
    paint_icon_button(ui, &resp, rect, icon, tooltip, on, false);
    resp
}

/// Paints an icon button on `rect` for `resp` (`forced_hover` draws it hovered).
pub fn paint_icon_button(
    ui: &Ui,
    resp: &Response,
    rect: Rect,
    icon: &str,
    tooltip: &str,
    on: bool,
    forced_hover: bool,
) {
    let p = style::pal(ui);
    let enabled = ui.is_enabled();
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, on, tooltip));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let side = rect.height();
    let hovered = enabled && (resp.hovered() || forced_hover);
    let painter = ui.painter();
    let radius = CornerRadius::same(if side <= 18.0 { 4 } else { RADIUS });
    if on {
        painter.rect_filled(rect, radius, p.accent_soft);
    } else if enabled && resp.is_pointer_button_down_on() {
        painter.rect_filled(rect, radius, p.subtle);
    } else if hovered {
        painter.rect_filled(rect, radius, p.hover);
    }
    if resp.has_focus() {
        painter.rect_stroke(rect, radius, Stroke::new(1.5, p.accent), StrokeKind::Inside);
    }
    let color = if on {
        p.accent_text
    } else if hovered {
        p.text
    } else {
        p.text_muted
    };
    let color = if enabled {
        color
    } else {
        color.gamma_multiply(0.45)
    };
    let icon_size = if side <= 18.0 {
        12.0
    } else if side <= 22.0 {
        14.0
    } else {
        16.0
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(icon_size),
        color,
    );
}

/// One segment of a [`button_group`]: an icon, its tooltip and whether it is "on".
pub struct Segment<'a> {
    pub icon: &'a str,
    pub tooltip: &'a str,
    pub on: bool,
}

/// A row of icon buttons in one outlined surface, split by hairlines.
pub fn button_group(ui: &mut Ui, segments: &[Segment]) -> Vec<Response> {
    let p = style::pal(ui);
    let (h, w) = (style::TOOLBAR_H, 28.0);
    let total = vec2(w * segments.len() as f32, h);
    let (rect, base) = ui.allocate_exact_size(total, Sense::hover());
    let enabled = ui.is_enabled();
    let radius = CornerRadius::same(RADIUS);
    ui.painter().rect_filled(rect, radius, p.button);
    let mut out = Vec::with_capacity(segments.len());
    for (i, seg) in segments.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + w * i as f32, rect.top()), vec2(w, h));
        let resp = ui
            .interact(r, base.id.with(("segment", i)), Sense::click())
            .on_hover_text(seg.tooltip);
        resp.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, seg.on, seg.tooltip));
        let hovered = enabled && resp.hovered();
        let painter = ui.painter();
        if seg.on {
            painter.rect_filled(r, radius, p.accent_soft);
        } else if hovered {
            painter.rect_filled(r, radius, p.hover);
        }
        let color = if seg.on {
            p.accent_text
        } else if hovered {
            p.text
        } else {
            p.text_muted
        };
        painter.text(
            r.center(),
            Align2::CENTER_CENTER,
            seg.icon,
            style::icon_font(15.0),
            color,
        );
        if i > 0 {
            painter.vline(
                r.left(),
                r.y_range().shrink(6.0),
                Stroke::new(1.0, p.border),
            );
        }
        out.push(resp);
    }
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, p.button_border),
        StrokeKind::Inside,
    );
    out
}

// ---------------------------------------------------------------------------------------------
// Choices
// ---------------------------------------------------------------------------------------------

/// A segmented control: one of `options`, the chosen one raised on a quiet track. `small` is the
/// 20 px variant. Returns each segment's response.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str)],
    small: bool,
) -> Vec<egui::Response> {
    let items: Vec<(T, Option<&str>, &str)> = options.iter().map(|(v, l)| (*v, None, *l)).collect();
    segmented_icons(ui, value, &items, small)
}

/// [`segmented`] with an icon before each label.
pub fn segmented_icons<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, Option<&str>, &str)],
    small: bool,
) -> Vec<egui::Response> {
    let p = style::pal(ui);
    let font = if small {
        style::medium(style::XS)
    } else {
        style::medium(style::SMALL)
    };
    let seg_h = if small { 20.0 } else { style::CONTROL_H - 6.0 };
    let pad = if small { 8.0 } else { 12.0 };
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, icon, label)| {
            let text = match icon {
                Some(i) => format!("{i}  {label}"),
                None => label.to_string(),
            };
            ui.painter()
                .layout_no_wrap(text, font.clone(), Color32::PLACEHOLDER)
        })
        .collect();
    let seg_w: Vec<f32> = galleys.iter().map(|g| g.size().x + 2.0 * pad).collect();
    let total = seg_w.iter().sum::<f32>() + 2.0 * (options.len() as f32 - 1.0) + 4.0 + 2.0;
    let (rect, base) = ui.allocate_exact_size(vec2(total, seg_h + 6.0), egui::Sense::hover());
    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS + 1),
        p.hover,
        Stroke::new(1.0, p.line_soft),
        StrokeKind::Inside,
    );
    let mut x = rect.left() + 3.0;
    let mut responses = Vec::with_capacity(options.len());
    for (i, ((option, _, label), galley)) in options.iter().zip(galleys).enumerate() {
        let seg = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(seg_w[i], seg_h));
        x += seg_w[i] + 2.0;
        let resp = ui.interact(seg, base.id.with(("segment", i)), egui::Sense::click());
        let selected = *value == *option;
        resp.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, selected, *label)
        });
        if resp.clicked() {
            *value = *option;
        }
        let painter = ui.painter();
        if selected {
            if !p.dark {
                painter.rect_filled(
                    seg.translate(vec2(0.0, 1.0)),
                    CornerRadius::same(5),
                    Color32::from_black_alpha(12),
                );
            } else {
                painter.rect_filled(
                    seg.translate(vec2(0.0, 1.0)),
                    CornerRadius::same(5),
                    Color32::from_black_alpha(90),
                );
            }
            painter.rect(
                seg,
                CornerRadius::same(5),
                p.segment_on,
                if p.dark {
                    Stroke::NONE
                } else {
                    Stroke::new(1.0, p.border)
                },
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            painter.rect_filled(seg, CornerRadius::same(5), p.hover);
        }
        let color = if selected || resp.hovered() {
            p.text
        } else {
            p.text_muted
        };
        painter.galley(seg.center() - galley.size() / 2.0, galley, color);
        responses.push(resp);
    }
    responses
}

/// A 30 × 18 switch; flips `on` when clicked.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(30.0, 18.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let value = *on;
    resp.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), value, ""));
    if ui.is_rect_visible(rect) {
        let p = style::pal(ui);
        let t = ui.ctx().animate_bool_with_time(resp.id, value, 0.12);
        let track = if value {
            if resp.hovered() {
                p.accent_hover
            } else {
                p.accent
            }
        } else {
            p.border_strong
        };
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(9), track);
        let x = egui::lerp(rect.left() + 9.0..=rect.right() - 9.0, t);
        let c = pos2(x, rect.center().y);
        painter.circle_filled(c + vec2(0.0, 1.0), 7.0, Color32::from_black_alpha(60));
        painter.circle_filled(c, 7.0, Color32::WHITE);
        if resp.has_focus() {
            painter.rect_stroke(
                rect.expand(2.0),
                CornerRadius::same(11),
                Stroke::new(2.0, p.accent_line),
                StrokeKind::Outside,
            );
        }
    }
    resp
}

/// A 16 px radio dot (the caller handles the click).
pub fn radio(ui: &mut Ui, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::click());
    paint_radio(ui.painter(), rect.center(), on, &style::pal(ui));
    resp
}

pub fn paint_radio(painter: &egui::Painter, c: egui::Pos2, on: bool, p: &Palette) {
    if on {
        painter.circle_filled(c, 8.0, p.accent);
        painter.circle_filled(c, 3.0, Color32::WHITE);
    } else {
        painter.circle(c, 7.5, p.field, Stroke::new(1.0, p.border_strong));
    }
}

/// A card that is a choice: an icon, a title and a line of explanation; accent ring when on.
pub fn method_card(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    desc: &str,
    on: bool,
    width: f32,
) -> Response {
    let p = style::pal(ui);
    let desc_g = ui.painter().layout(
        desc.to_string(),
        style::regular(style::XS),
        p.text_faint,
        width - 24.0,
    );
    let h = 10.0 + 18.0 + 4.0 + 16.0 + 4.0 + desc_g.size().y + 10.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(WidgetType::RadioButton, true, on, title));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let radius = CornerRadius::same(style::RADIUS_MD);
        if on {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(3.0, p.accent_soft),
                StrokeKind::Outside,
            );
        }
        painter.rect(
            rect,
            radius,
            if resp.hovered() && !on {
                blend(p.field, p.hover)
            } else {
                p.field
            },
            Stroke::new(1.0, if on { p.accent } else { p.border_strong }),
            StrokeKind::Inside,
        );
        let x = rect.left() + 12.0;
        let mut y = rect.top() + 10.0;
        painter.text(
            pos2(x, y + 9.0),
            Align2::LEFT_CENTER,
            icon,
            style::icon_font(18.0),
            if on { p.accent_text } else { p.text_muted },
        );
        y += 22.0;
        painter.text(
            pos2(x, y),
            Align2::LEFT_TOP,
            title,
            style::medium(style::BODY),
            p.text,
        );
        y += 20.0;
        painter.galley(pos2(x, y), desc_g, p.text_faint);
    }
    resp
}

/// A horizontal card with a radio dot, a title, an optional line under it, and room on the right
/// (`right`) for a control. Returns the card's response (a click picks it).
pub fn radio_card(
    ui: &mut Ui,
    on: bool,
    title: &str,
    desc: Option<&str>,
    right: impl FnOnce(&mut Ui),
) -> Response {
    let p = style::pal(ui);
    let h = if desc.is_some() { 50.0 } else { 42.0 };
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    let painter = ui.painter();
    let radius = CornerRadius::same(style::RADIUS_MD);
    if on {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(3.0, p.accent_soft),
            StrokeKind::Outside,
        );
    }
    painter.rect(
        rect,
        radius,
        p.field,
        Stroke::new(1.0, if on { p.accent } else { p.border_strong }),
        StrokeKind::Inside,
    );
    paint_radio(painter, pos2(rect.left() + 20.0, rect.center().y), on, &p);
    let x = rect.left() + 38.0;
    match desc {
        Some(d) => {
            painter.text(
                pos2(x, rect.top() + 9.0),
                Align2::LEFT_TOP,
                title,
                style::medium(style::BODY),
                p.text,
            );
            painter.text(
                pos2(x, rect.top() + 27.0),
                Align2::LEFT_TOP,
                d,
                style::regular(style::SMALL),
                p.text_faint,
            );
        }
        None => {
            painter.text(
                pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                title,
                style::medium(style::BODY),
                p.text,
            );
        }
    }
    let mut right_ui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                pos2(rect.center().x, rect.top()),
                pos2(rect.right() - 12.0, rect.bottom()),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right(&mut right_ui);
    resp
}

// ---------------------------------------------------------------------------------------------
// Small pieces
// ---------------------------------------------------------------------------------------------

/// A rounded label (20 px) on a soft fill of its tone: states, actions, key types.
pub fn badge(ui: &mut Ui, text: &str, tone: Tone, icon: Option<&str>) -> Response {
    let p = style::pal(ui);
    let color = p.tone(tone);
    let label = match icon {
        Some(i) => format!("{i} {text}"),
        None => text.to_string(),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label, style::medium(style::XS), color);
    let size = vec2(galley.size().x + 14.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(10), p.tone_soft(tone));
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    resp
}

/// Kept for older call sites: a badge colored `color`.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), style::medium(style::XS), color);
    let size = vec2(galley.size().x + 14.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let dark = ui.visuals().dark_mode;
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(10),
            style::soft(color, if dark { 0.14 } else { 0.10 }),
        );
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, color);
    }
    resp
}

/// A count pill (`9`): 10.5 px on the hover fill.
pub fn count(ui: &mut Ui, n: usize) -> Response {
    let p = style::pal(ui);
    let galley =
        ui.painter()
            .layout_no_wrap(n.to_string(), style::medium(style::MICRO), p.text_faint);
    let size = vec2(galley.size().x + 12.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint_count(ui.painter(), rect, galley, &p);
    resp
}

pub fn paint_count(
    painter: &egui::Painter,
    rect: Rect,
    galley: std::sync::Arc<egui::Galley>,
    p: &Palette,
) {
    painter.rect_filled(rect, CornerRadius::same(8), p.hover);
    painter.galley(rect.center() - galley.size() / 2.0, galley, p.text_faint);
}

/// A chip (22 px): an optional icon and a label on the hover fill with a faint border.
pub fn chip(ui: &mut Ui, icon: Option<&str>, text: &str, mono: bool) -> Response {
    chip_tone(ui, icon, text, mono, None)
}

/// A chip in the accent tone (the current hop of a route).
pub fn chip_tone(
    ui: &mut Ui,
    icon: Option<&str>,
    text: &str,
    mono: bool,
    tone: Option<Tone>,
) -> Response {
    let p = style::pal(ui);
    let font = if mono {
        style::mono(11.5)
    } else {
        style::regular(style::SMALL)
    };
    let color = tone.map_or(p.text_muted, |t| p.tone(t));
    let text_g = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let icon_g = icon.map(|i| {
        ui.painter()
            .layout_no_wrap(i.to_string(), style::icon_font(13.0), color)
    });
    let w = text_g.size().x + icon_g.as_ref().map_or(0.0, |g| g.size().x + 5.0) + 16.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let fill = tone.map_or(p.hover, |t| p.tone_soft(t));
        painter.rect(
            rect,
            CornerRadius::same(RADIUS),
            fill,
            Stroke::new(1.0, p.line_soft),
            StrokeKind::Inside,
        );
        let mut x = rect.left() + 8.0;
        if let Some(g) = icon_g {
            painter.galley(
                pos2(x, rect.center().y - g.size().y / 2.0),
                g.clone(),
                color,
            );
            x += g.size().x + 5.0;
        }
        painter.galley(
            pos2(x, rect.center().y - text_g.size().y / 2.0),
            text_g,
            color,
        );
    }
    resp
}

/// A keyboard shortcut drawn as a key cap (20 px).
pub fn kbd(ui: &mut Ui, keys: &str) -> Response {
    let p = style::pal(ui);
    let galley = ui.painter().layout_no_wrap(
        keys.to_string(),
        FontId::proportional(style::XS),
        p.text_muted,
    );
    let size = vec2((galley.size().x + 10.0).max(20.0), 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        paint_kbd(ui.painter(), rect, &p);
        ui.painter()
            .galley(rect.center() - galley.size() / 2.0, galley, p.text_muted);
    }
    resp
}

/// Several key caps 3 px apart.
pub fn kbds(ui: &mut Ui, keys: &[&str]) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        for k in keys {
            kbd(ui, k);
        }
    });
}

/// The key cap behind a shortcut hint.
pub fn paint_kbd(painter: &egui::Painter, rect: Rect, p: &Palette) {
    painter.rect(
        rect,
        CornerRadius::same(4),
        p.hover,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    painter.hline(
        rect.x_range().shrink(2.0),
        rect.bottom() - 0.5,
        Stroke::new(1.0, p.border),
    );
}

/// A key cap painted at `pos` (left center), returning its width.
pub fn paint_kbd_at(painter: &egui::Painter, pos: egui::Pos2, keys: &str, p: &Palette) -> f32 {
    let galley = painter.layout_no_wrap(
        keys.to_string(),
        FontId::proportional(style::XS),
        p.text_muted,
    );
    let rect = Rect::from_min_size(
        pos2(pos.x, pos.y - 10.0),
        vec2((galley.size().x + 10.0).max(20.0), 20.0),
    );
    paint_kbd(painter, rect, p);
    painter.galley(rect.center() - galley.size() / 2.0, galley, p.text_muted);
    rect.width()
}

/// A status dot (7 px) with a 3 px soft ring in its tone.
pub fn status_dot(painter: &egui::Painter, center: egui::Pos2, color: Color32, ring: bool) {
    if ring {
        painter.circle_filled(center, 6.5, style::soft(color, 0.16));
    }
    painter.circle_filled(center, 3.5, color);
}

/// A filled dot (connection state).
pub fn dot(ui: &mut Ui, color: Color32, radius: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(radius * 2.0 + 6.0), Sense::hover());
    status_dot(ui.painter(), rect.center(), color, radius >= 3.0);
    resp
}

/// A spinning ring (14 px): a track with an accent arc.
pub fn spinner(ui: &mut Ui, size: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_spinner(ui, rect.center(), size / 2.0);
    resp
}

pub fn paint_spinner(ui: &Ui, center: egui::Pos2, radius: f32) {
    let p = style::pal(ui);
    let painter = ui.painter();
    let r = radius - 1.0;
    painter.circle_stroke(center, r, Stroke::new(2.0, p.border_strong));
    let time = ui.input(|i| i.time);
    let start = (time * 5.0) as f32;
    let points: Vec<egui::Pos2> = (0..=16)
        .map(|i| {
            let a = start + i as f32 / 16.0 * std::f32::consts::FRAC_PI_2 * 1.1;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(2.0, p.accent)));
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(30));
}

/// The chevron of a drop-down, for `ComboBox::icon`.
pub fn combo_caret(ui: &Ui, rect: Rect, visuals: &egui::style::WidgetVisuals, open: bool) {
    let _ = visuals;
    let p = style::pal(ui);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        if open {
            icons::CARET_UP
        } else {
            icons::CARET_DOWN
        },
        style::icon_font(12.0),
        p.text_faint,
    );
}

/// A thin vertical divider (1 × 16) between toolbar groups, with 6 px either side.
pub fn vdivider(ui: &mut Ui, height: f32) {
    let p = style::pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(13.0, height), Sense::hover());
    ui.painter()
        .vline(rect.center().x, rect.y_range(), Stroke::new(1.0, p.border));
}

/// A section title in semibold, slightly smaller than body.
pub fn section_title(text: &str) -> RichText {
    RichText::new(text).font(style::semibold(13.0))
}

/// An icon on its own, as a label.
pub fn icon(ui: &mut Ui, icon: &str, size: f32, color: Color32) -> Response {
    ui.add(
        egui::Label::new(
            RichText::new(icon)
                .font(style::icon_font(size))
                .color(color),
        )
        .selectable(false),
    )
}

/// A bordered card (cards inside dialogs and pages): 12 px corners, a 1 px ring.
pub fn card(ui: &Ui) -> egui::Frame {
    let p = style::pal(ui);
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .inner_margin(egui::Margin::same(14))
}

/// An icon on a rounded tile in its tone (dialog headers): 36 px with 10 px corners, or 44 / 12.
pub fn icon_tile(ui: &mut Ui, icon: &str, tone: Tone, side: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::hover());
    paint_icon_tile(ui.painter(), rect, icon, tone, &style::pal(ui));
    resp
}

pub fn paint_icon_tile(painter: &egui::Painter, rect: Rect, icon: &str, tone: Tone, p: &Palette) {
    let side = rect.width();
    let radius = if side >= 44.0 {
        12
    } else if side >= 34.0 {
        10
    } else {
        7
    };
    let tone_fill = if tone == Tone::Neutral {
        p.accent_soft
    } else {
        p.tone_soft(tone)
    };
    let color = if tone == Tone::Neutral {
        p.accent_text
    } else {
        p.tone(tone)
    };
    painter.rect_filled(rect, CornerRadius::same(radius), tone_fill);
    let size = if side >= 44.0 {
        22.0
    } else if side >= 34.0 {
        19.0
    } else {
        15.0
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(size),
        color,
    );
}

/// Kept for older call sites: an icon in a soft tile of `color`.
pub fn icon_badge(ui: &mut Ui, icon: &str, color: Color32, diameter: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(diameter), Sense::hover());
    let dark = ui.visuals().dark_mode;
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(10),
        style::soft(color, if dark { 0.16 } else { 0.10 }),
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font((diameter * 0.52).round()),
        color,
    );
    resp
}

/// A tinted note with an icon (warnings in dialogs): 8 px corners, 10 × 12 padding, 12 px text.
pub fn callout(ui: &mut Ui, icon: &str, tone: Tone, text: &str) {
    callout_rich(ui, icon, tone, |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .font(style::regular(style::SMALL))
                    .color(style::pal(ui).text),
            )
            .wrap(),
        );
    });
}

/// A callout with custom content.
pub fn callout_rich(ui: &mut Ui, icon: &str, tone: Tone, add: impl FnOnce(&mut Ui)) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .fill(p.tone_soft(tone))
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let color = if tone == Tone::Neutral {
                    p.text_muted
                } else {
                    p.tone(tone)
                };
                ui.add_space(0.0);
                icon_label_top(ui, icon, 16.0, color);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    add(ui)
                });
            });
        });
}

/// An icon aligned with the first line of text beside it.
fn icon_label_top(ui: &mut Ui, icon: &str, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, 18.0), Sense::hover());
    ui.painter().text(
        pos2(rect.center().x, rect.top() + 9.0),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(size),
        color,
    );
}

/// A centered empty state: a 52 px icon tile, a title, a line of explanation, then whatever
/// `actions` adds.
pub fn empty_state(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    body: &str,
    actions: impl FnOnce(&mut Ui),
) {
    let p = style::pal(ui);
    ui.vertical_centered(|ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(52.0), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(14), p.hover);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon,
            style::icon_font(26.0),
            p.text_muted,
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(title)
                .font(style::semibold(14.0))
                .color(p.text),
        );
        // At most 260 wide, and never closer than 20 to the edges of a narrow column.
        let width = (ui.available_width() - 40.0).clamp(120.0, 260.0);
        ui.allocate_ui(vec2(width, 60.0), |ui| {
            ui.set_max_width(width);
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(body)
                            .font(style::regular(style::SMALL))
                            .color(p.text_muted),
                    )
                    .wrap()
                    .halign(egui::Align::Center),
                );
            });
        });
        ui.add_space(8.0);
        actions(ui);
    });
}

/// A single-line search field with a magnifier inside and a clear button.
pub fn search_field(
    ui: &mut Ui,
    id: egui::Id,
    text: &mut String,
    hint_text: &str,
    width: f32,
) -> Response {
    field_with_icon(
        ui,
        id,
        text,
        hint_text,
        width,
        icons::MAGNIFYING_GLASS,
        true,
    )
}

/// A single-line field with `icon` inside on the left (and, with `clear`, a button that
/// empties it on the right).
pub fn field_with_icon(
    ui: &mut Ui,
    id: egui::Id,
    text: &mut String,
    hint_text: &str,
    width: f32,
    icon: &str,
    clear: bool,
) -> Response {
    let p = style::pal(ui);
    let edit = egui::TextEdit::singleline(text)
        .id(id)
        .hint_text(RichText::new(hint_text).color(p.text_faint))
        .desired_width(width)
        .margin(egui::Margin {
            left: 33,
            right: if clear { 26 } else { 10 },
            top: 7,
            bottom: 7,
        });
    let resp = ui.add(edit);
    let r = resp.rect;
    if resp.has_focus() {
        style::focus_halo(ui, r, RADIUS);
    }
    ui.painter().text(
        pos2(r.left() + 18.0, r.center().y),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(15.0),
        p.text_faint,
    );
    if clear && !text.is_empty() {
        let x_rect =
            Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), Vec2::splat(18.0));
        let x = ui.interact(x_rect, resp.id.with("clear"), Sense::click());
        let color = if x.hovered() { p.text } else { p.text_faint };
        ui.painter().text(
            x_rect.center(),
            Align2::CENTER_CENTER,
            icons::X_CIRCLE,
            style::icon_font(14.0),
            color,
        );
        if x.clicked() {
            text.clear();
        }
    }
    resp
}

/// The quiet filter field of the sidebar (28 px, hover fill, no border), with a key cap on the
/// right when empty.
pub fn quiet_search(
    ui: &mut Ui,
    id: egui::Id,
    text: &mut String,
    hint_text: &str,
    keys: Option<&str>,
) -> Response {
    let p = style::pal(ui);
    let width = ui.available_width();
    let mut resp = None;
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        let fill = p.hover;
        v.text_edit_bg_color = Some(fill);
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered] {
            w.bg_stroke = Stroke::NONE;
        }
        let edit = egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(RichText::new(hint_text).color(p.text_faint))
            .desired_width(width)
            .margin(egui::Margin {
                left: 30,
                right: 34,
                top: 6,
                bottom: 6,
            });
        let r = ui.add(edit);
        resp = Some(r);
    });
    let resp = resp.unwrap();
    let r = resp.rect;
    if resp.has_focus() {
        style::focus_halo(ui, r, RADIUS);
    }
    ui.painter().text(
        pos2(r.left() + 16.0, r.center().y),
        Align2::CENTER_CENTER,
        icons::MAGNIFYING_GLASS,
        style::icon_font(15.0),
        p.text_faint,
    );
    if !text.is_empty() {
        let x_rect =
            Rect::from_center_size(pos2(r.right() - 14.0, r.center().y), Vec2::splat(18.0));
        let x = ui.interact(x_rect, resp.id.with("clear"), Sense::click());
        let color = if x.hovered() { p.text } else { p.text_faint };
        ui.painter().text(
            x_rect.center(),
            Align2::CENTER_CENTER,
            icons::X_CIRCLE,
            style::icon_font(14.0),
            color,
        );
        if x.clicked() {
            text.clear();
        }
    } else if let Some(k) = keys {
        let painter = ui.painter();
        let g =
            painter.layout_no_wrap(k.to_string(), FontId::proportional(style::XS), p.text_muted);
        let cap = Rect::from_min_size(
            pos2(
                r.right() - 8.0 - (g.size().x + 10.0).max(20.0),
                r.center().y - 9.0,
            ),
            vec2((g.size().x + 10.0).max(20.0), 18.0),
        );
        painter.rect_stroke(
            cap,
            CornerRadius::same(4),
            Stroke::new(1.0, p.border),
            StrokeKind::Inside,
        );
        painter.galley(cap.center() - g.size() / 2.0, g, p.text_muted);
    }
    resp
}

/// `ScrollArea::show_rows` for rows of exactly `row_h` laid out edge to edge.
pub fn show_rows<R>(
    area: egui::ScrollArea,
    ui: &mut Ui,
    row_h: f32,
    total: usize,
    add: impl FnOnce(&mut Ui, std::ops::Range<usize>) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        area.show_rows(ui, row_h, total, add)
    })
    .inner
}

/// Covers `rect` with `color`, from clear to opaque towards the right (`to_right`) or the left.
pub fn fade(painter: &egui::Painter, rect: Rect, color: Color32, to_right: bool) {
    let (left, right) = if to_right {
        (Color32::TRANSPARENT, color)
    } else {
        (color, Color32::TRANSPARENT)
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Covers the four corners of `rect` outside a rounded rect of `radius` with `color`: content
/// drawn square into a rounded card gets the card's corners.
pub fn round_corners(painter: &egui::Painter, rect: Rect, radius: f32, color: Color32) {
    let r = radius;
    let corners = [
        (
            rect.left_top(),
            pos2(rect.left() + r, rect.top() + r),
            std::f32::consts::PI,
        ),
        (
            rect.right_top(),
            pos2(rect.right() - r, rect.top() + r),
            -std::f32::consts::FRAC_PI_2,
        ),
        (
            rect.right_bottom(),
            pos2(rect.right() - r, rect.bottom() - r),
            0.0,
        ),
        (
            rect.left_bottom(),
            pos2(rect.left() + r, rect.bottom() - r),
            std::f32::consts::FRAC_PI_2,
        ),
    ];
    for (corner, center, start) in corners {
        let mut mesh = egui::Mesh::default();
        mesh.colored_vertex(corner, color);
        const N: usize = 10;
        for i in 0..=N {
            let a = start + std::f32::consts::FRAC_PI_2 * i as f32 / N as f32;
            mesh.colored_vertex(center + vec2(a.cos(), a.sin()) * r, color);
        }
        for i in 1..=N as u32 {
            mesh.add_triangle(0, i, i + 1);
        }
        painter.add(mesh);
    }
}

// ---------------------------------------------------------------------------------------------
// Menus
// ---------------------------------------------------------------------------------------------

/// A menu item (28 px): an icon in a 16 px column, the label, the shortcut on the right. The
/// hovered item is filled with the accent and drawn in white, as native menus do.
pub fn menu_item(ui: &mut Ui, icon: Option<&str>, label: &str, shortcut: Option<&str>) -> Response {
    menu_item_ex(ui, icon, label, shortcut, false, false)
}

/// A destructive menu item (red).
pub fn menu_item_danger(ui: &mut Ui, icon: Option<&str>, label: &str) -> Response {
    menu_item_ex(ui, icon, label, None, true, false)
}

pub fn menu_item_ex(
    ui: &mut Ui,
    icon: Option<&str>,
    label: &str,
    shortcut: Option<&str>,
    danger: bool,
    submenu: bool,
) -> Response {
    let p = style::pal(ui);
    let enabled = ui.is_enabled();
    let label_g = ui.painter().layout_no_wrap(
        label.to_string(),
        style::regular(style::BODY),
        Color32::PLACEHOLDER,
    );
    let sc_g = shortcut.map(|s| {
        ui.painter().layout_no_wrap(
            s.to_string(),
            style::regular(style::SMALL),
            Color32::PLACEHOLDER,
        )
    });
    let min_w = 8.0
        + 16.0
        + 9.0
        + label_g.size().x
        + sc_g.as_ref().map_or(0.0, |g| 18.0 + g.size().x)
        + if submenu { 20.0 } else { 0.0 }
        + 8.0;
    // While a popup measures itself its whole default size (600 wide) is available: take only
    // what the item needs then, and the menu's width afterwards.
    let width = if ui.is_sizing_pass() {
        min_w
    } else {
        ui.available_width().max(min_w)
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
    if ui.is_rect_visible(rect) {
        let open = submenu
            && egui::containers::menu::MenuState::from_ui(ui, |state, _| {
                state.open_item == Some(egui::containers::menu::SubMenu::id_from_widget_id(resp.id))
            });
        let hl = enabled && (resp.hovered() || open || resp.has_focus());
        let painter = ui.painter();
        if hl {
            painter.rect_filled(rect, CornerRadius::same(5), p.accent);
        }
        let base = if danger { p.err } else { p.text };
        let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.4) };
        let text_color = fade(if hl { p.on_accent } else { base });
        let icon_color = fade(if hl {
            p.on_accent
        } else if danger {
            p.err
        } else {
            p.text_muted
        });
        if let Some(i) = icon {
            painter.text(
                pos2(rect.left() + 8.0 + 8.0, rect.center().y),
                Align2::CENTER_CENTER,
                i,
                style::icon_font(15.0),
                icon_color,
            );
        }
        let x = rect.left() + 8.0 + 16.0 + 9.0;
        painter.galley(
            pos2(x, rect.center().y - label_g.size().y / 2.0),
            label_g,
            text_color,
        );
        let mut right = rect.right() - 8.0;
        if submenu {
            painter.text(
                pos2(right - 6.0, rect.center().y),
                Align2::CENTER_CENTER,
                icons::CARET_RIGHT,
                style::icon_font(12.0),
                if hl { p.on_accent } else { p.text_faint },
            );
            right -= 20.0;
        }
        if let Some(g) = sc_g {
            let color = if hl {
                p.on_accent.gamma_multiply(0.9)
            } else {
                p.text_faint
            };
            painter.galley(
                pos2(right - g.size().x, rect.center().y - g.size().y / 2.0),
                g,
                fade(color),
            );
        }
    }
    resp
}

/// A submenu: an item with a caret that opens `add` to its right on hover.
pub fn submenu(ui: &mut Ui, icon: Option<&str>, label: &str, add: impl FnOnce(&mut Ui)) {
    let resp = menu_item_ex(ui, icon, label, None, false, true);
    egui::containers::menu::SubMenu::new().show(ui, &resp, |ui| {
        ui.set_min_width(240.0);
        menu_body(ui);
        add(ui)
    });
}

/// A hairline between menu groups (5 px above and below, inset 4 px).
pub fn menu_sep(ui: &mut Ui) {
    let p = style::pal(ui);
    let width = if ui.is_sizing_pass() {
        0.0
    } else {
        ui.available_width()
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, 11.0), Sense::hover());
    ui.painter().hline(
        rect.x_range().shrink(4.0),
        rect.center().y,
        Stroke::new(
            1.0,
            if p.dark {
                style::soft(Color32::WHITE, 0.07)
            } else {
                p.border
            },
        ),
    );
}

/// A small uppercase label at the top of a menu group.
pub fn menu_label(ui: &mut Ui, text: &str) {
    let p = style::pal(ui);
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(style::section_label(text, p.text_faint));
    });
    ui.add_space(2.0);
}

/// Spacing inside a menu: items sit edge to edge.
pub fn menu_body(ui: &mut Ui) {
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
}

/// The frame of a popover (forwards, quick commands, hover cards): the raised fill, 12 px
/// corners, the popover shadow.
pub fn popover_frame(ui: &Ui) -> egui::Frame {
    let v = ui.visuals();
    egui::Frame::NONE
        .fill(style::pal(ui).raised)
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .shadow(v.popup_shadow)
        .stroke(v.window_stroke)
}

// ---------------------------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------------------------

/// The header of a dialog: an icon tile (optional), the title (15 px semibold) and a line under
/// it, and a close button on the right (optional). Padding 18 20 6. Returns whether × was
/// clicked.
pub fn dialog_header(
    ui: &mut Ui,
    icon: Option<(&str, Tone)>,
    title: &str,
    sub: Option<&str>,
    close: bool,
) -> bool {
    let p = style::pal(ui);
    let mut closed = false;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 16,
            top: 18,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                if let Some((icon, tone)) = icon {
                    icon_tile(ui, icon, tone, 36.0);
                }
                let right = if close { 30.0 } else { 0.0 };
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width() - right, 36.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        if sub.is_none() {
                            ui.add_space(8.0);
                        }
                        let title_color =
                            if matches!(icon, Some((_, Tone::Err))) && title.contains('!') {
                                p.err
                            } else {
                                p.text
                            };
                        ui.add(
                            egui::Label::new(
                                RichText::new(title)
                                    .font(style::semibold(style::TITLE))
                                    .color(title_color),
                            )
                            .wrap(),
                        );
                        if let Some(s) = sub {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(s)
                                        .font(style::regular(style::SMALL))
                                        .color(p.text_muted),
                                )
                                .wrap(),
                            );
                        }
                    },
                );
                if close {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if icon_button_ex(ui, icons::X, "", false, 22.0).clicked() {
                            closed = true;
                        }
                    });
                }
            });
        });
    closed
}

/// The body of a dialog: padding 10 20 18, muted text.
pub fn dialog_body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 10,
            bottom: 18,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.style_mut().visuals.override_text_color = Some(style::pal(ui).text_muted);
            add(ui)
        })
        .inner
}

/// The footer band of a dialog: a hairline on top, a faint fill, 12 × 16 padding. `left` draws
/// on the left (a hint or a quiet action), `right` lays buttons out from the right edge.
pub fn dialog_footer(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    let p = style::pal(ui);
    let width = ui.available_width();
    let r = egui::Frame::NONE
        .fill(p.footer)
        .inner_margin(egui::Margin::symmetric(16, 12))
        .corner_radius(CornerRadius {
            nw: 0,
            ne: 0,
            sw: style::RADIUS_XL,
            se: style::RADIUS_XL,
        })
        .show(ui, |ui| {
            ui.set_width(width - 32.0);
            ui.horizontal(|ui| {
                ui.set_height(style::CONTROL_H);
                ui.spacing_mut().item_spacing.x = 8.0;
                left(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    right(ui);
                });
            });
        });
    ui.painter().hline(
        r.response.rect.x_range(),
        r.response.rect.top() + 0.5,
        Stroke::new(1.0, p.border),
    );
}

/// A hint in the footer's left side (12 px, faint).
pub fn footer_hint(ui: &mut Ui, text: &str) {
    let p = style::pal(ui);
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .font(style::regular(style::SMALL))
                .color(p.text_faint),
        )
        .truncate(),
    );
}

/// A settings row: a title and a description on the left, the control on the right, a faint
/// hairline under it (12 px padding).
pub fn setting_row(
    ui: &mut Ui,
    title: &str,
    desc: &str,
    last: bool,
    control: impl FnOnce(&mut Ui),
) {
    let p = style::pal(ui);
    let desc = (!desc.is_empty()).then(|| {
        RichText::new(desc)
            .font(style::regular(style::SMALL))
            .color(p.text_faint)
    });
    setting_row_rich(ui, title, desc, last, control);
}

/// [`setting_row`] with a styled description (a path in mono).
pub fn setting_row_rich(
    ui: &mut Ui,
    title: &str,
    desc: Option<RichText>,
    last: bool,
    control: impl FnOnce(&mut Ui),
) {
    let p = style::pal(ui);
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let control_w = 260.0f32.min(ui.available_width() * 0.5);
        let text_w = ui.available_width() - control_w - 16.0;
        ui.allocate_ui_with_layout(
            vec2(text_w, 18.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.set_width(text_w);
                ui.label(
                    RichText::new(title)
                        .font(style::medium(style::BODY))
                        .color(p.text),
                );
                if let Some(d) = desc {
                    ui.add(egui::Label::new(d).wrap());
                }
            },
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            control(ui);
        });
    });
    ui.add_space(12.0);
    if !last {
        let y = ui.cursor().top();
        ui.painter()
            .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.line_soft));
    }
}

/// A group of settings: an uppercase label, then a card holding the rows (14 px side padding).
pub fn setting_group(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    let p = style::pal(ui);
    ui.label(style::section_label(title, p.text_faint));
    ui.add_space(4.0);
    egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .inner_margin(egui::Margin::symmetric(14, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    ui.add_space(18.0);
}

/// [`popover_frame`] from the context (for popups opened outside a `Ui`).
pub fn popover_frame_ctx(ctx: &egui::Context) -> egui::Frame {
    let style = ctx.global_style();
    let v = &style.visuals;
    egui::Frame::NONE
        .fill(style::palette(v.dark_mode).raised)
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .shadow(v.popup_shadow)
        .stroke(v.window_stroke)
}

/// The tooltip look: raised fill, 6 px corners, the popover shadow, 5 × 8 padding.
pub fn tooltip_frame(ui: &Ui) -> egui::Frame {
    let v = ui.visuals();
    egui::Frame::NONE
        .fill(style::pal(ui).raised)
        .corner_radius(CornerRadius::same(RADIUS))
        .shadow(v.popup_shadow)
        .stroke(v.window_stroke)
        .inner_margin(egui::Margin::symmetric(8, 5))
}

/// A thin progress bar (4 px) on a quiet track.
pub fn progress_bar(ui: &mut Ui, fraction: f32, width: f32, tone: Tone) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    paint_bar(ui.painter(), rect, fraction, tone, &style::pal(ui));
}

pub fn paint_bar(painter: &egui::Painter, rect: Rect, fraction: f32, tone: Tone, p: &Palette) {
    painter.rect_filled(rect, CornerRadius::same(2), p.border);
    let mut done = rect;
    done.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    let color = match tone {
        Tone::Ok => p.ok,
        Tone::Err => p.err,
        _ => p.accent,
    };
    if done.width() > 0.5 {
        painter.rect_filled(done, CornerRadius::same(2), color);
    }
}

/// The target for files dragged in from the file manager: a dashed accent outline on a faint
/// accent wash, an upload icon, "Drop to upload" and where to.
pub fn drop_zone(painter: &egui::Painter, rect: Rect, p: &style::Palette, to: &str) {
    painter.rect_filled(rect, CornerRadius::same(style::RADIUS_LG), p.accent_softer);
    dashed_rect(
        painter,
        rect,
        style::RADIUS_LG as f32,
        Stroke::new(2.0, p.accent),
    );
    let c = rect.center();
    painter.text(
        c - vec2(0.0, 26.0),
        Align2::CENTER_CENTER,
        icons::UPLOAD_SIMPLE,
        style::icon_font(30.0),
        p.accent_text,
    );
    painter.text(
        c + vec2(0.0, 4.0),
        Align2::CENTER_CENTER,
        crate::i18n::t("Drop to upload"),
        style::semibold(14.0),
        p.text,
    );
    painter.text(
        c + vec2(0.0, 24.0),
        Align2::CENTER_CENTER,
        to,
        style::mono(style::SMALL),
        p.text_muted,
    );
}

/// A dashed rounded rectangle outline.
pub fn dashed_rect(painter: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke) {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let mut points = Vec::new();
    let arc = |points: &mut Vec<egui::Pos2>, c: egui::Pos2, start: f32| {
        for i in 0..=6 {
            let a = start + std::f32::consts::FRAC_PI_2 * i as f32 / 6.0;
            points.push(c + vec2(a.cos(), a.sin()) * r);
        }
    };
    use std::f32::consts::{FRAC_PI_2, PI};
    arc(&mut points, pos2(rect.left() + r, rect.top() + r), PI);
    arc(
        &mut points,
        pos2(rect.right() - r, rect.top() + r),
        -FRAC_PI_2,
    );
    arc(&mut points, pos2(rect.right() - r, rect.bottom() - r), 0.0);
    arc(
        &mut points,
        pos2(rect.left() + r, rect.bottom() - r),
        FRAC_PI_2,
    );
    points.push(points[0]);
    painter.extend(egui::Shape::dashed_line(&points, stroke, 7.0, 5.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_with_icons() {
        assert_eq!(with_icon("+", "New"), "+  New");
    }
}
