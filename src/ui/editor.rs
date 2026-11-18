//! The connection editor (create and edit): a sheet with four sections (Server, Login, Network,
//! Session) in a side navigation that names the host being edited and sums up each section.
//! A "Same as" line spells the connection out as the equivalent `ssh` command. Secrets typed
//! here go to the Keychain on save.

use std::hash::{Hash, Hasher};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use uuid::Uuid;

use super::forms::{self, NavExtra};
use super::style::{self, Palette, Tone};
use super::{icons, kit};
use crate::bookmarks::{
    Auth, Bookmark, BookmarkStore, Forward, ForwardKind, HostIcon, Label, LabelColor,
};
use crate::i18n::{t, tf};
use crate::paths;
use crate::secrets::{self, SecretKind, SecretSet};
use crate::ssh::keys::{self, KeyCheck};
use crate::ssh::session::Connection;
use crate::ssh::types::ConnState;

/// How the connection logs in, as chosen in the editor.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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

/// A port forward being edited (as typed): the port listened on and, for -L / -R, where the
/// connections go (`host:port`).
#[derive(Clone)]
struct ForwardRow {
    kind: ForwardKind,
    listen: String,
    dest: String,
}

impl ForwardRow {
    fn from(f: &Forward) -> Self {
        let host = if f.host.contains(':') && !f.host.starts_with('[') {
            format!("[{}]", f.host)
        } else {
            f.host.clone()
        };
        Self {
            kind: f.kind,
            listen: f.listen_port.to_string(),
            dest: if f.port == 0 {
                host
            } else {
                format!("{host}:{}", f.port)
            },
        }
    }

    /// The destination split into host and port (`[::1]:80` for IPv6).
    fn split_dest(&self) -> (String, Option<u16>) {
        let d = self.dest.trim();
        let number = |p: &str| p.trim().parse::<u16>().ok().filter(|p| *p > 0);
        if let Some(inner) = d.strip_prefix('[') {
            return match inner.split_once(']') {
                Some((h, rest)) => (h.to_string(), rest.strip_prefix(':').and_then(number)),
                None => (inner.to_string(), None),
            };
        }
        match d.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_string(), number(p)),
            _ => (d.to_string(), None),
        }
    }

    fn to_forward(&self) -> Result<Forward, String> {
        let number = |text: &str| text.trim().parse::<u16>().ok().filter(|p| *p > 0);
        let listen = number(&self.listen)
            .ok_or_else(|| t("Port must be a number between 1 and 65535.").to_string())?;
        if self.kind == ForwardKind::Dynamic {
            return Ok(Forward {
                kind: self.kind,
                listen_port: listen,
                host: String::new(),
                port: 0,
            });
        }
        match self.split_dest() {
            (host, Some(port)) if !host.trim().is_empty() => Ok(Forward {
                kind: self.kind,
                listen_port: listen,
                host: host.trim().to_string(),
                port,
            }),
            _ => Err(tf(
                "Port forwarding: say where port {} goes (host and port).",
                &[&listen.to_string()],
            )),
        }
    }

    /// Nothing typed yet (a row just added).
    fn is_blank(&self) -> bool {
        self.listen.trim().is_empty()
            && (self.kind == ForwardKind::Dynamic
                || self.dest.trim().is_empty()
                || self.dest.trim() == "localhost")
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

/// What the pasted key is: its type, whether it is encrypted, its SHA256 fingerprint and its
/// comment (all from the public part, so encrypted OpenSSH keys tell too).
#[derive(Clone, Default)]
struct KeyInfo {
    kind: String,
    encrypted: bool,
    fingerprint: String,
    comment: String,
}

fn key_info(text: &str) -> Option<KeyInfo> {
    use russh::keys::ssh_key;
    let text = keys::normalize_key_text(text);
    let key = ssh_key::PrivateKey::from_openssh(&text).ok()?;
    let kind = match key.algorithm() {
        ssh_key::Algorithm::Ed25519 => "Ed25519",
        ssh_key::Algorithm::Ecdsa { .. } => "ECDSA",
        ssh_key::Algorithm::Rsa { .. } => "RSA",
        _ => "SSH",
    };
    Some(KeyInfo {
        kind: kind.to_string(),
        encrypted: key.is_encrypted(),
        fingerprint: key
            .public_key()
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string(),
        comment: key.comment().to_string(),
    })
}

/// A part of the "Same as" command: plain, an option (accent) or the target (green).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Plain,
    Flag,
    Target,
}

/// The connection as the equivalent `ssh` command line.
fn ssh_command(
    b: &Bookmark,
    store: &BookmarkStore,
    auth: &Auth,
    forwards: &[Forward],
) -> Vec<(String, Part)> {
    let mut out = vec![("ssh".to_string(), Part::Plain)];
    let mut flag = |s: String| out.push((s, Part::Flag));
    if b.port != crate::bookmarks::DEFAULT_PORT {
        flag(format!("-p {}", b.port));
    }
    match auth {
        Auth::Key { path } if !path.trim().is_empty() => flag(format!("-i {}", path.trim())),
        Auth::KeychainKey => flag(format!("-i {}", t("‹pasted key›"))),
        Auth::Agent { socket } if !socket.trim().is_empty() => {
            flag(format!("-o IdentityAgent={}", socket.trim()))
        }
        _ => {}
    }
    if let Ok(chain) = store.jump_chain(b)
        && !chain.is_empty()
    {
        let hops: Vec<String> = chain.iter().map(|h| h.address()).collect();
        flag(format!("-J {}", hops.join(",")));
    }
    for f in forwards {
        flag(match f.kind {
            ForwardKind::Local => format!("-L {}:{}:{}", f.listen_port, f.host, f.port),
            ForwardKind::Remote => format!("-R {}:{}:{}", f.listen_port, f.host, f.port),
            ForwardKind::Dynamic => format!("-D {}", f.listen_port),
        });
    }
    if b.legacy_algorithms {
        flag("-o HostKeyAlgorithms=+ssh-rsa".into());
    }
    let host = b.host.trim();
    let target = if b.username.trim().is_empty() {
        host.to_string()
    } else {
        format!("{}@{host}", b.username.trim())
    };
    out.push((
        if target.is_empty() {
            "host".to_string()
        } else {
            target
        },
        Part::Target,
    ));
    out
}

/// The "Same as" command: the hash of what it was made from, its parts, the plain line.
type SameAs = (u64, Vec<(String, Part)>, String);

