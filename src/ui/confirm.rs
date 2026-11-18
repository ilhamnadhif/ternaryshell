//! Confirmations (Redesign 3.0): a tinted icon tile, the title with the connection's label and
//! address under it, what happens spelled out one row per consequence (what goes, what is at
//! risk, what is not touched), and a footer with the Esc hint and the key of the main button.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, StrokeKind,
    Ui, pos2, vec2,
};

use super::actions::Action;
use super::style::{self, Palette, Tone};
use super::{icons, kit};
use crate::bookmarks::{Label, LabelColor};
use crate::i18n::t;

/// A connection named in a confirmation (the open sessions of "Quit?").
#[derive(Clone, Debug)]
pub struct HostChip {
    pub name: String,
    pub icon: &'static str,
    pub color: Option<LabelColor>,
}

/// One consequence of a confirmation.
#[derive(Clone, Debug)]
pub struct ConfirmRow {
    pub icon: &'static str,
    /// The icon's colour: removed or lost (`Err`), at risk (`Warn`), kept (`Ok`), plain.
    pub tone: Tone,
    pub text: String,
    /// A line under the text; `code` in backquotes is drawn monospaced.
    pub detail: Option<String>,
    /// Text at the right (a size, "removed", a folder).
    pub right: Option<String>,
    pub right_mono: bool,
    /// A link at the right ("Review") that runs an action and closes the dialog.
    pub link: Option<(String, Action)>,
    pub hosts: Vec<HostChip>,
}

impl ConfirmRow {
    pub fn new(icon: &'static str, text: impl Into<String>) -> Self {
        Self {
            icon,
            tone: Tone::Neutral,
            text: text.into(),
            detail: None,
            right: None,
            right_mono: false,
            link: None,
            hosts: Vec::new(),
        }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn right(mut self, right: impl Into<String>) -> Self {
        self.right = Some(right.into());
        self
    }

    pub fn right_mono(mut self, right: impl Into<String>) -> Self {
        self.right = Some(right.into());
        self.right_mono = true;
        self
    }

    pub fn link(mut self, label: impl Into<String>, action: Action) -> Self {
        self.link = Some((label.into(), action));
        self
    }

    pub fn hosts(mut self, hosts: Vec<HostChip>) -> Self {
        self.hosts = hosts;
        self
    }
}

impl From<(&'static str, String)> for ConfirmRow {
    fn from((icon, text): (&'static str, String)) -> Self {
        ConfirmRow::new(icon, text)
    }
}

/// "Delete group?": the group and what is in it, before and after.
#[derive(Clone, Debug)]
pub struct GroupTree {
    pub name: String,
    pub color: Option<LabelColor>,
    /// Where its contents go: the parent group, or `None` for the top level.
    pub parent: Option<String>,
    /// The connections and subgroups inside (name, icon, is a group).
    pub items: Vec<(String, &'static str, bool)>,
    /// How many more than `items` there are.
    pub more: usize,
}

/// What a confirmation shows besides its title, message and rows.
#[derive(Clone, Debug, Default)]
pub struct ConfirmExtra {
    /// The header tile; by default it follows the action.
    pub icon: Option<(&'static str, Tone)>,
    /// The connection's label and address, under the title.
    pub sub: Option<(Option<Label>, String)>,
    pub tree: Option<GroupTree>,
    /// What Esc does, after the key cap ("to cancel" by default).
    pub escape: Option<String>,
    /// The key cap on the main button ("↩" by default).
    pub keys: Option<String>,
    pub width: Option<f32>,
}

/// What the user did in a confirmation this frame.
#[derive(Default)]
pub struct Clicked {
    pub ok: bool,
    pub cancel: bool,
    pub link: Option<Action>,
}

/// The header tile (44 px): the icon in its tone on a 14 % wash of it, neutral on the hover fill.
fn tile(ui: &mut Ui, icon: &str, tone: Tone, side: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    paint_tile(ui.painter(), rect, icon, tone, &style::pal(ui));
}

pub fn paint_tile(painter: &egui::Painter, rect: Rect, icon: &str, tone: Tone, p: &Palette) {
    let side = rect.width();
    let (radius, size) = if side >= 40.0 { (12, 22.0) } else { (9, 17.0) };
    let (fill, color) = match tone {
        Tone::Neutral => (p.hover, p.text_muted),
        Tone::Accent => (style::soft(p.accent_text, 0.14), p.accent_text),
        t => (style::soft(p.tone(t), 0.14), p.tone(t)),
    };
    painter.rect_filled(rect, CornerRadius::same(radius), fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(size),
        color,
    );
}

/// A key cap as the design draws them (18 px, 10.5 px text) with `text` after it.
pub fn key_hint(ui: &mut Ui, keys: &str, text: &str) {
    let p = style::pal(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        key_cap(ui, keys);
        ui.label(
            RichText::new(text)
                .font(style::regular(style::SMALL))
                .color(p.text_faint),
        );
    });
}

/// An 18 px key cap.
pub fn key_cap(ui: &mut Ui, keys: &str) -> egui::Response {
    let p = style::pal(ui);
    let g =
        ui.painter()
            .layout_no_wrap(keys.to_string(), style::medium(style::MICRO), p.text_muted);
    let size = vec2((g.size().x + 10.0).max(18.0), 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(4),
            p.hover,
            Stroke::new(1.0, p.border),
            StrokeKind::Inside,
        );
        painter.hline(
            rect.x_range().shrink(1.0),
            rect.bottom() - 0.5,
            Stroke::new(1.0, p.border_strong),
        );
        painter.galley(rect.center() - g.size() / 2.0, g, p.text_muted);
    }
    resp
}

/// Text where `code` in backquotes is monospaced, as one wrapping job.
pub fn rich_job(text: &str, size: f32, color: Color32, code: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (i, part) in text.split('`').enumerate() {
        if part.is_empty() {
            continue;
        }
        let mono = i % 2 == 1;
        job.append(
            part,
            0.0,
            egui::TextFormat {
                font_id: if mono {
                    style::mono(size - 1.0)
                } else {
                    style::regular(size)
                },
                color: if mono { code } else { color },
                ..Default::default()
            },
        );
    }
    job
}

/// The header: tile, title (16 px semibold) and the label + address line.
pub fn header(
    ui: &mut Ui,
    icon: &str,
    tone: Tone,
    title: &str,
    sub: Option<&(Option<Label>, String)>,
) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 20,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                tile(ui, icon, tone, 44.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(title)
                                .font(style::semibold(16.0))
                                .color(p.text),
                        )
                        .wrap(),
                    );
                    if let Some((label, address)) = sub {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            kit::label_tag(ui, label.as_ref(), false);
                            ui.label(
                                RichText::new(address)
                                    .font(style::mono(11.0))
                                    .color(p.text_muted),
                            );
                        });
                    }
                });
            });
        });
}

