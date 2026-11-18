//! Toasts (Redesign 3.1): a short message in the bottom right corner of the canvas, a line
//! under it and the next step as buttons (Show in Finder, Edit forwards…); no icon tile.
//! At most four are shown, the newest at the bottom; information goes away after 6 s (a thin
//! bar shows the time left, and hovering pauses it), errors stay until closed.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, CornerRadius, Id, Rect, RichText, Sense, Ui, pos2, vec2};

use super::actions::Action;
use super::style;
use super::{confirm, icons, kit};
use crate::i18n::t;

/// Most toasts on screen; a new one pushes out the oldest.
const MAX: usize = 4;
/// How long information stays (errors stay until closed).
const LIFE: Duration = Duration::from_secs(6);
/// … and information with a button, which takes longer to act on.
const LIFE_ACTION: Duration = Duration::from_secs(9);
const WIDTH: f32 = 340.0;

/// What a toast is about, and so whether it stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Something finished well.
    Ok,
    /// Information (an export, an update).
    Info,
    /// Something needs attention but nothing was lost.
    Warn,
    /// It failed: stays until closed.
    Error,
}

/// What a toast button does.
#[derive(Clone, Debug, PartialEq)]
pub enum Do {
    Run(Action),
    /// Shows a file or folder in Finder / Explorer / the file manager.
    Reveal(PathBuf),
    /// Copies text (an error's details).
    Copy(String),
}

#[derive(Clone, Debug)]
struct Button {
    label: String,
    act: Do,
    /// Drawn as a ghost button (the second, quieter action).
    quiet: bool,
}

/// One toast; build it with [`Toast::new`] and give it to [`Toasts::add`].
#[derive(Clone, Debug)]
pub struct Toast {
    kind: Kind,
    title: String,
    detail: Option<String>,
    buttons: Vec<Button>,
    sticky: bool,
    at: Instant,
    /// Time spent hovered, which doesn't count towards its life.
    paused: Duration,
    hovered_since: Option<Instant>,
}

impl Toast {
    pub fn new(kind: Kind, title: impl Into<String>) -> Self {
        Self {
            kind,
            title: title.into(),
            detail: None,
            buttons: Vec::new(),
            sticky: kind == Kind::Error,
            at: Instant::now(),
            paused: Duration::ZERO,
            hovered_since: None,
        }
    }

    /// The line under the title; `code` in backquotes is monospaced.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// The main button.
    pub fn button(mut self, label: impl Into<String>, act: Do) -> Self {
        self.buttons.push(Button {
            label: label.into(),
            act,
            quiet: false,
        });
        self
    }

    /// A quieter second button.
    pub fn quiet_button(mut self, label: impl Into<String>, act: Do) -> Self {
        self.buttons.push(Button {
            label: label.into(),
            act,
            quiet: true,
        });
        self
    }

    /// Stays until closed.
    pub fn sticky(mut self) -> Self {
        self.sticky = true;
        self
    }

    fn life(&self) -> Duration {
        if self.buttons.is_empty() {
            LIFE
        } else {
            LIFE_ACTION
        }
    }

    /// Time lived, without the time it was hovered.
    fn lived(&self, now: Instant) -> Duration {
        let hovered = self
            .hovered_since
            .map_or(Duration::ZERO, |h| now.duration_since(h));
        now.duration_since(self.at)
            .saturating_sub(self.paused + hovered)
    }
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn info(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        #[cfg(debug_assertions)]
        eprintln!("DEVTEST LOG: toast info: {msg}");
        self.push(msg, None, false, None);
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        #[cfg(debug_assertions)]
        eprintln!("DEVTEST LOG: toast error: {msg}");
        self.push(msg, None, true, None);
    }