/// The state of "Test Connection".
enum TestState {
    Running(Connection),
    Ok,
    Failed(String),
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
    key_check: Option<(String, KeyCheck, Option<KeyInfo>)>,
    /// The login method when the editor opened (a saved passphrase belongs to that key).
    original_auth: Auth,
    agent_socket: String,
    agents: Vec<(&'static str, String)>,
    /// "Other socket…" is picked in the agent list.
    other_agent: bool,
    forwards: Vec<ForwardRow>,
    /// An address pasted into Host and split into Host, Port and Username: what was pasted
    /// (for Undo) and the fields before.
    split: Option<(String, String, String, String)>,
    /// The "Same as" command and the hash of what it was made from.
    command: Option<SameAs>,
    /// Tags used by the other connections of the group (suggested for a new one).
    tag_suggestions: Option<(Option<Uuid>, Vec<String>)>,
    test: Option<TestState>,
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
    /// Connect and log in with this bookmark (as typed), then close: "Test Connection". The
    /// app starts the connection and hands it back with [`Editor::start_test`].
    Test(Bookmark),
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

/// Width of the side navigation and of the form's label column.
const NAV_W: f32 = 236.0;
const LABEL_COL: f32 = 128.0;

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
        let agents = known_agents();
        let other_agent =
            !agent_socket.is_empty() && !agents.iter().any(|(_, p)| *p == agent_socket);
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
            agents,
            other_agent,
            split: None,
            command: None,
            tag_suggestions: None,
            test: None,
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

    /// The connection started for "Test Connection".
    pub fn start_test(&mut self, connection: Connection) {
        self.test = Some(TestState::Running(connection));
    }

    /// Checks the test connection: logged in (then it is closed), failed, or waiting for an
    /// answer the editor can't give (a host key or password prompt).
    fn poll_test(&mut self) {
        let Some(TestState::Running(conn)) = &self.test else {
            return;
        };
        let asks = conn.shared.lock().prompt.is_some();
        let next = match conn.state() {
            ConnState::Connected => Some(TestState::Ok),
            ConnState::Closed(Some(e)) => Some(TestState::Failed(e.message())),
            ConnState::Closed(None) => Some(TestState::Failed(t("Connection closed.").into())),
            _ if asks => Some(TestState::Failed(
                t("The server needs an answer (a host key or a password): use Save & Connect.")
                    .into(),
            )),
            _ => None,
        };
        if let Some(next) = next {
            conn.disconnect();
            self.test = Some(next);
        }
    }

    /// What the pasted key text is (re-checked only when it changes).
    fn pasted_key_check(&mut self) -> Option<KeyCheck> {
        if self.key_text.trim().is_empty() {
            return None;
        }
        if self
            .key_check
            .as_ref()
            .is_none_or(|(text, _, _)| *text != self.key_text)
        {
            self.key_check = Some((
                self.key_text.clone(),
                keys::check_key_text(&self.key_text),
                key_info(&self.key_text),
            ));
        }
        self.key_check.as_ref().map(|(_, c, _)| c.clone())
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

    /// Port forwards with a mistake, checked as they are typed (rows not filled in yet are
    /// left alone until saving): the row and the message.
    fn forward_problems(&self) -> Vec<(usize, String)> {
        let mut out = Vec::new();
        let mut here = std::collections::HashSet::new();
        let mut there = std::collections::HashSet::new();
        for (i, row) in self.forwards.iter().enumerate() {
            if row.is_blank() {
                continue;
            }
            match row.to_forward() {
                Err(m) => {
                    // A destination still being typed isn't a mistake yet.
                    if row
                        .listen
                        .trim()
                        .parse::<u16>()
                        .ok()
                        .filter(|p| *p > 0)
                        .is_none()
                        || !row.dest.trim().is_empty()
                    {
                        out.push((i, m));
                    }
                }
                Ok(f) => {
                    let fresh = match f.kind {
                        ForwardKind::Remote => there.insert(f.listen_port),
                        _ => here.insert(f.listen_port),
                    };
                    if !fresh {
                        out.push((
                            i,
                            tf(
                                "Port forwarding: port {} is used twice.",
                                &[&f.listen_port.to_string()],
                            ),
                        ));
                    }
                }
            }
        }
        out
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
        if let Some(l) = &mut self.draft.label {
            l.text = l.tag();
        }
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

    /// The login method as the bookmark will store it (without checking it).
    fn auth_now(&self) -> Auth {
        match self.auth {
            AuthKind::Password => Auth::Password,
            AuthKind::KeyFile => Auth::Key {
                path: self.key_path.trim().to_string(),
            },
            AuthKind::PastedKey => Auth::KeychainKey,
            AuthKind::Agent => Auth::Agent {
                socket: self.agent_socket.trim().to_string(),
            },
        }
    }

    /// The draft as it stands (port and forwards as far as they parse), for the "Same as"
    /// line, the side navigation and Test Connection.
    fn current(&self) -> (Bookmark, Vec<Forward>) {
        let mut b = self.draft.clone();
        if let Ok(p) = self.port_text.trim().parse::<u16>()
            && p > 0
        {
            b.port = p;
        }
        b.auth = self.auth_now();
        b.tags = self.tags.clone();
        let fwds: Vec<Forward> = self
            .forwards
            .iter()
            .filter_map(|r| r.to_forward().ok())
            .collect();
        b.forwards = fwds.clone();
        (b, fwds)
    }

    /// The "Same as" command, rebuilt only when what it shows has changed.
    fn same_as(&mut self, store: &BookmarkStore) -> (Vec<(String, Part)>, String) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.draft.username.hash(&mut h);
        self.draft.host.hash(&mut h);
        self.port_text.hash(&mut h);
        self.auth.hash(&mut h);
        self.key_path.hash(&mut h);
        self.agent_socket.hash(&mut h);
        self.draft.jump.hash(&mut h);
        self.draft.legacy_algorithms.hash(&mut h);
        store.bookmarks.len().hash(&mut h);
        for r in &self.forwards {
            (r.kind as u8, &r.listen, &r.dest).hash(&mut h);
        }
        let key = h.finish();
        if self.command.as_ref().is_none_or(|(k, _, _)| *k != key) {
            let (b, fwds) = self.current();
            let parts = ssh_command(&b, store, &b.auth, &fwds);
            let plain = parts
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            self.command = Some((key, parts, plain));
        }
        let (_, parts, plain) = self.command.as_ref().unwrap();
        (parts.clone(), plain.clone())
    }

    pub fn show(&mut self, ctx: &egui::Context, store: &BookmarkStore) -> EditorResult {
        self.poll_test();
        let mut result = EditorResult::Open;
        let screen = ctx.content_rect();
        let size = vec2(
            980.0f32.min(screen.width() - 40.0),
            664.0f32.min(screen.height() - 40.0),
        );
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
        let forward_problems = self.forward_problems();
        // A forward mistake that is fixed no longer holds saving back.
        if self
            .problem
            .as_ref()
            .is_some_and(|p| p.forward.is_some() && forward_problems.is_empty())
        {
            self.problem = None;
        }
        let live_problem = !forward_problems.is_empty();
        let mut save = false;
        let mut save_connect = enter && !live_problem;
        let mut test = false;
        let (cmd_parts, cmd_plain) = self.same_as(store);
        let modal = style::modal(ctx, egui::Id::new("bookmark-editor")).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let p = style::pal(ui);
            let full = Rect::from_min_size(ui.max_rect().min, size);
            let nav = Rect::from_min_size(full.min, vec2(NAV_W, size.y));
            forms::paint_nav(ui.painter(), nav, &p);
            self.nav(ui, nav, store, &forward_problems);

            let main = Rect::from_min_max(pos2(nav.right(), full.top()), full.max);
            let (title, sub) = match self.section {
                Section::Server if self.is_new => (
                    t("New connection"),
                    t("Where to connect, and where it lives in the sidebar."),
                ),
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
            if forms::sheet_head(
                ui,
                main.left() + 24.0,
                main.top() + 20.0,
                main.right() - 24.0,
                title,
                sub,
            ) {
                result = EditorResult::Cancel;
            }
            // Footer, "Same as", then the body in what is left.
            let foot = Rect::from_min_max(pos2(main.left(), main.bottom() - 55.0), main.max);
            forms::paint_footer(ui.painter(), foot, false, &p);
            let cmd = Rect::from_min_max(
                pos2(main.left() + 24.0, foot.top() - 14.0 - 36.0),
                pos2(main.right() - 24.0, foot.top() - 14.0),
            );
            self.same_as_line(ui, cmd, &cmd_parts, &cmd_plain);
            let body = Rect::from_min_max(
                pos2(main.left() + 24.0, main.top() + 84.0),
                pos2(main.right() - 24.0, cmd.top() - 12.0),
            );
            let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
            egui::ScrollArea::vertical()
                .id_salt(("editor-form", self.section as u8))
                .auto_shrink([false, false])
                .show(&mut body_ui, |ui| {
                    ui.set_width(body.width());
                    style::form_spacing(ui);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    match self.section {
                        Section::Server => self.server_section(ui, store),
                        Section::Login => self.login_section(ui),
                        Section::Network => self.network_section(ui, store, &forward_problems),
                        Section::Session => self.session_section(ui),
                    }
                });
            // Footer content.
            let mut f = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(foot.shrink2(vec2(16.0, 12.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            f.spacing_mut().item_spacing.x = 8.0;
            let problem_text = if live_problem {
                let n = forward_problems.len();
                Some(if n == 1 {
                    t("1 problem in Network").to_string()
                } else {
                    tf("{} problems in Network", &[&n.to_string()])
                })
            } else {
                self.problem.as_ref().map(|prob| {
                    if prob.section == Section::Network && prob.forward.is_some() {
                        t("1 problem in Network").to_string()
                    } else {
                        prob.message.clone()
                    }
                })
            };
            match problem_text {
                Some(text) => {
                    f.spacing_mut().item_spacing.x = 7.0;
                    kit::icon(&mut f, icons::WARNING_CIRCLE, 15.0, p.err);
                    let w = (f.available_width() - 330.0).max(60.0);
                    f.allocate_ui_with_layout(
                        vec2(w, 30.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&text).font(style::medium(12.5)).color(p.err),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&text);
                        },
                    );
                    f.spacing_mut().item_spacing.x = 8.0;
                }
                None => {
                    test = self.test_status(&mut f);
                }
            }
            f.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!live_problem, |ui| {
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
                });
                if kit::secondary(ui, None, t("Cancel")).clicked() {
                    result = EditorResult::Cancel;
                }
            });
        });
        if (save || save_connect) && matches!(result, EditorResult::Open) && self.validate() {
            result = self.saved(save_connect);
        }
        if test && matches!(result, EditorResult::Open) {
            let mut probe = self.current().0;
            if let Ok(target) = crate::bookmarks::model::parse_target(&probe.host) {
                if let Some(u) = target.user {
                    probe.username = u;
                }
                probe.host = target.host;
                if let Some(port) = target.port {
                    probe.port = port;
                }
                result = EditorResult::Test(probe);
            } else {
                self.test = Some(TestState::Failed(t("Host is required.").into()));
            }
        }
        if super::dialogs::escape_only(ctx, &modal) && matches!(result, EditorResult::Open) {
            result = EditorResult::Cancel;
        }
        if matches!(result, EditorResult::Cancel | EditorResult::Save { .. })
            && let Some(TestState::Running(c)) = &self.test
        {
            c.disconnect();
        }
        result
    }

    /// The footer's left side when nothing is wrong: Test Connection, or how the test went.
    /// Returns whether Test Connection was clicked.
    fn test_status(&mut self, ui: &mut Ui) -> bool {
        let p = style::pal(ui);
        let mut clicked = false;
        match &self.test {
            Some(TestState::Running(_)) => {
                kit::spinner(ui, 14.0);
                ui.label(
                    RichText::new(t("Testing…"))
                        .font(style::regular(12.5))
                        .color(p.text_muted),
                );
            }
            Some(TestState::Ok) => {
                kit::icon(ui, icons::CHECK_CIRCLE, 15.0, p.ok);
                ui.label(
                    RichText::new(t("Logged in. The test connection is closed."))
                        .font(style::medium(12.5))
                        .color(p.ok),
                );
                if kit::button_ex(ui, kit::Kind::Ghost, None, t("Test Again"), true).clicked() {
                    clicked = true;
                }
            }
            Some(TestState::Failed(m)) => {
                kit::icon(ui, icons::WARNING_CIRCLE, 15.0, p.err);
                let m = m.clone();
                let w = (ui.available_width() - 330.0 - 90.0).max(60.0);
                ui.allocate_ui_with_layout(
                    vec2(w, 30.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&m).font(style::medium(12.5)).color(p.err),
                            )
                            .truncate(),
                        )
                        .on_hover_text(&m);
                    },
                );
                if kit::button_ex(ui, kit::Kind::Ghost, None, t("Test Again"), true).clicked() {
                    clicked = true;
                }
            }
            None => {
                let r = kit::button_ex(
                    ui,
                    kit::Kind::Ghost,
                    Some(icons::PLUGS_CONNECTED),
                    t("Test Connection"),
                    true,
                );
                #[cfg(debug_assertions)]
                crate::devtest::record("editor-test", r.rect);
                clicked = r.clicked();
            }
        }
        clicked
    }

    /// The "Same as" well: the connection as an `ssh` command, with a copy button.
    fn same_as_line(&self, ui: &mut Ui, rect: Rect, parts: &[(String, Part)], plain: &str) {
        let p = style::pal(ui);
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(8),
            p.well,
            Stroke::new(1.0, p.border),
            StrokeKind::Inside,
        );
        let mut job = egui::text::LayoutJob::default();
        job.append(
            &t("Same as").to_uppercase(),
            0.0,
            egui::TextFormat {
                font_id: style::semibold(style::XS),
                color: p.text_faint,
                extra_letter_spacing: 0.44,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
        let label = painter.layout_job(job);
        let lx = rect.left() + 12.0;
        let label_w = label.size().x;
        painter.galley(
            pos2(lx, rect.center().y - label.size().y / 2.0),
            label,
            p.text_faint,
        );
        let copy = Rect::from_center_size(
            pos2(rect.right() - 6.0 - 10.0, rect.center().y),
            vec2(20.0, 20.0),
        );
        let code_left = lx + label_w + 10.0;
        let code_w = copy.left() - 10.0 - code_left;
        let mut job = egui::text::LayoutJob::default();
        for (i, (text, part)) in parts.iter().enumerate() {
            if i > 0 {
                job.append(
                    " ",
                    0.0,
                    egui::TextFormat::simple(style::mono(12.0), p.text),
                );
            }
            let color = match part {
                Part::Plain => p.text,
                Part::Flag => p.accent_text,
                Part::Target => p.ok,
            };
            job.append(
                text,
                0.0,
                egui::TextFormat::simple(style::mono(12.0), color),
            );
        }
        job.wrap = egui::text::TextWrapping::truncate_at_width(code_w.max(40.0));
        let g = painter.layout_job(job);
        painter.galley(
            pos2(code_left, rect.center().y - g.size().y / 2.0),
            g,
            p.text,
        );
        let r = ui
            .interact(copy, egui::Id::new("editor-copy-command"), Sense::click())
            .on_hover_text(plain);
        kit::paint_icon_button(ui, &r, copy, icons::COPY, t("Copy"), false, false);
        if r.clicked() {
            ui.ctx().copy_text(plain.to_string());
        }
    }

    /// The label colour shown for the draft (its own, or its group's), and the label.
    fn label_now<'a>(&'a self, store: &'a BookmarkStore) -> Option<&'a Label> {
        self.draft
            .label
            .as_ref()
            .or_else(|| store.group_label(self.draft.group))
    }

    /// The side navigation: who is being edited, the four sections with what they hold, the
    /// Keychain note.
    fn nav(
        &mut self,
        ui: &mut Ui,
        nav: Rect,
        store: &BookmarkStore,
        forward_problems: &[(usize, String)],
    ) {
        let p = style::pal(ui);
        let dark = p.dark;
        let painter = ui.painter().clone();
        // Identity: glyph, name + tag, address.
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
            let port = self.port_text.trim();
            let port = if port.is_empty() || port == "22" {
                String::new()
            } else {
                format!(":{port}")
            };
            if host.is_empty() {
                "—".to_string()
            } else if self.draft.username.trim().is_empty() {
                format!("{host}{port}")
            } else {
                format!("{}@{host}{port}", self.draft.username.trim())
            }
        };
        let label = self.label_now(store).cloned();
        let color = label.as_ref().map(|l| style::label_color(l.color, dark));
        let mut probe = self.draft.clone();
        probe.tags = self.tags.clone();
        let icon = kit::host_icon(probe.host_icon());
        let glyph =
            Rect::from_min_size(pos2(nav.left() + 16.0, nav.top() + 18.0), vec2(40.0, 40.0));
        kit::paint_host_glyph(&painter, glyph, icon, color, None, p.nav, &p);
        let tx = glyph.right() + 11.0;
        let text_right = nav.right() - 16.0;
        let tag = label
            .as_ref()
            .map(|l| l.tag())
            .filter(|s| !s.is_empty() && !self.is_new);
        let tag_w = tag
            .as_ref()
            .map_or(0.0, |s| kit::env_tag_width(ui, s, false) + 6.0);
        let mut job = egui::text::LayoutJob::single_section(
            name,
            egui::TextFormat::simple(style::semibold(14.0), p.text),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - tx - tag_w).max(30.0));
        let ng = painter.layout_job(job);
        let name_y = glyph.top() + 1.5 + 9.8;
        let name_w = ng.size().x;
        painter.galley(pos2(tx, name_y - ng.size().y / 2.0), ng, p.text);
        if let (Some(tag), Some(c)) = (tag, color) {
            kit::paint_env_tag(&painter, pos2(tx + name_w + 6.0, name_y), &tag, c, false);
        }
        let mut job = egui::text::LayoutJob::single_section(
            address,
            egui::TextFormat::simple(style::mono(11.0), p.text_faint),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((text_right - tx).max(30.0));
        let ag = painter.layout_job(job);
        painter.galley(
            pos2(tx, glyph.top() + 1.5 + 19.6 + 2.0 + 7.7 - ag.size().y / 2.0),
            ag,
            p.text_faint,
        );
        let line_y = nav.top() + 16.0 + 58.0;
        painter.hline(
            (nav.left() + 10.0)..=(nav.right() - 10.0),
            line_y + 0.5,
            Stroke::new(1.0, p.border),
        );
        // Sections.
        let (cur, fwds) = self.current();
        let server_sum = {
            let host = cur.host.trim();
            if host.is_empty() {
                String::new()
            } else if cur.username.trim().is_empty() {
                host.to_string()
            } else {
                format!("{}@{host}", cur.username.trim())
            }
        };
        let login_sum = match self.auth {
            AuthKind::Password => t("Password"),
            AuthKind::KeyFile => t("Key"),
            AuthKind::PastedKey => t("Pasted key"),
            AuthKind::Agent => t("SSH agent"),
        };
        let network_sum = match cur.jump.and_then(|j| store.bookmark(j)) {
            Some(j) => tf("via {}", &[&j.display_name()]),
            None if fwds.len() == 1 => t("1 forward").to_string(),
            None if !fwds.is_empty() => tf("{} forwards", &[&fwds.len().to_string()]),
            None => String::new(),
        };
        let session_sum = if cur.start_dir.trim().is_empty() {
            if self.is_new {
                String::new()
            } else {
                t("home").to_string()
            }
        } else {
            cur.start_dir.trim().to_string()
        };
        let problem = if !forward_problems.is_empty() {
            Some(Section::Network)
        } else {
            self.problem.as_ref().map(|p| p.section)
        };
        let items = Rect::from_min_max(
            pos2(nav.left() + 10.0, line_y + 1.0 + 9.5),
            pos2(nav.right() - 10.0, nav.bottom() - 60.0),
        );
        let mut nav_ui = ui.new_child(egui::UiBuilder::new().max_rect(items));
        nav_ui.spacing_mut().item_spacing.y = 2.0;
        for (section, icon, label, sum) in [
            (
                Section::Server,
                icons::HARD_DRIVES,
                t("Server"),
                server_sum.as_str(),
            ),
            (Section::Login, icons::KEY, t("Login"), login_sum),
            (
                Section::Network,
                icons::TREE_STRUCTURE,
                t("Network"),
                network_sum.as_str(),
            ),
            (
                Section::Session,
                icons::TERMINAL_WINDOW,
                t("Session"),
                session_sum.as_str(),
            ),
        ] {
            let r = forms::nav_item(
                &mut nav_ui,
                icon,
                label,
                self.section == section,
                NavExtra {
                    summary: Some(sum),
                    problem: problem == Some(section),
                    ..Default::default()
                },
            );
            #[cfg(debug_assertions)]
            crate::devtest::record(format!("editor-section:{}", section as u8), r.rect);
            if r.clicked() {
                self.section = section;
            }
        }
        // The Keychain note at the bottom.
        let note = tf(
            "Passwords and keys are saved in {}.",
            &[crate::platform::keychain()],
        );
        let x = nav.left() + 10.0 + 8.0;
        let g = painter.layout(
            note,
            style::regular(11.5),
            p.text_faint,
            nav.right() - 10.0 - 8.0 - (x + 14.0 + 8.0),
        );
        let top = nav.bottom() - 14.0 - g.size().y;
        painter.text(
            pos2(x + 7.0, top + 9.0),
            Align2::CENTER_CENTER,
            icons::LOCK_SIMPLE,
            style::icon_font(14.0),
            p.text_faint,
        );
        painter.galley(pos2(x + 14.0 + 8.0, top), g, p.text_faint);
    }

    // ----- Server ------------------------------------------------------------------------------

    fn server_section(&mut self, ui: &mut Ui, store: &BookmarkStore) {
        let p = style::pal(ui);
        let name_hint = if self.draft.host.trim().is_empty() {
            t("e.g. Production web").to_string()
        } else {
            self.draft.host.trim().to_string()
        };
        frow(ui, t("Name"), false, |ui| {
            let r = ui.add(style::input(&mut self.draft.name).hint_text(name_hint));
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
        let split = self.split.is_some();
        let mut host_changed = false;
        let host_before = self.draft.host.len();
        frow(ui, t("Host"), false, |ui| {
            // Left to right, so Tab goes from the host to the port.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                const PORT_W: f32 = 76.0;
                let label_w = style::text_width(ui, t("Port")) * 12.0 / 13.0;
                let host_w = ui.available_width() - PORT_W - label_w - 20.0;
                let port_bad = self
                    .port_text
                    .trim()
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .is_none();
                let host = ui.add(
                    style::input(&mut self.draft.host)
                        .hint_text("server.example.com")
                        .desired_width(host_w.max(80.0))
                        .mono()
                        .error(host_problem && !port_bad),
                );
                if split {
                    split_halo(ui, host.rect, &p);
                }
                host_changed = host.changed();
                ui.label(forms::label(t("Port"), &p));
                let port = ui.add(
                    style::input(&mut self.port_text)
                        .desired_width(PORT_W)
                        .mono()
                        .error(host_problem && port_bad),
                );
                if split && !port.has_focus() {
                    split_halo(ui, port.rect, &p);
                }
                if port.changed() {
                    self.split = None;
                }
                #[cfg(debug_assertions)]
                {
                    crate::devtest::record("editor-host", host.rect);
                    crate::devtest::record("editor-port", port.rect);
                }
            });
        });
        if host_changed {
            // A paste (several characters at once) is split; typing is left alone until saving.
            if self.draft.host.len() > host_before + 1 {
                self.on_host_typed();
            } else {
                self.split = None;
            }
        }
        // The tip, or what a pasted address was split into.
        let tip_h = if self.split.is_some() {
            8.0 + 26.0
        } else {
            6.0 + 17.4
        };
        let (tip, _) = ui.allocate_exact_size(vec2(ui.available_width(), tip_h), Sense::hover());
        let mut tip_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    pos2(
                        tip.left() + LABEL_COL + 16.0,
                        tip.top() + tip_h - if self.split.is_some() { 26.0 } else { 17.4 },
                    ),
                    tip.max,
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        if let Some((pasted, ..)) = self.split.clone() {
            if pasted_pill(&mut tip_ui, &pasted) {
                self.undo_split();
            }
        } else {
            let mut job = egui::text::LayoutJob::default();
            let small = |c: Color32| egui::TextFormat::simple(style::regular(style::SMALL), c);
            job.append(t("Tip: paste"), 0.0, small(p.text_faint));
            job.append(" ", 0.0, small(p.text_faint));
            job.append(
                "user@host:port",
                0.0,
                egui::TextFormat::simple(style::mono(11.5), p.text_faint),
            );
            job.append(" ", 0.0, small(p.text_faint));
            job.append(t("here and it is split for you."), 0.0, small(p.text_faint));
            tip_ui.label(job);
        }
        ui.add_space(12.0);
        frow(ui, t("Username"), false, |ui| {
            let user = ui.add(
                style::input(&mut self.draft.username)
                    .hint_text(std::env::var("USER").unwrap_or_default())
                    .mono(),
            );
            if split {
                split_halo(ui, user.rect, &p);
            }
            if user.changed() {
                self.split = None;
            }
            #[cfg(debug_assertions)]
            crate::devtest::record("editor-user", user.rect);
        });
        separator(ui);
        frow(ui, t("Group"), false, |ui| self.group_select(ui, store));
        ui.add_space(12.0);
        frow_h(
            ui,
            t("Label"),
            false,
            if self.draft.label.is_some() {
                30.0
            } else {
                24.0
            },
            |ui| self.label_picker(ui, store),
        );
        ui.add_space(12.0);
        frow_h(ui, t("Icon"), false, 28.0, |ui| self.icon_picker(ui, store));
        ui.add_space(12.0);
        frow(ui, t("Tags"), false, |ui| {
            self.tags_input(ui);
            if self.is_new {
                self.tag_suggestions(ui, store);
            }
        });
        ui.add_space(12.0);
        frow(ui, t("Notes"), true, |ui| {
            ui.add(style::input_multiline(&mut self.draft.notes).desired_rows(2));
        });
    }

    /// The host field changed: a pasted `user@host:port` is split into the three fields (with
    /// Undo); anything else typed there ends a split.
    fn on_host_typed(&mut self) {
        let typed = self.draft.host.trim().to_string();
        let looks_full = typed.contains('@')
            || (typed.matches(':').count() == 1 && !typed.starts_with('['))
            || (typed.starts_with('[') && typed.contains("]:"));
        if looks_full
            && let Ok(target) = crate::bookmarks::model::parse_target(&typed)
            && (target.user.is_some() || target.port.is_some())
        {
            let before = (
                typed.clone(),
                self.draft.username.clone(),
                self.port_text.clone(),
                String::new(),
            );
            if let Some(u) = target.user {
                self.draft.username = u;
            }
            if let Some(port) = target.port {
                self.port_text = port.to_string();
            }
            self.draft.host = target.host;
            self.split = Some(before);
        } else {
            self.split = None;
        }
    }

    fn undo_split(&mut self) {
        if let Some((pasted, user, port, _)) = self.split.take() {
            self.draft.host = pasted;
            self.draft.username = user;
            self.port_text = port;
        }
    }

    /// The group as a select: label dot and path; a menu of all groups.
    fn group_select(&mut self, ui: &mut Ui, store: &BookmarkStore) {
        let p = style::pal(ui);
        let dark = p.dark;
        let (rect, resp) = ui.allocate_exact_size(vec2(260.0, 30.0), Sense::click());
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        paint_select(ui, rect, &resp, open);
        let painter = ui.painter();
        let mut x = rect.left() + 10.0;
        if let Some(l) = store.group_label(self.draft.group) {
            painter.circle_filled(
                pos2(x + 4.0, rect.center().y),
                4.0,
                style::label_color(l.color, dark),
            );
            x += 8.0 + 8.0;
        } else if self.draft.group.is_some() {
            painter.text(
                pos2(x + 7.0, rect.center().y),
                Align2::CENTER_CENTER,
                icons::FOLDER_SIMPLE,
                style::icon_font(14.0),
                p.text_faint,
            );
            x += 14.0 + 8.0;
        }
        let text = if self.draft.group.is_some() {
            store.group_path(self.draft.group)
        } else {
            t("(none)").to_string()
        };
        painter.with_clip_rect(rect.shrink(1.0)).text(
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            style::regular(style::BODY),
            if self.draft.group.is_some() {
                p.text
            } else {
                p.text_faint
            },
        );
        egui::Popup::menu(&resp).width(260.0).show(|ui| {
            ui.set_min_width(250.0);
            if menu_row(ui, None, t("(none)"), self.draft.group.is_none()) {
                self.draft.group = None;
            }
            let mut groups: Vec<(String, Uuid)> = store
                .groups
                .iter()
                .map(|g| (store.group_path(Some(g.id)), g.id))
                .collect();
            groups.sort();
            for (path, id) in groups {
                let c = store
                    .group_label(Some(id))
                    .map(|l| style::label_color(l.color, dark));
                if menu_row(ui, Some(c), &path, self.draft.group == Some(id)) {
                    self.draft.group = Some(id);
                }
            }
        });
    }

    /// The label: "From group", or one of nine colours (and then its short text).
    fn label_picker(&mut self, ui: &mut Ui, store: &BookmarkStore) {
        let p = style::pal(ui);
        let dark = p.dark;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            ui.spacing_mut().interact_size.y = if self.draft.label.is_some() {
                30.0
            } else {
                24.0
            };
            // "From group".
            let group = store.group_label(self.draft.group).cloned();
            let on = self.draft.label.is_none();
            let text = t("From group");
            let g =
                ui.painter()
                    .layout_no_wrap(text.to_string(), style::regular(style::SMALL), p.text);
            let w = 9.0 + if group.is_some() { 8.0 + 6.0 } else { 0.0 } + g.size().x + 9.0;
            let (r, resp) = ui.allocate_exact_size(vec2(w, 24.0), Sense::click());
            let painter = ui.painter();
            painter.rect(
                r,
                CornerRadius::same(12),
                if resp.hovered() && !on {
                    p.hover
                } else {
                    Color32::TRANSPARENT
                },
                if on {
                    Stroke::new(1.5, p.accent)
                } else {
                    Stroke::new(1.0, p.border_strong)
                },
                StrokeKind::Inside,
            );
            let mut x = r.left() + 9.0;
            if let Some(l) = &group {
                painter.circle_filled(
                    pos2(x + 4.0, r.center().y),
                    4.0,
                    style::label_color(l.color, dark),
                );
                x += 8.0 + 6.0;
            }
            painter.galley(
                pos2(x, r.center().y - g.size().y / 2.0),
                g,
                if on { p.text } else { p.text_muted },
            );
            if resp.clicked() {
                self.draft.label = None;
            }
            for color in LabelColor::ALL {
                let c = style::label_color(color, dark);
                let selected = self.draft.label.as_ref().is_some_and(|l| l.color == color);
                let (r, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                let painter = ui.painter();
                if selected {
                    painter.circle_stroke(r.center(), 10.0 + 2.0 + 0.75, Stroke::new(1.5, c));
                }
                painter.circle_filled(
                    r.center(),
                    if resp.hovered() && !selected {
                        10.5
                    } else {
                        10.0
                    },
                    c,
                );
                if selected {
                    painter.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        icons::CHECK,
                        style::icon_font(12.0),
                        Color32::WHITE,
                    );
                }
                if resp.clicked() {
                    let text = self
                        .draft
                        .label
                        .as_ref()
                        .map(|l| l.text.clone())
                        .or_else(|| group.as_ref().map(|l| l.text.clone()))
                        .unwrap_or_default();
                    self.draft.label = Some(Label { color, text });
                }
            }
            if let Some(l) = &mut self.draft.label {
                ui.add_space(5.0);
                let r = ui.add(
                    style::input(&mut l.text)
                        .hint_text("PROD")
                        .desired_width(74.0)
                        .font(style::semibold(style::SMALL)),
                );
                if r.changed() {
                    l.text = l
                        .text
                        .chars()
                        .take(Label::MAX_TEXT)
                        .collect::<String>()
                        .to_uppercase();
                }
            }
        });
    }

    /// The kind of machine, as a row of icons; nothing chosen = guessed from the tags.
    fn icon_picker(&mut self, ui: &mut Ui, store: &BookmarkStore) {
        let p = style::pal(ui);
        let mut probe = self.draft.clone();
        probe.tags = self.tags.clone();
        let current = probe.host_icon();
        let color = self
            .label_now(store)
            .map(|l| style::label_color(l.color, p.dark))
            .unwrap_or(p.accent_text);
        ui.horizontal(|ui| {
            ui.spacing_mut().interact_size.y = 28.0;
            ui.spacing_mut().item_spacing.x = 4.0;
            for icon in HostIcon::ALL {
                let on = icon == current;
                let (r, resp) = ui.allocate_exact_size(vec2(30.0, 28.0), Sense::click());
                let painter = ui.painter();
                if on {
                    painter.rect(
                        r,
                        CornerRadius::same(7),
                        style::soft(color, 0.16),
                        Stroke::new(1.5, style::soft(color, 0.55)),
                        StrokeKind::Inside,
                    );
                } else if resp.hovered() {
                    painter.rect_filled(r, CornerRadius::same(7), p.hover);
                }
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    kit::host_icon(icon),
                    style::icon_font(16.0),
                    if on { color } else { p.text_muted },
                );
                if resp.clicked() {
                    self.draft.icon = if self.draft.icon == Some(icon) {
                        None
                    } else {
                        Some(icon)
                    };
                }
            }
            if self.draft.icon.is_none() {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(t("Auto from tags"))
                        .font(style::regular(style::SMALL))
                        .color(p.text_faint),
                );
            }
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
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.set_width(width - 12.0);
                ui.set_min_height(20.0);
                ui.spacing_mut().interact_size.y = 20.0;
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
                    for (i, tag) in self.tags.iter().enumerate() {
                        let g = ui.painter().layout_no_wrap(
                            tag.clone(),
                            style::regular(style::SMALL),
                            p.text,
                        );
                        let (r, _) = ui.allocate_exact_size(
                            vec2(7.0 + g.size().x + 5.0 + 12.0 + 4.0, 20.0),
                            Sense::hover(),
                        );
                        ui.painter().rect_filled(r, CornerRadius::same(5), p.hover);
                        ui.painter().galley(
                            pos2(r.left() + 7.0, r.center().y - g.size().y / 2.0),
                            g,
                            p.text,
                        );
                        let x_rect = Rect::from_center_size(
                            pos2(r.right() - 4.0 - 6.0, r.center().y),
                            vec2(14.0, 14.0),
                        );
                        let x = ui.interact(x_rect, egui::Id::new(("tag-x", i)), Sense::click());
                        ui.painter().text(
                            x_rect.center(),
                            Align2::CENTER_CENTER,
                            icons::X,
                            style::icon_font(12.0),
                            if x.hovered() { p.text } else { p.text_faint },
                        );
                        if x.clicked() {
                            remove = Some(i);
                        }
                    }
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.tag_input)
                            .id(id)
                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 2)))
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

    /// Tags the other connections of the group use, most used first (for a new connection).
    fn tag_suggestions(&mut self, ui: &mut Ui, store: &BookmarkStore) {
        let group = self.draft.group;
        if self
            .tag_suggestions
            .as_ref()
            .is_none_or(|(g, _)| *g != group)
        {
            let mut counts: std::collections::HashMap<&str, usize> = Default::default();
            for b in store
                .bookmarks
                .iter()
                .filter(|b| b.group == group && group.is_some())
            {
                for tag in &b.tags {
                    *counts.entry(tag.as_str()).or_default() += 1;
                }
            }
            let mut list: Vec<(&str, usize)> = counts.into_iter().collect();
            list.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            self.tag_suggestions = Some((
                group,
                list.into_iter()
                    .take(5)
                    .map(|(t, _)| t.to_string())
                    .collect(),
            ));
        }
        let suggestions: Vec<String> = self
            .tag_suggestions
            .as_ref()
            .map(|(_, l)| {
                l.iter()
                    .filter(|t| !self.tags.contains(t))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if suggestions.is_empty() {
            return;
        }
        let p = style::pal(ui);
        ui.add_space(1.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(tf("Used in {}:", &[&store.group_path(group)]))
                    .font(style::regular(11.5))
                    .color(p.text_faint),
            );
            for tag in suggestions {
                let g = ui.painter().layout_no_wrap(
                    tag.clone(),
                    style::regular(style::XS),
                    p.text_muted,
                );
                let (r, resp) =
                    ui.allocate_exact_size(vec2(g.size().x + 16.0, 19.0), Sense::click());
                ui.painter().rect(
                    r,
                    CornerRadius::same(10),
                    if resp.hovered() {
                        p.hover
                    } else {
                        Color32::TRANSPARENT
                    },
                    Stroke::new(1.0, p.border),
                    StrokeKind::Inside,
                );
                ui.painter()
                    .galley(r.center() - g.size() / 2.0, g, p.text_muted);
                if resp.clicked() {
                    self.tags.push(tag);
                }
            }
        });
    }

    // ----- Login -------------------------------------------------------------------------------

    fn login_section(&mut self, ui: &mut Ui) {
        // The method, as four cards.
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = (ui.available_width() - 24.0) / 4.0;
            let cards = [
                (
                    AuthKind::Password,
                    icons::PASSWORD,
                    t("Password"),
                    tf("{}, or asked each time", &[keychain_name()]),
                ),
                (
                    AuthKind::KeyFile,
                    icons::KEY,
                    t("Key file"),
                    t("A file in ~/.ssh").to_string(),
                ),
                (
                    AuthKind::PastedKey,
                    icons::CLIPBOARD_TEXT,
                    t("Pasted key"),
                    tf("Kept in {}", &[crate::platform::keychain()]),
                ),
                (
                    AuthKind::Agent,
                    icons::FINGERPRINT,
                    t("SSH agent"),
                    t("ssh-add, 1Password, Secretive").to_string(),
                ),
            ];
            for (kind, icon, title, desc) in cards {
                let r = method_card(ui, icon, title, &desc, self.auth == kind, w);
                #[cfg(debug_assertions)]
                {
                    match kind {
                        AuthKind::KeyFile => crate::devtest::record("editor-auth-key", r.rect),
                        AuthKind::PastedKey => crate::devtest::record("editor-auth-pasted", r.rect),
                        AuthKind::Agent => crate::devtest::record("editor-auth-agent", r.rect),
                        AuthKind::Password => {
                            crate::devtest::record("editor-auth-password", r.rect)
                        }
                    }
                }
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
                let id = self.draft.id;
                frow(ui, t("Password"), false, |ui| {
                    secret_block(
                        ui,
                        SecretUi {
                            text: &mut self.password,
                            shown: &mut self.show_password,
                            had: self.had_password,
                            clear: &mut self.clear_password,
                            forget_label: t("Forget saved password"),
                            hint: t("Leave empty to ask when connecting"),
                            saved: (id, SecretKind::Password),
                            record: "editor-show-password",
                        },
                        &mut self.problem,
                    );
                });
                help_row(
                    ui,
                    &tf(
                        "Typed once, kept in {}. Leave it out to be asked when connecting.",
                        &[crate::platform::keychain()],
                    ),
                );
            }
            AuthKind::PastedKey | AuthKind::KeyFile => {
                if self.auth == AuthKind::KeyFile {
                    frow(ui, t("Key file"), false, |ui| {
                        self.key_file_ui(ui, login_problem)
                    });
                    ui.add_space(12.0);
                } else {
                    self.pasted_key_ui(ui, login_problem);
                    ui.add_space(16.0);
                }
                let id = self.draft.id;
                frow(ui, t("Key passphrase"), false, |ui| {
                    secret_block(
                        ui,
                        SecretUi {
                            text: &mut self.passphrase,
                            shown: &mut self.show_passphrase,
                            had: self.had_passphrase,
                            clear: &mut self.clear_passphrase,
                            forget_label: t("Forget saved passphrase"),
                            hint: t("Only if the key is encrypted"),
                            saved: (id, SecretKind::Passphrase),
                            record: "editor-show-passphrase",
                        },
                        &mut self.problem,
                    );
                });
                help_row(
                    ui,
                    &tf(
                        "Typed once, kept in {}. Leave it out to be asked when connecting.",
                        &[crate::platform::keychain()],
                    ),
                );
            }
            AuthKind::Agent => self.agent_ui(ui),
        }
        if let Some(prob) = &self.problem
            && prob.section == Section::Login
        {
            ui.add_space(10.0);
            style::hint_tone(ui, prob.message.as_str(), Tone::Err);
        }
    }

    fn key_file_ui(&mut self, ui: &mut Ui, problem: bool) {
        let width = ui.available_width();
        ui.allocate_ui_with_layout(
            vec2(width, 30.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if kit::secondary(ui, None, t("Browse…")).clicked()
                    && let Some(p) = rfd::FileDialog::new()
                        .set_directory(paths::home_dir().join(".ssh"))
                        .pick_file()
                {
                    self.key_path = paths::contract_tilde(&p);
                }
                if !self.key_files.is_empty() {
                    egui::ComboBox::from_id_salt("key-files")
                        .icon(kit::combo_caret)
                        .selected_text("")
                        .width(30.0)
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
                ui.add(
                    style::input(&mut self.key_path)
                        .hint_text("~/.ssh/id_ed25519")
                        .desired_width(ui.available_width())
                        .mono()
                        .error(problem),
                );
            },
        );
    }

    /// A pasted key: a card naming it (type, encrypted, fingerprint) with its first lines, or
    /// the field to paste it into.
    fn pasted_key_ui(&mut self, ui: &mut Ui, problem: bool) {
        let p = style::pal(ui);
        let stored = self.had_stored_key && !self.replace_stored_key;
        let check = if stored {
            None
        } else {
            self.pasted_key_check()
        };
        let info = self
            .key_check
            .as_ref()
            .filter(|_| !stored)
            .and_then(|(_, _, i)| i.clone());
        let pasted_ok = matches!(check, Some(KeyCheck::Valid(_) | KeyCheck::Encrypted));
        if !stored && !pasted_ok {
            // The field to paste the key into.
            let empty_bad = problem && self.key_text.trim().is_empty();
            let key_box = egui::ScrollArea::vertical()
                .id_salt("editor-key-scroll")
                .max_height(120.0)
                .show(ui, |ui| {
                    ui.add(
                        style::input_multiline(&mut self.key_text)
                            .font(style::mono(11.5))
                            .hint_text("-----BEGIN OPENSSH PRIVATE KEY-----\n…\n-----END OPENSSH PRIVATE KEY-----")
                            .desired_rows(5)
                            .error(empty_bad || matches!(check, Some(KeyCheck::PublicKey | KeyCheck::Invalid))),
                    )
                })
                .inner;
            #[cfg(debug_assertions)]
            crate::devtest::record("editor-key-text", key_box.rect);
            let _ = key_box;
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                match &check {
                    Some(KeyCheck::PublicKey) => style::hint_tone(
                        ui,
                        t("This is a public key. Paste the private key instead (the file without .pub)."),
                        Tone::Err,
                    ),
                    Some(KeyCheck::Invalid) => {
                        style::hint_tone(ui, t("This does not look like a private key."), Tone::Err)
                    }
                    _ => style::hint(ui, t("Paste the private key.")),
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.import_key_button(ui);
                    if self.had_stored_key
                        && kit::button_ex(ui, kit::Kind::Ghost, None, t("Keep the saved key"), true)
                            .clicked()
                    {
                        self.replace_stored_key = false;
                        self.key_text.clear();
                    }
                });
            });
            return;
        }
        // The key card.
        let mut replace = false;
        let mut show = false;
        let width = ui.available_width();
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
                // Header.
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(12, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            kit::icon_tile(
                                ui,
                                if stored {
                                    icons::KEY
                                } else {
                                    icons::CHECK_CIRCLE
                                },
                                if stored { Tone::Neutral } else { Tone::Ok },
                                32.0,
                            );
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 2.0);
                                ui.horizontal(|ui| {
                                    let title = match (&info, &check) {
                                        (Some(i), _) => tf("{} private key", &[&i.kind]),
                                        (None, Some(KeyCheck::Valid(k))) => {
                                            tf("{} private key", &[k])
                                        }
                                        _ if stored => tf(
                                            "A private key is saved in {}.",
                                            &[crate::platform::keychain()],
                                        ),
                                        _ => t("Encrypted private key").to_string(),
                                    };
                                    ui.label(
                                        RichText::new(title)
                                            .font(style::semibold(style::BODY))
                                            .color(p.text),
                                    );
                                    let encrypted = info.as_ref().is_some_and(|i| i.encrypted)
                                        || matches!(check, Some(KeyCheck::Encrypted));
                                    if encrypted {
                                        kit::badge(
                                            ui,
                                            t("encrypted"),
                                            Tone::Neutral,
                                            Some(icons::LOCK_SIMPLE),
                                        );
                                    }
                                });
                                let line = match &info {
                                    Some(i) if !i.comment.is_empty() => {
                                        format!("{} · {}", i.fingerprint, i.comment)
                                    }
                                    Some(i) => i.fingerprint.clone(),
                                    None if stored => {
                                        t("Its text is shown only when you ask.").to_string()
                                    }
                                    None => String::new(),
                                };
                                if !line.is_empty() {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(line)
                                                .font(style::mono(11.0))
                                                .color(p.text_faint),
                                        )
                                        .truncate(),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    replace = kit::secondary_sm(ui, None, t("Replace…")).clicked();
                                    if stored {
                                        let r = kit::button_ex(
                                            ui,
                                            kit::Kind::Ghost,
                                            None,
                                            if self.shown_key.is_some() {
                                                t("Hide")
                                            } else {
                                                t("Show")
                                            },
                                            true,
                                        );
                                        #[cfg(debug_assertions)]
                                        crate::devtest::record("editor-show-key", r.rect);
                                        show = r.clicked();
                                    }
                                },
                            );
                        });
                    });
                // The first lines of the key.
                let text = if stored {
                    self.shown_key.clone()
                } else {
                    Some(self.key_text.clone())
                };
                if let Some(text) = text {
                    let lines: Vec<&str> = text.trim().lines().map(str::trim).collect();
                    let mut shown: Vec<String> = Vec::new();
                    if lines.len() > 3 {
                        shown.push(lines[0].to_string());
                        shown.push(format!(
                            "{}…",
                            lines[1].chars().take(70).collect::<String>()
                        ));
                        shown.push(lines[lines.len() - 1].to_string());
                    } else {
                        shown.extend(lines.iter().map(|l| l.to_string()));
                    }
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 12,
                            top: 0,
                            bottom: 10,
                        })
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for l in shown {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(l)
                                            .font(style::mono(11.0))
                                            .color(p.text_faint),
                                    )
                                    .truncate(),
                                );
                            }
                        });
                }
                // Footer.
                let (r, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(
                    r,
                    CornerRadius {
                        nw: 0,
                        ne: 0,
                        sw: 9,
                        se: 9,
                    },
                    p.hover,
                );
                painter.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, p.border));
                painter.text(
                    pos2(r.left() + 12.0 + 7.0, r.center().y),
                    Align2::CENTER_CENTER,
                    icons::LOCK_SIMPLE,
                    style::icon_font(14.0),
                    p.text_muted,
                );
                painter.text(
                    pos2(r.left() + 12.0 + 14.0 + 8.0, r.center().y),
                    Align2::LEFT_CENTER,
                    tf("Saved in {}, not on disk.", &[crate::platform::keychain()]),
                    style::regular(style::SMALL),
                    p.text_muted,
                );
                let mut right = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(r.shrink2(vec2(10.0, 0.0)))
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                self.import_key_button(&mut right);
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
            self.key_text.clear();
        }
    }

    /// "Import from file…": an accent text button that reads a key file into the paste field.
    fn import_key_button(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let g = ui.painter().layout_no_wrap(
            t("Import from file…").to_string(),
            style::regular(style::SMALL),
            p.accent_text,
        );
        let (r, resp) = ui.allocate_exact_size(g.size() + vec2(4.0, 6.0), Sense::click());
        ui.painter().galley(
            r.center() - g.size() / 2.0,
            g,
            if resp.hovered() {
                p.accent
            } else {
                p.accent_text
            },
        );
        if resp.clicked()
            && let Some(path) = rfd::FileDialog::new()
                .set_directory(paths::home_dir().join(".ssh"))
                .pick_file()
        {
            match std::fs::read(&path) {
                Ok(data) if data.len() <= keys::MAX_KEY_TEXT => {
                    self.key_text = String::from_utf8_lossy(&data).into_owned();
                    self.replace_stored_key = true;
                    self.shown_key = None;
                }
                Ok(_) => self.set_error(t("This does not look like a private key.").into()),
                Err(e) => self.set_error(format!("{}: {e}", path.display())),
            }
        }
    }

    /// The SSH agent: a callout on whether it is found, and the agents as a list to pick from.
    fn agent_ui(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let socket = self.agent_socket.trim().to_string();
        let (ok, text) = agent_status(&socket);
        if ok {
            kit::callout_rich(ui, icons::CHECK_CIRCLE, Tone::Ok, |ui| {
                ui.add(egui::Label::new(callout_job(&text, &p)).wrap());
            });
        } else {
            kit::callout(ui, icons::WARNING_CIRCLE, Tone::Err, &text);
        }
        ui.add_space(16.0);
        ui.label(forms::label(t("Use"), &p));
        ui.add_space(6.0);
        let width = ui.available_width();
        let mut pick: Option<(String, bool)> = None;
        egui::Frame::NONE
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut rows: Vec<(String, String, String, &str)> = Vec::new();
                let system_path = if cfg!(windows) {
                    t("Windows OpenSSH agent").to_string()
                } else {
                    "SSH_AUTH_SOCK".to_string()
                };
                rows.push((
                    t("System agent").to_string(),
                    String::new(),
                    system_path,
                    if cfg!(target_os = "macos") {
                        icons::APPLE_LOGO
                    } else {
                        icons::KEY
                    },
                ));
                for (name, path) in &self.agents {
                    rows.push((
                        name.to_string(),
                        path.clone(),
                        path.clone(),
                        if *name == "1Password" {
                            icons::LOCK_KEY
                        } else {
                            icons::CPU
                        },
                    ));
                }
                let n = rows.len();
                for (i, (name, value, shown, icon)) in rows.into_iter().enumerate() {
                    let on = !self.other_agent && socket == value;
                    let r = agent_row(ui, i == 0, on, icon, &name, &shown, on && ok);
                    if r.clicked() {
                        pick = Some((value, false));
                    }
                    let _ = n;
                }
                // Other socket…
                let on = self.other_agent;
                let r = agent_row_ex(
                    ui,
                    false,
                    on,
                    icons::FOLDER_SIMPLE,
                    t("Other socket…"),
                    if cfg!(windows) {
                        t("A pipe, or \"pageant\"")
                    } else {
                        t("Choose an agent's socket")
                    },
                    false,
                    false,
                );
                if r.clicked() {
                    pick = Some((String::new(), true));
                }
                if self.other_agent {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 12,
                            right: 12,
                            top: 0,
                            bottom: 10,
                        })
                        .show(ui, |ui| {
                            let hint = if cfg!(windows) {
                                t("OpenSSH agent, a pipe, or \"pageant\"")
                            } else {
                                t("System agent (SSH_AUTH_SOCK)")
                            };
                            ui.horizontal(|ui| {
                                ui.add_space(26.0);
                                ui.add(style::input(&mut self.agent_socket).hint_text(hint).mono());
                            });
                        });
                }
            });
        if let Some((value, other)) = pick {
            self.other_agent = other;
            if !other {
                self.agent_socket = value;
            }
        }
    }

    // ----- Network -----------------------------------------------------------------------------

    fn network_section(
        &mut self,
        ui: &mut Ui,
        store: &BookmarkStore,
        problems: &[(usize, String)],
    ) {
        let p = style::pal(ui);
        let dark = p.dark;
        // Jump host: the select on the right of its label.
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
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            ui.label(forms::label(t("Jump host"), &p));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (rect, resp) = ui.allocate_exact_size(vec2(220.0, 28.0), Sense::click());
                let open =
                    egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
                paint_select(ui, rect, &resp, open);
                let painter = ui.painter();
                let jump = self.draft.jump.and_then(|id| store.bookmark(id));
                let mut x = rect.left() + 10.0;
                if let Some(j) = jump {
                    let c = store.label_of(j).map(|l| style::label_color(l.color, dark));
                    kit::paint_host_glyph(
                        painter,
                        Rect::from_min_size(pos2(x, rect.center().y - 10.0), vec2(20.0, 20.0)),
                        kit::host_icon(j.host_icon()),
                        c,
                        None,
                        p.field,
                        &p,
                    );
                    x += 20.0 + 8.0;
                }
                painter.with_clip_rect(rect.shrink(1.0)).text(
                    pos2(x, rect.center().y),
                    Align2::LEFT_CENTER,
                    jump.map_or_else(
                        || t("None (connect directly)").to_string(),
                        |j| j.display_name(),
                    ),
                    style::regular(style::BODY),
                    if jump.is_some() { p.text } else { p.text_faint },
                );
                egui::Popup::menu(&resp).width(300.0).show(|ui| {
                    ui.set_min_width(290.0);
                    if menu_row(
                        ui,
                        None,
                        t("None (connect directly)"),
                        self.draft.jump.is_none(),
                    ) {
                        self.draft.jump = None;
                    }
                    for (name, address, id) in &options {
                        let c = store
                            .bookmark(*id)
                            .and_then(|b| store.label_of(b))
                            .map(|l| style::label_color(l.color, dark));
                        if menu_row(
                            ui,
                            Some(c),
                            &format!("{name}   {address}"),
                            self.draft.jump == Some(*id),
                        ) {
                            self.draft.jump = Some(*id);
                        }
                    }
                });
            });
        });
        // The route: This Mac → bastion → … → this server.
        let mut draft = self.draft.clone();
        draft.id = Uuid::nil();
        let hops: Vec<Bookmark> = store
            .jump_chain(&draft)
            .map(|c| c.into_iter().cloned().collect())
            .unwrap_or_default();
        ui.add_space(6.0);
        self.route(ui, store, &hops);
        ui.add_space(6.0);
        forms::help(
            ui,
            t("Like ssh -J. Each hop checks its own host key and logs in its own way."),
        );
        ui.add_space(12.0);
        // Port forwarding.
        ui.label(forms::label(t("Port forwarding"), &p));
        ui.add_space(6.0);
        self.forwards_table(ui, problems);
        ui.add_space(12.0);
        // Legacy algorithms.
        let width = ui.available_width();
        egui::Frame::NONE
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(width - 24.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    kit::switch(ui, &mut self.draft.legacy_algorithms);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(
                            RichText::new(t("Allow legacy algorithms"))
                                .font(style::medium(style::BODY))
                                .color(p.text),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(t("SHA-1 signatures, CBC ciphers and SHA-1 key exchange, for servers from before 2016."))
                                    .font(style::regular(style::SMALL))
                                    .color(p.text_faint),
                            )
                            .wrap(),
                        );
                    });
                });
            });
    }

    /// The chain of hops in a well: this computer, each jump host, this server.
    fn route(&self, ui: &mut Ui, store: &BookmarkStore, hops: &[Bookmark]) {
        let p = style::pal(ui);
        let dark = p.dark;
        let width = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(width, 66.0), Sense::hover());
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(10),
            p.well,
            Stroke::new(1.0, p.border),
            StrokeKind::Inside,
        );
        // The cards: (icon, label colour, title, address, is the destination).
        let me_name = if self.draft.name.trim().is_empty() {
            self.draft.host.trim().to_string()
        } else {
            self.draft.name.trim().to_string()
        };
        let mut me = self.draft.clone();
        me.tags = self.tags.clone();
        let mut cards: Vec<(&str, Option<Color32>, String, String, bool)> = vec![(
            icons::LAPTOP,
            None,
            capitalize(crate::platform::this_computer()),
            String::new(),
            false,
        )];
        for h in hops {
            cards.push((
                kit::host_icon(h.host_icon()),
                store.label_of(h).map(|l| style::label_color(l.color, dark)),
                h.display_name(),
                h.address(),
                false,
            ));
        }
        cards.push((
            kit::host_icon(me.host_icon()),
            self.label_now(store)
                .map(|l| style::label_color(l.color, dark)),
            if me_name.is_empty() {
                "—".into()
            } else {
                me_name
            },
            {
                let u = self.draft.username.trim();
                let h = self.draft.host.trim();
                if u.is_empty() {
                    h.to_string()
                } else {
                    format!("{u}@{h}")
                }
            },
            true,
        ));
        let mut widths = Vec::new();
        for (_, _, title, addr, _) in &cards {
            let tw = painter
                .layout_no_wrap(title.clone(), style::semibold(12.5), p.text)
                .size()
                .x;
            let aw = painter
                .layout_no_wrap(addr.clone(), style::mono(10.5), p.text_faint)
                .size()
                .x;
            widths.push(7.0 + 28.0 + 9.0 + tw.max(aw).min(150.0) + 10.0);
        }
        let total: f32 = widths.iter().sum();
        let links = (cards.len() - 1) as f32;
        let link_w = ((rect.width() - 24.0 - total) / links.max(1.0)).max(30.0);
        let mut x = rect.left() + 12.0;
        let cy = rect.center().y;
        let n = cards.len();
        for (i, ((icon, color, title, addr, dest), w)) in cards.into_iter().zip(widths).enumerate()
        {
            let card = Rect::from_min_size(pos2(x, cy - 22.5), vec2(w, 45.0));
            if !dark {
                painter.rect_filled(
                    card.translate(vec2(0.0, 1.0)),
                    CornerRadius::same(9),
                    Color32::from_black_alpha(10),
                );
            }
            painter.rect(
                card,
                CornerRadius::same(9),
                if dark {
                    Color32::from_rgb(0x22, 0x22, 0x22)
                } else {
                    p.surface
                },
                if dest {
                    Stroke::new(1.5, p.accent_line)
                } else {
                    Stroke::new(1.0, p.border)
                },
                if dest {
                    StrokeKind::Outside
                } else {
                    StrokeKind::Inside
                },
            );
            let tile = Rect::from_min_size(pos2(card.left() + 7.0, cy - 14.0), vec2(28.0, 28.0));
            if color.is_some() || i > 0 {
                kit::paint_host_glyph(painter, tile, icon, color, None, p.surface, &p);
            } else {
                kit::paint_icon_tile(painter, tile, icon, Tone::Neutral, &p);
            }
            let tx = tile.right() + 9.0;
            let clip = painter.with_clip_rect(Rect::from_min_max(
                pos2(tx, card.top()),
                pos2(card.right() - 6.0, card.bottom()),
            ));
            if addr.is_empty() {
                clip.text(
                    pos2(tx, cy),
                    Align2::LEFT_CENTER,
                    title,
                    style::semibold(12.5),
                    p.text,
                );
            } else {
                clip.text(
                    pos2(tx, cy - 7.0),
                    Align2::LEFT_CENTER,
                    title,
                    style::semibold(12.5),
                    p.text,
                );
                clip.text(
                    pos2(tx, cy + 8.0),
                    Align2::LEFT_CENTER,
                    addr,
                    style::mono(10.5),
                    p.text_faint,
                );
            }
            x = card.right();
            if i + 1 < n {
                let a = pos2(x + 6.0, cy);
                let b = pos2(x + link_w - 6.0, cy);
                painter.line_segment([a, b], Stroke::new(1.5, p.border_strong));
                painter.line_segment([b, b + vec2(-4.0, -4.0)], Stroke::new(1.5, p.border_strong));
                painter.line_segment([b, b + vec2(-4.0, 4.0)], Stroke::new(1.5, p.border_strong));
                let label = if i == 0 && n > 2 {
                    "ssh -J".to_string()
                } else if i + 2 == n {
                    tf("port {}", &[self.port_text.trim()])
                } else {
                    String::new()
                };
                if !label.is_empty() {
                    painter.text(
                        pos2((a.x + b.x) / 2.0, cy - 10.0),
                        Align2::CENTER_CENTER,
                        label,
                        style::regular(10.0),
                        p.text_faint,
                    );
                }
                x += link_w;
            }
        }
    }

    /// Port forwards as a table: type, port listened on, destination; a row with a mistake is
    /// tinted and says what is wrong.
    fn forwards_table(&mut self, ui: &mut Ui, problems: &[(usize, String)]) {
        let p = style::pal(ui);
        let width = ui.available_width();
        let saved_bad = self
            .problem
            .as_ref()
            .filter(|pr| pr.section == Section::Network)
            .and_then(|pr| pr.forward.map(|i| (i, pr.message.clone())));
        let mut remove = None;
        let mut add = false;
        egui::Frame::NONE
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
                let cols = |r: Rect| {
                    let x0 = r.left() + 12.0;
                    let type_r = Rect::from_min_size(pos2(x0, r.top()), vec2(150.0, 38.0));
                    let listen_r =
                        Rect::from_min_size(pos2(type_r.right() + 8.0, r.top()), vec2(92.0, 38.0));
                    let arrow_r = Rect::from_min_size(
                        pos2(listen_r.right() + 8.0, r.top()),
                        vec2(22.0, 38.0),
                    );
                    let dest_r = Rect::from_min_max(
                        pos2(arrow_r.right() + 8.0, r.top()),
                        pos2(r.right() - 8.0 - 26.0 - 8.0, r.top() + 38.0),
                    );
                    let del_r = Rect::from_min_size(
                        pos2(r.right() - 8.0 - 26.0, r.top()),
                        vec2(26.0, 38.0),
                    );
                    (type_r, listen_r, arrow_r, dest_r, del_r)
                };
                // Header.
                let (head, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(
                    head,
                    CornerRadius {
                        nw: 9,
                        ne: 9,
                        sw: 0,
                        se: 0,
                    },
                    p.hover,
                );
                let (tr, lr, _, dr, _) = cols(head);
                for (r, text) in [
                    (tr, t("Type")),
                    (lr, t("Listen on")),
                    (dr, t("Destination (from the server)")),
                ] {
                    painter.text(
                        pos2(r.left(), head.center().y),
                        Align2::LEFT_CENTER,
                        text,
                        style::medium(style::XS),
                        p.text_faint,
                    );
                }
                for i in 0..self.forwards.len() {
                    let err = problems
                        .iter()
                        .find(|(r, _)| *r == i)
                        .map(|(_, m)| m.clone())
                        .or_else(|| {
                            saved_bad
                                .as_ref()
                                .filter(|(r, _)| *r == i)
                                .map(|(_, m)| m.clone())
                        });
                    let h = if err.is_some() { 38.0 + 22.0 } else { 38.0 };
                    let (row, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
                    let painter = ui.painter();
                    if err.is_some() {
                        painter.rect_filled(row, 0.0, p.tone_soft(Tone::Err));
                    }
                    painter.hline(
                        row.x_range(),
                        row.top() + 0.5,
                        Stroke::new(1.0, p.line_soft),
                    );
                    let (type_r, listen_r, arrow_r, dest_r, del_r) =
                        cols(Rect::from_min_size(row.min, vec2(width, 38.0)));
                    let fr = &mut self.forwards[i];
                    // Type: a menu.
                    let type_resp = ui.interact(
                        type_r.shrink2(vec2(0.0, 5.0)),
                        egui::Id::new(("fwd-kind", i)),
                        Sense::click(),
                    );
                    if type_resp.hovered() {
                        ui.painter().rect_filled(
                            type_r.shrink2(vec2(-4.0, 5.0)),
                            CornerRadius::same(6),
                            p.hover,
                        );
                    }
                    paint_kind(ui.painter(), type_r, fr.kind, &p);
                    egui::Popup::menu(&type_resp).show(|ui| {
                        for (k, label) in [
                            (ForwardKind::Local, t("Local (-L)")),
                            (ForwardKind::Remote, t("Remote (-R)")),
                            (ForwardKind::Dynamic, t("SOCKS proxy (-D)")),
                        ] {
                            if kit::menu_item(ui, Some(kind_icon(k)), label, None).clicked() {
                                fr.kind = k;
                            }
                        }
                    });
                    // Listen port.
                    let mut cell = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(listen_r)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    cell.spacing_mut().interact_size.y = 28.0;
                    let listen_bad = err.is_some()
                        && fr
                            .listen
                            .trim()
                            .parse::<u16>()
                            .ok()
                            .filter(|p| *p > 0)
                            .is_none();
                    let lresp = cell.add(
                        style::input(&mut fr.listen)
                            .hint_text("8080")
                            .desired_width(92.0)
                            .margin(egui::Margin::symmetric(10, 6))
                            .mono()
                            .error(listen_bad),
                    );
                    #[cfg(debug_assertions)]
                    crate::devtest::record(format!("forward-listen:{i}"), lresp.rect);
                    let _ = lresp;
                    if fr.kind == ForwardKind::Dynamic {
                        ui.painter().text(
                            pos2(dest_r.left(), dest_r.center().y),
                            Align2::LEFT_CENTER,
                            t("Any host, through the server"),
                            style::regular(style::SMALL),
                            p.text_faint,
                        );
                    } else {
                        ui.painter().text(
                            arrow_r.center(),
                            Align2::CENTER_CENTER,
                            icons::ARROW_RIGHT,
                            style::icon_font(15.0),
                            p.text_faint,
                        );
                        let mut cell = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(dest_r)
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        cell.spacing_mut().interact_size.y = 28.0;
                        let dresp = cell.add(
                            style::input(&mut fr.dest)
                                .hint_text("localhost:80")
                                .desired_width(dest_r.width())
                                .margin(egui::Margin::symmetric(10, 6))
                                .mono(),
                        );
                        #[cfg(debug_assertions)]
                        crate::devtest::record(format!("forward-dest:{i}"), dresp.rect);
                        let _ = dresp;
                    }
                    let del = Rect::from_center_size(del_r.center(), vec2(20.0, 20.0));
                    let dr = ui.interact(del, egui::Id::new(("fwd-del", i)), Sense::click());
                    kit::paint_icon_button(
                        ui,
                        &dr,
                        del,
                        icons::MINUS_CIRCLE,
                        t("Remove"),
                        false,
                        false,
                    );
                    if dr.clicked() {
                        remove = Some(i);
                    }
                    if let Some(m) = err {
                        let painter = ui.painter();
                        let y = row.top() + 38.0 + 4.0;
                        painter.text(
                            pos2(listen_r.left() + 7.0, y + 6.0),
                            Align2::CENTER_CENTER,
                            icons::WARNING_CIRCLE,
                            style::icon_font(13.0),
                            p.err,
                        );
                        painter.text(
                            pos2(listen_r.left() + 14.0 + 6.0, y + 6.0),
                            Align2::LEFT_CENTER,
                            m,
                            style::regular(11.5),
                            p.err,
                        );
                    }
                }
                // Footer: Add Forward.
                let (foot, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
                ui.painter().hline(
                    foot.x_range(),
                    foot.top() + 0.5,
                    Stroke::new(1.0, p.line_soft),
                );
                let mut f = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(foot.shrink2(vec2(8.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let r = kit::button_ex(
                    &mut f,
                    kit::Kind::Ghost,
                    Some(icons::PLUS),
                    t("Add Forward"),
                    true,
                );
                #[cfg(debug_assertions)]
                crate::devtest::record("editor-add-forward", r.rect);
                if r.clicked() {
                    add = true;
                }
                f.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(t("Started when it connects"))
                            .font(style::regular(style::SMALL))
                            .color(p.text_faint),
                    );
                });
            });
        if let Some(i) = remove {
            self.forwards.remove(i);
            if self.problem.as_ref().is_some_and(|p| p.forward.is_some()) {
                self.problem = None;
            }
        }
        if add {
            self.forwards.push(ForwardRow {
                kind: ForwardKind::Local,
                listen: String::new(),
                dest: "localhost".into(),
            });
        }
    }

    // ----- Session -----------------------------------------------------------------------------

    fn session_section(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let width = ui.available_width();
        let right_w = 228.0;
        let left_w = width - right_w - 20.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            ui.allocate_ui_with_layout(
                vec2(left_w, 10.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(left_w);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.label(forms::label(t("Remote start folder"), &p));
                    kit::field_with_icon(
                        ui,
                        egui::Id::new("editor-start-dir"),
                        &mut self.draft.start_dir,
                        "/var/www",
                        left_w,
                        icons::FOLDER_SIMPLE,
                        false,
                    );
                    forms::help(
                        ui,
                        t("The terminal and the remote side of Files open here."),
                    );
                    ui.add_space(8.0);
                    ui.label(forms::label(t("Local start folder"), &p));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let browse_w = style::text_width(ui, t("Browse…")) + 26.0;
                        kit::field_with_icon(
                            ui,
                            egui::Id::new("editor-local-dir"),
                            &mut self.draft.local_start_dir,
                            t("Last folder used"),
                            left_w - browse_w - 8.0,
                            icons::LAPTOP,
                            false,
                        );
                        if kit::secondary(ui, None, t("Browse…")).clicked() {
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
                    forms::help(
                        ui,
                        t("The local side of Files opens here. Empty = last folder used."),
                    );
                    ui.add_space(8.0);
                    let commands = self
                        .draft
                        .startup_command
                        .lines()
                        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                        .count();
                    ui.horizontal(|ui| {
                        ui.label(forms::label(t("Run on connect"), &p));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if commands > 0 {
                                ui.label(
                                    RichText::new(if commands == 1 {
                                        t("1 command").to_string()
                                    } else {
                                        tf("{} commands", &[&commands.to_string()])
                                    })
                                    .font(style::regular(style::SMALL))
                                    .color(p.text_faint),
                                );
                            }
                        });
                    });
                    run_on_connect(ui, &mut self.draft.startup_command, left_w);
                    forms::help(
                        ui,
                        t("One command per line, sent to the shell after login."),
                    );
                },
            );
            ui.allocate_ui_with_layout(
                vec2(right_w, 10.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(right_w);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(forms::overline(t("On connect"), &p));
                    self.on_connect_flow(ui, commands_of(&self.draft.startup_command));
                    forms::help(
                        ui,
                        t("Imported startup commands only run if you opted in when importing."),
                    );
                },
            );
        });
    }

    /// What happens when connecting, as numbered steps.
    fn on_connect_flow(&self, ui: &mut Ui, commands: usize) {
        let p = style::pal(ui);
        let (_, fwds) = self.current();
        let user = if self.draft.username.trim().is_empty() {
            std::env::var("USER").unwrap_or_default()
        } else {
            self.draft.username.trim().to_string()
        };
        let mut steps: Vec<Vec<(String, bool)>> = vec![vec![
            (t("Log in as").to_string() + " ", false),
            (user, true),
        ]];
        let dir = self.draft.start_dir.trim();
        steps.push(vec![
            (t("Start in").to_string() + " ", false),
            (
                if dir.is_empty() {
                    "~".to_string()
                } else {
                    dir.to_string()
                },
                true,
            ),
        ]);
        if commands > 0 {
            steps.push(vec![(
                if commands == 1 {
                    t("Run 1 command").to_string()
                } else {
                    tf("Run {} commands", &[&commands.to_string()])
                },
                false,
            )]);
        }
        if !fwds.is_empty() {
            let mut s = vec![(t("Forward").to_string() + " ", false)];
            for (i, f) in fwds.iter().enumerate() {
                if i > 0 {
                    s.push((", ".into(), false));
                }
                s.push((f.listen_port.to_string(), true));
            }
            steps.push(s);
        }
        let local = self.draft.local_start_dir.trim();
        steps.push(vec![
            (t("Files:").to_string() + " ", false),
            (
                if local.is_empty() {
                    t("last folder").to_string()
                } else {
                    shorten(local, 14)
                },
                true,
            ),
            (" ⇄ ".into(), false),
            (
                if dir.is_empty() {
                    "~".into()
                } else {
                    shorten(dir, 14)
                },
                true,
            ),
        ]);
        let width = ui.available_width();
        egui::Frame::NONE
            .fill(p.well)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.set_width(width - 28.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                let n = steps.len();
                for (i, step) in steps.into_iter().enumerate() {
                    let (r, _) = ui.allocate_exact_size(vec2(width - 28.0, 30.0), Sense::hover());
                    let painter = ui.painter();
                    let c = pos2(r.left() + 10.0, r.center().y);
                    if i > 0 {
                        painter.vline(
                            c.x,
                            (r.top() - 1.0)..=(c.y - 10.0),
                            Stroke::new(1.5, p.border_strong),
                        );
                    }
                    if i + 1 < n {
                        painter.vline(
                            c.x,
                            (c.y + 10.0)..=(r.bottom() + 1.0),
                            Stroke::new(1.5, p.border_strong),
                        );
                    }
                    painter.circle_filled(c, 10.0, p.accent_tint);
                    painter.text(
                        c,
                        Align2::CENTER_CENTER,
                        (i + 1).to_string(),
                        style::semibold(10.5),
                        p.accent_text,
                    );
                    let mut job = egui::text::LayoutJob::default();
                    for (text, code) in step {
                        job.append(
                            &text,
                            0.0,
                            if code {
                                egui::TextFormat::simple(style::mono(11.5), p.text_muted)
                            } else {
                                egui::TextFormat::simple(style::regular(12.5), p.text)
                            },
                        );
                    }
                    job.wrap = egui::text::TextWrapping::truncate_at_width(r.width() - 32.0);
                    let g = painter.layout_job(job);
                    painter.galley(
                        pos2(r.left() + 32.0, r.center().y - g.size().y / 2.0),
                        g,
                        p.text,
                    );
                }
            });
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

