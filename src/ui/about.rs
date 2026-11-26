//! About TernaryShell (the same on macOS, Windows and Linux): the icon, the version with a copy
//! button (for bug reports), the update status, and the links that are useful.

use eframe::egui::{
    self, Align, Align2, CornerRadius, Id, Layout, Rect, RichText, Sense, Shadow, Stroke, Ui, pos2,
    vec2,
};

use super::actions::Action;
use super::style::{self, Palette};
use super::update_ui::Status;
use super::{icons, kit};
use crate::i18n::{t, tf};

const WIDTH: f32 = 400.0;
const REPO: &str = "ilhamnadhif/ternaryshell";

#[derive(Default)]
pub struct About {
    /// The version was just copied (the button says so for a moment).
    copied_at: Option<std::time::Instant>,
}

pub enum AboutResult {
    Open,
    Close,
    Run(Action),
}

/// `1.6.0 (aarch64)`: the version and what it was built for.
pub fn version_label() -> String {
    format!(
        "{} ({})",
        env!("CARGO_PKG_VERSION"),
        if option_env!("TERNARYSHELL_UNIVERSAL").is_some() {
            "universal"
        } else {
            std::env::consts::ARCH
        }
    )
}

/// What "copy version" puts on the clipboard: version, system and processor.
fn version_report() -> String {
    format!(
        "TernaryShell {} · {} · {}",
        env!("CARGO_PKG_VERSION"),
        match std::env::consts::OS {
            "macos" => "macOS",
            "windows" => "Windows",
            "linux" => "Linux",
            other => other,
        },
        std::env::consts::ARCH
    )
}

impl About {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        status: Status,
        last_check: Option<i64>,
    ) -> AboutResult {
        let mut result = AboutResult::Open;
        let modal = style::modal(ctx, Id::new("about")).show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            let top = ui.max_rect().top();
            // × in the corner.
            let x = Rect::from_min_size(
                pos2(ui.max_rect().right() - 12.0 - 24.0, top + 12.0),
                vec2(24.0, 24.0),
            );
            let close = ui.interact(x, Id::new("about-close"), Sense::click());
            kit::paint_icon_button(ui, &close, x, icons::X, "", false, false);
            if close.clicked() {
                result = AboutResult::Close;
            }
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 28,
                    right: 28,
                    top: 30,
                    bottom: 18,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        let (r, _) = ui.allocate_exact_size(vec2(92.0, 92.0), Sense::hover());
                        let shadow = Shadow {
                            offset: [0, 12],
                            blur: 32,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(if p.dark { 90 } else { 50 }),
                        };
                        ui.painter().add(shadow.as_shape(r, CornerRadius::same(22)));
                        icons::paint_logo(ui.painter(), r);
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("TernaryShell")
                                .font(style::semibold(20.0))
                                .color(p.text),
                        );
                        ui.label(
                            RichText::new(t("Lightweight SSH and SFTP client"))
                                .font(style::regular(style::BODY))
                                .color(p.text_muted),
                        );
                        ui.add_space(6.0);
                        // The version chip and its copy button, centred together.
                        let label = version_label();
                        let g = ui
                            .painter()
                            .layout_no_wrap(label, style::mono(11.5), p.text_muted);
                        let chip_w = g.size().x + 16.0;
                        let total = chip_w + 6.0 + 20.0;
                        let (row, _) = ui.allocate_exact_size(vec2(total, 22.0), Sense::hover());
                        let chip = Rect::from_min_size(row.min, vec2(chip_w, 22.0));
                        ui.painter()
                            .rect_filled(chip, CornerRadius::same(11), p.hover);
                        ui.painter()
                            .galley(chip.center() - g.size() / 2.0, g, p.text_muted);
                        let copy_rect = Rect::from_min_size(
                            pos2(chip.right() + 6.0, row.center().y - 10.0),
                            vec2(20.0, 20.0),
                        );
                        let copied = self
                            .copied_at
                            .is_some_and(|t| t.elapsed().as_secs_f32() < 1.5);
                        let copy = ui
                            .interact(copy_rect, Id::new("about-copy"), Sense::click())
                            .on_hover_text(t("Copy version info"));
                        kit::paint_icon_button(
                            ui,
                            &copy,
                            copy_rect,
                            if copied { icons::CHECK } else { icons::COPY },
                            "",
                            false,
                            false,
                        );
                        if copy.clicked() {
                            ui.ctx().copy_text(version_report());
                            self.copied_at = Some(std::time::Instant::now());
                            ui.ctx()
                                .request_repaint_after(std::time::Duration::from_millis(1600));
                        }
                    });
                });
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 20,
                    right: 20,
                    top: 0,
                    bottom: 18,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::Frame::NONE
                        .fill(p.well)
                        .stroke(Stroke::new(1.0, p.border))
                        .corner_radius(CornerRadius::same(style::RADIUS_MD))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 0.0;
                            if let Some(a) = rows(ui, &p, status, last_check) {
                                result = AboutResult::Run(a);
                            }
                        });
                });
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 24,
                    right: 24,
                    top: 0,
                    bottom: 20,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let small = |s: &str| {
                            RichText::new(s.to_string())
                                .font(style::regular(style::SMALL))
                                .color(p.text_faint)
                        };
                        ui.label(small(t(
                            "Built with Rust, egui, alacritty_terminal and russh.",
                        )));
                        ui.label(small(&format!(
                            "© 2026 ilhamnadhif · {}",
                            t("No telemetry.")
                        )));
                    });
                });
        });
        if super::dialogs::escape_only(ctx, &modal) || modal.should_close() {
            result = AboutResult::Close;
        }
        result
    }
}

