//! "Update available": background checks, the toolbar button and the update dialog.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Id, RichText};
use parking_lot::Mutex;

use super::style::{self, Tone};
use super::{icons, kit};
use crate::i18n::{t, tf};
use crate::settings::Settings;
use crate::sftp::entry::human_size;
use crate::update::{self, CURRENT, Prepared, Release, Stage};

/// Automatic checks: right after launch, where a new version opens the update dialog by
/// itself, then every few hours while TernaryShell stays open, where it only lights up the toolbar
/// button (a dialog popping up while you type would take your keys).
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(2);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

enum State {
    Idle,
    Checking,
    UpToDate,
    Error(String),
    Available(Release),
    Downloading(Release),
    Ready(Release, Prepared),
}

enum Msg {
    Checked(Result<Option<Release>, String>),
    /// The download goes for this release instead: it came out after the one offered.
    Newer(Release),
    Downloaded(Result<Prepared, String>),
}

/// What the app should do after drawing the dialog.
pub enum UpdateEvent {
    None,
    /// The update is ready: quit (after the usual confirmation) and let it install.
    Restart,
}

pub struct Updater {
    state: State,
    rx: Option<mpsc::Receiver<Msg>>,
    stage: Arc<Mutex<Stage>>,
    pub dialog_open: bool,
    next_check: Instant,
    /// No check has answered yet in this run: an update it finds is shown in the dialog.
    launch_check: bool,
    /// The dialog opens for a new version as soon as no other dialog is on screen.
    popup: bool,
    /// The version already announced (dialog or toast).
    announced: Option<String>,
    /// "Don't show again for this version" is ticked in the dialog.
    skip_ticked: bool,
    /// A version to skip from now on, for the settings.
    skipped: Option<String>,
    /// The previous run's update was reported and its leftovers cleaned (first frame).
    started: bool,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            state: State::Idle,
            rx: None,
            stage: Arc::default(),
            dialog_open: false,
            next_check: Instant::now() + FIRST_CHECK_AFTER,
            launch_check: true,
            popup: false,
            announced: None,
            skip_ticked: false,
            skipped: None,
            started: false,
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Updater {
    fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Starts a check unless one is running; whoever is waiting when it ends gets its answer.
    fn start_check(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Checked(update::check()));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// "Check for Updates…" from the menu.
    pub fn check_now(&mut self, ctx: &egui::Context) {
        self.dialog_open = true;
        match self.state {
            State::Downloading(_) => {}
            // Shown as it is, while a check looks for an even newer version.
            State::Available(_) | State::Ready(..) => self.start_check(ctx),
            // An automatic check that is already running answers this one too.
            _ => {
                self.state = State::Checking;
                self.start_check(ctx);
            }
        }
    }

    /// Closes the dialog. A check still running carries on like an automatic one. A version
    /// the user chose not to see again is dropped (no toolbar button either).
    pub fn close_dialog(&mut self) {
        self.dialog_open = false;
        if std::mem::take(&mut self.skip_ticked)
            && let State::Available(release) = &self.state
        {
            self.skipped = Some(release.version.clone());
            self.state = State::Idle;
        }
        if matches!(
            self.state,
            State::UpToDate | State::Error(_) | State::Checking
        ) {
            self.state = State::Idle;
        }
    }

    /// Shows the dialog with an error.
    fn show_error(&mut self, message: String) {
        self.state = State::Error(message);
        self.dialog_open = true;
    }

    /// First frame: reports how the update the previous run installed went, and removes what
    /// an interrupted one left behind.
    fn report_last_update(&mut self) -> Option<String> {
        std::thread::spawn(update::remove_leftovers);
        match update::take_outcome()? {
            update::Outcome::Installed(version) => (version == CURRENT)
                .then(|| tf("TernaryShell was updated to version {}.", &[CURRENT])),
            update::Outcome::Cancelled => {
                self.show_error(
                    t("The update was not installed because no administrator password was entered.")
                        .to_string(),
                );
                None
            }
            update::Outcome::Failed(reason) => {
                self.show_error(tf("The update could not be installed: {}", &[&reason]));
                None
            }
        }
    }

    /// Runs every frame: schedules automatic checks and collects results. Returns a message to
    /// show as a toast. `can_popup`: no other dialog is open, so the update dialog may appear.
    pub fn tick(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        can_popup: bool,
    ) -> Option<String> {
        let mut toast = None;
        if let Some(version) = self.skipped.take() {
            settings.skipped_version = Some(version);
        }
        if !self.started {
            self.started = true;
            toast = self.report_last_update();
        }
        if let Some(rx) = &self.rx
            && let Ok(msg) = rx.try_recv()
        {
            if !matches!(msg, Msg::Newer(_)) {
                self.rx = None;
            }
            match msg {
                Msg::Newer(release) => {
                    if let State::Downloading(r) = &mut self.state {
                        self.announced = Some(release.version.clone());
                        *r = release;
                    }
                }
                Msg::Checked(result) => {
                    // Someone is looking at "Checking for updates…" (whichever check it was).
                    let manual =
                        self.dialog_open && matches!(self.state, State::Idle | State::Checking);
                    let at_launch = std::mem::take(&mut self.launch_check);
                    settings.last_update_check = Some(now_secs());
                    match result {
                        Ok(Some(release)) => {
                            let skipped = settings.skipped_version.as_deref()
                                == Some(release.version.as_str());
                            // A version already offered (or downloaded) is only replaced by a
                            // newer one.
                            let held = match &self.state {
                                State::Available(r)
                                | State::Downloading(r)
                                | State::Ready(r, _) => Some(r.version.as_str()),
                                _ => None,
                            };
                            let newer = held.is_none_or(|v| update::is_newer(&release.version, v));
                            if newer && (manual || !skipped) {
                                if let State::Ready(_, prepared) = &self.state {
                                    let dir = prepared.dir.clone();
                                    std::thread::spawn(move || std::fs::remove_dir_all(dir));
                                }
                                if self.announced.as_deref() != Some(&release.version) && !manual {
                                    if at_launch {
                                        self.popup = true;
                                    } else if !self.dialog_open {
                                        toast = Some(tf(
                                            "TernaryShell {} is available. Use the Update button to install it.",
                                            &[&release.version],
                                        ));
                                    }
                                }
                                self.announced = Some(release.version.clone());
                                self.state = State::Available(release);
                            }
                        }
                        Ok(None) if manual => self.state = State::UpToDate,
                        Err(e) if manual => self.state = State::Error(e),
                        // Automatic checks fail silently.
                        Ok(None) | Err(_) => {}
                    }
                }
                Msg::Downloaded(result) => {
                    match (std::mem::replace(&mut self.state, State::Idle), result) {
                        (State::Downloading(release), Ok(prepared)) => {
                            self.state = State::Ready(release, prepared);
                            self.dialog_open = true;
                        }
                        (_, Err(e)) => {
                            self.state =
                                State::Error(tf("The update could not be installed: {}", &[&e]));
                            self.dialog_open = true;
                        }
                        (other, Ok(_)) => self.state = other,
                    }
                }
            }
        }
        // A new version found at launch: shown once nothing else is on screen.
        if self.popup && can_popup {
            self.popup = false;
            if matches!(self.state, State::Available(_)) {
                self.dialog_open = true;
            }
        }
        // Automatic checks.
        let now = Instant::now();
        // They go on while an update waits, so a newer release replaces it.
        if settings.check_updates
            && now >= self.next_check
            && !self.busy()
            && matches!(
                self.state,
                State::Idle | State::Available(_) | State::Ready(..)
            )
        {
            self.next_check = now + CHECK_EVERY;
            self.start_check(ctx);
        }
        if matches!(self.state, State::Downloading(_)) {
            ctx.request_repaint_after(Duration::from_millis(200));
        } else if settings.check_updates && !self.busy() {
            // Wake up for the next scheduled check even when nothing else happens.
            let wait = self.next_check.saturating_duration_since(now);
            ctx.request_repaint_after(
                wait.clamp(Duration::from_millis(50), Duration::from_secs(600)),
            );
        }
        toast
    }

    /// The toolbar pill, while an update is available. Returns true when clicked.
    pub fn toolbar_button(&mut self, ui: &mut egui::Ui) -> bool {
        let label = match &self.state {
            State::Available(r) => tf("Update {}", &[&r.version]),
            State::Downloading(_) => match *self.stage.lock() {
                Stage::Downloading { done, total } if total > 0 => tf(
                    "Updating {}%",
                    &[&format!("{}", (done as f64 / total as f64 * 100.0).floor())],
                ),
                _ => t("Updating…").to_string(),
            },
            State::Ready(..) => t("Restart to Update").to_string(),
            _ => return false,
        };
        let p = style::pal(ui);
        let g =
            ui.painter()
                .layout_no_wrap(label.clone(), style::medium(style::SMALL), p.accent_text);
        let w = 10.0 + 14.0 + 6.0 + g.size().x + 10.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 26.0), egui::Sense::click());
        let painter = ui.painter();
        let fill = if resp.hovered() {
            style::soft(p.accent, if p.dark { 0.24 } else { 0.16 })
        } else {
            p.accent_soft
        };
        painter.rect_filled(rect, egui::CornerRadius::same(13), fill);
        painter.text(
            egui::pos2(rect.left() + 10.0 + 7.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            super::icons::ARROW_CIRCLE_UP,
            style::icon_font(14.0),
            p.accent_text,
        );
        painter.galley(
            egui::pos2(
                rect.left() + 10.0 + 14.0 + 6.0,
                rect.center().y - g.size().y / 2.0,
            ),
            g,
            p.accent_text,
        );
        let resp = resp.on_hover_text(t("A new version of TernaryShell is available"));
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
        #[cfg(debug_assertions)]
        crate::devtest::record("update-button", resp.rect);
        let clicked = resp.clicked();
        if clicked {
            self.dialog_open = true;
        }
        clicked
    }