// ----- pieces ------------------------------------------------------------------------------

/// A form row: the label (128 px, centred on a 30 px control, or at its top for tall ones)
/// and the controls, 16 px apart.
fn frow<R>(ui: &mut Ui, label: &str, top: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    frow_h(ui, label, top, 30.0, add)
}

/// [`frow`] for a control `h` px high (the label is centred on it).
fn frow_h<R>(ui: &mut Ui, label: &str, top: bool, h: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        let (r, _) = ui.allocate_exact_size(vec2(LABEL_COL, h), Sense::hover());
        let p = style::pal(ui);
        ui.painter().text(
            pos2(r.left(), if top { r.top() + 15.0 } else { r.center().y }),
            Align2::LEFT_CENTER,
            label,
            style::medium(style::SMALL),
            p.text_muted,
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
    })
    .inner
}

/// Help text in the control column of the form, 6 px under the row above.
fn help_row(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(LABEL_COL + 16.0);
        ui.vertical(|ui| {
            ui.add_space(6.0);
            forms::help(ui, text);
        });
    });
}

/// The hairline between the address and the organisation of a connection.
fn separator(ui: &mut Ui) {
    let p = style::pal(ui);
    ui.add_space(14.0);
    let y = ui.cursor().top();
    ui.painter()
        .hline(ui.max_rect().x_range(), y + 0.5, Stroke::new(1.0, p.border));
    ui.add_space(15.0);
}

