//! The connection editor (create and edit): a dialog with four sections (Server, Login,
//! Network, Session) in a side navigation. Secrets typed here go to the Keychain on save.

use eframe::egui::{
    self, Align2, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use uuid::Uuid;

use super::style::{self, Tone};
use super::{icons, kit};
use crate::bookmarks::{Auth, Bookmark, BookmarkStore, Forward, ForwardKind};
use crate::i18n::{t, tf};
use crate::paths;
use crate::secrets::{self, SecretKind, SecretSet};
use crate::ssh::keys::{self, KeyCheck};

/// How the connection logs in, as chosen in the editor.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthKind {
    Password,
    /// A private key pasted into the editor, kept in the Keychain.
    PastedKey,
    /// A private key file on this computer.
    KeyFile,
    /// The keys of an SSH agent.
    Agent,
}

/// The editor's sections.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Section {
    #[default]
    Server,
    Login,
    Network,
    Session,
}

/// A port forward being edited (the numbers as typed).
#[derive(Clone)]
struct ForwardRow {
    kind: ForwardKind,
    listen: String,
    host: String,
    port: String,
}

impl ForwardRow {
    fn from(f: &Forward) -> Self {
        Self {
            kind: f.kind,
            listen: f.listen_port.to_string(),
            host: f.host.clone(),
            port: if f.port == 0 {
                String::new()
            } else {
                f.port.to_string()
            },
        }
    }

    fn to_forward(&self) -> Result<Forward, String> {
        let number = |text: &str| text.trim().parse::<u16>().ok().filter(|p| *p > 0);
        let listen = number(&self.listen).ok_or_else(|| {
            t("Port forwarding: each forward needs a port between 1 and 65535.").to_string()
        })?;
        if self.kind == ForwardKind::Dynamic {
            return Ok(Forward {
                kind: self.kind,
                listen_port: listen,
                host: String::new(),
                port: 0,
            });
        }
        let host = self.host.trim();
        match number(&self.port) {
            Some(port) if !host.is_empty() => Ok(Forward {
                kind: self.kind,
                listen_port: listen,
                host: host.to_string(),
                port,
            }),
            _ => Err(tf(
                "Port forwarding: say where port {} goes (host and port).",
                &[&listen.to_string()],
            )),
        }
    }
}

/// SSH agents found on this computer besides the system one: their name and socket (or, on
/// Windows, "pageant").
fn known_agents() -> Vec<(&'static str, String)> {
    #[cfg(target_os = "macos")]
    let candidates = vec![
        (
            "1Password",
            "~/Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock".to_string(),
        ),
        (
            "Secretive",
            "~/Library/Containers/com.maxgoedjen.Secretive.SecretAgent/Data/socket.ssh".to_string(),
        ),
    ];
    // PuTTY's agent, which KeePass and others also speak. 1Password and the OpenSSH agent use
    // the system pipe.
    #[cfg(windows)]
    return vec![("Pageant", "pageant".to_string())];
    #[cfg(not(any(target_os = "macos", windows)))]
    let candidates = {
        let mut list = vec![
            ("1Password", "~/.1password/agent.sock".to_string()),
            ("Bitwarden", "~/.bitwarden-ssh-agent.sock".to_string()),
        ];
        if let Some(run) = std::env::var_os("XDG_RUNTIME_DIR") {
            let run = std::path::PathBuf::from(run);
            list.push(("GNOME Keyring", run.join("gcr/ssh").display().to_string()));
        }
        list
    };
    #[cfg(not(windows))]
    candidates
        .into_iter()
        .filter(|(_, path)| paths::expand_tilde(path).exists())
        .collect()
}

/// Why saving failed: the message, the section it is in, and the forward row it concerns.
struct Problem {
    message: String,
    section: Section,
    forward: Option<usize>,
}

pub struct Editor {
    pub draft: Bookmark,
    pub is_new: bool,
    section: Section,
    port_text: String,
    tags: Vec<String>,
    tag_input: String,
    password: String,
    passphrase: String,
    had_password: bool,
    had_passphrase: bool,
    clear_password: bool,
    clear_passphrase: bool,
    /// Why saving failed (and where), or an error from the Keychain.
    problem: Option<Problem>,
    focus_first: bool,
    key_files: Vec<String>,
    auth: AuthKind,
    key_path: String,
    key_text: String,
    /// A pasted key is already in the Keychain (editing an existing connection).
    had_stored_key: bool,
    replace_stored_key: bool,
    /// Password and passphrase fields show their text (and a saved one, loaded to be read).
    show_password: bool,
    show_passphrase: bool,
    /// The saved pasted key, while it is shown.
    shown_key: Option<String>,
    /// The last checked key text and what it is.
    key_check: Option<(String, KeyCheck)>,
    /// The login method when the editor opened (a saved passphrase belongs to that key).
    original_auth: Auth,
    agent_socket: String,
    agents: Vec<(&'static str, String)>,
    forwards: Vec<ForwardRow>,
}

#[allow(clippy::large_enum_variant)] // short-lived, returned once per frame
pub enum EditorResult {
    Open,
    Cancel,
    Save {
        bookmark: Bookmark,
        set: SecretSet,
        clear: Vec<SecretKind>,
        connect: bool,
    },
}

fn ssh_keys() -> Vec<String> {
    let dir = paths::home_dir().join(".ssh");
    let mut keys: Vec<String> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let looks_private = !name.ends_with(".pub")
                && !matches!(
                    name.as_str(),
                    "known_hosts" | "known_hosts.old" | "config" | "authorized_keys"
                )
                && path.is_file()
                && std::fs::read_to_string(&path)
                    .map(|s| s.contains("PRIVATE KEY-----"))
                    .unwrap_or(false);
            looks_private.then(|| format!("~/.ssh/{name}"))
        })
        .collect();
    keys.sort();
    keys
}

