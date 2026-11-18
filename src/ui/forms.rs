//! Redesign 3.0 pieces shared by the two big sheets (Edit Connection and Settings) and the
//! import / export dialogs: the side navigation, the sheet header and footer band, setting
//! groups drawn as cards of rows, field labels, and search-match highlighting.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::style::{self, Palette};
use super::{icons, kit};

/// What a navigation item shows besides its icon and label.
#[derive(Default, Clone, Copy)]
pub struct NavExtra<'a> {
    /// A short summary of the page, right-aligned and faint (`Key`, `2 forwards`).
    pub summary: Option<&'a str>,
    /// A red dot: the page has a problem.
    pub problem: bool,
    /// A count pill (search results on the page).
    pub count: Option<usize>,
    /// Dimmed (no search results on the page).
    pub dim: bool,
}

/// An item of a sheet's side navigation (32 px): icon, label, and [`NavExtra`]. Selected: the
/// neutral selection fill, the label in the text colour and the icon in the accent.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, on: bool, extra: NavExtra) -> egui::Response {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(7), p.accent_soft);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), p.hover);
    }
    let dim = |c: Color32| if extra.dim { c.gamma_multiply(0.42) } else { c };
    let text = if on || resp.hovered() {
        p.text
    } else {
        p.text_muted
    };
    let icon_color = if on { p.accent_text } else { text };
    painter.text(
        pos2(rect.left() + 10.0 + 8.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(16.0),
        dim(icon_color),
    );
    let label_x = rect.left() + 10.0 + 16.0 + 10.0;
    let label_g = painter.layout_no_wrap(label.to_string(), style::medium(style::BODY), dim(text));
    let label_w = label_g.size().x;
    painter.galley(
        pos2(label_x, rect.center().y - label_g.size().y / 2.0),
        label_g,
        dim(text),
    );
    let mut right = rect.right() - 10.0;
    if extra.problem {
        painter.circle_filled(pos2(right - 3.5, rect.center().y), 3.5, p.err);
        right -= 7.0 + 6.0;
    }
    if let Some(n) = extra.count {
        let g = painter.layout_no_wrap(n.to_string(), style::medium(style::MICRO), p.text_faint);
        let w = (g.size().x + 10.0).max(18.0);
        let r = Rect::from_min_size(pos2(right - w, rect.center().y - 8.0), vec2(w, 16.0));
        kit::paint_count(painter, r, g, &p);
        right -= w + 6.0;
    }
    if let Some(sum) = extra.summary.filter(|s| !s.is_empty()) {
        let max_w = (right - (label_x + label_w + 8.0)).clamp(0.0, 86.0);
        if max_w > 12.0 {
            let mut job = egui::text::LayoutJob::single_section(
                sum.to_string(),
                egui::TextFormat {
                    font_id: style::regular(style::XS),
                    color: dim(p.text_faint),
                    ..Default::default()
                },
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
            let g = painter.layout_job(job);
            painter.galley(
                pos2(right - g.size().x, rect.center().y - g.size().y / 2.0),
                g,
                dim(p.text_faint),
            );
        }
    }
    resp
}

/// The side navigation's background and its hairline on the right; the dialog's left corners
/// are rounded.
pub fn paint_nav(painter: &egui::Painter, nav: Rect, p: &Palette) {
    painter.rect_filled(
        nav,
        CornerRadius {
            nw: style::RADIUS_XL,
            sw: style::RADIUS_XL,
            ne: 0,
            se: 0,
        },
        p.nav,
    );
    painter.vline(nav.right() - 0.5, nav.y_range(), Stroke::new(1.0, p.border));
}

/// A sheet's header: the title (16 px semibold) and a close button at the top right (Redesign
/// 3.1: no subtitle). Returns whether close was clicked.
pub fn sheet_head(ui: &mut Ui, left: f32, top: f32, right: f32, title: &str) -> bool {
    let p = style::pal(ui);
    let painter = ui.painter();
    let title_g = painter.layout_no_wrap(title.to_string(), style::semibold(16.0), p.text);
    // CSS line box: 16 px × 1.4.
    painter.galley(
        pos2(left, top + (22.4 - title_g.size().y) / 2.0),
        title_g,
        p.text,
    );
    let close = Rect::from_min_size(pos2(right - 24.0, top - 1.0), vec2(24.0, 24.0));
    let r = ui.interact(close, egui::Id::new(("sheet-close", title)), Sense::click());
    kit::paint_icon_button(
        ui,
        &r,
        close,
        icons::X,
        crate::i18n::t("Close"),
        false,
        false,
    );
    r.clicked()
}

/// The footer band of a sheet or dialog: a hairline on top and the footer fill, rounded at the
/// bottom right (and bottom left when `round_left`).
pub fn paint_footer(painter: &egui::Painter, rect: Rect, round_left: bool, p: &Palette) {
    painter.rect_filled(
        rect,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: if round_left { style::RADIUS_XL } else { 0 },
            se: style::RADIUS_XL,
        },
        p.footer,
    );
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
}