    fn start_download(&mut self, ctx: &egui::Context, release: Release) {
        let (tx, rx) = mpsc::channel();
        let stage = self.stage.clone();
        *stage.lock() = Stage::Starting;
        let ctx = ctx.clone();
        let offered = release.clone();
        std::thread::spawn(move || {
            // Always the latest: a release that came out since this one was found (TernaryShell
            // open for days) is installed instead, not after another update.
            let release = match update::check() {
                Ok(Some(latest)) if update::is_newer(&latest.version, &offered.version) => {
                    let _ = tx.send(Msg::Newer(latest.clone()));
                    latest
                }
                _ => offered,
            };
            let _ = tx.send(Msg::Downloaded(update::download(&release, &stage)));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
        self.state = State::Downloading(release);
    }

    /// Hands the downloaded app to the installer helper; the app must quit right after.
    pub fn launch_installer(&self) -> Result<(), String> {
        let State::Ready(_, prepared) = &self.state else {
            return Err("no update is ready".into());
        };
        update::launch_installer(prepared)
    }

    pub fn show_dialog(&mut self, ctx: &egui::Context, settings: &mut Settings) -> UpdateEvent {
        if !self.dialog_open {
            return UpdateEvent::None;
        }
        let mut event = UpdateEvent::None;
        let mut close = false;
        let mut install_release = None;
        let screen = ctx.content_rect();
        let modal = style::modal(ctx, Id::new("update-dialog")).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            match &self.state {
                State::Idle | State::Checking => {
                    ui.set_width(420.0);
                    kit::dialog_header(ui, Some((icons::ARROW_CIRCLE_UP, Tone::Neutral)), t("Software Update"), None, false);
                    kit::dialog_body(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            kit::spinner(ui, 14.0);
                            ui.label(t("Checking for updates…"));
                        });
                    });
                    kit::dialog_footer(ui, |_| {}, |ui| {
                        if kit::secondary(ui, None, t("Cancel")).clicked() {
                            close = true;
                        }
                    });
                }
                State::UpToDate => {
                    ui.set_width(380.0);
                    kit::dialog_header(ui, Some((icons::SEAL_CHECK, Tone::Ok)), t("TernaryShell is up to date"), Some(&tf("Version {} is the latest.", &[CURRENT])), false);
                    ui.add_space(12.0);
                    kit::dialog_footer(ui, |_| {}, |ui| {
                        if kit::primary(ui, t("OK")).clicked() {
                            close = true;
                        }
                    });
                }
                State::Error(e) => {
                    let e = e.clone();
                    ui.set_width(420.0);
                    kit::dialog_header(ui, Some((icons::WARNING_CIRCLE, Tone::Err)), t("Software Update"), None, false);
                    kit::dialog_body(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(e).color(p.err)).wrap());
                    });
                    kit::dialog_footer(ui, |_| {}, |ui| {
                        if kit::primary(ui, t("OK")).clicked() {
                            close = true;
                        }
                        if kit::secondary(ui, None, t("Open Releases Page")).clicked() {
                            update::open_page(&format!("https://github.com/{}/releases/latest", update::REPO));
                        }
                    });
                }
                State::Available(release) => {
                    let release = release.clone();
                    ui.set_width(560.0f32.min(screen.width() - 60.0));
                    // The app's icon, the new version, and what it is.
                    egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 22, bottom: 6 }).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let (r, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                            super::icons::paint_logo(ui.painter(), r);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.add_space(6.0);
                                ui.label(RichText::new(tf("TernaryShell {} is available", &[&release.version])).font(style::semibold(style::TITLE)).color(p.text));
                                let mut sub = vec![tf("You have {}", &[CURRENT])];
                                if let Some(d) = &release.date {
                                    sub.push(tf("released {}", &[d]));
                                }
                                if let Some(a) = &release.download {
                                    sub.push(human_size(a.size));
                                }
                                ui.label(RichText::new(sub.join(" · ")).font(style::regular(style::SMALL)).color(p.text_muted));
                            });
                        });
                    });
                    let notes_h = (screen.height() - 420.0).clamp(120.0, 190.0);
                    let installable = update::installable(&release);
                    kit::dialog_body(ui, |ui| {
                        ui.add_space(2.0);
                        release_notes(ui, &release.notes, notes_h);
                        if !installable {
                            ui.add_space(8.0);
                            style::hint(ui, update::manual_reason());
                        }
                        ui.add_space(12.0);
                        let skip = style::checkbox(ui, &mut self.skip_ticked, RichText::new(tf("Don't show again for {}", &[&release.version])).color(p.text));
                        #[cfg(debug_assertions)]
                        crate::devtest::record("update-skip", skip.rect);
                        let _ = skip;
                    });
                    kit::dialog_footer(ui, |ui| {
                        if kit::button_ex(ui, kit::Kind::Ghost, Some(icons::ARROW_SQUARE_OUT), t("View on GitHub"), false).clicked() {
                            update::open_page(&release.page);
                        }
                    }, |ui| {
                        if installable {
                            let install = kit::primary(ui, t("Install Update"));
                            #[cfg(debug_assertions)]
                            crate::devtest::record("update-install", install.rect);
                            if install.clicked() {
                                install_release = Some(release.clone());
                            }
                        } else if kit::primary(ui, t("Download…")).clicked() {
                            update::open_page(&release.page);
                            close = true;
                        }
                        let later = kit::secondary(ui, None, t("Later"));
                        #[cfg(debug_assertions)]
                        crate::devtest::record("update-close", later.rect);
                        if later.clicked() {
                            close = true;
                        }
                    });
                }
                State::Downloading(release) => {
                    let version = release.version.clone();
                    ui.set_width(380.0);
                    kit::dialog_header(ui, None, &tf("Updating to {}", &[&version]), None, false);
                    let stage = self.stage.lock().clone();
                    let (fraction, text) = match stage {
                        Stage::Starting => (0.0, t("Starting…").to_string()),
                        Stage::Downloading { done, total } => (
                            if total > 0 { done as f32 / total as f32 } else { 0.0 },
                            tf("Downloading… {} of {}", &[&human_size(done), &human_size(total)]),
                        ),
                        Stage::Verifying => (1.0, t("Verifying…").to_string()),
                    };
                    kit::dialog_body(ui, |ui| {
                        ui.add_space(4.0);
                        let w = ui.available_width();
                        kit::progress_bar(ui, fraction, w, Tone::Accent);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(text).font(style::regular(style::SMALL)).color(p.text));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("{}%", (fraction * 100.0).floor())).font(style::regular(style::SMALL)).color(p.text_faint));
                            });
                        });
                    });
                    kit::dialog_footer(ui, |_| {}, |ui| {
                        if kit::secondary(ui, None, t("Hide")).clicked() {
                            close = true;
                        }
                    });
                }
                State::Ready(release, _) => {
                    let version = release.version.clone();
                    ui.set_width(380.0);
                    kit::dialog_header(ui, Some((icons::CHECK_CIRCLE, Tone::Ok)), &tf("{} is ready to install", &[&version]), Some(t("Verified (size and SHA-256).")), false);
                    kit::dialog_body(ui, |ui| {
                        ui.add(egui::Label::new(t("TernaryShell quits, installs the new version and opens again. Open sessions are closed.")).wrap());
                    });
                    kit::dialog_footer(ui, |_| {}, |ui| {
                        let restart = kit::primary(ui, t("Restart Now"));
                        #[cfg(debug_assertions)]
                        crate::devtest::record("update-restart", restart.rect);
                        if restart.clicked() {
                            event = UpdateEvent::Restart;
                            close = true;
                        }
                        if kit::secondary(ui, None, t("Later")).clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if super::dialogs::escape_only(ctx, &modal) {
            close = true;
        }
        if let Some(release) = install_release {
            self.skip_ticked = false;
            self.start_download(ctx, release);
        }
        if close {
            self.close_dialog();
            if let Some(version) = self.skipped.take() {
                settings.skipped_version = Some(version);
            }
        }
        event
    }
}

/// Marks where the release notes end and the web-only part (how to install) begins.
const NOTES_END: &str = "<!-- ternaryshell:end-of-notes -->";

/// One piece of the release notes.
#[derive(Debug, PartialEq)]
enum Block {
    Heading(String),
    /// A list item, with its nesting level.
    Item(usize, String),
    Paragraph(String),
    Code(String),
}

/// A small Markdown reader for release notes: headings, (nested) lists, paragraphs and code
/// blocks; lines that continue a list item or paragraph are joined to it. Stops where the
/// install instructions begin.
fn parse_notes(notes: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut in_code = false;
    // The previous line continues: a list item or paragraph that a next line may extend.
    let mut open = false;
    for line in notes.lines() {
        let line = line.trim_end();
        if line.trim() == NOTES_END || line == "## Install" {
            break;
        }
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            open = false;
            continue;
        }
        if in_code {
            blocks.push(Block::Code(line.to_string()));
            continue;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with("<!--") {
            open = false;
        } else if trimmed.starts_with('#') {
            blocks.push(Block::Heading(
                trimmed.trim_start_matches('#').trim().to_string(),
            ));
            open = false;
        } else if let Some(text) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            blocks.push(Block::Item(indent / 2, text.trim().to_string()));
            open = true;
        } else if open
            && let Some(Block::Item(_, text) | Block::Paragraph(text)) = blocks.last_mut()
        {
            text.push(' ');
            text.push_str(trimmed);
        } else {
            blocks.push(Block::Paragraph(trimmed.to_string()));
            open = true;
        }
    }
    blocks
}