/// A field filled in from a pasted address: accent border on an accent wash.
fn split_halo(ui: &Ui, rect: Rect, p: &Palette) {
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(style::RADIUS),
        Stroke::new(1.0, p.accent),
        StrokeKind::Inside,
    );
    ui.painter().rect_filled(
        rect.shrink(1.0),
        CornerRadius::same(style::RADIUS - 1),
        p.accent_softer,
    );
}

/// "Split from deploy@host:2222  Undo": returns whether Undo was clicked.
fn pasted_pill(ui: &mut Ui, pasted: &str) -> bool {
    let p = style::pal(ui);
    let painter = ui.painter();
    let a = painter.layout_no_wrap(
        t("Split from").to_string(),
        style::medium(style::SMALL),
        p.accent_text,
    );
    let code = painter.layout_no_wrap(pasted.to_string(), style::mono(11.5), p.accent_text);
    let undo = painter.layout_no_wrap(
        t("Undo").to_string(),
        style::medium(style::SMALL),
        p.text_muted,
    );
    let w = 9.0 + 14.0 + 8.0 + a.size().x + 8.0 + code.size().x + 8.0 + undo.size().x + 14.0 + 6.0;
    let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(r, CornerRadius::same(7), p.accent_tint);
    let mut x = r.left() + 9.0;
    painter.text(
        pos2(x + 7.0, r.center().y),
        Align2::CENTER_CENTER,
        icons::MAGIC_WAND,
        style::icon_font(14.0),
        p.accent_text,
    );
    x += 14.0 + 8.0;
    let aw = a.size().x;
    painter.galley(pos2(x, r.center().y - a.size().y / 2.0), a, p.accent_text);
    x += aw + 8.0;
    let cw = code.size().x;
    painter.galley(
        pos2(x, r.center().y - code.size().y / 2.0),
        code,
        p.accent_text,
    );
    x += cw + 8.0;
    let ur = Rect::from_min_size(
        pos2(x, r.center().y - 10.0),
        vec2(undo.size().x + 14.0, 20.0),
    );
    let resp = ui.interact(ur, egui::Id::new("editor-undo-split"), Sense::click());
    ui.painter().rect_filled(
        ur,
        CornerRadius::same(5),
        if resp.hovered() { p.subtle } else { p.hover },
    );
    ui.painter().galley(
        ur.center() - undo.size() / 2.0,
        undo,
        if resp.hovered() { p.text } else { p.text_muted },
    );
    resp.clicked()
}

