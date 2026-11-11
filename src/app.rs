//! The application: window layout, tabs, sidebar, dialogs and the action dispatcher.

use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, KeyboardShortcut, Theme, Ui, vec2};
#[cfg(target_os = "macos")]
use muda::MenuEvent;
use uuid::Uuid;

use crate::bookmarks::store::NewerSchema;
use crate::bookmarks::transfer::{self, ExportScope, ImportPlan};
use crate::bookmarks::{Auth, Bookmark, BookmarkStore, ssh_config};
use crate::i18n::{self, Language, t, tf};
use crate::paths;
use crate::secrets::{self, SecretKind};
use crate::settings::{FontChoice, Settings, ThemeMode};
use crate::ssh::types::ConnState;
use crate::term::core::GridSize;
use crate::term::scheme::{self, Scheme};
use crate::ui::actions::Action;
use crate::ui::dialogs::{
    self, ConfirmAction, Dialog, DialogResult, ExportDialog, GroupTarget, ImportDialog, Palette,
    PaletteResult, SettingsDialog, Toasts,
};
use crate::ui::editor::{Editor, EditorResult};
use crate::ui::layout::SplitDir;
#[cfg(target_os = "macos")]
use crate::ui::menu::{self, AppMenu};
use crate::ui::sftp_panel::SftpPanel;
use crate::ui::sidebar::{Live, Sidebar, TreeMove};
use crate::ui::tabs::{Env, Tab};
use crate::ui::{fonts, icons, keys, style, terminal};

/// Settings that need work when they change.
#[derive(Clone, PartialEq)]
struct Applied {
    language: Language,
    theme: ThemeMode,
    font: FontChoice,
    cjk: bool,
    terminal: (usize, crate::settings::CursorStyle, bool, String, String),
}

impl Applied {
    fn of(s: &Settings) -> Self {
        Self {
            language: s.language,
            theme: s.theme,
            font: fonts::effective_choice(&s.font),
            cjk: s.load_cjk_font,
            terminal: (
                s.scrollback,
                s.cursor_style,
                s.cursor_blink,
                s.scheme_dark.clone(),
                s.scheme_light.clone(),
            ),
        }
    }
}

pub struct TernaryShellApp {
    rt: tokio::runtime::Runtime,
    store: BookmarkStore,
    /// False when the bookmarks file must not be overwritten (written by a newer version, or
    /// unreadable and could not be moved aside).
    store_writable: bool,
    settings: Settings,
    settings_writable: bool,
    settings_saved: Settings,
    settings_changed_at: Option<Instant>,
    applied: Applied,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    sidebar: Sidebar,
    editor: Option<Editor>,
    palette: Option<Palette>,
    dialog: Option<Dialog>,
    /// The dialog that was open when ⌘Q asked for confirmation; back if the user cancels.
    dialog_below: Option<Dialog>,
    toasts: Toasts,
    #[cfg(target_os = "macos")]
    menu: Option<AppMenu>,
    #[cfg(target_os = "macos")]
    menu_rx: Option<mpsc::Receiver<MenuEvent>>,
    actions: Vec<Action>,
    cell: (FontChoice, f32, egui::Vec2),
    allow_close: bool,
    title: String,
    first_frame: bool,
    startup: Vec<String>,
    updater: crate::ui::update_ui::Updater,
    /// When a panic in a frame was last caught (to show one notice, not one per frame).
    last_recovered_panic: Option<Instant>,
    /// The tab that was active last frame (to notice tab switches).
    last_active_tab: Option<u64>,
    modal_was_open: bool,
    /// A dialog just closed with a key still held (the Enter that confirmed it): its
    /// auto-repeat must not reach a terminal.
    swallow_repeats: bool,
    /// What Option currently does in the window (Meta or composing), once set.
    #[cfg(target_os = "macos")]
    option_as_alt: Option<bool>,
    /// Arguments of later launches, forwarded by `crate::instance`.
    instance_rx: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Network changes (VPN, Wi-Fi, sleep), after which connections are checked.
    netwatch: crate::netwatch::NetWatch,
    net_changes_seen: u64,
    /// (tab id, pane id) that last received keyboard focus.
    focus_target: Option<(u64, crate::ui::layout::PaneId)>,
    /// Modification time of bookmarks.json as last loaded or saved by this window.
    store_mtime: Option<std::time::SystemTime>,
    /// Settings opens on this page next time (quick commands from ⚡, shortcuts from Help).
    settings_page: Option<crate::ui::dialogs::SettingsTab>,
    /// The connection editor opens on this section next time (forwards from the status strip).
    editor_section: Option<crate::ui::editor::Section>,
    /// The window is full screen (no room for the traffic lights).
    fullscreen: bool,
    /// Hosts in ~/.ssh/config, counted once for the welcome page.
    ssh_config_hosts: Option<usize>,
    /// The steps each connection went through, for its progress card.
    conn_steps: HashMap<u64, canvas::StepTrack>,
    /// The window size the traffic lights were last placed for (macOS).
    #[cfg(target_os = "macos")]
    traffic_placed: Option<(f32, f32)>,
    #[cfg(debug_assertions)]
    dev: crate::devtest::Script,
}

static PROCESS_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Records the launch time (for `TERNARYSHELL_STARTUP_LOG=1`).
pub fn mark_process_start() {
    PROCESS_START.get_or_init(Instant::now);
}

fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Moves an unreadable data file aside (`name.corrupt-<time>.json`); `None` if that failed.
fn backup_corrupt(path: &std::path::Path) -> Option<PathBuf> {
    let backup = path.with_extension(format!(
        "corrupt-{}.json",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    ));
    std::fs::rename(path, &backup).ok().map(|_| backup)
}

fn font_error(font: &FontChoice) -> String {
    let name = font.path.as_deref().unwrap_or(&font.name);
    tf(
        "The font {} could not be loaded; the built-in Hack font is used instead.",
        &[name],
    )
}

/// A bookmark for `user@host[:port]` typed on the command line (not saved).
fn adhoc_bookmark(target: &str) -> Option<Bookmark> {
    let target = crate::bookmarks::model::parse_target(target).ok()?;
    Some(Bookmark {
        host: target.host,
        port: target.port.unwrap_or(crate::bookmarks::DEFAULT_PORT),
        username: target.user.unwrap_or_default(),
        ..Default::default()
    })
}

mod canvas;
mod chrome;

pub use chrome::{address_with_port, forward_rule};

/// Height of the title row (traffic lights, tabs, the session toolbar).
const TITLE_H: f32 = 44.0;
/// Height of the status strip along the bottom of the canvas card.
const STATUS_H: f32 = 26.0;