/// The fill of a settings card: a faint white wash in the dark theme, the surface in the light.
pub fn card_fill(p: &Palette) -> Color32 {
    if p.dark {
        style::soft(Color32::WHITE, 0.022)
    } else {
        p.surface
    }
}

/// A group of settings: a title in sentence case (with optional controls on its right, such as a
/// segmented switch) and a card holding rows (see [`row`]). The rows are drawn by `add`, which
/// gets a [`Rows`] to count them so hairlines go between rows only.
pub fn group(
    ui: &mut Ui,
    title: &str,
    title_right: Option<&mut dyn FnMut(&mut Ui)>,
    add: impl FnOnce(&mut Ui, &mut Rows),
) {
    group_q(ui, title, "", title_right, add);
}

/// [`group`] with the title's matches of a search `query` in the accent.
pub fn group_q(
    ui: &mut Ui,
    title: &str,
    query: &str,
    title_right: Option<&mut dyn FnMut(&mut Ui)>,
    add: impl FnOnce(&mut Ui, &mut Rows),
) {
    let p = style::pal(ui);
    let h = if title_right.is_some() { 26.0 } else { 15.4 };
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let mut job = highlight(
        title,
        query,
        style::semibold(style::SMALL),
        p.text_muted,
        p.accent_text,
    );
    for section in &mut job.sections {
        section.format.font_id = style::semibold(style::SMALL);
    }
    let g = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(r.left() + 2.0, r.center().y - g.size().y / 2.0),
        g,
        p.text_muted,
    );
    if let Some(f) = title_right {
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(r)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        f(&mut right);
    }
    ui.add_space(8.0);
    let frame = egui::Frame::NONE
        .fill(card_fill(&p))
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            let mut rows = Rows::default();
            add(ui, &mut rows);
        });
    let _ = frame;
}

/// A section title: 12 px semibold, muted, in sentence case (Redesign 3.1).
pub fn overline(text: &str, p: &Palette) -> RichText {
    RichText::new(text)
        .font(style::semibold(style::SMALL))
        .color(p.text_muted)
}

/// Counts the rows of a [`group`] card.
#[derive(Default)]
pub struct Rows {
    n: usize,
}

/// A row of a settings card (at least 50 px): the label (13 px medium) and a description under
/// it (12 px faint) on the left, the control on the right. `hit` paints it as a search match.
pub fn row(
    ui: &mut Ui,
    rows: &mut Rows,
    label: egui::WidgetText,
    desc: Option<egui::WidgetText>,
    hit: bool,
    control: impl FnOnce(&mut Ui),
) {
    let p = style::pal(ui);
    let first = rows.n == 0;
    rows.n += 1;
    let width = ui.available_width();
    let top = ui.cursor().top();
    // The control's width as measured last frame (a guess the first time).
    let key = ui
        .id()
        .with(("settings-row", rows.n, label.text().to_string()));
    let ctrl_w: f32 = ui.data(|d| d.get_temp(key)).unwrap_or(160.0);
    let text_w = (width - 28.0 - 16.0 - ctrl_w).max(120.0);
    let label_g = label.into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        text_w,
        egui::FontSelection::FontId(style::medium(style::BODY)),
    );
    let desc_g = desc.map(|d| {
        d.into_galley(
            ui,
            Some(egui::TextWrapMode::Wrap),
            text_w,
            egui::FontSelection::FontId(style::regular(style::SMALL)),
        )
    });
    let label_h = label_g.size().y.max(18.2);
    let text_h = label_h + desc_g.as_ref().map_or(0.0, |g| 2.0 + g.size().y.max(16.8));
    let h = (text_h + 18.0).max(50.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let painter = ui.painter();
    if hit {
        let r = if first {
            rect
        } else {
            rect.with_min_y(rect.top() + 0.5)
        };
        painter.rect_filled(
            r,
            if first {
                CornerRadius {
                    nw: 9,
                    ne: 9,
                    sw: 0,
                    se: 0,
                }
            } else {
                CornerRadius::ZERO
            },
            p.accent_softer,
        );
    }
    if !first {
        painter.hline(rect.x_range(), top + 0.5, Stroke::new(1.0, p.line_soft));
    }
    let y0 = rect.center().y - text_h / 2.0;
    painter.galley(
        pos2(rect.left() + 14.0, y0 + (label_h - label_g.size().y) / 2.0),
        label_g,
        p.text,
    );
    if let Some(g) = desc_g {
        painter.galley(
            pos2(rect.left() + 14.0, y0 + label_h + 2.0),
            g,
            p.text_faint,
        );
    }
    let mut ctrl = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(
                pos2(rect.left() + 14.0 + 120.0, rect.top()),
                pos2(rect.right() - 14.0, rect.bottom()),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    ctrl.spacing_mut().item_spacing.x = 8.0;
    control(&mut ctrl);
    let used = ctrl.min_rect().width();
    if (used - ctrl_w).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(key, used));
        ui.ctx().request_repaint();
    }
}