/// A select (30 px field look with an up-down caret on the right).
fn paint_select(ui: &Ui, rect: Rect, resp: &egui::Response, open: bool) {
    let p = style::pal(ui);
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(style::RADIUS),
        p.field,
        Stroke::new(
            1.0,
            if open {
                p.accent
            } else if resp.hovered() {
                p.border_strong
            } else {
                p.field_border
            },
        ),
        StrokeKind::Inside,
    );
    if open {
        style::focus_halo(ui, rect, style::RADIUS);
    }
    painter.text(
        pos2(rect.right() - 10.0 - 6.0, rect.center().y),
        Align2::CENTER_CENTER,
        icons::CARET_UP_DOWN,
        style::icon_font(12.0),
        p.text_faint,
    );
}

/// A menu row with an optional label dot (`Some(None)` keeps the dot's room empty); returns
/// whether it was clicked.
fn menu_row(ui: &mut Ui, dot: Option<Option<Color32>>, text: &str, on: bool) -> bool {
    let p = style::pal(ui);
    let w = ui.available_width().max(200.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(r, CornerRadius::same(6), p.hover);
    }
    let mut x = r.left() + 8.0;
    if let Some(dot) = dot {
        if let Some(c) = dot {
            painter.circle_filled(pos2(x + 4.0, r.center().y), 4.0, c);
        }
        x += 8.0 + 9.0;
    }
    painter.text(
        pos2(x, r.center().y),
        Align2::LEFT_CENTER,
        text,
        style::regular(style::BODY),
        p.text,
    );
    if on {
        painter.text(
            pos2(r.right() - 8.0 - 7.0, r.center().y),
            Align2::CENTER_CENTER,
            icons::CHECK,
            style::icon_font(14.0),
            p.accent_text,
        );
    }
    if resp.clicked() {
        ui.close();
    }
    resp.clicked()
}