/// The rows card: a well with one row per consequence. Returns a clicked link's action.
pub fn rows_card(ui: &mut Ui, rows: &[ConfirmRow]) -> Option<Action> {
    let p = style::pal(ui);
    let mut clicked = None;
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    let y = ui.cursor().top();
                    ui.painter()
                        .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.line_soft));
                }
                if let Some(a) = row_ui(ui, row, &p) {
                    clicked = Some(a);
                }
            }
        });
    clicked
}

fn row_ui(ui: &mut Ui, row: &ConfirmRow, p: &Palette) -> Option<Action> {
    let mut clicked = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let full = ui.available_width();
            // The right side first, to know how much room the text has.
            let right_galley = row.right.as_ref().map(|r| {
                ui.painter().layout_no_wrap(
                    r.clone(),
                    if row.right_mono {
                        style::mono(11.5)
                    } else {
                        style::regular(style::SMALL)
                    },
                    p.text_faint,
                )
            });
            let link_galley = row.link.as_ref().map(|(l, _)| {
                ui.painter()
                    .layout_no_wrap(l.clone(), style::regular(style::SMALL), p.accent_text)
            });
            let right_w = right_galley
                .as_ref()
                .or(link_galley.as_ref())
                .map_or(0.0, |g| g.size().x + 12.0);
            let top = ui.cursor().top();
            let r = ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let color = match row.tone {
                    Tone::Neutral => p.text_muted,
                    t => p.tone(t),
                };
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    kit::icon(ui, row.icon, 15.0, color);
                });
                ui.vertical(|ui| {
                    ui.set_width(full - 25.0 - right_w);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.add(
                        egui::Label::new(rich_job(&row.text, style::BODY, p.text, p.text)).wrap(),
                    );
                    if let Some(d) = &row.detail {
                        ui.add(
                            egui::Label::new(rich_job(d, style::SMALL, p.text_faint, p.text_muted))
                                .wrap(),
                        );
                    }
                    if !row.hosts.is_empty() {
                        ui.add_space(5.0);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(5.0, 5.0);
                            for h in &row.hosts {
                                host_chip(ui, h);
                            }
                        });
                    }
                });
            });
            let area = Rect::from_min_max(
                pos2(ui.max_rect().right() - right_w, top),
                pos2(ui.max_rect().right(), r.response.rect.bottom()),
            );
            if let Some(g) = right_galley {
                ui.painter().galley(
                    pos2(
                        area.right() - g.size().x,
                        area.center().y - g.size().y / 2.0,
                    ),
                    g,
                    p.text_faint,
                );
            } else if let (Some(g), Some((label, action))) = (link_galley, &row.link) {
                let rect = Rect::from_min_size(
                    pos2(
                        area.right() - g.size().x,
                        area.center().y - g.size().y / 2.0,
                    ),
                    g.size(),
                );
                let resp = ui
                    .interact(rect, ui.id().with(("confirm-link", label)), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                let color = if resp.hovered() {
                    p.accent_hover
                } else {
                    p.accent_text
                };
                ui.painter().galley(rect.min, g, color);
                if resp.clicked() {
                    clicked = Some(action.clone());
                }
            }
        });
    clicked
}

