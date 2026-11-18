//! Bookmark and group data model.
//!
//! Secrets (passwords, key passphrases, imported key material) are never stored here; they live
//! in the macOS Keychain, keyed by bookmark id (see `crate::secrets`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_PORT: u16 = 22;

/// The colour of an environment label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelColor {
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Violet,
    Pink,
    Grey,
}

impl LabelColor {
    pub const ALL: [LabelColor; 9] = [
        LabelColor::Red,
        LabelColor::Orange,
        LabelColor::Yellow,
        LabelColor::Green,
        LabelColor::Teal,
        LabelColor::Blue,
        LabelColor::Violet,
        LabelColor::Pink,
        LabelColor::Grey,
    ];
}

/// An environment label (`PROD` in red): set on a group (its connections inherit it) or on one
/// connection. It colours the host's icon, its tabs, the top edge of its terminal and the
/// status bar, so production is never mistaken for staging.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub color: LabelColor,
    /// A short tag such as `PROD` (at most [`Label::MAX_TEXT`] characters, upper case).
    #[serde(default)]
    pub text: String,
}

impl Label {
    pub const MAX_TEXT: usize = 6;

    /// The text as it is shown: trimmed, upper case, at most 6 characters.
    pub fn tag(&self) -> String {
        self.text
            .trim()
            .chars()
            .take(Self::MAX_TEXT)
            .collect::<String>()
            .to_uppercase()
    }
}

/// What kind of machine a connection is: picks the icon in the sidebar, tabs and cards.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostIcon {
    #[default]
    Server,
    Database,
    Bastion,
    Worker,
    Board,
    Storage,
    Desktop,
    Cloud,
    Container,
}

impl HostIcon {
    pub const ALL: [HostIcon; 9] = [
        HostIcon::Server,
        HostIcon::Database,
        HostIcon::Bastion,
        HostIcon::Worker,
        HostIcon::Board,
        HostIcon::Storage,
        HostIcon::Desktop,
        HostIcon::Cloud,
        HostIcon::Container,
    ];

    /// The icon a connection gets when none is chosen: from its tags and name (`db`, `jump`,
    /// `pi`, `nas`…), a server otherwise.
    pub fn guess(b: &Bookmark) -> HostIcon {
        let words: Vec<String> = b
            .tags
            .iter()
            .map(|t| t.to_ascii_lowercase())
            .chain(
                b.name
                    .to_ascii_lowercase()
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .filter(|w| !w.is_empty())
                    .map(str::to_string),
            )
            .collect();
        let any = |keys: &[&str]| words.iter().any(|w| keys.contains(&w.as_str()));
        if any(&[
            "db",
            "database",
            "postgres",
            "postgresql",
            "mysql",
            "mariadb",
            "redis",
            "mongo",
            "mongodb",
            "sql",
        ]) {
            HostIcon::Database
        } else if any(&["bastion", "jump", "jumphost", "gateway", "gw"]) {
            HostIcon::Bastion
        } else if any(&["pi", "raspberry", "raspberrypi", "arm", "iot", "board"]) {
            HostIcon::Board
        } else if any(&["nas", "storage", "backup", "s3", "minio"]) {
            HostIcon::Storage
        } else if any(&[
            "worker", "workers", "queue", "job", "jobs", "cpu", "gpu", "ci", "runner",
        ]) {
            HostIcon::Worker
        } else if any(&["vm", "dev", "desktop", "laptop", "workstation", "mac"]) {
            HostIcon::Desktop
        } else if any(&["docker", "container", "k8s", "kube", "kubernetes", "pod"]) {
            HostIcon::Container
        } else if any(&["cloud", "aws", "gcp", "azure", "ec2", "droplet", "vps"]) {
            HostIcon::Cloud
        } else {
            HostIcon::Server
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    /// Parent group; `None` means top level.
    #[serde(default)]
    pub parent: Option<Uuid>,
    /// The environment label of the group; its connections and subgroups inherit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
}

impl Group {
    pub fn new(name: impl Into<String>, parent: Option<Uuid>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            parent,
            label: None,
        }
    }
}

/// How the client authenticates. The matching secret (password or passphrase) is looked up in
/// the Keychain at connect time; when it is missing the user is prompted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Auth {
    #[default]
    Password,
    /// Private key file on disk. Only the path is stored and exported, never the key itself.
    Key { path: String },
    /// Private key material stored in the Keychain (used when importing from tools that embed
    /// key contents, such as Electerm), so no key is written to disk in plain text.
    KeychainKey,
    /// The keys of an SSH agent (the macOS one, 1Password, Secretive, a YubiKey…); the agent
    /// signs, TernaryShell never sees the keys. `socket` is the agent's socket, empty for the one in
    /// `SSH_AUTH_SOCK`.
    Agent {
        #[serde(default)]
        socket: String,
    },
}