impl Editor {
    pub fn new(bookmark: Bookmark, is_new: bool) -> Self {
        let had_password = !is_new && secrets::has(bookmark.id, SecretKind::Password);
        let had_passphrase = !is_new && secrets::has(bookmark.id, SecretKind::Passphrase);
        let had_stored_key = !is_new
            && matches!(bookmark.auth, Auth::KeychainKey)
            && secrets::has(bookmark.id, SecretKind::PrivateKey);
        let key_files = ssh_keys();
        let first_key = key_files.first().cloned().unwrap_or_default();
        let (auth, key_path, agent_socket) = match &bookmark.auth {
            Auth::Password => (AuthKind::Password, first_key, String::new()),
            Auth::Key { path } => (AuthKind::KeyFile, path.clone(), String::new()),
            Auth::KeychainKey => (AuthKind::PastedKey, first_key, String::new()),
            Auth::Agent { socket } => (AuthKind::Agent, first_key, socket.clone()),
        };
        Self {
            original_auth: bookmark.auth.clone(),
            port_text: bookmark.port.to_string(),
            tags: bookmark.tags.clone(),
            tag_input: String::new(),
            forwards: bookmark.forwards.iter().map(ForwardRow::from).collect(),
            draft: bookmark,
            is_new,
            section: Section::Server,
            password: String::new(),
            passphrase: String::new(),
            had_password,
            had_passphrase,
            clear_password: false,
            clear_passphrase: false,
            problem: None,
            focus_first: true,
            key_files,
            auth,
            key_path,
            key_text: String::new(),
            had_stored_key,
            replace_stored_key: false,
            show_password: false,
            show_passphrase: false,
            shown_key: None,
            key_check: None,
            agent_socket,
            agents: known_agents(),
        }
    }

    /// Opens on `section` (e.g. Network from the forwards popover).
    pub fn with_section(mut self, section: Option<Section>) -> Self {
        if let Some(s) = section {
            self.section = s;
            self.focus_first = s == Section::Server;
        }
        self
    }

    /// Shows why saving failed; the editor stays open with everything typed.
    pub fn set_error(&mut self, error: String) {
        self.problem = Some(Problem {
            message: error,
            section: Section::Login,
            forward: None,
        });
        self.section = Section::Login;
    }

    /// What the pasted key text is (re-checked only when it changes).
    fn pasted_key_check(&mut self) -> Option<KeyCheck> {
        if self.key_text.trim().is_empty() {
            return None;
        }
        if self
            .key_check
            .as_ref()
            .is_none_or(|(text, _)| *text != self.key_text)
        {
            self.key_check = Some((self.key_text.clone(), keys::check_key_text(&self.key_text)));
        }
        self.key_check.as_ref().map(|(_, c)| c.clone())
    }

    /// A new pasted key will be saved (not keeping the one already in the Keychain).
    fn new_pasted_key(&self) -> bool {
        self.auth == AuthKind::PastedKey && (!self.had_stored_key || self.replace_stored_key)
    }

    fn validate(&mut self) -> bool {
        self.take_tag_input();
        match self.check() {
            Ok(()) => {
                self.problem = None;
                true
            }
            Err(p) => {
                self.section = p.section;
                self.problem = Some(p);
                false
            }
        }
    }

    fn check(&mut self) -> Result<(), Problem> {
        let server = |m: String| Problem {
            message: m,
            section: Section::Server,
            forward: None,
        };
        let login = |m: String| Problem {
            message: m,
            section: Section::Login,
            forward: None,
        };
        self.draft.username = self.draft.username.trim().to_string();
        // Accept "user@host:port" (and "[ipv6]:port") typed into the host field.
        let target = crate::bookmarks::model::parse_target(&self.draft.host).map_err(server)?;
        if let Some(user) = target.user {
            self.draft.username = user;
        }
        self.draft.host = target.host;
        if let Some(p) = target.port {
            self.port_text = p.to_string();
        }
        match self.port_text.trim().parse::<u16>() {
            Ok(p) if p > 0 => self.draft.port = p,
            _ => {
                return Err(server(
                    t("Port must be a number between 1 and 65535.").into(),
                ));
            }
        }
        self.draft.auth = match self.auth {
            AuthKind::Password => Auth::Password,
            AuthKind::KeyFile => {
                let path = self.key_path.trim().to_string();
                if path.is_empty() {
                    return Err(login(t("Choose a private key file.").into()));
                }
                if !paths::expand_tilde(&path).is_file() {
                    return Err(login(t("The private key file does not exist.").into()));
                }
                Auth::Key { path }
            }
            AuthKind::PastedKey => {
                if self.new_pasted_key() {
                    match self.pasted_key_check() {
                        None => return Err(login(t("Paste the private key.").into())),
                        Some(KeyCheck::PublicKey) => {
                            return Err(login(t("This is a public key. Paste the private key instead (the file without .pub).").into()));
                        }
                        Some(KeyCheck::Invalid) => {
                            return Err(login(t("This does not look like a private key.").into()));
                        }
                        Some(KeyCheck::Valid(_) | KeyCheck::Encrypted) => {}
                    }
                }
                Auth::KeychainKey
            }
            AuthKind::Agent => Auth::Agent {
                socket: self.agent_socket.trim().to_string(),
            },
        };
        let mut forwards = Vec::new();
        for (i, row) in self.forwards.iter().enumerate() {
            forwards.push(row.to_forward().map_err(|m| Problem {
                message: m,
                section: Section::Network,
                forward: Some(i),
            })?);
        }
        // One port can only be listened on once: here (local, SOCKS) or on the server.
        let mut here = std::collections::HashSet::new();
        let mut there = std::collections::HashSet::new();
        for (i, f) in forwards.iter().enumerate() {
            let fresh = match f.kind {
                ForwardKind::Remote => there.insert(f.listen_port),
                _ => here.insert(f.listen_port),
            };
            if !fresh {
                return Err(Problem {
                    message: tf(
                        "Port forwarding: port {} is used twice.",
                        &[&f.listen_port.to_string()],
                    ),
                    section: Section::Network,
                    forward: Some(i),
                });
            }
        }
        self.draft.forwards = forwards;
        self.draft.tags = self.tags.clone();
        Ok(())
    }

    /// A tag being typed becomes a chip.
    fn take_tag_input(&mut self) {
        for part in self.tag_input.split(',') {
            let tag = part.trim();
            if !tag.is_empty() && !self.tags.iter().any(|t| t == tag) {
                self.tags.push(tag.to_string());
            }
        }
        self.tag_input.clear();
    }