/// A connection as a 22 px chip: its glyph (16 px) and name.
pub fn host_chip(ui: &mut Ui, h: &HostChip) {
    let p = style::pal(ui);
    let g = ui
        .painter()
        .layout_no_wrap(h.name.clone(), style::medium(style::SMALL), p.text);
    let size = vec2(4.0 + 16.0 + 6.0 + g.size().x + 8.0, 22.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(6), p.hover);
        mini_glyph(
            painter,
            Rect::from_min_size(
                pos2(rect.left() + 4.0, rect.center().y - 8.0),
                vec2(16.0, 16.0),
            ),
            h.icon,
            h.color.map(|c| style::label_color(c, p.dark)),
            &p,
        );
        painter.galley(
            pos2(rect.left() + 26.0, rect.center().y - g.size().y / 2.0),
            g,
            p.text,
        );
    }
}

/// A 16 px host glyph (4 px corners, 10 px icon).
fn mini_glyph(
    painter: &egui::Painter,
    rect: Rect,
    icon: &str,
    color: Option<Color32>,
    p: &Palette,
) {
    let (fill, ink) = match color {
        Some(c) => (style::soft(c, 0.15), c),
        None => (p.hover, p.text_faint),
    };
    painter.rect_filled(rect, CornerRadius::same(4), fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(10.0),
        ink,
    );
}

/// Before → after for "Delete group?": the group (struck out) with what is in it, an arrow,
/// and the same items one level up.
pub fn tree(ui: &mut Ui, g: &GroupTree) {
    let p = style::pal(ui);
    let w = ui.available_width();
    let col_w = ((w - 22.0 - 12.0) / 2.0).floor();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let label = g.color.map(|c| style::label_color(c, p.dark));
        let left = column(ui, col_w, |ui| {
            let r = tree_row(ui, 0.0, false);
            let painter = ui.painter();
            let mut x = r.left() + 6.0;
            if let Some(c) = label {
                painter.circle_filled(pos2(x + 4.0, r.center().y), 4.0, c);
                x += 15.0;
            }
            let gal = painter.layout_no_wrap(g.name.clone(), style::semibold(style::SMALL), p.err);
            let tr = Rect::from_min_size(pos2(x, r.center().y - gal.size().y / 2.0), gal.size());
            painter.galley(tr.min, gal, p.err);
            painter.hline(
                tr.x_range(),
                tr.center().y + 0.5,
                Stroke::new(1.0, style::soft(p.err, 0.6)),
            );
            for (name, icon, _) in &g.items {
                let r = tree_row(ui, 20.0, false);
                item(ui, r, name, icon, label, &p);
            }
            more(ui, g.more, &p);
        });
        // The arrow, centred between the columns.
        let (ar, _) = ui.allocate_exact_size(vec2(22.0, left.height()), Sense::hover());
        ui.painter().text(
            ar.center(),
            Align2::CENTER_CENTER,
            icons::ARROW_RIGHT,
            style::icon_font(16.0),
            p.text_faint,
        );
        column(ui, col_w, |ui| {
            let r = tree_row(ui, 0.0, false);
            let painter = ui.painter();
            painter.text(
                pos2(r.left() + 6.0 + 8.0, r.center().y),
                Align2::CENTER_CENTER,
                icons::TREE_STRUCTURE,
                style::icon_font(15.0),
                p.text_faint,
            );
            let place = match &g.parent {
                Some(parent) => parent.clone(),
                None => t("Top level").to_string(),
            };
            painter.text(
                pos2(r.left() + 6.0 + 16.0 + 7.0, r.center().y),
                Align2::LEFT_CENTER,
                place,
                style::regular(style::SMALL),
                p.text_faint,
            );
            for (name, icon, _) in &g.items {
                let r = tree_row(ui, 20.0, true);
                item(ui, r, name, icon, None, &p);
            }
            more(ui, g.more, &p);
        });
    });
}