impl Auth {
    /// A private key TernaryShell reads itself, which may have a passphrase.
    pub fn uses_key(&self) -> bool {
        matches!(self, Auth::Key { .. } | Auth::KeychainKey)
    }
}

/// A port forwarded through the connection while it is open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub kind: ForwardKind,
    /// The port that is listened on: on this Mac (local, dynamic) or on the server (remote).
    pub listen_port: u16,
    /// Where connections are sent: a host as the server sees it (local) or as this Mac sees it
    /// (remote). Not used for dynamic forwarding.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    /// `ssh -L`: a port on this Mac reaches `host:port` from the server.
    Local,
    /// `ssh -R`: a port on the server reaches `host:port` from this Mac.
    Remote,
    /// `ssh -D`: a SOCKS proxy on this Mac whose connections leave from the server.
    Dynamic,
}

impl Forward {
    /// `-L 8080:localhost:80` style, for lists and tooltips.
    pub fn describe(&self) -> String {
        match self.kind {
            ForwardKind::Local => format!("L {} → {}:{}", self.listen_port, self.host, self.port),
            ForwardKind::Remote => format!("R {} → {}:{}", self.listen_port, self.host, self.port),
            ForwardKind::Dynamic => format!("D {} (SOCKS)", self.listen_port),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub group: Option<Uuid>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    /// Remote directory to `cd` into after login (also the initial remote SFTP directory).
    #[serde(default)]
    pub start_dir: String,
    /// Local directory the SFTP panel opens in (empty: the last one used).
    #[serde(default)]
    pub local_start_dir: String,
    /// Command sent to the shell right after login.
    #[serde(default)]
    pub startup_command: String,
    /// Also offer older algorithms (ssh-rsa, SHA-1 key exchange, CBC ciphers) for old servers.
    #[serde(default)]
    pub legacy_algorithms: bool,
    /// Reach the server through another saved connection (`ssh -J`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<Forward>,
    /// Its own environment label; `None` inherits the group's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Label>,
    /// The kind of machine; `None` guesses it from the tags and name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<HostIcon>,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

impl Default for Bookmark {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            host: String::new(),
            port: DEFAULT_PORT,
            username: String::new(),
            auth: Auth::Password,
            group: None,
            tags: Vec::new(),
            notes: String::new(),
            start_dir: String::new(),
            local_start_dir: String::new(),
            startup_command: String::new(),
            legacy_algorithms: false,
            jump: None,
            forwards: Vec::new(),
            label: None,
            icon: None,
        }
    }
}

impl Bookmark {
    /// The kind of machine, chosen or guessed.
    pub fn host_icon(&self) -> HostIcon {
        self.icon.unwrap_or_else(|| HostIcon::guess(self))
    }