/// A login method as a card: icon tile, title and a line about it, the radio at the top right.
fn method_card(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    desc: &str,
    on: bool,
    width: f32,
) -> egui::Response {
    let p = style::pal(ui);
    let desc_g = ui.painter().layout(
        desc.to_string(),
        style::regular(11.5),
        p.text_faint,
        width - 22.0,
    );
    let h = 11.0 + 28.0 + 9.0 + 17.5 + 1.0 + desc_g.size().y.max(16.0) + 10.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, h), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, true, on, title));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if !on && resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(10), p.hover);
        }
        forms::choice_outline(painter, rect, 10, on, &p);
        let tile = Rect::from_min_size(rect.min + vec2(11.0, 11.0), vec2(28.0, 28.0));
        painter.rect_filled(
            tile,
            CornerRadius::same(8),
            if on { p.accent_tint } else { p.hover },
        );
        painter.text(
            tile.center(),
            Align2::CENTER_CENTER,
            icon,
            style::icon_font(15.0),
            if on { p.accent_text } else { p.text_muted },
        );
        kit::paint_radio(
            painter,
            pos2(rect.right() - 11.0 - 8.0, rect.top() + 11.0 + 8.0),
            on,
            &p,
        );
        let y = tile.bottom() + 9.0;
        painter.text(
            pos2(rect.left() + 11.0, y + 8.75),
            Align2::LEFT_CENTER,
            title,
            style::semibold(12.5),
            p.text,
        );
        painter.galley(
            pos2(rect.left() + 11.0, y + 17.5 + 1.0),
            desc_g,
            p.text_faint,
        );
    }
    resp
}