fn column(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui)) -> Rect {
    let p = style::pal(ui);
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(width - 14.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                add(ui);
            });
        })
        .response
        .rect
}

/// A 24 px row of the tree; `moved` rows get the green wash.
fn tree_row(ui: &mut Ui, indent: f32, moved: bool) -> Rect {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
    if moved {
        let p = style::pal(ui);
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), p.tone_soft(Tone::Ok));
    }
    Rect::from_min_max(pos2(rect.left() + indent, rect.top()), rect.max)
}

fn item(ui: &Ui, r: Rect, name: &str, icon: &str, color: Option<Color32>, p: &Palette) {
    let painter = ui.painter();
    mini_glyph(
        painter,
        Rect::from_min_size(pos2(r.left() + 6.0, r.center().y - 8.0), vec2(16.0, 16.0)),
        icon,
        color,
        p,
    );
    painter.text(
        pos2(r.left() + 6.0 + 16.0 + 7.0, r.center().y),
        Align2::LEFT_CENTER,
        name,
        style::regular(style::SMALL),
        p.text,
    );
}

fn more(ui: &mut Ui, n: usize, p: &Palette) {
    if n > 0 {
        let r = tree_row(ui, 20.0, false);
        ui.painter().text(
            pos2(r.left() + 6.0, r.center().y),
            Align2::LEFT_CENTER,
            crate::i18n::tf("and {} more", &[&n.to_string()]),
            style::regular(style::SMALL),
            p.text_faint,
        );
    }
}

/// The footer: a hairline, the Esc hint on the left, Cancel and the main button on the right.
pub fn footer(
    ui: &mut Ui,
    escape: &str,
    button: &str,
    keys: &str,
    danger: bool,
) -> (egui::Response, egui::Response) {
    let mut out = None;
    footer_with(
        ui,
        |ui| key_hint(ui, "esc", escape),
        |ui| {
            let kind = if danger {
                kit::Kind::Danger
            } else {
                kit::Kind::Primary
            };
            let ok = kit::button_kbd(
                ui,
                kind,
                None,
                button,
                (!keys.is_empty()).then_some(keys),
                false,
                false,
            );
            let cancel = kit::secondary(ui, None, t("Cancel"));
            out = Some((ok, cancel));
        },
    );
    out.expect("drawn")
}

/// A dialog footer: a hairline on top (a faint band in the dark theme), 12 × 14–18 padding,
/// `left` from the left, `right` laid out from the right edge.
pub fn footer_with(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    let p = style::pal(ui);
    let width = ui.available_width();
    let r = egui::Frame::NONE
        .fill(if p.dark {
            p.footer
        } else {
            Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin {
            left: 18,
            right: 14,
            top: 12,
            bottom: 12,
        })
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
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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

/// A small badge as the design draws it (18 px, 5 px corners, 10.5 px semibold): neutral on
/// the hover fill, a tone on a 14 % wash of it.
pub fn small_badge(ui: &mut Ui, text: &str, tone: Tone, icon: Option<&str>) -> egui::Response {
    let p = style::pal(ui);
    let color = match tone {
        Tone::Neutral => p.text_muted,
        t => p.tone(t),
    };
    let fill = match tone {
        Tone::Neutral => p.hover,
        Tone::Accent => p.accent_tint,
        t => p.tone_soft(t),
    };
    let mut job = egui::text::LayoutJob::default();
    if let Some(i) = icon {
        job.append(
            i,
            0.0,
            egui::TextFormat {
                font_id: style::icon_font(11.0),
                color,
                valign: Align::Center,
                ..Default::default()
            },
        );
        job.append(" ", 0.0, egui::TextFormat::default());
    }
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: style::semibold(style::MICRO),
            color,
            extra_letter_spacing: 0.2,
            valign: Align::Center,
            ..Default::default()
        },
    );
    let g = ui.painter().layout_job(job);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 18.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
        ui.painter()
            .galley(rect.center() - g.size() / 2.0, g, color);
    }
    resp
}
