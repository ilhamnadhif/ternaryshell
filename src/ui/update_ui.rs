//! "Update available": background checks, the toolbar button and the update dialog.

use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Id, RichText};
use parking_lot::Mutex;

use super::style::{self, Tone};
use super::{confirm, icons, kit};
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
    /// When and at how many bytes the download was first seen, for its speed.
    speed: Option<(Instant, u64)>,
    /// When the dialog or card appeared (Enter typed just before must not install).
    shown_at: Option<Instant>,
    /// Debug screenshots: draw a fake release as installable.
    force_installable: bool,
}

/// Where updates stand (for About).
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Never checked in this run or before.
    Unknown,
    UpToDate,
    /// This version can be installed.
    Available(String),
    /// Downloading or verifying.
    Busy,
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
            speed: None,
            shown_at: None,
            force_installable: false,
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

    /// The title-row pill while an update is available, downloading or ready. Returns true
    /// when clicked (the dialog or card opens).
    pub fn toolbar_button(&mut self, ui: &mut egui::Ui) -> bool {
        let stage = *self.stage.lock();
        let (label, progress) = match &self.state {
            State::Available(r) => (tf("Update {}", &[&r.version]), None),
            State::Downloading(_) => match stage {
                Stage::Downloading { done, total } if total > 0 => {
                    let f = done as f64 / total as f64;
                    (
                        tf("Updating {}%", &[&format!("{}", (f * 100.0).floor())]),
                        Some(f as f32),
                    )
                }
                Stage::Verifying(_) => (t("Verifying…").to_string(), Some(1.0)),
                _ => (t("Updating…").to_string(), Some(0.0)),
            },
            State::Ready(..) => (t("Restart to Update").to_string(), None),
            _ => return false,
        };
        let p = style::pal(ui);
        let busy = progress.is_some();
        let color = if busy { p.text_muted } else { p.accent_text };
        let g = ui
            .painter()
            .layout_no_wrap(label.clone(), style::medium(style::SMALL), color);
        let w = 10.0 + 16.0 + 6.0 + g.size().x + 10.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::click());
        let painter = ui.painter();
        let fill = match (busy, resp.hovered()) {
            (true, false) => p.hover,
            (true, true) => p.subtle,
            (false, false) => p.accent_tint,
            (false, true) => style::soft(p.accent, if p.dark { 0.26 } else { 0.18 }),
        };
        painter.rect_filled(rect, egui::CornerRadius::same(12), fill);
        let icon_c = egui::pos2(rect.left() + 10.0 + 8.0, rect.center().y);
        match progress {
            Some(f) => style::progress_ring(painter, icon_c, 7.0, f, p.dark),
            None => {
                painter.text(
                    icon_c,
                    egui::Align2::CENTER_CENTER,
                    icons::ARROW_CIRCLE_UP,
                    style::icon_font(16.0),
                    color,
                );
            }
        }
        painter.galley(
            egui::pos2(
                rect.left() + 10.0 + 16.0 + 6.0,
                rect.center().y - g.size().y / 2.0,
            ),
            g,
            color,
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
        self.speed = None;
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

    /// Debug builds: shows one state of the update with a made-up release, for screenshots.
    #[cfg(debug_assertions)]
    pub fn dev_state(&mut self, what: &str) {
        let release = Release {
            version: "1.6.1".into(),
            notes: "### Changed\n- **Files** follows the terminal's folder: `cd` in the shell and the remote list goes there too.\n- Labels for groups: production tabs get a red line and a **PROD** tag.\n- The command palette shows details of the selected connection.\n\n### Fixed\n- A jump host with a changed key no longer asks twice.\n- Uploading a folder of symlinks keeps the links.\n\n### Tip\n- Press `⌘1`–`⌘9` on Home to open a recent connection.\n".into(),
            date: Some("2026-11-16".into()),
            page: String::new(),
            download: Some(update::Asset {
                name: "TernaryShell.zip".into(),
                url: String::new(),
                size: 15_200_000,
                sha256: Some("0".into()),
            }),
        };
        self.force_installable = true;
        self.dialog_open = true;
        self.state = match what {
            "available" => State::Available(release),
            "downloading" => {
                *self.stage.lock() = Stage::Downloading {
                    done: 8_100_000,
                    total: 15_200_000,
                };
                self.speed = Some((Instant::now() - Duration::from_secs(2), 0));
                State::Downloading(release)
            }
            "verifying" => {
                *self.stage.lock() = Stage::Verifying(2);
                State::Downloading(release)
            }
            "ready" => State::Ready(
                release,
                Prepared {
                    version: "1.6.1".into(),
                    dir: std::path::PathBuf::from("/nonexistent/ternaryshell-dev-update"),
                    file: std::path::PathBuf::new(),
                },
            ),
            "uptodate" => State::UpToDate,
            "checking" => State::Checking,
            "hidden" => {
                self.dialog_open = false;
                *self.stage.lock() = Stage::Downloading {
                    done: 8_100_000,
                    total: 15_200_000,
                };
                State::Downloading(release)
            }
            _ => State::Error("the download does not match its published checksum".into()),
        };
    }

    /// Where updates stand, for About.
    pub fn status(&self, last_check: Option<i64>) -> Status {
        match &self.state {
            State::Available(r) | State::Ready(r, _) => Status::Available(r.version.clone()),
            State::Downloading(_) => Status::Busy,
            State::UpToDate => Status::UpToDate,
            _ if last_check.is_some() => Status::UpToDate,
            _ => Status::Unknown,
        }
    }

    /// The update dialog (a new version, an error) or the card in the bottom right corner
    /// (checking, up to date, downloading, verifying, ready). `sessions`: open sessions, which
    /// a restart closes.
    pub fn show_dialog(
        &mut self,
        ctx: &egui::Context,
        settings: &mut Settings,
        sessions: usize,
    ) -> UpdateEvent {
        let card = matches!(
            self.state,
            State::Idle
                | State::Checking
                | State::UpToDate
                | State::Downloading(_)
                | State::Ready(..)
        );
        if !self.dialog_open || !card {
            set_inset(ctx, 0.0);
        }
        if !self.dialog_open {
            self.shown_at = None;
            return UpdateEvent::None;
        }
        let shown_at = *self.shown_at.get_or_insert_with(Instant::now);
        let mut event = UpdateEvent::None;
        let mut close = false;
        let mut install_release = None;
        if card {
            self.card(ctx, &mut close, &mut event);
        } else {
            let screen = ctx.content_rect();
            let id = Id::new("update-dialog");
            let enter = shown_at.elapsed() >= Duration::from_millis(400)
                && ctx.memory(|m| m.top_modal_layer().is_none_or(|l| l.id == id))
                && ctx.input(|i| {
                    i.events.iter().any(|e| {
                        matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, repeat: false, modifiers, .. } if modifiers.is_none())
                    })
                });
            let modal = style::modal(ctx, id).show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                match &self.state {
                    State::Available(release) => {
                        let release = release.clone();
                        ui.set_width(520.0f32.min(screen.width() - 60.0));
                        let installable = update::installable(&release) || self.force_installable;
                        let (install, closed) = available(
                            ui,
                            &release,
                            installable,
                            &mut self.skip_ticked,
                            sessions,
                            (screen.height() - 470.0).clamp(110.0, 270.0),
                            enter,
                        );
                        if install {
                            install_release = Some(release);
                        }
                        close |= closed;
                    }
                    State::Error(e) => {
                        let e = e.clone();
                        ui.set_width(420.0);
                        confirm::header(
                            ui,
                            icons::WARNING_CIRCLE,
                            Tone::Err,
                            t("Software Update"),
                            None,
                        );
                        let p = style::pal(ui);
                        egui::Frame::NONE
                            .inner_margin(egui::Margin {
                                left: 20,
                                right: 20,
                                top: 14,
                                bottom: 16,
                            })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.add(egui::Label::new(RichText::new(e).color(p.err)).wrap());
                            });
                        confirm::footer_with(
                            ui,
                            |ui| {
                                if kit::button_ex(
                                    ui,
                                    kit::Kind::Ghost,
                                    Some(icons::ARROW_SQUARE_OUT),
                                    t("Open Releases Page"),
                                    true,
                                )
                                .clicked()
                                {
                                    update::open_page(&format!(
                                        "https://github.com/{}/releases/latest",
                                        update::REPO
                                    ));
                                }
                            },
                            |ui| {
                                if kit::button_kbd(
                                    ui,
                                    kit::Kind::Primary,
                                    None,
                                    t("OK"),
                                    Some("↩"),
                                    false,
                                    false,
                                )
                                .clicked()
                                    || enter
                                {
                                    close = true;
                                }
                            },
                        );
                    }
                    _ => {}
                }
            });
            if super::dialogs::escape_only(ctx, &modal) {
                close = true;
            }
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

    /// The card for the quiet phases, in the bottom right corner above the toasts' place.
    fn card(&mut self, ctx: &egui::Context, close: &mut bool, event: &mut UpdateEvent) {
        let stage = *self.stage.lock();
        if let Stage::Downloading { done, .. } = stage
            && self.speed.is_none_or(|(_, from)| done < from)
        {
            self.speed = Some((Instant::now(), done));
        }
        let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        let area = egui::Area::new(Id::new("update-card"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-20.0, -44.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let p = style::pal(ui);
                egui::Frame::NONE
                    .fill(p.dialog)
                    .corner_radius(egui::CornerRadius::same(style::RADIUS_LG))
                    .shadow(ui.visuals().popup_shadow)
                    .stroke(ui.visuals().window_stroke)
                    .inner_margin(egui::Margin { left: 16, right: 14, top: 14, bottom: 14 })
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 30.0);
                        ui.spacing_mut().item_spacing.y = 10.0;
                        match &self.state {
                            State::Idle | State::Checking => {
                                card_title_row(ui, true, t("Checking for updates…"), close);
                            }
                            State::UpToDate => {
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 12.0;
                                    let (r, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                                    confirm::paint_tile(ui.painter(), r, icons::CHECK_CIRCLE, Tone::Ok, &p);
                                    let w = ui.available_width() - 50.0;
                                    ui.allocate_ui_with_layout(egui::vec2(w, 32.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.label(RichText::new(t("TernaryShell is up to date")).font(style::semibold(style::BODY)).color(p.text));
                                        ui.label(RichText::new(tf("Version {} is the latest.", &[CURRENT])).font(style::regular(style::SMALL)).color(p.text_muted));
                                    });
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if kit::secondary_sm(ui, None, t("OK")).clicked() {
                                            *close = true;
                                        }
                                    });
                                });
                            }
                            State::Downloading(release) => match stage {
                                Stage::Verifying(step) => {
                                    card_title_row(ui, true, t("Verifying…"), close);
                                    let total = release.download.as_ref().map_or(0, |a| a.size);
                                    let checks = [
                                        tf("Size matches ({})", &[&human_size(total)]),
                                        t("SHA-256 matches GitHub").to_string(),
                                        if cfg!(target_os = "macos") {
                                            t("Code signature and bundle id").to_string()
                                        } else {
                                            t("Package contents").to_string()
                                        },
                                    ];
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 6.0;
                                        ui.spacing_mut().interact_size.y = 18.0;
                                        for (i, text) in checks.iter().enumerate() {
                                            check_row(ui, i as u8, step, text);
                                        }
                                    });
                                }
                                _ => {
                                    card_title_row(ui, false, &tf("Updating to {}", &[&release.version]), close);
                                    let (fraction, left) = match stage {
                                        Stage::Downloading { done, total } => (
                                            if total > 0 { done as f32 / total as f32 } else { 0.0 },
                                            tf("Downloading… {} of {}", &[&human_size(done), &human_size(total)]),
                                        ),
                                        _ => (0.0, t("Starting…").to_string()),
                                    };
                                    let w = ui.available_width();
                                    kit::progress_bar(ui, fraction, w, Tone::Accent);
                                    let right = match (stage, self.speed) {
                                        (Stage::Downloading { done, total }, Some((at, from))) if done > from => {
                                            let secs = at.elapsed().as_secs_f64().max(0.001);
                                            let rate = (done - from) as f64 / secs;
                                            let left_s = ((total.saturating_sub(done)) as f64 / rate.max(1.0)).ceil() as u64;
                                            format!("{}/s · {}", human_size(rate as u64), short_duration(left_s))
                                        }
                                        _ => String::new(),
                                    };
                                    ui.horizontal(|ui| {
                                        let small = |s: &str| RichText::new(s.to_string()).font(style::regular(style::SMALL)).color(p.text_faint);
                                        ui.label(small(&left));
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(small(&right));
                                        });
                                    });
                                }
                            },
                            State::Ready(release, _) => {
                                let version = release.version.clone();
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    let (r, _) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), egui::Sense::hover());
                                    confirm::paint_tile(ui.painter(), r, icons::SEAL_CHECK, Tone::Ok, &p);
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 1.0;
                                        ui.label(RichText::new(tf("{} is ready to install", &[&version])).font(style::semibold(style::BODY)).color(p.text));
                                        ui.add(egui::Label::new(RichText::new(t("TernaryShell restarts and opens your connections again.")).font(style::regular(style::SMALL)).color(p.text_muted)).wrap());
                                    });
                                });
                                let w = ui.available_width();
                                ui.allocate_ui_with_layout(egui::vec2(w, style::TOOLBAR_H), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.spacing_mut().item_spacing.x = 6.0;
                                    let restart = kit::button_ex(ui, kit::Kind::Primary, None, t("Restart to Update"), true);
                                    #[cfg(debug_assertions)]
                                    crate::devtest::record("update-restart", restart.rect);
                                    if restart.clicked() {
                                        *event = UpdateEvent::Restart;
                                        *close = true;
                                    }
                                    if kit::button_ex(ui, kit::Kind::Ghost, None, t("Later"), true).clicked() {
                                        *close = true;
                                    }
                                });
                            }
                            _ => {}
                        }
                    });
            });
        if esc && !matches!(self.state, State::Downloading(_)) {
            *close = true;
        }
        set_inset(ctx, area.response.rect.height() + 8.0);
        if matches!(self.state, State::Checking | State::Idle)
            || matches!(stage, Stage::Verifying(_))
        {
            // The spinners turn.
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
}