    /// A toast with a title, a line under it, and an action button.
    pub fn push(
        &mut self,
        title: String,
        detail: Option<String>,
        error: bool,
        action: Option<(String, Action)>,
    ) {
        // A message "Title: detail" or "Title. Detail" splits into its two lines.
        let (title, detail) = match detail {
            Some(d) => (title, Some(d)),
            None => split_message(&title),
        };
        let mut toast = Toast::new(if error { Kind::Error } else { Kind::Ok }, title);
        toast.detail = detail;
        if let Some((label, action)) = action {
            toast = toast.button(label, Do::Run(action));
        }
        self.add(toast);
    }

    pub fn add(&mut self, toast: Toast) {
        #[cfg(debug_assertions)]
        if !matches!(toast.kind, Kind::Ok | Kind::Error) {
            eprintln!("DEVTEST LOG: toast: {}", toast.title);
        }
        self.items.push(toast);
        if self.items.len() > MAX {
            let extra = self.items.len() - MAX;
            self.items.drain(..extra);
        }
    }

    /// Draws the toasts in the bottom right corner of the canvas; returns an action clicked.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        let now = Instant::now();
        self.items.retain(|t| t.sticky || t.lived(now) < t.life());
        if self.items.is_empty() {
            return None;
        }
        // Room taken by the update card in the same corner (see `update_ui`).
        let inset: f32 = ctx.data(|d| d.get_temp(Id::new(INSET_ID)).unwrap_or(0.0));
        let mut chosen = None;
        let mut next_wake: Option<Duration> = None;
        let newest = self.items.len() - 1;
        egui::Area::new(Id::new("toasts"))
            // Above the status strip, inside the canvas.
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -44.0 - inset))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let mut dismiss = None;
                ui.spacing_mut().item_spacing.y = 8.0;
                for (i, toast) in self.items.iter_mut().enumerate() {
                    if i != newest {
                        ui.set_opacity(0.92);
                    } else {
                        ui.set_opacity(1.0);
                    }
                    let (resp, clicked, closed) = toast_ui(ui, toast, now);
                    if closed {
                        dismiss = Some(i);
                    }
                    if let Some(act) = clicked {
                        dismiss = Some(i);
                        match act {
                            Do::Run(a) => chosen = Some(a),
                            Do::Reveal(path) => crate::platform::reveal(&path),
                            Do::Copy(text) => ui.ctx().copy_text(text),
                        }
                    }
                    // Hovering pauses the clock.
                    let hovered = resp.contains_pointer();
                    match (hovered, toast.hovered_since) {
                        (true, None) => toast.hovered_since = Some(now),
                        (false, Some(h)) => {
                            toast.paused += now.duration_since(h);
                            toast.hovered_since = None;
                        }
                        _ => {}
                    }
                    if !toast.sticky && toast.hovered_since.is_none() {
                        let left = toast.life().saturating_sub(toast.lived(now));
                        // The life bar moves in small steps; wake for it, not every frame.
                        let step = Duration::from_millis(120).min(left);
                        next_wake = Some(next_wake.map_or(step, |w| w.min(step)));
                    }
                }
                if let Some(i) = dismiss {
                    self.items.remove(i);
                }
            });
        if let Some(w) = next_wake {
            ctx.request_repaint_after(w.max(Duration::from_millis(16)));
        }
        chosen
    }
}

/// Where the update card tells the toasts how much room it takes at the bottom.
pub const INSET_ID: &str = "toasts-inset";