    /// Name shown in lists: the bookmark name, or `user@host` when it has none.
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            self.address()
        } else {
            self.name.clone()
        }
    }

    /// `user@host[:port]` (IPv6 addresses in brackets).
    pub fn address(&self) -> String {
        let mut s = String::new();
        if !self.username.is_empty() {
            s.push_str(&self.username);
            s.push('@');
        }
        if self.host.contains(':') && !self.host.starts_with('[') {
            s.push_str(&format!("[{}]", self.host));
        } else {
            s.push_str(&self.host);
        }
        if self.port != DEFAULT_PORT {
            s.push_str(&format!(":{}", self.port));
        }
        s
    }

    /// Same server and login: used to match bookmarks across imports when ids differ.
    pub fn same_endpoint(&self, other: &Bookmark) -> bool {
        self.host.eq_ignore_ascii_case(&other.host)
            && self.port == other.port
            && self.username == other.username
    }

    /// Copy with a fresh id (the "Duplicate" action and "keep both" on import).
    pub fn duplicate(&self) -> Bookmark {
        Bookmark {
            id: Uuid::new_v4(),
            ..self.clone()
        }
    }
}

/// An address typed by the user: `[ssh://][user@]host[:port]`, where an IPv6 host with a port
/// is written in brackets (`[::1]:2222`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

/// Parses a typed address; the error says what is wrong with it.
pub fn parse_target(input: &str) -> Result<Target, String> {
    use crate::i18n::t;
    let input = input.trim();
    let input = input
        .strip_prefix("ssh://")
        .unwrap_or(input)
        .trim_end_matches('/');
    let (user, rest) = match input.rsplit_once('@') {
        Some((u, r)) => (
            Some(u.trim().to_string()).filter(|u| !u.is_empty()),
            r.trim(),
        ),
        None => (None, input),
    };
    let bad_port = || t("Port must be a number between 1 and 65535.").to_string();
    let parse_port = |p: &str| {
        p.parse::<u16>()
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(bad_port)
    };
    let (host, port) = if let Some(inner) = rest.strip_prefix('[') {
        let (host, after) = inner
            .split_once(']')
            .ok_or_else(|| t("Missing \"]\" after the IPv6 address.").to_string())?;
        let port = match after {
            "" => None,
            p => Some(parse_port(p.strip_prefix(':').ok_or_else(bad_port)?)?),
        };
        (host.to_string(), port)
    } else {
        match rest.matches(':').count() {
            0 => (rest.to_string(), None),
            1 => {
                let (h, p) = rest.split_once(':').unwrap_or((rest, ""));
                (h.to_string(), Some(parse_port(p)?))
            }
            // Several colons: a bare IPv6 address (a port needs the brackets).
            _ => (rest.to_string(), None),
        }
    };
    if host.is_empty() {
        return Err(t("Host is required.").into());
    }
    if crate::ssh::session::clean_host(&host).is_none() {
        return Err(t("The host name contains characters that are not allowed.").into());
    }
    Ok(Target { user, host, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(user: Option<&str>, host: &str, port: Option<u16>) -> Target {
        Target {
            user: user.map(str::to_string),
            host: host.into(),
            port,
        }
    }

    #[test]
    fn typed_addresses() {
        assert_eq!(
            parse_target("web.example.com"),
            Ok(target(None, "web.example.com", None))
        );
        assert_eq!(
            parse_target(" root@10.0.0.1:2222 "),
            Ok(target(Some("root"), "10.0.0.1", Some(2222)))
        );
        assert_eq!(
            parse_target("ssh://deploy@host/"),
            Ok(target(Some("deploy"), "host", None))
        );
        assert_eq!(
            parse_target("[::1]:2299"),
            Ok(target(None, "::1", Some(2299)))
        );
        assert_eq!(
            parse_target("admin@fe80::1"),
            Ok(target(Some("admin"), "fe80::1", None))
        );
        assert!(parse_target("host:abc").is_err());
        assert!(parse_target("host:0").is_err());
        assert!(parse_target("[::1").is_err());
        assert!(parse_target("bad host").is_err());
        assert!(parse_target("user@").is_err());
        let b = Bookmark {
            host: "::1".into(),
            port: 2299,
            username: "me".into(),
            ..Default::default()
        };
        assert_eq!(b.address(), "me@[::1]:2299");
    }
}