const CARD_W: f32 = 396.0;

/// Tells the toasts how much room the update card takes in their corner.
fn set_inset(ctx: &egui::Context, h: f32) {
    let id = Id::new(super::toasts::INSET_ID);
    let old: f32 = ctx.data(|d| d.get_temp(id).unwrap_or(0.0));
    if (old - h).abs() > 0.5 {
        ctx.data_mut(|d| d.insert_temp(id, h));
        ctx.request_repaint();
    }
}

/// A card's first row: a spinner (when `busy`), the title, and × to hide the card.
fn card_title_row(ui: &mut egui::Ui, busy: bool, title: &str, close: &mut bool) {
    let p = style::pal(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if busy {
            kit::spinner(ui, 14.0);
        }
        ui.label(
            RichText::new(title)
                .font(style::semibold(style::BODY))
                .color(p.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if kit::icon_button_ex(ui, icons::X, t("Hide"), false, 20.0).clicked() {
                *close = true;
            }
        });
    });
}

/// One check of the verification: done (✓), running (spinner) or waiting.
fn check_row(ui: &mut egui::Ui, i: u8, step: u8, text: &str) {
    let p = style::pal(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        if i < step {
            ui.painter().text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                icons::CHECK,
                style::icon_font(14.0),
                p.ok,
            );
        } else if i == step {
            kit::paint_spinner(ui, r.center(), 6.0);
        } else {
            ui.painter()
                .circle_stroke(r.center(), 5.0, egui::Stroke::new(1.5, p.border_strong));
        }
        let color = if i <= step {
            p.text_muted
        } else {
            p.text_faint
        };
        ui.label(
            RichText::new(text)
                .font(style::regular(style::SMALL))
                .color(color),
        );
    });
}