impl TernaryShellApp {
    /// `targets` are command-line arguments: bookmark names or `user@host[:port]`.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        targets: Vec<String>,
        inbox: Option<crate::instance::Inbox>,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        let mut toasts = Toasts::default();
        let mut settings_writable = true;
        let settings = Settings::load(&paths::settings_file()).unwrap_or_else(|e| {
            // Moved aside rather than overwritten by the defaults; if that fails, settings
            // are not saved in this session.
            match backup_corrupt(&paths::settings_file()) {
                Some(backup) => toasts.error(tf(
                    "Could not read the settings ({}). The file was kept as {}.",
                    &[&format!("{e:#}"), &backup.display().to_string()],
                )),
                None => {
                    settings_writable = false;
                    toasts.error(format!("{e:#}"));
                }
            }
            Settings::default()
        });
        i18n::set_language(settings.language);
        let mut store_writable = true;
        let store = match BookmarkStore::load(&paths::bookmarks_file()) {
            Ok(s) => s,
            Err(e) if e.downcast_ref::<NewerSchema>().is_some() => {
                store_writable = false;
                toasts.error(format!("{e:#}"));
                BookmarkStore::default()
            }
            Err(e) => {
                // Keep the unreadable file instead of overwriting it with an empty store.
                match backup_corrupt(&paths::bookmarks_file()) {
                    Some(backup) => toasts.error(tf(
                        "Could not read the connections file ({}). It was kept as {}.",
                        &[&format!("{e:#}"), &backup.display().to_string()],
                    )),
                    None => {
                        store_writable = false;
                        toasts.error(format!("{e:#}"));
                    }
                }
                BookmarkStore::default()
            }
        };
        let font = fonts::effective_choice(&settings.font);
        if !fonts::apply(ctx, &font, settings.load_cjk_font) {
            toasts.error(font_error(&font));
        }
        style::apply(ctx, settings.theme);
        ctx.options_mut(|o| {
            o.zoom_with_keyboard = false;
            o.quit_shortcuts.clear();
        });
        #[cfg(target_os = "macos")]
        let menu = AppMenu::install();
        #[cfg(target_os = "macos")]
        let menu_rx = menu::event_channel(ctx);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ternaryshell-io")
            .enable_all()
            .build()
            .expect("tokio runtime");
        Self {
            rt,
            store_mtime: file_mtime(&paths::bookmarks_file()),
            store,
            store_writable,
            settings_writable,
            applied: Applied::of(&settings),
            settings_saved: settings.clone(),
            settings,
            settings_changed_at: None,
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            sidebar: Sidebar::default(),
            editor: None,
            palette: None,
            dialog: None,
            dialog_below: None,
            toasts,
            #[cfg(target_os = "macos")]
            menu: Some(menu),
            #[cfg(target_os = "macos")]
            menu_rx: Some(menu_rx),
            actions: Vec::new(),
            cell: (FontChoice::default(), 0.0, vec2(8.0, 16.0)),
            allow_close: false,
            title: String::new(),
            first_frame: true,
            startup: targets,
            instance_rx: inbox.map(|i| i.attach(ctx)),
            netwatch: crate::netwatch::NetWatch::start(ctx.clone()),
            net_changes_seen: 0,
            updater: Default::default(),
            last_recovered_panic: None,
            last_active_tab: None,
            modal_was_open: false,
            swallow_repeats: false,
            #[cfg(target_os = "macos")]
            option_as_alt: None,
            focus_target: None,
            settings_page: None,
            editor_section: None,
            fullscreen: false,
            ssh_config_hosts: None,
            conn_steps: HashMap::new(),
            #[cfg(target_os = "macos")]
            traffic_placed: None,
            #[cfg(debug_assertions)]
            dev: crate::devtest::Script::from_env(),
        }
    }

    /// Debug builds only: one step of `TERNARYSHELL_DEV_SCRIPT` (see `crate::devtest`).
    #[cfg(debug_assertions)]
    fn run_dev_script(&mut self, ctx: &egui::Context) {
        use crate::devtest;
        let Some(step) = self.dev.next(ctx) else {
            return;
        };
        let (cmd, arg) = step.split_once(' ').unwrap_or((step.as_str(), ""));
        let target = |name: &str| -> Option<egui::Pos2> {
            match name.strip_prefix('@') {
                Some(n) => devtest::rect_of(n).map(|r| r.center()),
                None => {
                    let (x, y) = name.split_once(',')?;
                    Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
                }
            }
        };
        let screen = |app: &Self| -> String {
            let Some(p) = app.tabs.get(app.active).and_then(|t| t.focused_pane()) else {
                return String::new();
            };
            use alacritty_terminal::grid::Dimensions;
            let term = p.shared.term.lock();
            term.bounds_to_string(
                alacritty_terminal::index::Point::new(
                    term.topmost_line(),
                    alacritty_terminal::index::Column(0),
                ),
                alacritty_terminal::index::Point::new(term.bottommost_line(), term.last_column()),
            )
        };
        match cmd {
            // Input sent straight to the remote (bypasses the keyboard path).
            "send" => {
                if let Some(p) = self.tabs.get(self.active).and_then(|t| t.focused_pane()) {
                    p.send(devtest::unescape(arg).into_bytes());
                }
            }
            // Real keyboard and mouse events.
            "keys" => devtest::type_text(&devtest::unescape(arg)),
            "key" => devtest::key(arg),
            // A right click: pressed in this frame, released in a later one (see `devtest`).
            "rclick" => match target(arg) {
                Some(pos) => {
                    devtest::push_egui_events(devtest::secondary_button(pos, true));
                    self.dev
                        .push_front(vec![format!("rclick-release {},{}", pos.x, pos.y)]);
                }
                None => self
                    .dev
                    .check(false, &format!("{cmd} {arg}: target not found")),
            },
            "rclick-release" => {
                if let Some(pos) = target(arg) {
                    devtest::push_egui_events(devtest::secondary_button(pos, false));
                }
            }
            "click" | "dclick" => match target(arg) {
                Some(pos) => devtest::click(pos, if cmd == "dclick" { 2 } else { 1 }, false),
                None => self
                    .dev
                    .check(false, &format!("{cmd} {arg}: target not found")),
            },
            "connect" => {
                if let Some(b) = self
                    .store
                    .bookmarks
                    .iter()
                    .find(|b| b.display_name() == arg)
                    .cloned()
                {
                    self.open_tab(ctx, b);
                }
            }
            "search" => self.sidebar.search = arg.to_string(),
            "import" => self.import_file(PathBuf::from(arg)),
            // `export-to <path>`: everything, without the save panel.
            "export-to" => self.export_to(std::path::Path::new(arg), &ExportScope::All),
            "action" => {
                let first = self.store.bookmarks.first().map(|b| b.id);
                let action = match arg {
                    "quick" => Some(Action::QuickConnect),
                    "settings" => Some(Action::Settings),
                    "settings-appearance" | "settings-terminal" | "settings-quick" => {
                        use crate::ui::dialogs::SettingsTab;
                        self.settings_page = Some(match arg {
                            "settings-appearance" => SettingsTab::Appearance,
                            "settings-terminal" => SettingsTab::Terminal,
                            _ => SettingsTab::QuickCommands,
                        });
                        Some(Action::Settings)
                    }
                    "sftp" => Some(Action::ToggleSftp),
                    "split-right" => Some(Action::SplitRight),
                    "split-down" => Some(Action::SplitDown),
                    "new-bookmark" => Some(Action::NewBookmark { group: None }),
                    "edit" => first.map(Action::EditBookmark),
                    "export" => Some(Action::Export(ExportScope::All)),
                    "import-ssh" => Some(Action::ImportSshConfig),
                    "shortcuts" => Some(Action::Shortcuts),
                    "next-tab" => Some(Action::NextTab),
                    "reconnect" => Some(Action::Reconnect),
                    "check-updates" => Some(Action::CheckForUpdates),
                    "bigger" => Some(Action::FontBigger),
                    "sidebar" => Some(Action::ToggleSidebar),
                    "close" => Some(Action::CloseTabOrPane),
                    "dark" => {
                        self.settings.theme = ThemeMode::Dark;
                        None
                    }
                    "light" => {
                        self.settings.theme = ThemeMode::Light;
                        None
                    }
                    "indonesian" => {
                        self.settings.language = Language::Indonesian;
                        None
                    }
                    "english" => {
                        self.settings.language = Language::English;
                        None
                    }
                    _ => None,
                };
                self.actions.extend(action);
            }
            // Checks.
            "expect-screen" => {
                let text = screen(self);
                self.dev
                    .check(text.contains(arg), &format!("screen contains {arg:?}"));
            }
            "expect-not-screen" => {
                let text = screen(self);
                self.dev.check(
                    !text.contains(arg),
                    &format!("screen does not contain {arg:?}"),
                );
            }
            "expect-tabs" => {
                let n = self.tabs.len();
                self.dev
                    .check(arg.parse() == Ok(n), &format!("tabs = {arg} (got {n})"));
            }
            "expect-panes" => {
                let n = self.tabs.get(self.active).map_or(0, |t| t.panes.len());
                self.dev
                    .check(arg.parse() == Ok(n), &format!("panes = {arg} (got {n})"));
            }
            "expect-keyboard-on-pane" => {
                let ok = self.tabs.get(self.active).is_some_and(|t| {
                    let idx: usize = arg.parse().unwrap_or(usize::MAX);
                    let panes = t.layout.panes();
                    panes.get(idx).is_some_and(|id| *id == t.focused)
                        && ctx.memory(|m| m.has_focus(terminal::pane_widget_id(t.focused)))
                });
                self.dev.check(ok, &format!("keyboard focus on pane {arg}"));
            }
            "expect-remote-cwd" => {
                let cwd = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.sftp.as_ref())
                    .map(|p| p.remote_cwd().to_string())
                    .unwrap_or_default();
                self.dev
                    .check(cwd == arg, &format!("remote cwd = {arg} (got {cwd})"));
            }
            "expect-local-cwd" => {
                let cwd = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.sftp.as_ref())
                    .map(|p| p.local_cwd().to_string())
                    .unwrap_or_default();
                self.dev
                    .check(cwd == arg, &format!("local cwd = {arg} (got {cwd})"));
            }
            "expect-root" => {
                let root = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.sftp.as_ref())
                    .is_some_and(SftpPanel::is_root);
                self.dev.check(
                    root == (arg == "yes"),
                    &format!("SFTP as root = {arg} (got {root})"),
                );
            }
            "expect-modal" => {
                let open = self.modal_open();
                self.dev.check(
                    open == (arg == "yes"),
                    &format!("modal open = {arg} (got {open})"),
                );
            }
            "expect-bookmark" => {
                let found = self.store.bookmarks.iter().any(|b| b.display_name() == arg);
                self.dev.check(found, &format!("bookmark {arg:?} exists"));
            }
            "expect-theme-dark" => {
                let dark = ctx.theme() == Theme::Dark;
                self.dev.check(
                    dark == (arg == "yes"),
                    &format!("dark theme = {arg} (got {dark})"),
                );
            }
            "log" => eprintln!("DEVTEST LOG: {arg}"),
            // `screenshot <path.bmp>`: the window as it is drawn (written when it arrives).
            "screenshot" => ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::UserData::new(PathBuf::from(arg)),
            )),
            // Checks that a panic while drawing is contained.
            "panic" => panic!("devtest: deliberate panic"),
            // Focus on a widget that is never drawn (what crashed the app with VoiceOver on).
            "focus-ghost" => ctx.memory_mut(|m| m.request_focus(egui::Id::new("devtest-ghost"))),
            // As if a VPN had just been turned on.
            "net-change" => self.netwatch.simulate_change(),
            "debug-hits" => ctx.all_styles_mut(|s| {
                s.debug.show_widget_hits = true;
                s.debug.show_interactive_widgets = true;
            }),
            // `drag @name dx,dy`: drag from a named widget's center by an offset.
            "drag" => {
                let mut parts = arg.split_whitespace();
                let from = parts.next().and_then(target);
                let delta = parts.next().and_then(|d| {
                    let (x, y) = d.split_once(',')?;
                    Some(egui::vec2(x.parse().ok()?, y.parse().ok()?))
                });
                match (from, delta) {
                    (Some(from), Some(delta)) => {
                        let mut steps = vec![format!("mouse down {},{}", from.x, from.y)];
                        for i in 1..=6 {
                            let p = from + delta * (i as f32 / 6.0);
                            steps.push(format!("mouse drag {},{}", p.x, p.y));
                        }
                        let to = from + delta;
                        steps.push(format!("mouse up {},{}", to.x, to.y));
                        self.dev.push_front(steps);
                    }
                    _ => self.dev.check(false, &format!("drag {arg}: bad target")),
                }
            }
            "mouse" => {
                let (kind, pos) = arg.split_once(' ').unwrap_or((arg, ""));
                match target(pos) {
                    Some(p) => devtest::mouse(kind, p),
                    None => self.dev.check(false, &format!("mouse {arg}: bad point")),
                }
            }
            "fullscreen" => ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(arg == "on")),
            "maximize" => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(arg == "on")),
            "resize" => {
                if let Some((w, h)) = arg.split_once(',')
                    && let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>())
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w, h)));
                }
            }
            "move" => {
                if let Some(pos) = target(arg) {
                    devtest::click(pos, 0, false);
                }
            }
            "quit" => {
                eprintln!(
                    "DEVTEST DONE pass={} fail={}",
                    self.dev.passed, self.dev.failed
                );
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            other => self.dev.check(false, &format!("unknown step {other:?}")),
        }
    }

    /// Opens the tabs requested on the command line.
    fn open_startup_targets(&mut self, ctx: &egui::Context) {
        for target in std::mem::take(&mut self.startup) {
            let found = self
                .store
                .bookmarks
                .iter()
                .find(|b| b.display_name().eq_ignore_ascii_case(&target))
                .cloned();
            match found.or_else(|| adhoc_bookmark(&target)) {
                Some(b) => self.open_tab(ctx, b),
                None => self
                    .toasts
                    .error(tf("Unknown connection or address: {}", &[&target])),
            }
        }
    }

    fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn scheme(&self, ctx: &egui::Context) -> &'static Scheme {
        let dark = ctx.theme() == Theme::Dark;
        scheme::find(
            if dark {
                &self.settings.scheme_dark
            } else {
                &self.settings.scheme_light
            },
            dark,
        )
    }

    fn save_store(&mut self) {
        if !self.store_writable {
            self.toasts.error(t(
                "Connection changes are not saved: the connections file could not be read safely.",
            ));
            return;
        }
        if let Err(e) = self.store.save(&paths::bookmarks_file()) {
            self.toasts.error(format!("{e:#}"));
        }
        self.store_mtime = file_mtime(&paths::bookmarks_file());
    }

    /// Another TernaryShell window may have changed the bookmarks: reload them when this window gets
    /// focus again, so the next save doesn't overwrite those changes.
    fn reload_store_if_changed(&mut self, ctx: &egui::Context) {
        let focused_now = ctx.input(|i| {
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::WindowFocused(true)))
        });
        if !focused_now || self.modal_open() {
            return;
        }
        let path = paths::bookmarks_file();
        let mtime = file_mtime(&path);
        if mtime.is_none() || mtime == self.store_mtime {
            return;
        }
        if let Ok(store) = BookmarkStore::load(&path) {
            self.store = store;
            self.store_mtime = mtime;
            self.store_writable = true;
        }
    }

    /// Whether a dialog or prompt has the keyboard (the terminal and shortcuts must not).
    fn modal_open(&self) -> bool {
        self.editor.is_some()
            || self.palette.is_some()
            || self.dialog.is_some()
            || self.updater.dialog_open
            || self
                .tabs
                .get(self.active)
                .is_some_and(|t| t.sftp.as_ref().is_some_and(SftpPanel::dialog_open))
    }

    /// Whether the keyboard is in the active tab's focused terminal pane.
    fn terminal_focused(&self, ctx: &egui::Context) -> bool {
        self.tabs.get(self.active).is_some_and(|t| {
            ctx.memory(|m| m.focused()) == Some(terminal::pane_widget_id(t.focused))
        })
    }

    fn live_map(&self) -> HashMap<Uuid, Live> {
        let mut map = HashMap::new();
        for tab in &self.tabs {
            let live = match tab.state() {
                ConnState::Connected => Live::Connected,
                ConnState::Connecting | ConnState::Authenticating => Live::Connecting,
                ConnState::Closed(Some(_)) => Live::Failed,
                ConnState::Closed(None) => continue,
            };
            let entry = map.entry(tab.bookmark.id).or_insert(live);
            if live == Live::Connected {
                *entry = Live::Connected;
            }
        }
        map
    }

    /// Measured every frame: a font change only takes effect on the frame after `set_fonts`,
    /// so a cached measurement could keep the old font's metrics.
    fn cell_size(&mut self, ctx: &egui::Context) -> egui::Vec2 {
        let c = terminal::cell_size(ctx, self.settings.font_size);
        if c.x > 0.0 && c.y > 0.0 {
            self.cell = (
                fonts::effective_choice(&self.settings.font),
                self.settings.font_size,
                c,
            );
        }
        self.cell.2
    }

    // ----- input routing ----------------------------------------------------------------------

    /// Clicks in the macOS menu bar (and its key equivalents). Elsewhere there is no menu bar:
    /// the shortcuts are handled in `handle_shortcuts` and the commands sit in the app menu.
    #[cfg(not(target_os = "macos"))]
    fn handle_menu_events(&mut self) {}

    #[cfg(target_os = "macos")]
    fn handle_menu_events(&mut self) {
        let Some(rx) = &self.menu_rx else { return };
        while let Ok(ev) = rx.try_recv() {
            if let Some(action) = self.menu.as_ref().and_then(|m| m.action(ev.id())) {
                self.actions.push(action);
            }
        }
    }

    /// Shortcuts not covered by the native menu.
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if self.modal_open() {
            return;
        }
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        // Without a menu bar (Windows, Linux) the app handles the menu's shortcuts itself.
        if !cfg!(target_os = "macos") {
            for shortcut in keys::Shortcut::HANDLED {
                if ctx.input_mut(|inp| shortcut.consume(inp))
                    && let Some(action) = shortcut.action()
                {
                    self.actions.push(action);
                }
            }
        }
        // ⌘F: the sidebar's filter; ⌃1–⌃9: quick commands (macOS, where Control+digit sends
        // nothing a shell uses).
        if cfg!(target_os = "macos") {
            if ctx.input_mut(|i| {
                i.consume_shortcut(&KeyboardShortcut::new(egui::Modifiers::COMMAND, Key::F))
            }) {
                self.actions.push(Action::FocusSidebarSearch);
            }
            for (i, key) in digits.iter().enumerate() {
                if i < self.settings.quick_commands.len()
                    && !self.tabs.is_empty()
                    && ctx.input_mut(|inp| {
                        inp.consume_shortcut(&KeyboardShortcut::new(egui::Modifiers::CTRL, *key))
                    })
                {
                    self.actions.push(Action::SendQuickCommandAt(i));
                }
            }
        }
        let tab_mods = keys::tab_number_modifiers();
        for (i, key) in digits.iter().enumerate() {
            if ctx.input_mut(|inp| inp.consume_shortcut(&KeyboardShortcut::new(tab_mods, *key))) {
                let idx = if i == 8 {
                    self.tabs.len().saturating_sub(1)
                } else {
                    i
                };
                self.actions.push(Action::SelectTab(idx));
            }
        }
    }

    fn handle_close_request(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested())
            && !self.allow_close
            && let Some(details) = self.quit_warning()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.dialog = Some(dialogs::confirm_details(
                t("Quit TernaryShell?"),
                String::new(),
                details,
                t("Quit"),
                ConfirmAction::Quit,
            ));
        }
    }

    /// What quitting would interrupt, if the user should be asked first (one line each, with
    /// its icon). Running file transfers always ask; open sessions only when the setting says so.
    fn quit_warning(&self) -> Option<Vec<(&'static str, String)>> {
        let live = self.tabs.iter().filter(|t| t.is_active()).count();
        let transfers: usize = self.tabs.iter().map(Tab::running_transfers).sum();
        let mut lines = Vec::new();
        if live > 0 && (self.settings.confirm_close_connected || transfers > 0) {
            lines.push((
                icons::TERMINAL_WINDOW,
                tf("{} open session(s)", &[&live.to_string()]),
            ));
        }
        if transfers > 0 {
            lines.push((
                icons::ARROWS_DOWN_UP,
                tf(
                    "{} transfer(s) will be cancelled",
                    &[&transfers.to_string()],
                ),
            ));
        }
        for tab in &self.tabs {
            if let Some(file) = tab.unsaved_file() {
                lines.push((
                    icons::PENCIL_SIMPLE,
                    tf("{} on {} has unsaved changes", &[&file, &tab.title()]),
                ));
            }
        }
        (!lines.is_empty()).then_some(lines)
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        if self.modal_open() {
            self.toasts
                .error(t("Close the open dialog first, then drop the files again."));
            return;
        }
        let rt = self.rt.handle().clone();
        let Some(tab) = self
            .tabs
            .get_mut(self.active)
            .filter(|t| t.state().is_connected())
        else {
            // Dropping an export file with no session open imports it.
            if dropped.len() == 1 && dropped[0].extension().is_some_and(|e| e == "json") {
                self.import_file(dropped[0].clone());
            } else {
                self.toasts.error(t("Connect to a server to upload files."));
            }
            return;
        };
        if tab.sftp.is_none() {
            tab.sftp = Some(tab.new_sftp_panel(self.settings.last_local_dir.as_deref()));
        }
        tab.show_sftp = true;
        if let Some(panel) = &mut tab.sftp {
            panel.upload_paths(&rt, ctx, dropped);
        }
    }

    fn apply_settings_changes(&mut self, ctx: &egui::Context) {
        self.settings.clamp();
        let now = Applied::of(&self.settings);
        if now != self.applied || self.first_frame {
            if now.language != self.applied.language {
                i18n::set_language(now.language);
                #[cfg(target_os = "macos")]
                {
                    self.menu = Some(AppMenu::install());
                }
            }
            if now.theme != self.applied.theme {
                style::apply(ctx, now.theme);
            }
            if (now.font != self.applied.font || now.cjk != self.applied.cjk)
                && !fonts::apply(ctx, &now.font, now.cjk)
            {
                self.toasts.error(font_error(&now.font));
            }

            if now.terminal != self.applied.terminal {
                let scheme = self.scheme(ctx);
                for tab in &self.tabs {
                    tab.apply_settings(&self.settings, scheme);
                }
            }
            self.applied = now;
        }
        // Terminal colours follow the effective (system) theme, which can change at any time.
        let scheme = self.scheme(ctx);
        for tab in &self.tabs {
            if tab
                .panes
                .first()
                .is_some_and(|p| p.shared.palette.lock().bg != scheme.background)
            {
                tab.apply_settings(&self.settings, scheme);
            }
        }
        if self.settings != self.settings_saved {
            let at = *self.settings_changed_at.get_or_insert_with(Instant::now);
            if at.elapsed() > Duration::from_millis(600) {
                if !self.settings_writable {
                    // The unreadable settings file is kept as it is.
                } else if let Err(e) = self.settings.save(&paths::settings_file()) {
                    self.toasts.error(format!("{e:#}"));
                }
                self.settings_saved = self.settings.clone();
                self.settings_changed_at = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(650));
            }
        }
    }

    // ----- tabs -------------------------------------------------------------------------------

    fn env<'a>(&'a self, ctx: &'a egui::Context) -> Env<'a> {
        Env {
            rt: self.rt.handle(),
            ctx,
            settings: &self.settings,
            scheme: self.scheme(ctx),
            known_hosts: paths::known_hosts_file(),
            store: Some(&self.store),
        }
    }

    fn open_tab(&mut self, ctx: &egui::Context, bookmark: Bookmark) {
        let size = self
            .tabs
            .get(self.active)
            .and_then(|t| t.focused_pane())
            .and_then(|p| p.view.grid)
            .unwrap_or(GridSize {
                cols: 120,
                rows: 34,
            });
        let (tab_id, pane_id) = (self.next_id(), self.next_id());
        let tab = Tab::open(tab_id, pane_id, bookmark, size, &self.env(ctx));
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
    }

    fn close_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            let tab = self.tabs.remove(idx);
            tab.close(self.rt.handle());
            if self.active >= self.tabs.len() {
                self.active = self.tabs.len().saturating_sub(1);
            } else if idx < self.active {
                self.active -= 1;
            }
        }
    }

    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    fn request_close_tab(&mut self, idx: usize) {
        let Some(tab) = self.tabs.get(idx) else {
            return;
        };
        let transfers = tab.running_transfers();
        let unsaved = tab.unsaved_file();
        if (self.settings.confirm_close_connected && tab.is_active())
            || transfers > 0
            || unsaved.is_some()
        {
            let mut details = Vec::new();
            if tab.is_active() {
                details.push((
                    icons::TERMINAL,
                    t("The session is still running").to_string(),
                ));
            }
            if transfers > 0 {
                details.push((
                    icons::ARROWS_DOWN_UP,
                    tf(
                        "{} transfer(s) will be cancelled",
                        &[&transfers.to_string()],
                    ),
                ));
            }
            if let Some(file) = &unsaved {
                details.push((icons::PENCIL_SIMPLE, tf("{} has unsaved changes", &[file])));
            }
            // Only the plain "it's connected" question can be turned off; lost transfers and
            // edits are always asked about.
            let ask = if transfers == 0 && unsaved.is_none() {
                dialogs::confirm_close
            } else {
                dialogs::confirm_details
            };
            self.dialog = Some(ask(
                &tf("Close {}?", &[&tab.title()]),
                String::new(),
                details,
                t("Close Tab"),
                ConfirmAction::CloseTab(tab.id),
            ));
        } else {
            self.close_tab(idx);
        }
    }

    /// Reconnects a tab, picking up edits made to its bookmark meanwhile.
    fn reconnect_tab(&mut self, ctx: &egui::Context, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let id = self.tabs[idx].bookmark.id;
        if let Some(b) = self.store.bookmark(id).cloned() {
            self.tabs[idx].bookmark = b;
        }
        let env = Env {
            rt: self.rt.handle(),
            ctx,
            settings: &self.settings,
            scheme: self.scheme(ctx),
            known_hosts: paths::known_hosts_file(),
            store: Some(&self.store),
        };
        self.tabs[idx].reconnect(&env);
    }

    // ----- actions ----------------------------------------------------------------------------

    fn process_actions(&mut self, ctx: &egui::Context) {
        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            // Native menu shortcuts arrive even while a dialog is open: only editing commands
            // and Quit pass through, and ⌘W closes the dialog instead of a tab.
            if self.modal_open() {
                match action {
                    Action::Copy | Action::Paste | Action::Cut | Action::Quit => {}
                    Action::CloseTabOrPane => {
                        if self.palette.is_some() {
                            self.palette = None;
                        } else if self.dialog.is_some() {
                            self.dialog = self.dialog_below.take();
                        } else if self.editor.is_some() {
                            self.editor = None;
                        } else if self.updater.dialog_open {
                            self.updater.close_dialog();
                        } else if let Some(tab) = self.tabs.get_mut(self.active) {
                            if let Some(panel) = tab.sftp.as_mut().filter(|p| p.dialog_open()) {
                                panel.close_dialog();
                            } else {
                                // A login prompt: ⌘W cancels it, as Esc does.
                                tab.answer_prompt(false, None);
                            }
                        }
                        continue;
                    }
                    _ => continue,
                }
            }
            self.run_action(ctx, action);
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::NewBookmark { group } => {
                self.editor = Some(Editor::new(
                    Bookmark {
                        group,
                        ..Default::default()
                    },
                    true,
                ));
            }
            Action::EditBookmark(id) => {
                let section = self.editor_section.take();
                if let Some(b) = self.store.bookmark(id).cloned() {
                    self.editor = Some(Editor::new(b, false).with_section(section));
                } else if let Some(tab) = self.tabs.iter().find(|t| t.bookmark.id == id) {
                    // An ad-hoc connection (command line): offer to save it as a bookmark.
                    self.editor = Some(Editor::new(tab.bookmark.clone(), true));
                }
            }
            Action::DuplicateBookmark(id) => {
                if let Some(new_id) = self.store.duplicate_bookmark(id, t("(copy)")) {
                    secrets::copy_all(id, new_id);
                    self.save_store();
                    self.sidebar.reveal(&self.store, new_id);
                }
            }
            Action::DeleteBookmark(id) => {
                if let Some(b) = self.store.bookmark(id) {
                    self.dialog = Some(dialogs::confirm(
                        &tf("Delete “{}”?", &[&b.display_name()]),
                        tf(
                            "The connection and its saved password are removed from {}. The server isn't touched.",
                            &[crate::platform::this_computer()],
                        ),
                        t("Delete"),
                        ConfirmAction::DeleteBookmark(id),
                    ));
                }
            }
            Action::Connect(id) => {
                if let Some(b) = self.store.bookmark(id).cloned() {
                    self.sidebar.selected = Some(id);
                    self.settings.touch_recent(id);
                    self.open_tab(ctx, b);
                }
            }
            Action::NewGroup { parent } => {
                let context = match parent {
                    Some(g) => tf("in {}", &[&self.store.group_path(Some(g))]),
                    None => String::new(),
                };
                self.dialog = Some(Dialog::GroupName {
                    target: GroupTarget::New { parent },
                    context,
                    name: String::new(),
                    focus: true,
                });
            }
            Action::RenameGroup(id) => {
                let name = self
                    .store
                    .group(id)
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::GroupName {
                    target: GroupTarget::Rename(id),
                    context: name.clone(),
                    name,
                    focus: true,
                });
            }
            Action::DeleteGroup(id) => {
                if let Some(g) = self.store.group(id) {
                    let count = self.store.bookmarks_in(Some(id)).count();
                    self.dialog = Some(dialogs::confirm(
                        &tf("Delete group “{}”?", &[&g.name]),
                        tf(
                            "Its {} connection(s) and subgroups move up one level. Nothing else is deleted.",
                            &[&count.to_string()],
                        ),
                        t("Delete Group"),
                        ConfirmAction::DeleteGroup(id),
                    ));
                }
            }
            Action::QuickConnect => self.palette = Some(Palette::default()),
            Action::SaveAddress(address) => {
                if let Some(b) = adhoc_bookmark(&address) {
                    self.editor = Some(Editor::new(b, true));
                }
            }
            Action::CollapseAllGroups => self.sidebar.collapse_all(),
            Action::FocusSidebarSearch => {
                self.settings.sidebar_visible = true;
                self.sidebar.focus_search = true;
            }
            Action::CloseOtherTabs(id) => {
                let others: Vec<u64> = self
                    .tabs
                    .iter()
                    .map(|t| t.id)
                    .filter(|t| *t != id)
                    .collect();
                for other in others {
                    if let Some(i) = self.tab_index(other) {
                        self.request_close_tab(i);
                        // A question about one tab stops the rest (it would replace it).
                        if self.dialog.is_some() {
                            break;
                        }
                    }
                }
            }
            Action::CancelConnect(id) => {
                if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
                    tab.disconnect();
                }
            }
            Action::ToggleTheme => self.toggle_theme(ctx),
            Action::SendQuickCommandAt(i) => {
                if let Some(qc) = self.settings.quick_commands.get(i) {
                    let id = qc.id;
                    self.run_action(ctx, Action::SendQuickCommand(id));
                }
            }
            Action::NewTab => match self.tabs.get(self.active) {
                Some(tab) => {
                    let b = self
                        .store
                        .bookmark(tab.bookmark.id)
                        .cloned()
                        .unwrap_or_else(|| tab.bookmark.clone());
                    self.open_tab(ctx, b);
                }
                None => self.palette = Some(Palette::default()),
            },
            Action::DuplicateTab(id) => {
                if let Some(tab) = self.tabs.iter().find(|t| t.id == id) {
                    let b = self
                        .store
                        .bookmark(tab.bookmark.id)
                        .cloned()
                        .unwrap_or_else(|| tab.bookmark.clone());
                    self.open_tab(ctx, b);
                }
            }
            Action::CloseTabOrPane => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if tab.panes.len() > 1 {
                        let focused = tab.focused;
                        tab.close_pane(focused);
                    } else {
                        self.request_close_tab(self.active);
                    }
                }
            }
            Action::CloseTab(id) => {
                if let Some(i) = self.tab_index(id) {
                    self.request_close_tab(i);
                }
            }
            Action::SelectTab(i) => {
                if i < self.tabs.len() {
                    self.active = i;
                }
            }
            Action::NextTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + 1) % self.tabs.len();
                }
            }
            Action::PrevTab => {
                if !self.tabs.is_empty() {
                    self.active = (self.active + self.tabs.len() - 1) % self.tabs.len();
                }
            }
            Action::MoveTab { from, to } => {
                if let (Some(from), Some(to)) = (self.tab_index(from), self.tab_index(to))
                    && from != to
                {
                    let tab = self.tabs.remove(from);
                    self.tabs.insert(to, tab);
                    self.active = to;
                }
            }
            Action::Reconnect => {
                if let Some(tab) = self.tabs.get(self.active) {
                    let transfers = tab.running_transfers();
                    if tab.is_active() || transfers > 0 {
                        // Running programs would be cut off: ask first.
                        let mut details = Vec::new();
                        if transfers > 0 {
                            details.push((
                                icons::ARROWS_DOWN_UP,
                                tf(
                                    "{} transfer(s) will be cancelled",
                                    &[&transfers.to_string()],
                                ),
                            ));
                        }
                        self.dialog = Some(dialogs::confirm_details(
                            &tf("Reconnect {}?", &[&tab.title()]),
                            t("The running sessions are closed and opened again. Scrollback is kept.").to_string(),
                            details,
                            t("Reconnect"),
                            ConfirmAction::Reconnect(tab.id),
                        ));
                    } else {
                        self.reconnect_tab(ctx, self.active);
                    }
                }
            }
            Action::RestartPane(pane_id) => {
                if self.active < self.tabs.len() && !self.tabs[self.active].restart_pane(pane_id) {
                    self.reconnect_tab(ctx, self.active);
                }
            }
            Action::SplitRight | Action::SplitDown => {
                if self.active < self.tabs.len() {
                    let dir = if action == Action::SplitRight {
                        SplitDir::Horizontal
                    } else {
                        SplitDir::Vertical
                    };
                    let pane_id = self.next_id();
                    let env = Env {
                        rt: self.rt.handle(),
                        ctx,
                        settings: &self.settings,
                        scheme: self.scheme(ctx),
                        known_hosts: paths::known_hosts_file(),
                        store: Some(&self.store),
                    };
                    self.tabs[self.active].split(pane_id, dir, &env);
                }
            }
            Action::NextPane | Action::PrevPane => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.focused = tab.layout.neighbor(tab.focused, action == Action::NextPane);
                    // `central` moves the keyboard there when it draws the pane.
                    ctx.request_repaint();
                }
            }
            Action::ToggleSidebar => self.settings.sidebar_visible = !self.settings.sidebar_visible,
            Action::ToggleSftp => {
                let last = self.settings.last_local_dir.clone();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    tab.show_sftp = !tab.show_sftp;
                    if tab.show_sftp && tab.sftp.is_none() && tab.state().is_connected() {
                        tab.sftp = Some(tab.new_sftp_panel(last.as_deref()));
                    }
                }
            }
            Action::FontBigger => {
                self.settings.font_size = (self.settings.font_size + 1.0).min(Settings::MAX_FONT)
            }
            Action::FontSmaller => {
                self.settings.font_size = (self.settings.font_size - 1.0).max(Settings::MIN_FONT)
            }
            Action::FontReset => self.settings.font_size = Settings::default().font_size,
            // On Windows and Linux a terminal reads a plain Ctrl+C as ^C, so the menu's Copy
            // copies the terminal's selection itself, and its Paste is marked as a paste.
            Action::Copy | Action::Cut
                if !cfg!(target_os = "macos") && self.terminal_focused(ctx) =>
            {
                if let Some(p) = self.tabs.get(self.active).and_then(|t| t.focused_pane()) {
                    terminal::copy_selection(ctx, &p.shared);
                }
            }
            Action::Paste if !cfg!(target_os = "macos") && self.terminal_focused(ctx) => {
                terminal::request_paste();
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
            }
            Action::Copy => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCopy),
            Action::Paste => ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste),
            Action::Cut => ctx.send_viewport_cmd(egui::ViewportCommand::RequestCut),
            Action::ToggleFullScreen => {
                let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
            }
            Action::SelectAll => {
                if let Some(p) = self.tabs.get(self.active).and_then(|t| t.focused_pane()) {
                    terminal::select_all(&p.shared);
                }
            }
            Action::ClearScrollback => {
                if let Some(p) = self.tabs.get(self.active).and_then(|t| t.focused_pane()) {
                    terminal::clear_scrollback(&p.shared);
                }
            }
            Action::SendQuickCommand(id) => {
                let Some(qc) = self
                    .settings
                    .quick_commands
                    .iter()
                    .find(|q| q.id == id)
                    .cloned()
                else {
                    return;
                };
                if let Some(p) = self.tabs.get(self.active).and_then(|t| t.focused_pane()) {
                    let mut bytes = qc.command.replace('\n', "\r").into_bytes();
                    if qc.run {
                        bytes.push(b'\r');
                    }
                    p.send(bytes);
                    self.focus_target = None;
                    ctx.request_repaint();
                }
            }
            Action::Import => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(t("Import Connections"))
                    .add_filter(t("Connection exports"), &["json"])
                    .add_filter(t("All files"), &["*"])
                    .pick_file()
                {
                    self.import_file(path);
                }
            }
            Action::ImportSshConfig => {
                let path = paths::home_dir().join(".ssh/config");
                match ssh_config::parse_file(&path) {
                    Ok(hosts) if hosts.is_empty() => {
                        self.toasts.info(t("No hosts found in ~/.ssh/config."))
                    }
                    Ok(hosts) => {
                        let plan = ImportPlan::new(transfer::from_ssh_config(&hosts), &self.store);
                        self.dialog = Some(Dialog::Import(ImportDialog::new(
                            plan,
                            "~/.ssh/config".into(),
                        )));
                    }
                    Err(e) => self.toasts.error(format!("~/.ssh/config: {e}")),
                }
            }
            Action::Export(scope) => {
                if self.store.bookmarks.is_empty() {
                    self.toasts
                        .info(t("There are no connections to export yet."));
                } else {
                    self.dialog = Some(Dialog::Export(ExportDialog::new(scope)));
                }
            }
            Action::Settings => {
                let page = self.settings_page.take();
                self.dialog = Some(Dialog::Settings(SettingsDialog::new(page)));
            }
            Action::About => {}
            Action::CheckForUpdates => self.updater.check_now(ctx),
            Action::Shortcuts => {
                self.dialog = Some(Dialog::Settings(SettingsDialog::new(Some(
                    crate::ui::dialogs::SettingsTab::Shortcuts,
                ))))
            }
            Action::Quit => {
                if matches!(
                    self.dialog,
                    Some(Dialog::Confirm {
                        action: ConfirmAction::Quit,
                        ..
                    })
                ) {
                    return;
                }
                if let Some(details) = self.quit_warning() {
                    if self.dialog.is_some() {
                        self.dialog_below = self.dialog.take();
                    }
                    self.dialog = Some(dialogs::confirm_details(
                        t("Quit TernaryShell?"),
                        String::new(),
                        details,
                        t("Quit"),
                        ConfirmAction::Quit,
                    ));
                } else {
                    self.allow_close = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    fn import_file(&mut self, path: PathBuf) {
        // Checked before reading: a huge file or a device must not be read into memory.
        match std::fs::metadata(&path) {
            Ok(m) if !m.is_file() || m.len() > transfer::MAX_IMPORT_BYTES => {
                self.toasts.error(tf(
                    "The file is too large to be a connection export (more than {} MB).",
                    &[&(transfer::MAX_IMPORT_BYTES / 1024 / 1024).to_string()],
                ));
                return;
            }
            _ => {}
        }
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                self.toasts.error(format!("{}: {e}", path.display()));
                return;
            }
        };
        match transfer::read_import(&data) {
            Ok(incoming) => {
                let plan = ImportPlan::new(incoming, &self.store);
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.dialog = Some(Dialog::Import(ImportDialog::new(plan, name)));
            }
            Err(e) => self.toasts.error(format!("{e:#}")),
        }
    }

    fn do_export(&mut self, scope: ExportScope) {
        let default_name = format!(
            "ternaryshell-connections-{}.json",
            chrono::Local::now().format("%Y-%m-%d")
        );
        let Some(path) = rfd::FileDialog::new()
            .set_title(t("Export Connections"))
            .set_file_name(default_name)
            .add_filter("JSON", &["json"])
            .save_file()
        else {
            return;
        };
        self.export_to(&path, &scope);
    }

    fn export_to(&mut self, path: &std::path::Path, scope: &ExportScope) {
        // Key files go along too, for a Mac that doesn't have them.
        let mut unreadable = Vec::new();
        let result = transfer::export(&self.store, scope, |id| {
            let mut set = secrets::try_load_set(id)?;
            if let Some(b) = self.store.bookmark(id)
                && let Auth::Key { path } = &b.auth
            {
                set.private_key = crate::ssh::keys::read_key_file(&paths::expand_tilde(path)).ok();
                if set.private_key.is_none() {
                    unreadable.push(b.display_name());
                }
            }
            Ok(set)
        })
        .and_then(|file| Ok(serde_json::to_vec_pretty(&file)?))
        .and_then(|json| Ok(paths::write_private(path, &json)?));
        match result {
            Ok(()) if unreadable.is_empty() => {
                let name = path
                    .file_name()
                    .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                let folder = path.parent().map(paths::contract_tilde);
                self.toasts
                    .push(tf("Exported to {}", &[&name]), folder, false, None);
            }
            Ok(()) => self.toasts.error(tf(
                "Exported to {}, but the key file of {} could not be read: copy it to the other Mac yourself.",
                &[&path.display().to_string(), &unreadable.join(", ")],
            )),
            Err(e) => self.toasts.error(format!("{e:#}")),
        }
    }

    fn apply_import(&mut self, mut plan: ImportPlan) {
        plan.adopt_key_files(|path| {
            crate::ssh::keys::read_key_file(&paths::expand_tilde(path)).ok()
        });
        let outcome = plan.apply(&mut self.store);
        let mut failed = 0;
        for id in &outcome.clear_secrets {
            secrets::delete_all(*id);
        }
        for (id, set) in &outcome.secrets {
            if secrets::store_set(*id, set).is_err() {
                failed += 1;
            }
        }
        self.save_store();
        self.toasts.info(tf(
            "Import finished: {} added, {} updated, {} skipped.",
            &[
                &outcome.added.to_string(),
                &outcome.updated.to_string(),
                &outcome.skipped.to_string(),
            ],
        ));
        if failed > 0 {
            self.toasts.error(tf(
                "{} credential(s) could not be saved to {}.",
                &[&failed.to_string(), crate::platform::keychain()],
            ));
        }
    }

    fn apply_move(&mut self, mv: TreeMove) {
        match mv {
            TreeMove::Bookmark { id, group, before } => self.store.move_bookmark(id, group, before),
            TreeMove::Group { id, parent, before } => {
                if !self.store.move_group(id, parent, before) {
                    return;
                }
            }
        }
        self.save_store();
    }

    fn overlays(&mut self, ctx: &egui::Context) {
        if let Some(editor) = &mut self.editor {
            match editor.show(ctx, &self.store) {
                EditorResult::Open => {}
                EditorResult::Cancel => self.editor = None,
                EditorResult::Save {
                    bookmark,
                    set,
                    clear,
                    connect,
                } => {
                    let id = bookmark.id;
                    // Nothing is changed if the Keychain refuses the new secrets: switching the
                    // login method anyway would leave a connection that can't log in.
                    if let Err(e) = secrets::store_set(id, &set) {
                        let message = format!("{e:#}");
                        editor.set_error(tf(
                            "Could not save to {}: {}",
                            &[crate::platform::keychain(), &message],
                        ));
                    } else {
                        self.save_bookmark(ctx, bookmark, clear, connect);
                    }
                }
            }
        }
        self.overlay_dialogs(ctx);
    }

    /// Saves an edited connection whose new secrets are already in the Keychain.
    fn save_bookmark(
        &mut self,
        ctx: &egui::Context,
        bookmark: Bookmark,
        clear: Vec<SecretKind>,
        connect: bool,
    ) {
        let id = bookmark.id;
        for kind in clear {
            let _ = secrets::set(id, kind, None);
        }
        // Secrets the chosen login method doesn't use are removed, so switching a
        // bookmark to another method leaves nothing stale in the Keychain.
        if !matches!(bookmark.auth, Auth::Password) {
            let _ = secrets::set(id, SecretKind::Password, None);
        }
        if !bookmark.auth.uses_key() {
            let _ = secrets::set(id, SecretKind::Passphrase, None);
        }
        if !matches!(bookmark.auth, Auth::KeychainKey) {
            let _ = secrets::set(id, SecretKind::PrivateKey, None);
        }
        self.store.upsert_bookmark(bookmark.clone());
        self.save_store();
        self.sidebar.reveal(&self.store, id);
        self.editor = None;
        if connect {
            self.open_tab(ctx, bookmark);
        }
    }

    fn overlay_dialogs(&mut self, ctx: &egui::Context) {
        if self.palette.is_some() {
            let recent: Vec<Uuid> = self.settings.recent.iter().map(|r| r.id).collect();
            let live: std::collections::HashSet<Uuid> = self
                .live_map()
                .into_iter()
                .filter(|(_, l)| *l == Live::Connected)
                .map(|(id, _)| id)
                .collect();
            let commands = self.palette_commands(ctx);
            let palette = self.palette.as_mut().expect("checked");
            match palette.show(ctx, &self.store, &recent, &live, &commands) {
                PaletteResult::Open => {}
                PaletteResult::Close => self.palette = None,
                PaletteResult::SaveAs(address) => {
                    self.palette = None;
                    self.actions.push(Action::SaveAddress(address));
                }
                PaletteResult::Run(action) => {
                    self.palette = None;
                    self.actions.push(action);
                }
                PaletteResult::Connect(id) => {
                    self.palette = None;
                    self.actions.push(Action::Connect(id));
                }
                PaletteResult::ConnectTo(address) => {
                    self.palette = None;
                    if let Some(b) = adhoc_bookmark(&address) {
                        self.open_tab(ctx, b);
                    }
                }
            }
        }
        if let Some(dialog) = &mut self.dialog {
            match dialog.show(ctx, &self.store, &mut self.settings) {
                DialogResult::Open => {}
                DialogResult::Close => self.dialog = self.dialog_below.take(),
                DialogResult::Confirmed(action) => {
                    self.dialog = None;
                    self.dialog_below = None;
                    match action {
                        ConfirmAction::DeleteBookmark(id) => {
                            self.store.remove_bookmark(id);
                            secrets::delete_all(id);
                            self.save_store();
                        }
                        ConfirmAction::DeleteGroup(id) => {
                            self.store.remove_group(id);
                            self.save_store();
                        }
                        ConfirmAction::CloseTab(id) => {
                            if let Some(i) = self.tab_index(id) {
                                self.close_tab(i);
                            }
                        }
                        ConfirmAction::Reconnect(id) => {
                            if let Some(i) = self.tab_index(id) {
                                self.reconnect_tab(ctx, i);
                            }
                        }
                        ConfirmAction::RestartForUpdate => self.install_update(ctx),
                        ConfirmAction::Quit => {
                            self.allow_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
                DialogResult::GroupName(target, name) => {
                    self.dialog = None;
                    match target {
                        GroupTarget::New { parent } => {
                            let id = self.store.add_group(name, parent);
                            self.sidebar.reveal(&self.store, id);
                        }
                        GroupTarget::Rename(id) => self.store.rename_group(id, name),
                    }
                    self.save_store();
                }
                DialogResult::Export { scope } => {
                    self.dialog = None;
                    self.do_export(scope);
                }
                DialogResult::Import(plan) => {
                    self.dialog = None;
                    self.apply_import(*plan);
                }
                DialogResult::Run(action) => self.actions.push(action),
            }
        }
        if let Some(action) = self.toasts.show(ctx) {
            self.actions.push(action);
        }
    }

    /// Everything the command palette can run, in the order it lists them.
    fn palette_commands(&self, ctx: &egui::Context) -> Vec<crate::ui::dialogs::Command> {
        use crate::ui::dialogs::Command;
        use keys::Shortcut as S;
        let has_tab = !self.tabs.is_empty();
        let dark = ctx.theme() == Theme::Dark;
        let mut list = vec![Command {
            icon: icons::PLUS,
            label: t("New Connection…").into(),
            shortcut: Some(S::NewConnection.label()),
            action: Action::NewBookmark { group: None },
        }];
        if has_tab {
            list.push(Command {
                icon: icons::ARROWS_DOWN_UP,
                label: t("Toggle SFTP Panel").into(),
                shortcut: Some(S::Sftp.label()),
                action: Action::ToggleSftp,
            });
            list.push(Command {
                icon: icons::SQUARE_SPLIT_HORIZONTAL,
                label: t("Split Right").into(),
                shortcut: Some(S::SplitRight.label()),
                action: Action::SplitRight,
            });
        }
        list.push(Command {
            icon: icons::CIRCLE_HALF,
            label: if dark {
                t("Switch to Light Theme")
            } else {
                t("Switch to Dark Theme")
            }
            .into(),
            shortcut: None,
            action: Action::ToggleTheme,
        });
        list.push(Command {
            icon: icons::FILE_CODE,
            label: t("Import from ~/.ssh/config").into(),
            shortcut: None,
            action: Action::ImportSshConfig,
        });
        if has_tab {
            list.push(Command {
                icon: icons::SQUARE_SPLIT_VERTICAL,
                label: t("Split Down").into(),
                shortcut: Some(S::SplitDown.label()),
                action: Action::SplitDown,
            });
            list.push(Command {
                icon: icons::TERMINAL_WINDOW,
                label: t("New Tab").into(),
                shortcut: Some(S::NewTab.label()),
                action: Action::NewTab,
            });
            list.push(Command {
                icon: icons::ARROWS_CLOCKWISE,
                label: t("Reconnect").into(),
                shortcut: Some(S::Reconnect.label()),
                action: Action::Reconnect,
            });
            list.push(Command {
                icon: icons::BROOM,
                label: t("Clear Scrollback").into(),
                shortcut: Some(S::ClearScrollback.label()),
                action: Action::ClearScrollback,
            });
        }
        list.extend([
            Command {
                icon: icons::SIDEBAR_SIMPLE,
                label: t("Show/Hide Sidebar").into(),
                shortcut: Some(S::Sidebar.label()),
                action: Action::ToggleSidebar,
            },
            Command {
                icon: icons::FOLDER_PLUS,
                label: t("New Group").into(),
                shortcut: None,
                action: Action::NewGroup { parent: None },
            },
            Command {
                icon: icons::DOWNLOAD_SIMPLE,
                label: t("Import Connections…").into(),
                shortcut: None,
                action: Action::Import,
            },
            Command {
                icon: icons::EXPORT,
                label: t("Export Connections…").into(),
                shortcut: Some(S::Export.label()),
                action: Action::Export(ExportScope::All),
            },
            Command {
                icon: icons::GEAR_SIX,
                label: t("Settings…").into(),
                shortcut: Some(S::Settings.label()),
                action: Action::Settings,
            },
            Command {
                icon: icons::KEYBOARD,
                label: t("Keyboard Shortcuts").into(),
                shortcut: None,
                action: Action::Shortcuts,
            },
            Command {
                icon: icons::ARROW_CIRCLE_UP,
                label: t("Check for Updates…").into(),
                shortcut: None,
                action: Action::CheckForUpdates,
            },
        ]);
        if has_tab {
            for qc in &self.settings.quick_commands {
                let name = if qc.name.trim().is_empty() {
                    qc.command.clone()
                } else {
                    qc.name.clone()
                };
                list.push(Command {
                    icon: icons::LIGHTNING,
                    label: tf("Run: {}", &[&name]),
                    shortcut: None,
                    action: Action::SendQuickCommand(qc.id),
                });
            }
        }
        list
    }

    /// Starts the update installer and quits; the new version opens by itself.
    fn install_update(&mut self, ctx: &egui::Context) {
        match self.updater.launch_installer() {
            Ok(()) => {
                self.allow_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => self
                .toasts
                .error(tf("The update could not be installed: {}", &[&e])),
        }
    }

    /// SFTP panels that are not on screen still finish their transfers: their events are
    /// processed here, finished batches are announced, and progress keeps being drawn.
    fn background_transfers(&mut self, ctx: &egui::Context) {
        let rt = self.rt.handle().clone();
        let mut running = false;
        let several = self.tabs.len() > 1;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let visible = i == self.active && tab.show_sftp;
            let Some(panel) = &mut tab.sftp else { continue };
            if !visible {
                panel.poll(&rt, ctx);
            }
            running |= panel.transfer_summary().is_some_and(|s| s.running());
            if let Some((text, is_error)) = panel.take_notice() {
                let text = if several {
                    format!("{} — {text}", tab.title())
                } else {
                    text
                };
                if is_error {
                    self.toasts.error(text);
                } else {
                    self.toasts.info(text);
                }
            }
        }
        if running {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Checks every connection after a network change, and one whose typed keys got no reply:
    /// a connection whose route went away (a VPN turned on, another Wi-Fi, a night asleep) is
    /// closed within seconds and says it needs a reconnect, instead of hanging for minutes.
    fn watch_connections(&mut self, ctx: &egui::Context) {
        let changes = self.netwatch.changes();
        if changes != self.net_changes_seen {
            self.net_changes_seen = changes;
            for tab in &self.tabs {
                tab.conn
                    .probe(crate::ssh::types::ProbeReason::NetworkChanged);
            }
        }
        for tab in &mut self.tabs {
            if let Some(wait) = tab.watch_replies() {
                ctx.request_repaint_after(wait);
            }
            for error in tab.new_forward_errors() {
                self.toasts
                    .error(tf("{}: port forwarding: {}", &[&tab.title(), &error]));
            }
        }
    }

    /// End-of-frame focus bookkeeping.
    fn track_focus(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        // Leaving a tab: its panes lose focus (a focus-out report for programs that asked).
        let active_id = self.tabs.get(self.active).map(|t| t.id);
        if active_id != self.last_active_tab {
            if let Some(old) = self
                .last_active_tab
                .and_then(|id| self.tabs.iter_mut().find(|t| t.id == id))
            {
                for pane in &mut old.panes {
                    let alive = pane.is_live();
                    let bytes = pane.view.blur(&pane.shared, alive);
                    if !bytes.is_empty() {
                        pane.send(bytes);
                    }
                }
                // Its login prompt starts over (focus, settle delay) when the tab is back.
                old.prompt.focus = false;
                old.prompt.armed = false;
                old.prompt.shown_at = None;
            }
            self.last_active_tab = active_id;
        }
        // A dialog just closed: the terminal gets the keyboard back. Focus is requested where the
        // pane is drawn (`central`, next frame), never here: the active tab may have changed this
        // frame (the dialog closed it), and focusing a widget that was not drawn makes the
        // accessibility tree invalid, which aborts the app when an accessibility client
        // (VoiceOver, window managers, …) is attached.
        let modal = self.modal_open();
        if self.modal_was_open && !modal {
            self.focus_target = None;
            self.swallow_repeats = true;
            ctx.request_repaint();
        }
        self.modal_was_open = modal;
        // Option acts as Meta only while a terminal has the keyboard: dialogs and search
        // fields get the normal Option characters (such as @ on a German keyboard).
        let terminal_focused = !modal
            && self.tabs.get(self.active).is_some_and(|t| {
                ctx.memory(|m| m.focused()) == Some(terminal::pane_widget_id(t.focused))
            });
        let want = self.settings.option_as_meta && terminal_focused;
        // On macOS, Option has to be told to act as Alt (it composes characters otherwise);
        // Windows and Linux deliver Alt as it is.
        #[cfg(target_os = "macos")]
        if self.option_as_alt != Some(want)
            && let Some(w) = frame.winit_window()
        {
            use winit::platform::macos::{OptionAsAlt, WindowExtMacOS};
            w.set_option_as_alt(if want {
                OptionAsAlt::Both
            } else {
                OptionAsAlt::None
            });
            self.option_as_alt = Some(want);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (want, frame);
    }

    /// Keeps the window frame in the settings (saved with them), and on the first frame moves a
    /// window restored onto a screen that is no longer connected back onto the main screen.
    fn remember_window(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.first_frame
            && let Some(w) = frame.winit_window()
            && w.current_monitor().is_none()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(80.0, 80.0)));
            return;
        }
        let (outer, inner, fullscreen, minimized) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.outer_rect,
                v.inner_rect,
                v.fullscreen.unwrap_or(false),
                v.minimized.unwrap_or(false),
            )
        });
        if fullscreen || minimized {
            return;
        }
        if let (Some(outer), Some(inner)) = (outer, inner) {
            let frame = [
                outer.min.x.round(),
                outer.min.y.round(),
                inner.width().round(),
                inner.height().round(),
            ];
            if self.settings.window != Some(frame) {
                self.settings.window = Some(frame);
            }
        }
    }

    /// Moves the traffic lights to the middle of the 44 px title row (the title bar is
    /// transparent and the content runs under it). Checked every frame: AppKit lays the title
    /// bar out again on resizes and title changes; nothing is moved when they are in place.
    #[cfg(target_os = "macos")]
    fn place_traffic_lights(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if self.fullscreen {
            self.traffic_placed = None;
            return;
        }
        let _ = ctx;
        if let Some(w) = frame.winit_window() {
            crate::ui::macos::place_traffic_lights(w, 13.0, f64::from(TITLE_H / 2.0));
            self.traffic_placed = Some((0.0, 0.0));
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.tabs.get(self.active) {
            Some(tab) => {
                let remote = tab
                    .focused_pane()
                    .and_then(|p| p.title())
                    .filter(|t| !t.trim().is_empty());
                match remote {
                    Some(r) => format!("{} — {r}", tab.title()),
                    None => format!("{} — TernaryShell", tab.title()),
                }
            }
            None => "TernaryShell".to_string(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

impl eframe::App for TernaryShellApp {
    /// Debug builds: the events a dev script queued (see `devtest::push_egui_events`).
    #[cfg(debug_assertions)]
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        raw_input.events.extend(crate::devtest::take_egui_events());
    }

    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        // A bug that panics while drawing must not take the whole app (and every open session)
        // down: the panic hook has already written a report; the frame is skipped and the app
        // carries on.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.frame_ui(ui, frame)));
        if result.is_err() {
            let now = Instant::now();
            if self
                .last_recovered_panic
                .is_none_or(|t| now.duration_since(t) > Duration::from_secs(10))
            {
                self.toasts.error(tf(
                    "Something went wrong and TernaryShell recovered. A report was saved in {}.",
                    &[&paths::contract_tilde(&paths::logs_dir())],
                ));
            }
            self.last_recovered_panic = Some(now);
            ui.ctx().request_repaint();
        }
        self.guard_accessibility_focus(ui.ctx());
        self.first_frame = false;
    }

    #[cfg(target_os = "macos")]
    fn on_exit(&mut self) {
        self.exit();
    }

    // With OpenGL (Windows and Linux) eframe passes its context along.
    #[cfg(not(target_os = "macos"))]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.exit();
    }
}

impl TernaryShellApp {
    /// egui hands the focused widget to accessibility clients without checking that it was drawn
    /// this frame, and a focus outside the tree aborts the app (accesskit panics) whenever
    /// VoiceOver, a window manager or a writing assistant is attached. Such focus is dropped
    /// before the frame ends; a terminal pane gets it back when it is drawn next frame.
    fn guard_accessibility_focus(&mut self, ctx: &egui::Context) {
        let Some(id) = ctx.memory(|m| m.focused()) else {
            return;
        };
        let missing = ctx.viewport(|vp| {
            vp.this_pass
                .accesskit_state
                .as_ref()
                .is_some_and(|state| !state.nodes.contains_key(&id))
        });
        if missing {
            #[cfg(debug_assertions)]
            eprintln!("DEVTEST LOG: dropped focus on a widget that was not drawn ({id:?})");
            ctx.memory_mut(|m| m.surrender_focus(id));
            self.focus_target = None;
            ctx.request_repaint();
        }
    }

    fn frame_ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(debug_assertions)]
        crate::devtest::save_screenshots(&ctx);
        self.handle_menu_events();
        self.apply_settings_changes(&ctx);
        self.handle_shortcuts(&ctx);
        self.handle_close_request(&ctx);
        self.handle_dropped_files(&ctx);
        self.reload_store_if_changed(&ctx);
        if let Some(rx) = &self.instance_rx {
            while let Ok(args) = rx.try_recv() {
                self.startup.extend(args);
            }
        }
        if !self.startup.is_empty() {
            self.open_startup_targets(&ctx);
        }
        if self.first_frame
            && std::env::var_os("TERNARYSHELL_STARTUP_LOG").is_some()
            && let Some(start) = PROCESS_START.get()
        {
            eprintln!("first frame after {} ms", start.elapsed().as_millis());
        }
        #[cfg(debug_assertions)]
        self.run_dev_script(&ctx);

        let p = style::pal(ui);
        self.fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        #[cfg(target_os = "macos")]
        self.place_traffic_lights(&ctx, frame);
        self.track_steps();
        // Panels are laid out with the root Ui's item spacing between them; keep them flush and
        // restore normal spacing inside each panel.
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        // The sidebar first, so it runs the full height of the window (the traffic lights sit
        // in its top row); the title row and the canvas share the rest.
        let mut sidebar_visible = self.settings.sidebar_visible;
        let sidebar = egui::Panel::left("sidebar")
            .resizable(true)
            .show_separator_line(false)
            .default_size(self.settings.sidebar_width)
            .size_range(200.0..=480.0)
            .frame(egui::Frame::NONE.fill(p.chrome))
            .show_collapsible(ui, &mut sidebar_visible, |ui| {
                ui.spacing_mut().item_spacing = spacing;
                self.sidebar_panel(ui);
            });
        if let Some(r) = sidebar {
            let w = r.response.rect.width();
            if (w - self.settings.sidebar_width).abs() > 1.0 && sidebar_visible {
                self.settings.sidebar_width = w;
            }
        }
        self.settings.sidebar_visible = sidebar_visible;

        egui::Panel::top("title-row")
            .frame(egui::Frame::NONE.fill(p.chrome))
            .show_separator_line(false)
            .exact_size(TITLE_H)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = spacing;
                self.title_row(ui)
            });

        let gutter = chrome::GUTTER as i8;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.chrome).inner_margin(egui::Margin {
                left: if self.settings.sidebar_visible {
                    0
                } else {
                    gutter
                },
                right: gutter,
                top: 0,
                bottom: gutter,
            }))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = spacing;
                self.canvas(ui)
            });

        self.background_transfers(&ctx);
        let can_popup = !self.modal_open();
        if let Some(text) = self.updater.tick(&ctx, &mut self.settings, can_popup) {
            self.toasts.info(text);
        }
        self.overlays(&ctx);
        if let crate::ui::update_ui::UpdateEvent::Restart =
            self.updater.show_dialog(&ctx, &mut self.settings)
        {
            match self.quit_warning() {
                Some(details) => {
                    self.dialog = Some(dialogs::confirm_details(
                        t("Restart to update?"),
                        t("TernaryShell restarts and opens your connections again.").to_string(),
                        details,
                        t("Restart"),
                        ConfirmAction::RestartForUpdate,
                    ))
                }
                None => self.install_update(&ctx),
            }
        }
        self.process_actions(&ctx);
        self.watch_connections(&ctx);
        self.track_focus(&ctx, frame);
        self.remember_window(&ctx, frame);
        self.update_title(&ctx);
    }

    fn exit(&mut self) {
        // Transfers cut off by quitting remove their partial files first (a few seconds at
        // most), while the connections are still open.
        let pending: Vec<_> = self.tabs.iter().flat_map(Tab::cancel_transfers).collect();
        let deadline = Instant::now() + Duration::from_secs(3);
        while pending.iter().any(|t| !t.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
        }
        for tab in &self.tabs {
            tab.disconnect();
        }
        if self.settings_writable && self.settings != self.settings_saved {
            let _ = self.settings.save(&paths::settings_file());
        }
    }
}