/// How a secret field looks: saved (a green band with Show and Forget), or a field to type in.
struct SecretUi<'a> {
    text: &'a mut String,
    shown: &'a mut bool,
    had: bool,
    clear: &'a mut bool,
    forget_label: &'a str,
    hint: &'a str,
    saved: (Uuid, SecretKind),
    record: &'a str,
}

fn secret_block(ui: &mut Ui, s: SecretUi, problem: &mut Option<Problem>) {
    let p = style::pal(ui);
    let saved = s.had && !*s.clear;
    if saved && !*s.shown {
        // The green band.
        let width = ui.available_width();
        let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
        let painter = ui.painter();
        painter.rect_filled(r, CornerRadius::same(style::RADIUS), p.tone_soft(Tone::Ok));
        painter.text(
            pos2(r.left() + 10.0 + 7.5, r.center().y),
            Align2::CENTER_CENTER,
            icons::CHECK_CIRCLE,
            style::icon_font(15.0),
            p.ok,
        );
        painter.text(
            pos2(r.left() + 10.0 + 15.0 + 10.0, r.center().y),
            Align2::LEFT_CENTER,
            tf("Saved in {}", &[crate::platform::keychain()]),
            style::medium(12.5),
            p.ok,
        );
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(r.shrink2(vec2(3.0, 0.0)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right.spacing_mut().item_spacing.x = 2.0;
        if kit::button_ex(&mut right, kit::Kind::Ghost, None, s.forget_label, true).clicked() {
            *s.clear = true;
            s.text.clear();
        }
        let show = kit::button_ex(
            &mut right,
            kit::Kind::Ghost,
            Some(icons::EYE),
            t("Show"),
            true,
        );
        #[cfg(debug_assertions)]
        crate::devtest::record(s.record, show.rect);
        if show.clicked() {
            match secrets::get(s.saved.0, s.saved.1) {
                Ok(Some(value)) => {
                    *s.text = value;
                    *s.shown = true;
                }
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
        return;
    }
    let hint = if saved {
        tf(
            "Saved in {} — type to replace",
            &[crate::platform::keychain()],
        )
    } else {
        s.hint.to_string()
    };
    secret_input(
        ui,
        s.text,
        s.shown,
        &hint,
        saved.then_some(s.saved),
        problem,
        s.record,
    );
    if s.had && *s.clear {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            kit::badge(ui, t("Will be forgotten"), Tone::Warn, Some(icons::TRASH));
            if kit::button_ex(ui, kit::Kind::Ghost, None, t("Keep"), true).clicked() {
                *s.clear = false;
            }
        });
    }
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
    let eye = Rect::from_center_size(
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

/// An agent in the list (42 px): radio, icon tile, name and its socket in mono; "Found" when it
/// is the one in use and answers.
fn agent_row(
    ui: &mut Ui,
    first: bool,
    on: bool,
    icon: &str,
    name: &str,
    path: &str,
    found: bool,
) -> egui::Response {
    agent_row_ex(ui, first, on, icon, name, path, found, true)
}

#[allow(clippy::too_many_arguments)]
fn agent_row_ex(
    ui: &mut Ui,
    first: bool,
    on: bool,
    icon: &str,
    name: &str,
    path: &str,
    found: bool,
    mono: bool,
) -> egui::Response {
    let p = style::pal(ui);
    let width = ui.available_width();
    let (r, resp) = ui.allocate_exact_size(vec2(width, 42.0), Sense::click());
    let painter = ui.painter();
    if on {
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
    } else if resp.hovered() {
        painter.rect_filled(r, CornerRadius::ZERO, p.hover);
    }
    if !first {
        painter.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, p.line_soft));
    }
    kit::paint_radio(painter, pos2(r.left() + 12.0 + 8.0, r.center().y), on, &p);
    let tile = Rect::from_min_size(
        pos2(r.left() + 12.0 + 16.0 + 10.0, r.center().y - 16.0),
        vec2(32.0, 32.0),
    );
    kit::paint_icon_tile(
        painter,
        tile,
        icon,
        if on { Tone::Accent } else { Tone::Neutral },
        &p,
    );
    let tx = tile.right() + 10.0;
    let right = if found {
        r.right() - 70.0
    } else {
        r.right() - 12.0
    };
    let clip = painter.with_clip_rect(Rect::from_min_max(
        pos2(tx, r.top()),
        pos2(right, r.bottom()),
    ));
    clip.text(
        pos2(tx, r.center().y - 7.5),
        Align2::LEFT_CENTER,
        name,
        style::medium(style::BODY),
        p.text,
    );
    clip.text(
        pos2(tx, r.center().y + 8.0),
        Align2::LEFT_CENTER,
        path,
        if mono {
            style::mono(11.0)
        } else {
            style::regular(style::SMALL)
        },
        p.text_faint,
    );
    if found {
        let w = kit::env_tag_width(ui, "x", false);
        let _ = w;
        let g = painter.layout_no_wrap(t("Found").to_string(), style::semibold(style::MICRO), p.ok);
        let br = Rect::from_min_size(
            pos2(r.right() - 12.0 - g.size().x - 12.0, r.center().y - 9.0),
            vec2(g.size().x + 12.0, 18.0),
        );
        painter.rect_filled(br, CornerRadius::same(5), p.tone_soft(Tone::Ok));
        painter.galley(br.center() - g.size() / 2.0, g, p.ok);
    }
    resp
}

/// Whether the agent socket answers and what that means.
fn agent_status(socket: &str) -> (bool, String) {
    if cfg!(windows) && socket.is_empty() {
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
            Some(_) => (
                true,
                t("Agent found. It signs for TernaryShell; the keys never leave it.").to_string(),
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
    }
}

/// "Agent found. It signs…": the first sentence bold.
fn callout_job(text: &str, p: &Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let (head, rest) = match text.find(". ") {
        Some(i) => (&text[..=i], &text[i + 1..]),
        None => (text, ""),
    };
    job.append(
        head,
        0.0,
        egui::TextFormat::simple(style::semibold(style::SMALL), p.text),
    );
    job.append(
        rest,
        0.0,
        egui::TextFormat::simple(style::regular(style::SMALL), p.text_muted),
    );
    job
}

/// The icon of a kind of forward.
fn kind_icon(k: ForwardKind) -> &'static str {
    match k {
        ForwardKind::Local => icons::ARROW_RIGHT,
        ForwardKind::Remote => icons::ARROW_LEFT,
        ForwardKind::Dynamic => icons::GLOBE_HEMISPHERE_WEST,
    }
}

/// "→ Local -L" in the Type column.
fn paint_kind(painter: &egui::Painter, r: Rect, k: ForwardKind, p: &Palette) {
    let (name, flag) = match k {
        ForwardKind::Local => (t("Local"), "-L"),
        ForwardKind::Remote => (t("Remote"), "-R"),
        ForwardKind::Dynamic => (t("SOCKS proxy"), "-D"),
    };
    let cy = r.center().y;
    painter.text(
        pos2(r.left() + 7.0, cy),
        Align2::CENTER_CENTER,
        kind_icon(k),
        style::icon_font(14.0),
        p.text_muted,
    );
    let g = painter.layout_no_wrap(name.to_string(), style::medium(12.5), p.text);
    let x = r.left() + 14.0 + 7.0;
    let nw = g.size().x;
    painter.galley(pos2(x, cy - g.size().y / 2.0), g, p.text);
    let fg = painter.layout_no_wrap(flag.to_string(), style::mono(10.5), p.text_faint);
    let fr = Rect::from_min_size(pos2(x + nw + 7.0, cy - 8.0), vec2(fg.size().x + 10.0, 16.0));
    painter.rect_filled(fr, CornerRadius::same(4), p.hover);
    painter.galley(fr.center() - fg.size() / 2.0, fg, p.text_faint);
}

/// The startup commands as a small script: line numbers in a gutter, comments (`#`) faint.
fn run_on_connect(ui: &mut Ui, text: &mut String, width: f32) {
    let p = style::pal(ui);
    let lines = text.lines().count().max(1) + usize::from(text.ends_with('\n'));
    let rows = lines.max(4);
    let line_h = 20.0;
    let h = 8.0 + rows as f32 * line_h + 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let id = egui::Id::new("editor-run-on-connect");
    let focused = ui.memory(|m| m.has_focus(id));
    let painter = ui.painter().clone();
    painter.rect(
        rect,
        CornerRadius::same(style::RADIUS),
        p.field,
        Stroke::new(1.0, if focused { p.accent } else { p.field_border }),
        StrokeKind::Inside,
    );
    let gut = Rect::from_min_size(rect.min, vec2(34.0, h));
    painter.rect_filled(
        gut.shrink(1.0),
        CornerRadius {
            nw: style::RADIUS - 1,
            sw: style::RADIUS - 1,
            ne: 0,
            se: 0,
        },
        p.hover,
    );
    let faint = p.text_faint;
    let color = p.text;
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, wrap: f32| {
        let mut job = egui::text::LayoutJob::default();
        let s = buf.as_str();
        if s.is_empty() {
            job.append(
                "",
                0.0,
                egui::TextFormat {
                    font_id: style::mono(12.0),
                    color,
                    line_height: Some(line_h),
                    ..Default::default()
                },
            );
        }
        for line in s.split_inclusive('\n') {
            let c = if line.trim_start().starts_with('#') {
                faint
            } else {
                color
            };
            job.append(
                line,
                0.0,
                egui::TextFormat {
                    font_id: style::mono(12.0),
                    color: c,
                    line_height: Some(line_h),
                    ..Default::default()
                },
            );
        }
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(Rect::from_min_max(pos2(gut.right(), rect.top()), rect.max)),
    );
    let out = egui::TextEdit::multiline(text)
        .id(id)
        .frame(egui::Frame::NONE)
        .margin(egui::Margin::symmetric(10, 8))
        .desired_width(width - 34.0 - 20.0)
        .desired_rows(rows)
        .layouter(&mut layouter)
        .show(&mut child);
    // Line numbers on the rows as laid out (a wrapped line keeps one number).
    let mut n = 0;
    let mut last_y = out.galley_pos.y + 8.0;
    let mut new_line = true;
    for row in &out.galley.rows {
        let y = out.galley_pos.y + row.rect().center().y;
        if new_line {
            n += 1;
            painter.text(
                pos2(gut.right() - 9.0, y),
                Align2::RIGHT_CENTER,
                n.to_string(),
                style::mono(11.0),
                p.text_ghost,
            );
        }
        new_line = row.ends_with_newline;
        last_y = y;
    }
    while n < rows {
        n += 1;
        last_y += line_h;
        painter.text(
            pos2(gut.right() - 9.0, last_y),
            Align2::RIGHT_CENTER,
            n.to_string(),
            style::mono(11.0),
            p.text_ghost,
        );
    }
    if text.is_empty() && !focused {
        let y = out.galley_pos.y
            + out
                .galley
                .rows
                .first()
                .map_or(line_h / 2.0, |r| r.rect().center().y);
        painter.text(
            pos2(gut.right() + 10.0, y),
            Align2::LEFT_CENTER,
            t("One command per line"),
            style::mono(12.0),
            p.text_faint,
        );
    }
    if focused {
        style::focus_halo(ui, rect, style::RADIUS);
    }
}

/// Commands that will run (not empty, not comments).
fn commands_of(text: &str) -> usize {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .count()
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `~/Projects/storefront` → `~/Projects/…` when longer than `max` characters.
fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    match s.rfind('/') {
        Some(i) if i > 0 => format!(
            "{}/…",
            s[..i]
                .chars()
                .take(max.saturating_sub(2))
                .collect::<String>()
        ),
        _ => format!("{}…", s.chars().take(max).collect::<String>()),
    }
}

/// The secret store's name at the start of a sentence ("Keychain, or asked each time").
fn keychain_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Keychain"
    } else if cfg!(windows) {
        "Credential Manager"
    } else {
        t("System keyring")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_destination_is_split() {
        let row = |kind, listen: &str, dest: &str| ForwardRow {
            kind,
            listen: listen.into(),
            dest: dest.into(),
        };
        let f = row(ForwardKind::Local, "5432", "localhost:5432")
            .to_forward()
            .unwrap();
        assert_eq!(
            (f.host.as_str(), f.port, f.listen_port),
            ("localhost", 5432, 5432)
        );
        let f = row(ForwardKind::Remote, "80", "[::1]:8080")
            .to_forward()
            .unwrap();
        assert_eq!((f.host.as_str(), f.port), ("::1", 8080));
        assert!(
            row(ForwardKind::Local, "80800", "localhost:1")
                .to_forward()
                .is_err()
        );
        assert!(
            row(ForwardKind::Local, "80", "localhost")
                .to_forward()
                .is_err()
        );
        assert!(row(ForwardKind::Dynamic, "1080", "").to_forward().is_ok());
        let back = ForwardRow::from(&Forward {
            kind: ForwardKind::Local,
            listen_port: 1,
            host: "::1".into(),
            port: 2,
        });
        assert_eq!(back.dest, "[::1]:2");
    }

    #[test]
    fn same_as_command() {
        let store = BookmarkStore::default();
        let b = Bookmark {
            host: "web-01.acme.io".into(),
            username: "deploy".into(),
            port: 2222,
            ..Default::default()
        };
        let fwd = vec![Forward {
            kind: ForwardKind::Local,
            listen_port: 8080,
            host: "localhost".into(),
            port: 80,
        }];
        let auth = Auth::Key {
            path: "~/.ssh/id_ed25519".into(),
        };
        let parts = ssh_command(&b, &store, &auth, &fwd);
        let line: Vec<&str> = parts.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            line.join(" "),
            "ssh -p 2222 -i ~/.ssh/id_ed25519 -L 8080:localhost:80 deploy@web-01.acme.io"
        );
        assert!(parts.last().is_some_and(|(_, p)| *p == Part::Target));
    }
}