/// Inline Markdown: **bold** stands out, `code` is monospace, links show their text.
fn inline(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let body = egui::TextStyle::Body.resolve(ui.style());
    let mono = egui::TextStyle::Monospace.resolve(ui.style());
    let normal = ui.visuals().text_color();
    let strong = ui.visuals().strong_text_color();
    // [text](url) → text
    let mut plain = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        match after.find("](").and_then(|close| {
            after[close + 2..]
                .find(')')
                .map(|end| (close, close + 2 + end))
        }) {
            Some((close, end)) => {
                plain.push_str(&rest[..open]);
                plain.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            None => {
                plain.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    let (mut bold, mut code) = (false, false);
    let mut chunk = String::new();
    let mut chars = plain.chars().peekable();
    let flush = |job: &mut egui::text::LayoutJob, chunk: &mut String, bold: bool, code: bool| {
        if chunk.is_empty() {
            return;
        }
        let format = egui::TextFormat {
            font_id: if code { mono.clone() } else { body.clone() },
            color: if bold { strong } else { normal },
            ..Default::default()
        };
        job.append(&std::mem::take(chunk), 0.0, format);
    };
    while let Some(c) = chars.next() {
        if c == '*' && chars.peek() == Some(&'*') && !code {
            chars.next();
            flush(&mut job, &mut chunk, bold, code);
            bold = !bold;
        } else if c == '`' {
            flush(&mut job, &mut chunk, bold, code);
            code = !code;
        } else {
            chunk.push(c);
        }
    }
    flush(&mut job, &mut chunk, bold, code);
    job
}

/// Release notes from GitHub (Markdown), in a scrolling box.
fn release_notes(ui: &mut egui::Ui, notes: &str, max_height: f32) {
    let blocks = parse_notes(notes);
    let p = style::pal(ui);
    egui::Frame::NONE
        .fill(p.card)
        .stroke(egui::Stroke::new(1.0, p.border))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .corner_radius(style::RADIUS_LG)
        .show(ui, |ui| {
            ui.style_mut().visuals.override_text_color = Some(p.text_muted);
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, style::regular(12.5));
            ui.spacing_mut().item_spacing.y = 4.0;
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .min_scrolled_height(max_height.min(140.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if blocks.is_empty() {
                        ui.label(RichText::new(t("No release notes.")).weak());
                    }
                    for (i, block) in blocks.iter().enumerate() {
                        match block {
                            Block::Heading(text) => {
                                if i > 0 {
                                    ui.add_space(6.0);
                                }
                                ui.label(
                                    RichText::new(text)
                                        .font(style::semibold(12.5))
                                        .color(p.text),
                                );
                            }
                            Block::Item(level, text) => {
                                ui.horizontal_top(|ui| {
                                    ui.add_space(4.0 + *level as f32 * 16.0);
                                    ui.label(if *level == 0 { "•" } else { "◦" });
                                    ui.vertical(|ui| {
                                        let job = inline(ui, text);
                                        ui.add(egui::Label::new(job).wrap());
                                    });
                                });
                            }
                            Block::Paragraph(text) => {
                                let job = inline(ui, text);
                                ui.add(egui::Label::new(job).wrap());
                            }
                            Block::Code(text) => {
                                ui.label(RichText::new(text).monospace().small());
                            }
                        }
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: version.into(),
            notes: String::new(),
            date: None,
            page: String::new(),
            download: None,
        }
    }

    /// An updater with an automatic check running: the one right after launch, or a later
    /// one. (`started` skips the first-frame report, which reads the real data folder.)
    fn checking_in_background(at_launch: bool) -> (Updater, mpsc::Sender<Msg>) {
        let (tx, rx) = mpsc::channel();
        let updater = Updater {
            rx: Some(rx),
            started: true,
            launch_check: at_launch,
            ..Default::default()
        };
        (updater, tx)
    }

    /// "Check for Updates…" while the automatic check runs, which then answers. Returns the
    /// state the dialog shows.
    fn manual_check_during_automatic_check(answer: Result<Option<Release>, String>) -> State {
        let ctx = egui::Context::default();
        // Shown even when skipped: the user asked.
        let mut settings = Settings {
            skipped_version: Some("9.0.0".into()),
            ..Default::default()
        };
        let (mut updater, tx) = checking_in_background(true);
        updater.check_now(&ctx);
        assert!(matches!(updater.state, State::Checking) && updater.dialog_open);
        tx.send(Msg::Checked(answer)).unwrap();
        assert_eq!(updater.tick(&ctx, &mut settings, false), None);
        assert!(updater.dialog_open && !updater.busy());
        updater.state
    }

    #[test]
    fn manual_check_during_automatic_check_gets_an_answer() {
        assert!(matches!(
            manual_check_during_automatic_check(Ok(None)),
            State::UpToDate
        ));
        assert!(matches!(
            manual_check_during_automatic_check(Err("offline".into())),
            State::Error(e) if e == "offline"
        ));
        assert!(matches!(
            manual_check_during_automatic_check(Ok(Some(release("9.0.0")))),
            State::Available(_)
        ));
    }

    /// Closing the dialog while checking turns the check into an automatic one.
    #[test]
    fn closed_dialog_gets_the_automatic_behavior() {
        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        let (mut updater, tx) = checking_in_background(false);
        updater.check_now(&ctx);
        updater.close_dialog();
        assert!(matches!(updater.state, State::Idle) && !updater.dialog_open);
        tx.send(Msg::Checked(Ok(Some(release("9.0.0"))))).unwrap();
        assert!(
            updater.tick(&ctx, &mut settings, true).is_some(),
            "announced"
        );
        assert!(matches!(updater.state, State::Available(_)) && !updater.dialog_open);

        let (mut updater, tx) = checking_in_background(false);
        tx.send(Msg::Checked(Err("offline".into()))).unwrap();
        assert_eq!(updater.tick(&ctx, &mut settings, true), None);
        assert!(matches!(updater.state, State::Idle) && !updater.dialog_open);
    }

    fn prepared(version: &str) -> Prepared {
        Prepared {
            version: version.into(),
            dir: std::path::PathBuf::from("/nonexistent/ternaryshell-update-test"),
            file: std::path::PathBuf::new(),
        }
    }

    fn version(state: &State) -> Option<(&'static str, &str)> {
        match state {
            State::Available(r) => Some(("available", &r.version)),
            State::Downloading(r) => Some(("downloading", &r.version)),
            State::Ready(r, _) => Some(("ready", &r.version)),
            _ => None,
        }
    }

    /// TernaryShell stays open while two releases come out: the second replaces the first, even one
    /// already downloaded, and finding the same version again changes nothing.
    #[test]
    fn a_newer_release_replaces_the_one_offered() {
        let ctx = egui::Context::default();
        let check = |state: State, found: &str| {
            let (mut updater, tx) = checking_in_background(false);
            updater.state = state;
            tx.send(Msg::Checked(Ok(Some(release(found))))).unwrap();
            let toast = updater.tick(&ctx, &mut Settings::default(), true);
            (updater, toast)
        };
        let (updater, toast) = check(State::Available(release("9.0.0")), "9.0.1");
        assert_eq!(version(&updater.state), Some(("available", "9.0.1")));
        assert!(toast.is_some(), "announced");
        let (updater, _) = check(State::Ready(release("9.0.0"), prepared("9.0.0")), "9.0.1");
        assert_eq!(version(&updater.state), Some(("available", "9.0.1")));
        let (updater, toast) = check(State::Ready(release("9.0.1"), prepared("9.0.1")), "9.0.1");
        assert_eq!(version(&updater.state), Some(("ready", "9.0.1")));
        assert_eq!(toast, None);
        let (updater, _) = check(State::Available(release("9.0.1")), "9.0.0");
        assert_eq!(
            version(&updater.state),
            Some(("available", "9.0.1")),
            "never older"
        );
    }

    /// Installing an update goes for the release that came out after the one offered.
    #[test]
    fn the_download_goes_for_the_newest_release() {
        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        let (mut updater, tx) = checking_in_background(false);
        updater.state = State::Downloading(release("9.0.0"));
        tx.send(Msg::Newer(release("9.0.1"))).unwrap();
        updater.tick(&ctx, &mut settings, true);
        assert!(updater.busy(), "still downloading");
        assert_eq!(version(&updater.state), Some(("downloading", "9.0.1")));
        tx.send(Msg::Downloaded(Ok(prepared("9.0.1")))).unwrap();
        updater.tick(&ctx, &mut settings, true);
        assert_eq!(version(&updater.state), Some(("ready", "9.0.1")));
        assert!(updater.dialog_open && !updater.busy());
    }

    #[test]
    fn notes_are_read_like_markdown() {
        let notes = "## Fixes\n\n- **TernaryShell no longer quits** when you close\n  a tab.\n- Dialogs\n  - Esc closes `Export` again.\n\nSee [the page](https://x) for more.\n\n<!-- ternaryshell:end-of-notes -->\n\n## Install\n\ncurl …";
        assert_eq!(
            parse_notes(notes),
            [
                Block::Heading("Fixes".into()),
                Block::Item(
                    0,
                    "**TernaryShell no longer quits** when you close a tab.".into()
                ),
                Block::Item(0, "Dialogs".into()),
                Block::Item(1, "Esc closes `Export` again.".into()),
                Block::Paragraph("See [the page](https://x) for more.".into()),
            ]
        );
        // Releases made before the marker existed stop at their install section.
        assert_eq!(
            parse_notes("- One\n\n## Install\n\n- curl"),
            [Block::Item(0, "One".into())]
        );
    }

    /// A new version found right after launch opens the dialog, once no other dialog is up;
    /// a skipped version doesn't, and later checks only announce it.
    #[test]
    fn launch_check_opens_the_dialog() {
        let ctx = egui::Context::default();
        let mut settings = Settings::default();
        let (mut updater, tx) = checking_in_background(true);
        tx.send(Msg::Checked(Ok(Some(release("9.0.0"))))).unwrap();
        assert_eq!(updater.tick(&ctx, &mut settings, false), None, "no toast");
        assert!(!updater.dialog_open, "waits for the other dialog");
        assert_eq!(updater.tick(&ctx, &mut settings, true), None);
        assert!(updater.dialog_open && matches!(updater.state, State::Available(_)));
        updater.close_dialog();
        assert_eq!(updater.tick(&ctx, &mut settings, true), None);
        assert!(!updater.dialog_open, "Later: not shown again in this run");

        let mut settings = Settings {
            skipped_version: Some("9.0.0".into()),
            ..Default::default()
        };
        let (mut updater, tx) = checking_in_background(true);
        tx.send(Msg::Checked(Ok(Some(release("9.0.0"))))).unwrap();
        assert_eq!(updater.tick(&ctx, &mut settings, true), None);
        assert!(!updater.dialog_open && matches!(updater.state, State::Idle));
    }
}