/// The rows: update status, GitHub, license, data folder. Returns an action to run.
fn rows(ui: &mut Ui, p: &Palette, status: Status, last_check: Option<i64>) -> Option<Action> {
    let mut action = None;
    let line = |ui: &mut Ui, first: bool| -> Rect {
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
        if !first {
            ui.painter()
                .hline(r.x_range(), r.top(), Stroke::new(1.0, p.line_soft));
        }
        r
    };
    let left = |ui: &Ui, r: Rect, icon: &str, color: egui::Color32, text: &str| {
        let painter = ui.painter();
        painter.text(
            pos2(r.left() + 12.0 + 7.5, r.center().y),
            Align2::CENTER_CENTER,
            icon,
            style::icon_font(15.0),
            color,
        );
        painter.text(
            pos2(r.left() + 12.0 + 15.0 + 10.0, r.center().y),
            Align2::LEFT_CENTER,
            text,
            style::regular(style::BODY),
            p.text,
        );
    };
    // Update status.
    let r = line(ui, true);
    let (icon, color, text) = match &status {
        Status::Available(v) => (
            icons::ARROW_CIRCLE_UP,
            p.accent_text,
            tf("TernaryShell {} is available", &[v]),
        ),
        Status::Busy => (
            icons::ARROW_CIRCLE_UP,
            p.accent_text,
            t("Updating…").to_string(),
        ),
        Status::UpToDate => (icons::CHECK_CIRCLE, p.ok, t("Up to date").to_string()),
        Status::Unknown => (
            icons::ARROW_CIRCLE_UP,
            p.text_faint,
            t("Updates").to_string(),
        ),
    };
    left(ui, r, icon, color, &text);
    let mut right = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(r.shrink2(vec2(8.0, 0.0)))
            .layout(Layout::right_to_left(Align::Center)),
    );
    right.spacing_mut().item_spacing.x = 6.0;
    match &status {
        Status::Available(_) => {
            if kit::button_ex(&mut right, kit::Kind::Ghost, None, t("Show"), true).clicked() {
                action = Some(Action::CheckForUpdates);
            }
        }
        Status::Busy => {}
        _ => {
            if kit::button_ex(&mut right, kit::Kind::Ghost, None, t("Check Now"), true).clicked() {
                action = Some(Action::CheckForUpdates);
            }
            if let Some(at) = last_check {
                right.label(
                    RichText::new(tf("checked {}", &[&ago(at)]))
                        .font(style::regular(style::SMALL))
                        .color(p.text_faint),
                );
            }
        }
    }
    // GitHub.
    let r = line(ui, false);
    left(ui, r, icons::GITHUB_LOGO, p.text_faint, "GitHub");
    let resp = ui
        .interact(r, Id::new("about-github"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter()
            .rect_filled(r.shrink(1.0), CornerRadius::same(6), p.hover);
        left(ui, r, icons::GITHUB_LOGO, p.text_faint, "GitHub");
    }
    let painter = ui.painter();
    let ext = pos2(r.right() - 12.0 - 7.0, r.center().y);
    painter.text(
        ext,
        Align2::CENTER_CENTER,
        icons::ARROW_SQUARE_OUT,
        style::icon_font(13.0),
        p.text_faint,
    );
    painter.text(
        pos2(ext.x - 7.0 - 6.0, r.center().y),
        Align2::RIGHT_CENTER,
        REPO,
        style::regular(style::SMALL),
        p.text_faint,
    );
    if resp.clicked() {
        crate::platform::open(format!("https://github.com/{REPO}"));
    }
    // License.
    let r = line(ui, false);
    left(ui, r, icons::SCALES, p.text_faint, t("License"));
    ui.painter().text(
        pos2(r.right() - 12.0, r.center().y),
        Align2::RIGHT_CENTER,
        "MIT",
        style::regular(style::SMALL),
        p.text_faint,
    );
    // Data folder.
    let r = line(ui, false);
    left(ui, r, icons::FOLDER_SIMPLE, p.text_faint, t("Data folder"));
    let label = crate::platform::reveal_label();
    let g = ui.painter().layout_no_wrap(
        label.to_string(),
        style::regular(style::SMALL),
        p.accent_text,
    );
    let lr = Rect::from_min_size(
        pos2(
            r.right() - 12.0 - g.size().x,
            r.center().y - g.size().y / 2.0,
        ),
        g.size(),
    );
    let link = ui
        .interact(lr.expand(3.0), Id::new("about-data"), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter().galley(
        lr.min,
        g,
        if link.hovered() {
            p.accent_hover
        } else {
            p.accent_text
        },
    );
    if link.clicked() {
        crate::platform::reveal(crate::paths::data_dir());
    }
    action
}

/// "2 h ago" from a Unix time.
fn ago(at: i64) -> String {
    let secs = (chrono::Utc::now().timestamp() - at).max(0);
    if secs < 60 {
        t("Just now").to_string()
    } else if secs < 3600 {
        tf("{} min ago", &[&(secs / 60).to_string()])
    } else if secs < 86_400 {
        tf("{} h ago", &[&(secs / 3600).to_string()])
    } else {
        tf("{} d ago", &[&(secs / 86_400).to_string()])
    }
}