    pub fn show(&mut self, ctx: &egui::Context, store: &BookmarkStore) -> EditorResult {
        let mut result = EditorResult::Open;
        let screen = ctx.content_rect();
        let size = vec2(
            800.0f32.min(screen.width() - 40.0),
            590.0f32.min(screen.height() - 40.0),
        );
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
        let mut save = false;
        let mut save_connect = enter;
        let modal = style::modal(ctx, egui::Id::new("bookmark-editor")).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let p = style::pal(ui);
            let full = ui.max_rect();
            let nav = egui::Rect::from_min_size(full.min, vec2(200.0, size.y));
            ui.painter().rect_filled(
                nav,
                CornerRadius {
                    nw: style::RADIUS_XL,
                    sw: style::RADIUS_XL,
                    ne: 0,
                    se: 0,
                },
                p.nav,
            );
            ui.painter()
                .vline(nav.right() - 0.5, nav.y_range(), Stroke::new(1.0, p.border));
            let mut nav_ui =
                ui.new_child(egui::UiBuilder::new().max_rect(nav.shrink2(vec2(8.0, 14.0))));
            self.nav(&mut nav_ui);
            let main = egui::Rect::from_min_max(pos2(nav.right(), full.top()), full.max);
            let mut main_ui = ui.new_child(egui::UiBuilder::new().max_rect(main));
            let (title, sub) = match self.section {
                Section::Server => (
                    t("Server"),
                    t("Where to connect, and where it lives in the sidebar."),
                ),
                Section::Login => (t("Login"), t("How TernaryShell proves who you are.")),
                Section::Network => (
                    t("Network"),
                    t("Reach the server through other hosts, and forward ports while connected."),
                ),
                Section::Session => (t("Session"), t("What happens right after you log in.")),
            };
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 24,
                    right: 16,
                    top: 18,
                    bottom: 4,
                })
                .show(&mut main_ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(
                                RichText::new(title)
                                    .font(style::semibold(style::TITLE))
                                    .color(p.text),
                            );
                            ui.label(
                                RichText::new(sub)
                                    .font(style::regular(style::SMALL))
                                    .color(p.text_muted),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            if kit::icon_button_ex(ui, icons::X, t("Close"), false, 22.0).clicked()
                            {
                                result = EditorResult::Cancel;
                            }
                        });
                    });
                });
            let foot_h = 12.0 + style::CONTROL_H + 12.0;
            let body_h = main_ui.available_height() - foot_h;
            egui::ScrollArea::vertical()
                .id_salt(("editor-form", self.section as u8))
                .max_height(body_h)
                .min_scrolled_height(body_h)
                .auto_shrink([false, false])
                .show(&mut main_ui, |ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 24,
                            right: 24,
                            top: 10,
                            bottom: 18,
                        })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                            style::form_spacing(ui);
                            ui.spacing_mut().item_spacing.y = 0.0;
                            match self.section {
                                Section::Server => self.server_section(ui, store),
                                Section::Login => self.login_section(ui),
                                Section::Network => self.network_section(ui, store),
                                Section::Session => self.session_section(ui),
                            }
                        });
                });
            kit::dialog_footer(
                &mut main_ui,
                |ui| match &self.problem {
                    Some(prob) => {
                        let text = if prob.section == Section::Network && prob.forward.is_some() {
                            t("1 problem in Network").to_string()
                        } else {
                            prob.message.clone()
                        };
                        ui.spacing_mut().item_spacing.x = 6.0;
                        kit::icon(ui, icons::WARNING_CIRCLE, 14.0, p.err);
                        ui.add(
                            egui::Label::new(
                                RichText::new(text)
                                    .font(style::regular(style::SMALL))
                                    .color(p.err),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&prob.message);
                    }
                    None if self.section == Section::Login && self.auth == AuthKind::PastedKey => {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        kit::icon(ui, icons::INFO, 14.0, p.text_faint);
                        kit::footer_hint(
                            ui,
                            &tf(
                                "The pasted key is stored in {}, not written to disk.",
                                &[crate::platform::keychain()],
                            ),
                        );
                    }
                    None => {}
                },
                |ui| {
                    let sc = super::keys::Shortcut::SaveAndConnect.label();
                    let r = kit::button_kbd(
                        ui,
                        kit::Kind::Primary,
                        None,
                        t("Save & Connect"),
                        Some(&sc),
                        false,
                        false,
                    );
                    #[cfg(debug_assertions)]
                    crate::devtest::record("editor-save-connect", r.rect);
                    if r.clicked() {
                        save_connect = true;
                    }
                    if kit::secondary(ui, None, t("Save")).clicked() {
                        save = true;
                    }
                    if kit::secondary(ui, None, t("Cancel")).clicked() {
                        result = EditorResult::Cancel;
                    }
                },
            );
        });
        if (save || save_connect) && matches!(result, EditorResult::Open) && self.validate() {
            result = self.saved(save_connect);
        }
        if super::dialogs::escape_only(ctx, &modal) && matches!(result, EditorResult::Open) {
            result = EditorResult::Cancel;
        }
        result
    }

    /// The side navigation: who is being edited, the four sections, the Keychain note.
    fn nav(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        ui.spacing_mut().item_spacing.y = 1.0;
        let name = if self.draft.name.trim().is_empty() {
            if self.draft.host.trim().is_empty() {
                if self.is_new {
                    t("New connection").to_string()
                } else {
                    self.draft.display_name()
                }
            } else {
                self.draft.host.trim().to_string()
            }
        } else {
            self.draft.name.trim().to_string()
        };
        let address = {
            let host = self.draft.host.trim();
            if host.is_empty() {
                "—".to_string()
            } else if self.draft.username.trim().is_empty() {
                host.to_string()
            } else {
                format!("{}@{}", self.draft.username.trim(), host)
            }
        };
        let (id_rect, _) = ui.allocate_exact_size(
            vec2(ui.available_width(), 34.0 + 4.0 + 16.0),
            Sense::hover(),
        );
        let tile = egui::Rect::from_min_size(id_rect.min + vec2(6.0, 4.0), vec2(34.0, 34.0));
        kit::paint_icon_tile(ui.painter(), tile, icons::HARD_DRIVES, Tone::Neutral, &p);
        let text_x = tile.right() + 10.0;
        let clip = egui::Rect::from_min_max(
            pos2(text_x, id_rect.top()),
            pos2(id_rect.right() - 4.0, id_rect.bottom()),
        );
        ui.painter().with_clip_rect(clip).text(
            pos2(text_x, tile.top() + 8.0),
            Align2::LEFT_CENTER,
            name,
            style::semibold(style::BODY),
            p.text,
        );
        ui.painter().with_clip_rect(clip).text(
            pos2(text_x, tile.top() + 26.0),
            Align2::LEFT_CENTER,
            address,
            style::mono(style::XS),
            p.text_faint,
        );
        let problem = self.problem.as_ref().map(|p| p.section);
        for (section, icon, label) in [
            (Section::Server, icons::HARD_DRIVES, t("Server")),
            (Section::Login, icons::KEY, t("Login")),
            (Section::Network, icons::TREE_STRUCTURE, t("Network")),
            (Section::Session, icons::TERMINAL, t("Session")),
        ] {
            let r = super::dialogs::nav_item(
                ui,
                icon,
                label,
                self.section == section,
                problem == Some(section),
            );
            #[cfg(debug_assertions)]
            crate::devtest::record(format!("editor-section:{}", section as u8), r.rect);
            if r.clicked() {
                self.section = section;
            }
        }
        let foot = ui.max_rect();
        let note = tf(
            "Passwords and keys are saved in {}.",
            &[crate::platform::keychain()],
        );
        let g = ui.painter().layout(
            format!("{}  {note}", icons::LOCK_SIMPLE),
            style::regular(style::XS),
            p.text_faint,
            foot.width() - 20.0,
        );
        ui.painter().galley(
            pos2(foot.left() + 10.0, foot.bottom() - g.size().y - 2.0),
            g,
            p.text_faint,
        );
    }

    fn separator(ui: &mut Ui) {
        let p = style::pal(ui);
        ui.add_space(16.0);
        let y = ui.cursor().top();
        ui.painter()
            .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.border));
        ui.add_space(16.0);
    }

    fn server_section(&mut self, ui: &mut Ui, store: &BookmarkStore) {
        style::row(ui, t("Name"), |ui| {
            let r = ui.add(style::input(&mut self.draft.name).hint_text(t("e.g. Production web")));
            #[cfg(debug_assertions)]
            crate::devtest::record("editor-name", r.rect);
            if std::mem::take(&mut self.focus_first) {
                r.request_focus();
            }
        });
        ui.add_space(12.0);
        let host_problem = self
            .problem
            .as_ref()
            .is_some_and(|p| p.section == Section::Server);
        style::row(ui, t("Host"), |ui| {
            // Left to right, so Tab goes from the host to the port.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                const PORT_W: f32 = 76.0;
                let label_w = style::text_width(ui, t("Port"));
                let host_w = ui.available_width() - PORT_W - label_w - 16.0;
                let _host = ui.add(
                    style::input(&mut self.draft.host)
                        .hint_text("server.example.com")
                        .desired_width(host_w.max(80.0))
                        .mono()
                        .error(host_problem && !self.port_text.trim().parse::<u16>().is_err()),
                );
                ui.label(
                    RichText::new(t("Port"))
                        .font(style::medium(style::SMALL))
                        .color(style::pal(ui).text_muted),
                );
                let port_bad = self
                    .port_text
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .is_none();
                let _port = ui.add(
                    style::input(&mut self.port_text)
                        .desired_width(PORT_W)
                        .mono()
                        .error(host_problem && port_bad),
                );
                #[cfg(debug_assertions)]
                {
                    crate::devtest::record("editor-host", _host.rect);
                    crate::devtest::record("editor-port", _port.rect);
                }
            });
            style::hint(
                ui,
                t("Tip: paste user@host:port here and it is split for you."),
            );
        });
        ui.add_space(12.0);
        style::row(ui, t("Username"), |ui| {
            let _user = ui.add(
                style::input(&mut self.draft.username)
                    .hint_text(std::env::var("USER").unwrap_or_default())
                    .mono(),
            );
            #[cfg(debug_assertions)]
            crate::devtest::record("editor-user", _user.rect);
        });
        Self::separator(ui);
        style::row(ui, t("Group"), |ui| {
            let current = if self.draft.group.is_some() {
                format!(
                    "{}  {}",
                    icons::FOLDER_SIMPLE,
                    store.group_path(self.draft.group)
                )
            } else {
                t("(none)").to_string()
            };
            egui::ComboBox::from_id_salt("group")
                .icon(kit::combo_caret)
                .selected_text(current)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.draft.group.is_none(), t("(none)"))
                        .clicked()
                    {
                        self.draft.group = None;
                    }
                    let mut groups: Vec<(String, Uuid)> = store
                        .groups
                        .iter()
                        .map(|g| (store.group_path(Some(g.id)), g.id))
                        .collect();
                    groups.sort();
                    for (path, id) in groups {
                        if ui
                            .selectable_label(self.draft.group == Some(id), path)
                            .clicked()
                        {
                            self.draft.group = Some(id);
                        }
                    }
                });
        });
        ui.add_space(12.0);
        style::row(ui, t("Tags"), |ui| self.tags_input(ui));
        ui.add_space(12.0);
        style::row(ui, t("Notes"), |ui| {
            ui.add(style::input_multiline(&mut self.draft.notes).desired_rows(3));
        });
    }

    /// Tags as chips in a field: type and press Enter or a comma; Backspace in an empty field
    /// removes the last one.
    fn tags_input(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let id = egui::Id::new("editor-tag-input");
        let focused = ui.memory(|m| m.has_focus(id));
        let width = ui.available_width();
        let mut remove = None;
        let frame = egui::Frame::NONE
            .fill(p.field)
            .stroke(Stroke::new(
                1.0,
                if focused { p.accent } else { p.field_border },
            ))
            .corner_radius(CornerRadius::same(style::RADIUS))
            .inner_margin(egui::Margin::symmetric(6, 3))
            .show(ui, |ui| {
                ui.set_width(width - 12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                    for (i, tag) in self.tags.iter().enumerate() {
                        let g = ui.painter().layout_no_wrap(
                            tag.clone(),
                            style::regular(style::SMALL),
                            p.text_muted,
                        );
                        let (r, _) = ui.allocate_exact_size(
                            vec2(g.size().x + 8.0 + 4.0 + 14.0 + 6.0, 22.0),
                            Sense::hover(),
                        );
                        ui.painter().rect(
                            r,
                            CornerRadius::same(style::RADIUS),
                            p.hover,
                            Stroke::new(1.0, p.line_soft),
                            StrokeKind::Inside,
                        );
                        ui.painter().galley(
                            pos2(r.left() + 8.0, r.center().y - g.size().y / 2.0),
                            g,
                            p.text_muted,
                        );
                        let x_rect = egui::Rect::from_center_size(
                            pos2(r.right() - 6.0 - 7.0, r.center().y),
                            vec2(14.0, 14.0),
                        );
                        let x = ui.interact(x_rect, egui::Id::new(("tag-x", i)), Sense::click());
                        ui.painter().text(
                            x_rect.center(),
                            Align2::CENTER_CENTER,
                            icons::X,
                            style::icon_font(11.0),
                            if x.hovered() { p.text } else { p.text_faint },
                        );
                        if x.clicked() {
                            remove = Some(i);
                        }
                    }
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.tag_input)
                            .id(id)
                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 3)))
                            .hint_text(RichText::new(t("Add tag…")).color(p.text_faint))
                            .desired_width(120.0),
                    );
                    if edit.changed() && self.tag_input.contains(',') {
                        self.take_tag_input();
                    }
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.take_tag_input();
                        edit.request_focus();
                    } else if edit.lost_focus() {
                        self.take_tag_input();
                    }
                    if edit.has_focus()
                        && self.tag_input.is_empty()
                        && ui.input(|i| i.key_pressed(egui::Key::Backspace))
                    {
                        self.tags.pop();
                    }
                });
            });
        if focused {
            style::focus_halo(ui, frame.response.rect, style::RADIUS);
        }
        if frame.response.interact(Sense::click()).clicked() {
            ui.memory_mut(|m| m.request_focus(id));
        }
        if let Some(i) = remove {
            self.tags.remove(i);
        }
    }

    fn login_section(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        // The method, as three cards.
        let key_kind = if self.auth == AuthKind::KeyFile {
            AuthKind::KeyFile
        } else {
            AuthKind::PastedKey
        };
        let picked = match self.auth {
            AuthKind::PastedKey | AuthKind::KeyFile => key_kind,
            other => other,
        };
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = (ui.available_width() - 16.0) / 3.0;
            let cards = [
                (
                    AuthKind::Password,
                    icons::LOCK_KEY,
                    t("Password"),
                    tf("Typed once, kept in {}", &[crate::platform::keychain()]),
                ),
                (
                    key_kind,
                    icons::KEY,
                    t("Private key"),
                    t("Pasted, or a file in ~/.ssh").to_string(),
                ),
                (
                    AuthKind::Agent,
                    icons::FINGERPRINT,
                    t("SSH agent"),
                    t("ssh-add, 1Password, Secretive").to_string(),
                ),
            ];
            for (i, (kind, icon, title, desc)) in cards.into_iter().enumerate() {
                let r = kit::method_card(ui, icon, title, &desc, picked == kind, w);
                #[cfg(debug_assertions)]
                {
                    if i == 1 {
                        crate::devtest::record("editor-auth-key", r.rect);
                    }
                    if i == 2 {
                        crate::devtest::record("editor-auth-agent", r.rect);
                    }
                }
                let _ = i;
                if r.clicked() {
                    self.auth = kind;
                }
            }
        });
        ui.add_space(16.0);
        let login_problem = self
            .problem
            .as_ref()
            .is_some_and(|p| p.section == Section::Login);
        match self.auth {
            AuthKind::Password => {
                style::row(ui, t("Password"), |ui| {
                    let saved = self.had_password && !self.clear_password;
                    let hint = if saved {
                        tf(
                            "Saved in {} — type to replace",
                            &[crate::platform::keychain()],
                        )
                    } else {
                        t("Leave empty to ask when connecting").to_string()
                    };
                    secret_input(
                        ui,
                        &mut self.password,
                        &mut self.show_password,
                        &hint,
                        saved.then_some((self.draft.id, SecretKind::Password)),
                        &mut self.problem,
                        "editor-show-password",
                    );
                    if self.had_password {
                        if saved_row(ui, &mut self.clear_password, t("Forget saved password"))
                            && self.clear_password
                        {
                            // A shown saved password would otherwise be saved again.
                            self.password.clear();
                            self.show_password = false;
                        }
                    } else {
                        style::hint(ui, t("Leave empty to be asked when connecting."));
                    }
                });
            }
            AuthKind::PastedKey | AuthKind::KeyFile => {
                style::row(ui, t("Key"), |ui| {
                    kit::segmented(
                        ui,
                        &mut self.auth,
                        &[
                            (AuthKind::PastedKey, t("Paste key")),
                            (AuthKind::KeyFile, t("Key file")),
                        ],
                        true,
                    );
                    if self.auth == AuthKind::KeyFile {
                        self.key_file_ui(ui, login_problem);
                    } else {
                        self.pasted_key_ui(ui, login_problem);
                    }
                });
                ui.add_space(12.0);
                style::row(ui, t("Passphrase"), |ui| {
                    let saved = self.had_passphrase && !self.clear_passphrase;
                    let hint = if saved {
                        tf(
                            "Saved in {} — type to replace",
                            &[crate::platform::keychain()],
                        )
                    } else {
                        t("Only if the key is encrypted").to_string()
                    };
                    secret_input(
                        ui,
                        &mut self.passphrase,
                        &mut self.show_passphrase,
                        &hint,
                        saved.then_some((self.draft.id, SecretKind::Passphrase)),
                        &mut self.problem,
                        "editor-show-passphrase",
                    );
                    if self.had_passphrase
                        && saved_row(ui, &mut self.clear_passphrase, t("Forget saved passphrase"))
                        && self.clear_passphrase
                    {
                        self.passphrase.clear();
                        self.show_passphrase = false;
                    }
                });
            }
            AuthKind::Agent => {
                style::row(ui, t("Agent"), |ui| self.agent_ui(ui));
            }
        }
        let _ = p;
    }

    fn key_file_ui(&mut self, ui: &mut Ui, problem: bool) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let browse_w = style::text_width(ui, t("Browse…")) + 18.0;
            let combo_w = if self.key_files.is_empty() { 0.0 } else { 36.0 };
            let field_w = ui.available_width() - browse_w - combo_w - 16.0;
            ui.add(
                style::input(&mut self.key_path)
                    .hint_text("~/.ssh/id_ed25519")
                    .desired_width(field_w)
                    .mono()
                    .error(problem),
            );
            if !self.key_files.is_empty() {
                egui::ComboBox::from_id_salt("key-files")
                    .icon(kit::combo_caret)
                    .selected_text("")
                    .width(28.0)
                    .show_ui(ui, |ui| {
                        for k in &self.key_files {
                            if ui.selectable_label(self.key_path == *k, k).clicked() {
                                self.key_path = k.clone();
                            }
                        }
                    })
                    .response
                    .on_hover_text(t("Keys in ~/.ssh"));
            }
            if kit::secondary_sm(ui, None, t("Browse…")).clicked()
                && let Some(p) = rfd::FileDialog::new()
                    .set_directory(paths::home_dir().join(".ssh"))
                    .pick_file()
            {
                self.key_path = paths::contract_tilde(&p);
            }
        });
    }

    fn pasted_key_ui(&mut self, ui: &mut Ui, problem: bool) {
        let p = style::pal(ui);
        if self.had_stored_key && !self.replace_stored_key {
            let mut show = false;
            let mut replace = false;
            kit::callout_rich(ui, icons::KEY, Tone::Neutral, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tf(
                            "A private key is saved in {}.",
                            &[crate::platform::keychain()],
                        ))
                        .font(style::regular(style::SMALL))
                        .color(p.text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        replace = kit::secondary_sm(ui, None, t("Replace…")).clicked();
                        let r = kit::ghost(
                            ui,
                            None,
                            if self.shown_key.is_some() {
                                t("Hide")
                            } else {
                                t("Show")
                            },
                        );
                        #[cfg(debug_assertions)]
                        crate::devtest::record("editor-show-key", r.rect);
                        show = r.clicked();
                    });
                });
            });
            if show && self.shown_key.take().is_none() {
                match secrets::get(self.draft.id, SecretKind::PrivateKey) {
                    Ok(Some(key)) => self.shown_key = Some(key),
                    Ok(None) => self.set_error(tf(
                        "The private key is not in {} any more.",
                        &[crate::platform::keychain()],
                    )),
                    Err(e) => self.set_error(format!("{e:#}")),
                }
            }
            if replace {
                self.replace_stored_key = true;
                self.shown_key = None;
            }
            // Shown as it is saved, to read or copy; changing it goes through Replace.
            if let Some(key) = &self.shown_key {
                let mut view = key.as_str();
                egui::ScrollArea::vertical()
                    .id_salt("editor-shown-key")
                    .max_height(110.0)
                    .show(ui, |ui| {
                        ui.add(
                            style::input_multiline(&mut view)
                                .font(style::mono(11.5))
                                .desired_rows(5),
                        )
                    });
                if kit::ghost(ui, Some(icons::COPY), t("Copy")).clicked() {
                    ui.ctx().copy_text(key.clone());
                }
            }
            return;
        }
        // Long keys (RSA is ~50 lines) scroll inside the box.
        let empty_bad = problem && self.key_text.trim().is_empty();
        let _key_box = egui::ScrollArea::vertical()
            .id_salt("editor-key-scroll")
            .max_height(110.0)
            .show(ui, |ui| {
                ui.add(
                    style::input_multiline(&mut self.key_text)
                        .font(style::mono(11.5))
                        .hint_text("-----BEGIN OPENSSH PRIVATE KEY-----\n…\n-----END OPENSSH PRIVATE KEY-----")
                        .desired_rows(5)
                        .error(empty_bad),
                )
            })
            .inner;
        #[cfg(debug_assertions)]
        crate::devtest::record("editor-key-text", _key_box.rect);
        let check = self.pasted_key_check();
        ui.horizontal(|ui| {
            match &check {
                Some(KeyCheck::Valid(kind)) => {
                    kit::badge(
                        ui,
                        &tf("{} private key", &[kind]),
                        Tone::Ok,
                        Some(icons::CHECK_CIRCLE),
                    );
                }
                Some(KeyCheck::Encrypted) => {
                    kit::badge(
                        ui,
                        t("Encrypted private key"),
                        Tone::Ok,
                        Some(icons::CHECK_CIRCLE),
                    );
                }
                Some(KeyCheck::PublicKey) | Some(KeyCheck::Invalid) => {
                    kit::badge(ui, t("Not a private key"), Tone::Err, Some(icons::X_CIRCLE));
                }
                None => {}
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::ghost(ui, Some(icons::FILE_ARROW_UP), t("Import from file…")).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .set_directory(paths::home_dir().join(".ssh"))
                        .pick_file()
                {
                    match std::fs::read(&path) {
                        Ok(data) if data.len() <= keys::MAX_KEY_TEXT => {
                            self.key_text = String::from_utf8_lossy(&data).into_owned()
                        }
                        Ok(_) => self.set_error(t("This does not look like a private key.").into()),
                        Err(e) => self.set_error(format!("{}: {e}", path.display())),
                    }
                }
                if self.had_stored_key && kit::ghost(ui, None, t("Keep the saved key")).clicked() {
                    self.replace_stored_key = false;
                    self.key_text.clear();
                }
            });
        });
        match check {
            Some(KeyCheck::PublicKey) => style::hint_tone(
                ui,
                t("This is a public key. Paste the private key instead (the file without .pub)."),
                Tone::Err,
            ),
            Some(KeyCheck::Invalid) => {
                style::hint_tone(ui, t("This does not look like a private key."), Tone::Err)
            }
            Some(KeyCheck::Encrypted) => {
                style::hint(ui, t("Enter its passphrase below, or when connecting."))
            }
            _ => {}
        }
    }

    fn agent_ui(&mut self, ui: &mut Ui) {
        let hint = if cfg!(windows) {
            t("OpenSSH agent, a pipe, or \"pageant\"")
        } else {
            t("System agent (SSH_AUTH_SOCK)")
        };
        ui.add(style::input(&mut self.agent_socket).hint_text(hint).mono());
        if !self.agents.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(
                    RichText::new(t("Use:"))
                        .font(style::regular(style::SMALL))
                        .color(style::pal(ui).text_faint),
                );
                let mut pick = None;
                for (name, path) in &self.agents {
                    if kit::button_full(
                        ui,
                        kit::Kind::Ghost,
                        None,
                        name,
                        true,
                        self.agent_socket == *path,
                    )
                    .on_hover_text(path)
                    .clicked()
                    {
                        pick = Some(path.clone());
                    }
                }
                if !self.agent_socket.is_empty()
                    && kit::ghost(ui, None, t("System agent")).clicked()
                {
                    pick = Some(String::new());
                }
                if let Some(p) = pick {
                    self.agent_socket = p;
                }
            });
        }
        let socket = self.agent_socket.trim();
        let (ok, text) = if cfg!(windows) && socket.is_empty() {
            (
                true,
                t("Uses the Windows OpenSSH agent (keys added with ssh-add).").to_string(),
            )
        } else if cfg!(windows) && socket.eq_ignore_ascii_case("pageant") {
            (
                true,
                t("Uses Pageant (PuTTY, KeePass and others).").to_string(),
            )
        } else if cfg!(windows) {
            (true, t("Uses the agent at this pipe.").to_string())
        } else if socket.is_empty() {
            match std::env::var_os("SSH_AUTH_SOCK") {
                Some(_) if cfg!(target_os = "macos") => (
                    true,
                    t("Uses the macOS SSH agent (keys added with ssh-add).").to_string(),
                ),
                Some(_) => (
                    true,
                    t("Uses your session's SSH agent (keys added with ssh-add).").to_string(),
                ),
                None => (
                    false,
                    t("No system agent is running: choose an agent's socket.").to_string(),
                ),
            }
        } else if paths::expand_tilde(socket).exists() {
            (
                true,
                t("Agent found. It signs for TernaryShell; the keys never leave it.").to_string(),
            )
        } else {
            (false, t("No agent socket at this path.").to_string())
        };
        if ok {
            style::hint(ui, text);
        } else {
            style::hint_tone(ui, text, Tone::Err);
        }
    }

    fn network_section(&mut self, ui: &mut Ui, store: &BookmarkStore) {
        let p = style::pal(ui);
        style::row(ui, t("Jump host"), |ui| {
            // Other saved connections that don't already go through this one.
            let own = self.draft.id;
            let mut options: Vec<(String, String, Uuid)> = store
                .bookmarks
                .iter()
                .filter(|b| b.id != own)
                .filter(|b| {
                    store
                        .jump_chain(b)
                        .is_ok_and(|chain| chain.iter().all(|hop| hop.id != own))
                })
                .map(|b| (b.display_name(), b.address(), b.id))
                .collect();
            options.sort();
            let current = self
                .draft
                .jump
                .and_then(|id| options.iter().find(|(_, _, o)| *o == id))
                .map_or_else(
                    || t("None (connect directly)").to_string(),
                    |(n, a, _)| format!("{}  {n}   {a}", icons::HARD_DRIVES),
                );
            egui::ComboBox::from_id_salt("jump-host")
                .icon(kit::combo_caret)
                .selected_text(current)
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(self.draft.jump.is_none(), t("None (connect directly)"))
                        .clicked()
                    {
                        self.draft.jump = None;
                    }
                    for (name, address, id) in options {
                        if ui
                            .selectable_label(
                                self.draft.jump == Some(id),
                                format!("{name}   {address}"),
                            )
                            .clicked()
                        {
                            self.draft.jump = Some(id);
                        }
                    }
                });
            // The route: This Mac → bastion → … → this server.
            let mut draft = self.draft.clone();
            draft.id = Uuid::nil();
            let hops: Vec<String> = store
                .jump_chain(&draft)
                .map(|c| c.iter().map(|b| b.display_name()).collect())
                .unwrap_or_default();
            if !hops.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    kit::chip(
                        ui,
                        Some(icons::LAPTOP),
                        crate::platform::this_computer(),
                        false,
                    );
                    for h in &hops {
                        kit::icon(ui, icons::ARROW_RIGHT, 13.0, p.text_faint);
                        kit::chip(ui, Some(icons::HARD_DRIVES), h, false);
                    }
                    kit::icon(ui, icons::ARROW_RIGHT, 13.0, p.text_faint);
                    let me = if self.draft.name.trim().is_empty() {
                        self.draft.host.trim().to_string()
                    } else {
                        self.draft.name.trim().to_string()
                    };
                    kit::chip_tone(ui, Some(icons::HARD_DRIVES), &me, false, Some(Tone::Accent));
                });
            }
            style::hint(
                ui,
                t("Like ssh -J. Each hop checks its own host key and logs in its own way."),
            );
        });
        Self::separator(ui);
        let bad_row = self
            .problem
            .as_ref()
            .filter(|p| p.section == Section::Network)
            .and_then(|p| p.forward.map(|i| (i, p.message.clone())));
        let me = if self.draft.name.trim().is_empty() {
            self.draft.host.trim().to_string()
        } else {
            self.draft.name.trim().to_string()
        };
        style::row(ui, t("Port forwarding"), |ui| {
            let mut remove = None;
            for (i, row) in self.forwards.iter_mut().enumerate() {
                let bad = bad_row.as_ref().is_some_and(|(r, _)| *r == i);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let (letter, name) = match row.kind {
                        ForwardKind::Local => ("L", t("Local")),
                        ForwardKind::Remote => ("R", t("Remote")),
                        ForwardKind::Dynamic => ("D", "SOCKS"),
                    };
                    egui::ComboBox::from_id_salt(("forward-kind", i))
                        .icon(kit::combo_caret)
                        .width(92.0)
                        .selected_text({
                            let mut job = egui::text::LayoutJob::default();
                            job.append(
                                letter,
                                0.0,
                                egui::TextFormat {
                                    font_id: style::semibold(style::BODY),
                                    color: p.text,
                                    ..Default::default()
                                },
                            );
                            job.append(
                                &format!("  {name}"),
                                0.0,
                                egui::TextFormat {
                                    font_id: style::regular(style::BODY),
                                    color: p.text,
                                    ..Default::default()
                                },
                            );
                            job
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut row.kind, ForwardKind::Local, t("Local (-L)"));
                            ui.selectable_value(
                                &mut row.kind,
                                ForwardKind::Remote,
                                t("Remote (-R)"),
                            );
                            ui.selectable_value(
                                &mut row.kind,
                                ForwardKind::Dynamic,
                                t("SOCKS proxy (-D)"),
                            );
                        });
                    let _listen = ui.add(
                        style::input(&mut row.listen)
                            .hint_text("8080")
                            .desired_width(76.0)
                            .mono()
                            .error(bad),
                    );
                    #[cfg(debug_assertions)]
                    crate::devtest::record(format!("forward-listen:{i}"), _listen.rect);
                    if row.kind == ForwardKind::Dynamic {
                        let hint_w = (ui.available_width() - 22.0 - 8.0).max(40.0);
                        let hint = tf(
                            "SOCKS proxy on {}; traffic leaves from {}.",
                            &[crate::platform::this_computer(), &me],
                        );
                        ui.allocate_ui_with_layout(
                            vec2(hint_w, style::CONTROL_H),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.set_width(hint_w);
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&hint)
                                            .font(style::regular(style::SMALL))
                                            .color(p.text_faint),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&hint);
                            },
                        );
                        if kit::icon_button_ex(ui, icons::TRASH, t("Remove"), false, 22.0).clicked()
                        {
                            remove = Some(i);
                        }
                    } else {
                        kit::icon(ui, icons::ARROW_RIGHT, 14.0, p.text_faint);
                        let host_w = (ui.available_width() - 64.0 - 22.0 - 16.0).max(80.0);
                        let _host = ui.add(
                            style::input(&mut row.host)
                                .hint_text("localhost")
                                .desired_width(host_w)
                                .mono(),
                        );
                        let _port = ui.add(
                            style::input(&mut row.port)
                                .hint_text("80")
                                .desired_width(64.0)
                                .mono(),
                        );
                        #[cfg(debug_assertions)]
                        {
                            crate::devtest::record(format!("forward-host:{i}"), _host.rect);
                            crate::devtest::record(format!("forward-port:{i}"), _port.rect);
                        }
                        if kit::icon_button_ex(ui, icons::TRASH, t("Remove"), false, 22.0).clicked()
                        {
                            remove = Some(i);
                        }
                    }
                });
                if let Some((r, m)) = &bad_row
                    && *r == i
                {
                    style::hint_tone(ui, m.as_str(), Tone::Err);
                }
                ui.add_space(2.0);
            }
            if let Some(i) = remove {
                self.forwards.remove(i);
            }
            let add = kit::ghost(ui, Some(icons::PLUS), t("Add forward"));
            #[cfg(debug_assertions)]
            crate::devtest::record("editor-add-forward", add.rect);
            if add.clicked() {
                self.forwards.push(ForwardRow {
                    kind: ForwardKind::Local,
                    listen: String::new(),
                    host: "localhost".into(),
                    port: String::new(),
                });
            }
            style::hint(
                ui,
                tf(
                    "Started when the connection opens. L = {} → server side · R = server → {} · D = SOCKS proxy.",
                    &[
                        crate::platform::this_computer(),
                        crate::platform::this_computer(),
                    ],
                ),
            );
        });
    }

    fn session_section(&mut self, ui: &mut Ui) {
        style::row(ui, t("Remote folder"), |ui| {
            ui.add(
                style::input(&mut self.draft.start_dir)
                    .hint_text("/var/www")
                    .mono(),
            );
            style::hint(ui, t("The terminal and the remote side of SFTP open here."));
        });
        ui.add_space(12.0);
        style::row(ui, t("Local folder"), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let browse_w = style::text_width(ui, t("Browse…")) + 18.0;
                ui.add(
                    style::input(&mut self.draft.local_start_dir)
                        .hint_text(t("Last folder used"))
                        .desired_width(ui.available_width() - browse_w - 8.0)
                        .mono(),
                );
                if kit::secondary_sm(ui, None, t("Browse…")).clicked() {
                    let start = paths::expand_tilde(self.draft.local_start_dir.trim());
                    let dialog = rfd::FileDialog::new().set_title(t("Local start folder"));
                    let dialog = if start.is_dir() {
                        dialog.set_directory(start)
                    } else {
                        dialog.set_directory(paths::home_dir())
                    };
                    if let Some(dir) = dialog.pick_folder() {
                        self.draft.local_start_dir = paths::contract_tilde(&dir);
                    }
                }
            });
            style::hint(
                ui,
                t("The local side of SFTP opens here. Empty = last folder used."),
            );
        });
        ui.add_space(12.0);
        style::row(ui, t("Run on connect"), |ui| {
            ui.add(
                style::input_multiline(&mut self.draft.startup_command)
                    .hint_text(t("One command per line"))
                    .desired_rows(3)
                    .mono(),
            );
            style::hint(
                ui,
                t("One command per line, sent to the shell after login."),
            );
        });
        Self::separator(ui);
        ui.add_space(-12.0);
        kit::setting_row(
            ui,
            t("Allow legacy algorithms"),
            t(
                "Also offer ssh-rsa (SHA-1), SHA-1 key exchange and CBC ciphers. Only for old servers that need it.",
            ),
            true,
            |ui| {
                kit::switch(ui, &mut self.draft.legacy_algorithms);
            },
        );
    }

    /// The bookmark and the secrets to store or forget.
    fn saved(&self, connect: bool) -> EditorResult {
        let mut set = SecretSet::default();
        let mut clear = Vec::new();
        if matches!(self.draft.auth, Auth::Password) {
            if !self.password.is_empty() {
                set.password = Some(self.password.clone());
            } else if self.clear_password {
                clear.push(SecretKind::Password);
            }
        } else if self.draft.auth.uses_key() {
            if self.new_pasted_key() {
                set.private_key = Some(keys::normalize_key_text(&self.key_text));
            }
            // A saved passphrase belongs to the old key: a different key without a typed
            // passphrase asks for it when connecting.
            let key_changed = set.private_key.is_some() || self.draft.auth != self.original_auth;
            if !self.passphrase.is_empty() {
                set.passphrase = Some(self.passphrase.clone());
            } else if self.clear_passphrase || key_changed {
                clear.push(SecretKind::Passphrase);
            }
        }
        EditorResult::Save {
            bookmark: self.draft.clone(),
            set,
            clear,
            connect,
        }
    }
}