/// One toast: returns its response (for hovering), a clicked button and whether × was clicked.
fn toast_ui(ui: &mut Ui, toast: &Toast, now: Instant) -> (egui::Response, Option<Do>, bool) {
    let p = style::pal(ui);
    let mut clicked = None;
    let mut closed = false;
    let frame = egui::Frame::NONE
        .fill(p.raised)
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .shadow(ui.visuals().popup_shadow)
        .stroke(ui.visuals().window_stroke)
        .inner_margin(egui::Margin {
            left: 14,
            right: 12,
            top: 12,
            bottom: 12,
        })
        .show(ui, |ui| {
            ui.set_width(WIDTH - 26.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 11.0;
                let text_w = ui.available_width() - 20.0 - 11.0;
                ui.allocate_ui_with_layout(
                    vec2(text_w, 10.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(text_w);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.add(
                            egui::Label::new(
                                RichText::new(&toast.title)
                                    .font(style::semibold(style::BODY))
                                    .color(p.text),
                            )
                            .wrap(),
                        );
                        if let Some(d) = &toast.detail {
                            let mut job =
                                confirm::rich_job(d, style::SMALL, p.text_muted, p.text_muted);
                            for s in &mut job.sections {
                                s.format.line_height = Some(17.5);
                            }
                            ui.add(egui::Label::new(job).wrap());
                        }
                        if !toast.buttons.is_empty() {
                            ui.add_space(7.0);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.spacing_mut().interact_size.y = style::TOOLBAR_H;
                                for b in &toast.buttons {
                                    let kind = if b.quiet {
                                        kit::Kind::Ghost
                                    } else {
                                        kit::Kind::Secondary
                                    };
                                    let r = kit::button_ex(ui, kind, None, &b.label, true);
                                    if r.clicked() {
                                        clicked = Some(b.act.clone());
                                    }
                                }
                            });
                        }
                    },
                );
                if kit::icon_button_ex(ui, icons::X, t("Dismiss"), false, 20.0).clicked() {
                    closed = true;
                }
            });
        });
    let rect = frame.response.rect;
    // The life left, as a thin bar along the bottom edge.
    if !toast.sticky {
        let bar = Rect::from_min_max(
            pos2(rect.left() + 14.0, rect.bottom() - 2.0),
            pos2(rect.right() - 14.0, rect.bottom()),
        );
        let left = 1.0 - toast.lived(now).as_secs_f32() / toast.life().as_secs_f32();
        let painter = ui.painter();
        painter.rect_filled(bar, CornerRadius::same(1), p.subtle);
        let mut fill = bar;
        fill.set_width(bar.width() * left.clamp(0.0, 1.0));
        painter.rect_filled(fill, CornerRadius::same(1), p.text_ghost);
    }
    let resp = ui.interact(rect, ui.id().with(("toast", toast.at)), Sense::hover());
    (resp, clicked, closed)
}

/// "Uploaded 4 files. 1 transfer failed." → title and detail; short messages stay whole.
fn split_message(msg: &str) -> (String, Option<String>) {
    if let Some((a, b)) = msg.split_once(": ")
        && a.len() <= 48
        && !b.is_empty()
    {
        return (a.to_string(), Some(b.to_string()));
    }
    if let Some((a, b)) = msg.split_once(". ")
        && a.len() <= 60
        && !b.trim().is_empty()
    {
        return (format!("{a}."), Some(b.to_string()));
    }
    (msg.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_four_newest_last() {
        let mut t = Toasts::default();
        for i in 0..6 {
            t.info(format!("toast {i}"));
        }
        assert_eq!(t.items.len(), MAX);
        assert_eq!(t.items[0].title, "toast 2");
        assert_eq!(t.items[3].title, "toast 5");
    }

    #[test]
    fn errors_stay_and_hover_pauses() {
        let e = Toast::new(Kind::Error, "x");
        assert!(e.sticky);
        let mut info = Toast::new(Kind::Ok, "y");
        let start = info.at;
        info.hovered_since = Some(start);
        // While hovered, no time is lived.
        assert!(info.lived(start + Duration::from_secs(30)) < Duration::from_millis(1));
    }

    #[test]
    fn messages_split_into_title_and_detail() {
        assert_eq!(
            split_message("Uploaded 4 files. 1 transfer failed."),
            (
                "Uploaded 4 files.".to_string(),
                Some("1 transfer failed.".to_string())
            )
        );
        assert_eq!(split_message("Saved."), ("Saved.".to_string(), None));
    }
}
