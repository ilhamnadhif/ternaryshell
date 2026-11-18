//! Confirmations (Redesign 3.1): the title, what happens in one or two plain sentences, a short
//! list only when several different things are at stake, and Cancel beside the main button. No
//! icon tiles, no diagrams, no keyboard hints: Esc and Return work as they do everywhere.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Ui};

use super::actions::Action;
use super::kit;
use super::style;
use crate::i18n::t;

/// One item of a confirmation's list ("1 transfer at 62% (partial files are removed)").
#[derive(Clone, Debug)]
pub struct ConfirmRow {
    /// The item; `code` in backquotes is drawn monospaced.
    pub text: String,
    /// A link after the text ("Review") that runs an action and closes the dialog.
    pub link: Option<(String, Action)>,
}

impl ConfirmRow {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            link: None,
        }
    }

    pub fn link(mut self, label: impl Into<String>, action: Action) -> Self {
        self.link = Some((label.into(), action));
        self
    }
}

impl From<String> for ConfirmRow {
    fn from(text: String) -> Self {
        ConfirmRow::new(text)
    }
}

/// What a confirmation needs besides its title, message and list.
#[derive(Clone, Debug, Default)]
pub struct ConfirmExtra {
    pub width: Option<f32>,
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

/// The header: the title alone (16 px semibold).
pub fn header(ui: &mut Ui, title: &str) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 20,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(title)
                        .font(style::semibold(16.0))
                        .color(p.text),
                )
                .wrap(),
            );
        });
}

/// A plain bulleted list, one item per thing at stake. Returns a clicked link's action.
pub fn list(ui: &mut Ui, rows: &[ConfirmRow]) -> Option<Action> {
    let p = style::pal(ui);
    let mut clicked = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        for row in rows {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.spacing_mut().interact_size.y = 20.0;
                let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 20.0), Sense::hover());
                ui.painter().circle_filled(
                    egui::pos2(r.left() + 6.0, r.center().y),
                    2.2,
                    p.text_muted,
                );
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let mut job = rich_job(&row.text, style::BODY, p.text_muted, p.text);
                    for s in &mut job.sections {
                        s.format.line_height = Some(20.0);
                    }
                    ui.add(egui::Label::new(job).wrap());
                    if let Some((label, action)) = &row.link {
                        ui.label(
                            RichText::new(" · ")
                                .font(style::regular(style::BODY))
                                .color(p.text_faint),
                        );
                        let resp = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(label.as_str())
                                        .font(style::regular(style::BODY))
                                        .color(p.accent_text),
                                )
                                .sense(Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        #[cfg(debug_assertions)]
                        crate::devtest::record("confirm-link", resp.rect);
                        if resp.clicked() {
                            clicked = Some(action.clone());
                        }
                    }
                });
            });
        }
    });
    clicked
}

/// The footer of a confirmation: Cancel and the main button at the right (red when it deletes
/// or disconnects). No band, no hairline.
pub fn footer(ui: &mut Ui, button: &str, danger: bool) -> (egui::Response, egui::Response) {
    let width = ui.available_width();
    let mut out = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 18,
            right: 14,
            top: 4,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.set_width(width - 32.0);
            ui.horizontal(|ui| {
                ui.set_height(style::CONTROL_H);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let kind = if danger {
                        kit::Kind::Danger
                    } else {
                        kit::Kind::Primary
                    };
                    let ok = kit::button_ex(ui, kind, None, button, false);
                    let cancel = kit::secondary(ui, None, t("Cancel"));
                    out = Some((ok, cancel));
                });
            });
        });
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