/// A field label (`.label`): 12 px medium, muted.
pub fn label(text: &str, p: &Palette) -> RichText {
    RichText::new(text)
        .font(style::medium(style::SMALL))
        .color(p.text_muted)
}

/// Help text under a control (`.help`): 12 px, faint.
pub fn help(ui: &mut Ui, text: impl Into<String>) {
    let p = style::pal(ui);
    ui.add(
        egui::Label::new(
            RichText::new(text.into())
                .font(style::regular(style::SMALL))
                .color(p.text_faint),
        )
        .wrap(),
    );
}

/// `text` with every case-insensitive occurrence of `query` in the accent (semibold).
pub fn highlight(
    text: &str,
    query: &str,
    font: egui::FontId,
    color: Color32,
    hit: Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let q = query.trim().to_lowercase();
    let fmt = |c: Color32, strong: bool| egui::TextFormat {
        font_id: if strong {
            style::semibold(font.size)
        } else {
            font.clone()
        },
        color: c,
        ..Default::default()
    };
    if q.is_empty() {
        job.append(text, 0.0, fmt(color, false));
        return job;
    }
    let lower = text.to_lowercase();
    // Matching on the lower-cased copy is only safe when lower-casing kept byte offsets.
    if lower.len() != text.len() {
        job.append(text, 0.0, fmt(color, false));
        return job;
    }
    let mut i = 0;
    while let Some(pos) = lower[i..].find(&q) {
        let s = i + pos;
        if s > i {
            job.append(&text[i..s], 0.0, fmt(color, false));
        }
        job.append(&text[s..s + q.len()], 0.0, fmt(hit, true));
        i = s + q.len();
    }
    if i < text.len() {
        job.append(&text[i..], 0.0, fmt(color, false));
    }
    job
}

/// Whether `query` (trimmed, case-insensitive) is in `text`.
pub fn matches(text: &str, query: &str) -> bool {
    let q = query.trim();
    !q.is_empty() && text.to_lowercase().contains(&q.to_lowercase())
}

/// An inset frame (`.well`): code, previews.
pub fn well(p: &Palette) -> egui::Frame {
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
}

/// A rounded outline around `rect` (`box-shadow: inset 0 0 0 1px`), for cards that are choices.
pub fn choice_outline(painter: &egui::Painter, rect: Rect, radius: u8, on: bool, p: &Palette) {
    if on {
        painter.rect(
            rect,
            CornerRadius::same(radius),
            p.accent_softer,
            Stroke::new(1.5, p.accent),
            StrokeKind::Inside,
        );
    } else {
        painter.rect_stroke(
            rect,
            CornerRadius::same(radius),
            Stroke::new(
                1.0,
                if p.dark {
                    style::soft(Color32::WHITE, 0.09)
                } else {
                    p.border_strong
                },
            ),
            StrokeKind::Inside,
        );
    }
}

/// The up-down caret of a select (for `ComboBox::icon`).
pub fn updown_caret(ui: &Ui, rect: Rect, _visuals: &egui::style::WidgetVisuals, _open: bool) {
    let p = style::pal(ui);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        icons::CARET_UP_DOWN,
        style::icon_font(12.0),
        p.text_faint,
    );
}