/// "2 s", "3 min".
fn short_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min", secs.div_ceil(60))
    }
}

/// "2026-11-16" → "Nov 16, 2026".
fn release_date(date: &str) -> String {
    let mut parts = date.split('-');
    let (Some(y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return date.to_string();
    };
    let month = match m {
        "01" => t("Jan"),
        "02" => t("Feb"),
        "03" => t("Mar"),
        "04" => t("Apr"),
        "05" => t("May"),
        "06" => t("Jun"),
        "07" => t("Jul"),
        "08" => t("Aug"),
        "09" => t("Sep"),
        "10" => t("Oct"),
        "11" => t("Nov"),
        "12" => t("Dec"),
        _ => return date.to_string(),
    };
    format!("{month} {}, {y}", d.trim_start_matches('0'))
}

/// The dialog for a new version: what it is, what's new, what installing does. Returns
/// (install, close).
fn available(
    ui: &mut egui::Ui,
    release: &Release,
    installable: bool,
    skip: &mut bool,
    sessions: usize,
    notes_h: f32,
    enter: bool,
) -> (bool, bool) {
    let p = style::pal(ui);
    let (mut install, mut close) = (false, false);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 22,
            right: 22,
            top: 22,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let (r, _) = ui.allocate_exact_size(egui::vec2(56.0, 56.0), egui::Sense::hover());
                let shadow = egui::Shadow {
                    offset: [0, 6],
                    blur: 18,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(if p.dark { 77 } else { 40 }),
                };
                ui.painter()
                    .add(shadow.as_shape(r, egui::CornerRadius::same(14)));
                icons::paint_logo(ui.painter(), r);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(
                        RichText::new(tf("TernaryShell {} is available", &[&release.version]))
                            .font(style::semibold(16.0))
                            .color(p.text),
                    );
                    let mut sub = vec![tf("You have {}", &[CURRENT])];
                    if let Some(d) = &release.date {
                        sub.push(tf("released {}", &[&release_date(d)]));
                    }
                    ui.label(
                        RichText::new(sub.join(" · "))
                            .font(style::regular(style::BODY))
                            .color(p.text_muted),
                    );
                    ui.add_space(7.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        version_chip(ui, CURRENT, false);
                        kit::icon(ui, icons::ARROW_RIGHT, 13.0, p.text_faint);
                        version_chip(ui, &release.version, true);
                        if let Some(a) = &release.download {
                            confirm::small_badge(ui, &human_size(a.size), Tone::Neutral, None);
                            if a.sha256.is_some() {
                                confirm::small_badge(
                                    ui,
                                    t("Verified download"),
                                    Tone::Ok,
                                    Some(icons::SEAL_CHECK),
                                );
                            }
                        }
                    });
                });
            });
        });
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 12,
            bottom: 18,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 12.0;
            ui.label(style::section_label(t("What's new"), p.text_muted));
            release_notes(ui, &release.notes, notes_h);
            callout(ui, sessions);
            if !installable {
                style::hint(ui, update::manual_reason());
            }
            let skip_r = style::checkbox(
                ui,
                skip,
                RichText::new(tf("Don't show again for {}", &[&release.version]))
                    .color(p.text_muted),
            );
            #[cfg(debug_assertions)]
            crate::devtest::record("update-skip", skip_r.rect);
            let _ = skip_r;
        });
    confirm::footer_with(
        ui,
        |ui| {
            if kit::button_ex(
                ui,
                kit::Kind::Ghost,
                Some(icons::ARROW_SQUARE_OUT),
                t("View on GitHub"),
                true,
            )
            .clicked()
            {
                update::open_page(&release.page);
            }
        },
        |ui| {
            if installable {
                let r = kit::button_kbd(
                    ui,
                    kit::Kind::Primary,
                    None,
                    t("Install Update"),
                    Some("↩"),
                    false,
                    false,
                );
                #[cfg(debug_assertions)]
                crate::devtest::record("update-install", r.rect);
                if r.clicked() || enter {
                    install = true;
                }
            } else if kit::button_kbd(
                ui,
                kit::Kind::Primary,
                None,
                t("Download…"),
                Some("↩"),
                false,
                false,
            )
            .clicked()
                || enter
            {
                update::open_page(&release.page);
                close = true;
            }
            let later = kit::secondary(ui, None, t("Later"));
            #[cfg(debug_assertions)]
            crate::devtest::record("update-close", later.rect);
            if later.clicked() {
                close = true;
            }
        },
    );
    (install, close)
}