/// Under a saved secret: "Saved in Keychain" and a quiet Forget toggle. Returns true when it
/// was toggled.
fn saved_row(ui: &mut Ui, forget: &mut bool, label: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if *forget {
            kit::badge(ui, t("Will be forgotten"), Tone::Warn, Some(icons::TRASH));
            if kit::ghost(ui, None, t("Keep")).clicked() {
                *forget = false;
                changed = true;
            }
        } else {
            kit::badge(
                ui,
                &tf("Saved in {}", &[crate::platform::keychain()]),
                Tone::Neutral,
                Some(icons::LOCK_SIMPLE),
            );
            if kit::ghost(ui, None, label).clicked() {
                *forget = true;
                changed = true;
            }
        }
    });
    changed
}

/// A password field with an eye button inside it. Showing a value that is saved (`saved`) and
/// not typed over loads it from the Keychain into the field, to be read or copied; saving it
/// unchanged stores the same value.
fn secret_input(
    ui: &mut Ui,
    text: &mut String,
    shown: &mut bool,
    hint: &str,
    saved: Option<(Uuid, SecretKind)>,
    problem: &mut Option<Problem>,
    _record: &str,
) -> egui::Response {
    let p = style::pal(ui);
    let field = ui.add(
        style::input(text)
            .password(!*shown)
            .hint_text(hint)
            .margin(egui::Margin {
                left: 10,
                right: 30,
                top: 7,
                bottom: 7,
            }),
    );
    let eye = egui::Rect::from_center_size(
        pos2(field.rect.right() - 15.0, field.rect.center().y),
        vec2(18.0, 18.0),
    );
    let resp = ui
        .interact(eye, egui::Id::new(_record), Sense::click())
        .on_hover_text(if *shown { t("Hide") } else { t("Show") });
    kit::paint_icon_button(
        ui,
        &resp,
        eye,
        if *shown { icons::EYE_SLASH } else { icons::EYE },
        "",
        false,
        false,
    );
    let _ = p;
    #[cfg(debug_assertions)]
    crate::devtest::record(_record, eye);
    if resp.clicked() {
        *shown = !*shown;
        if *shown
            && text.is_empty()
            && let Some((id, kind)) = saved
        {
            match secrets::get(id, kind) {
                Ok(Some(value)) => *text = value,
                Ok(None) => {}
                Err(e) => {
                    *problem = Some(Problem {
                        message: format!("{e:#}"),
                        section: Section::Login,
                        forward: None,
                    })
                }
            }
        }
    }
    field
}