/// `1.6.0` in a 22 px monospaced chip; the new version in the accent.
fn version_chip(ui: &mut egui::Ui, version: &str, new: bool) {
    let p = style::pal(ui);
    let (font, color, fill) = if new {
        (
            egui::FontId::new(11.5, egui::FontFamily::Monospace),
            p.accent_text,
            p.accent_tint,
        )
    } else {
        (style::mono(11.5), p.text_muted, p.hover)
    };
    let g = ui
        .painter()
        .layout_no_wrap(version.to_string(), font, color);
    let (r, _) = ui.allocate_exact_size(egui::vec2(g.size().x + 16.0, 22.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(r, egui::CornerRadius::same(6), fill);
    let pos = r.center() - g.size() / 2.0;
    if new {
        // Monospace has no bold face: draw it twice, a hair apart.
        ui.painter()
            .galley(pos + egui::vec2(0.35, 0.0), g.clone(), color);
    }
    ui.painter().galley(pos, g, color);
}

/// What installing does to the open sessions.
fn callout(ui: &mut egui::Ui, sessions: usize) {
    let p = style::pal(ui);
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat {
        font_id: style::regular(style::SMALL),
        color: p.text_muted,
        line_height: Some(18.0),
        ..Default::default()
    };
    job.append(
        t("TernaryShell quits, installs the new version and opens again."),
        0.0,
        plain.clone(),
    );
    if sessions > 0 {
        job.append(" ", 0.0, plain.clone());
        job.append(
            &tf("{} open session(s)", &[&sessions.to_string()]),
            0.0,
            egui::TextFormat {
                font_id: style::semibold(style::SMALL),
                color: p.text,
                line_height: Some(18.0),
                ..Default::default()
            },
        );
        job.append(
            &format!(" {}", t("are closed; they reconnect after the restart.")),
            0.0,
            plain,
        );
    }
    egui::Frame::NONE
        .fill(p.hover)
        .corner_radius(egui::CornerRadius::same(style::RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.vertical(|ui| {
                    ui.add_space(1.0);
                    kit::icon(ui, icons::INFO, 16.0, p.text_muted);
                });
                ui.add(egui::Label::new(job).wrap());
            });
        });
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

/// Inline Markdown: **bold** stands out, `code` is monospace on a soft fill, links show their
/// text.
fn inline(text: &str, p: &style::Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
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
            font_id: if code {
                style::mono(11.5)
            } else if bold {
                style::semibold(style::BODY)
            } else {
                style::regular(style::BODY)
            },
            color: if bold || code { p.text } else { p.text_muted },
            background: if code {
                p.hover
            } else {
                egui::Color32::TRANSPARENT
            },
            line_height: Some(20.0),
            ..Default::default()
        };
        let text = std::mem::take(chunk);
        if code {
            job.append(&format!("\u{2009}{text}\u{2009}"), 0.0, format);
        } else {
            job.append(&text, 0.0, format);
        }
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

/// A heading of the notes as a badge: CHANGED (accent), FIXED (green), others neutral.
fn heading_tone(text: &str) -> Tone {
    let l = text.to_ascii_lowercase();
    if l.contains("fix") {
        Tone::Ok
    } else if l.contains("change") || l.contains("new") || l.contains("add") || l.contains("improv")
    {
        Tone::Accent
    } else if l.contains("security") || l.contains("remov") || l.contains("break") {
        Tone::Warn
    } else {
        Tone::Neutral
    }
}

/// Release notes from GitHub (Markdown), in a scrolling well that fades out at the bottom while
/// there is more.
fn release_notes(ui: &mut egui::Ui, notes: &str, max_height: f32) {
    let blocks = parse_notes(notes);
    let p = style::pal(ui);
    egui::Frame::NONE
        .fill(p.well)
        .stroke(egui::Stroke::new(1.0, p.border))
        .inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: 4,
            bottom: 12,
        })
        .corner_radius(style::RADIUS_MD)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let out = egui::ScrollArea::vertical()
                .max_height(max_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if blocks.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new(t("No release notes.")).color(p.text_faint));
                    }
                    for block in &blocks {
                        match block {
                            Block::Heading(text) => {
                                ui.add_space(12.0);
                                confirm::small_badge(
                                    ui,
                                    &text.to_uppercase(),
                                    heading_tone(text),
                                    None,
                                );
                                ui.add_space(6.0);
                            }
                            Block::Item(level, text) => {
                                ui.add_space(3.0);
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 0.0;
                                    ui.add_space(*level as f32 * 16.0);
                                    let (r, _) = ui.allocate_exact_size(
                                        egui::vec2(16.0, 20.0),
                                        egui::Sense::hover(),
                                    );
                                    ui.painter().circle_filled(
                                        egui::pos2(r.left() + 4.0, r.center().y),
                                        if *level == 0 { 2.2 } else { 1.7 },
                                        p.text_muted,
                                    );
                                    ui.add(egui::Label::new(inline(text, &p)).wrap());
                                });
                                ui.add_space(3.0);
                            }
                            Block::Paragraph(text) => {
                                ui.add_space(4.0);
                                ui.add(egui::Label::new(inline(text, &p)).wrap());
                                ui.add_space(4.0);
                            }
                            Block::Code(text) => {
                                ui.label(RichText::new(text).font(style::mono(11.5)).color(p.text));
                            }
                        }
                    }
                });
            // More below: fade the last lines into the well.
            let more = out.content_size.y - out.state.offset.y - out.inner_rect.height();
            if more > 1.0 {
                let r = out.inner_rect;
                let fade = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 36.0), r.max);
                fade_down(ui.painter(), fade, p.well);
            }
        });
}

/// A vertical gradient from transparent to `color` over `rect`.
fn fade_down(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let mut mesh = egui::Mesh::default();
    let clear = egui::Color32::TRANSPARENT;
    mesh.colored_vertex(rect.left_top(), clear);
    mesh.colored_vertex(rect.right_top(), clear);
    mesh.colored_vertex(rect.right_bottom(), color);
    mesh.colored_vertex(rect.left_bottom(), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
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
