//! The SFTP panel: local and remote file lists side by side, transfers below.
//!
//! Every operation carries the folder it was started in (dialogs, right-click menus, drags), so
//! a listing that changes in the meantime can never redirect it to another file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use eframe::egui::{
    self, Align2, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use russh_sftp::client::SftpSession;
use tokio::sync::oneshot;

use crate::i18n::{t, tf, tn};
use crate::paths;
use crate::sftp::entry::{self, FileEntry, SortKey, remote_join, remote_parent};
use crate::sftp::remote::{self, Sftp};
use crate::sftp::transfer::{Direction, Summary, Transfer, TransferState, Transfers};
use crate::ssh::session::{RootError, RootLogin, SftpOpener};
use crate::ui::file_editor::{EditorOutput, FileEditor};
use crate::ui::kit;
use crate::ui::style::Tone;
use crate::ui::{icons, style};

enum Session {
    Opening(oneshot::Receiver<Result<SftpSession, String>>),
    Ready(Sftp),
    Failed(String),
}

/// A session being opened to switch root on or off, with the folder to show once it is ready.
enum Reopen {
    ToRoot(oneshot::Receiver<Result<SftpSession, RootError>>, String),
    ToUser(oneshot::Receiver<Result<SftpSession, String>>, String),
}

enum Event {
    Listed {
        /// Request number: replies to older requests are ignored.
        seq: u64,
        path: String,
        result: Result<Vec<FileEntry>, String>,
    },
    OpDone {
        result: Result<(), String>,
        refresh_remote: bool,
    },
    /// The folders of `dir` on the server, for "Go to folder…" suggestions.
    DirsListed {
        seq: u64,
        dir: String,
        result: Result<Vec<String>, String>,
    },
    /// The names in a remote folder files are about to be uploaded into (dropped on a folder
    /// of the list), to ask before replacing any of them.
    DestListed {
        sources: Vec<String>,
        dest: String,
        result: Result<Vec<FileEntry>, String>,
    },
}

#[derive(Default)]
struct SideState {
    cwd: String,
    path_edit: String,
    entries: Vec<FileEntry>,
    loading: bool,
    error: Option<String>,
    selected: HashSet<String>,
    anchor: Option<String>,
    sort: SortKey,
    descending: bool,
    /// Whether this side shows dot-files; each side has its own toggle.
    show_hidden: bool,
    /// The path is being typed (instead of shown as a breadcrumb).
    editing: bool,
}

impl SideState {
    fn visible(&self, show_hidden: bool) -> impl Iterator<Item = &FileEntry> {
        self.entries
            .iter()
            .filter(move |e| show_hidden || !e.is_hidden())
    }

    /// Replaces the listing. Within the same folder the selection of entries that still exist
    /// is kept; a different folder starts fresh.
    fn set_listing(&mut self, cwd: String, entries: Vec<FileEntry>) {
        if cwd == self.cwd {
            self.selected
                .retain(|n| entries.iter().any(|e| &e.name == n));
        } else {
            self.selected.clear();
            self.anchor = None;
            self.path_edit = cwd.clone();
        }
        self.cwd = cwd;
        self.entries = entries;
        self.error = None;
    }
}

/// Files being dragged from one side to the other.
#[derive(Clone)]
struct DragFiles {
    from_remote: bool,
    dir: String,
    names: Vec<String>,
    /// The first one's kind (the icon on the drag's card).
    first_kind: icons::FileKind,
}

/// What a right-click menu acts on, captured when the menu opens.
#[derive(Clone)]
struct MenuTarget {
    remote: bool,
    dir: String,
    names: Vec<String>,
    entry: FileEntry,
}

/// One entry listed in a dialog (Delete, Replace): its kind and what is known about it.
struct DialogItem {
    name: String,
    kind: icons::FileKind,
    /// Right-hand detail: "folder", a size, or the new file's "2.4 KB, today".
    meta: String,
    /// The existing file's size and date (Replace), drawn fainter after an arrow.
    old: String,
}

/// Dialogs remember the folder they were opened for.
enum Dialog {
    Mkdir {
        remote: bool,
        dir: String,
        name: String,
    },
    Rename {
        remote: bool,
        dir: String,
        from: String,
        to: String,
    },
    Delete {
        remote: bool,
        dir: String,
        names: Vec<String>,
        items: Vec<DialogItem>,
    },
    Chmod {
        path: String,
        original: u32,
        mode: String,
    },
    /// Upload: `sources` are local paths and `dest` the remote folder. Download: `sources`
    /// are remote paths and `dest` the local folder.
    Overwrite {
        upload: bool,
        sources: Vec<String>,
        dest: String,
        conflicts: Vec<String>,
        items: Vec<DialogItem>,
    },
    /// Switch the remote session to root via sudo, with a password the user types. Asked only
    /// when sudo wants a password that isn't known (or rejected the one tried); `note` says why.
    Sudo {
        password: String,
        note: Option<String>,
        show: bool,
    },
    /// Type a folder to open, with suggestions from the folder typed so far.
    GoTo {
        remote: bool,
        text: String,
        /// The highlighted suggestion.
        pick: Option<usize>,
    },
}

pub struct SftpPanel {
    session: Session,
    /// Opens new sessions on the same connection, to switch root on or off.
    opener: SftpOpener,
    /// The remote session is running as root (sudo).
    is_root: bool,
    /// A new session being opened to switch root on/off. The working session is kept until this
    /// one is ready, so a failed switch never loses it.
    reopening: Option<Reopen>,
    events_tx: mpsc::Sender<Event>,
    events_rx: mpsc::Receiver<Event>,
    local: SideState,
    remote: SideState,
    remote_seq: u64,
    pub transfers: Transfers,
    dialog: Option<Dialog>,
    focus_dialog: bool,
    /// Finder drops waiting for the session (or for an open dialog to close).
    pending_uploads: Vec<PathBuf>,
    /// A remote path to show once the session is ready ("Open in Files" in the terminal).
    pending_remote: Option<String>,
    start_dir: String,
    message: Option<(String, bool)>,
    /// Label shown next to the pointer while files are dragged between the two sides.
    drag_label: Option<String>,
    menu_target: Option<MenuTarget>,
    refresh_remote_pending: bool,
    /// A remote text file open in the built-in editor.
    editor: Option<FileEditor>,
    /// The last transfer batch whose completion was announced.
    reported_batch: u64,
    /// Distinguishes this panel's UI state (panel sizes) from other tabs'.
    uid: u64,
    /// The floating transfers/errors popup is open. It opens itself on new activity (a transfer
    /// or an error), and the user can hide (collapse) or close it.
    show_activity: bool,
    /// A finished batch to announce (text, is an error).
    notice: Option<(String, bool)>,
    /// The server's name and the user logged in (headers, dialogs).
    host: String,
    user: String,
    /// `user@host[:port]` (the panel's header).
    address: String,
    /// The host's icon and label colour (the remote side's header).
    glyph: &'static str,
    label: Option<crate::bookmarks::LabelColor>,
    /// The panel fills the tab (the terminal shrinks to one line).
    maximized: bool,
    /// The side the user last worked in: its selection is drawn in the accent.
    active_remote: bool,
    /// A folder of a list hovered by files dragged from the other side: (remote side, name).
    drop_dir: Option<(bool, String)>,
    /// Folders for "Go to folder…": the folder they are in, and their names.
    goto_dirs: (String, Vec<String>),
    goto_seq: u64,
    /// Debug builds: file names to show as if dragged in from Finder.
    #[cfg(debug_assertions)]
    dev_hover: Vec<String>,
}

/// Something the app should do after the panel was drawn.
pub enum PanelOutput {
    None,
    /// Remember this local folder for next time.
    LocalDir(String),
    /// Hide the panel (× in its header).
    Close,
}

enum SideAction {
    DragStarted(String),
    MenuOpened(MenuTarget),
    Up(bool),
    Home(bool),
    Refresh(bool),
    Cd(bool, String),
    Mkdir(bool),
    Open {
        remote: bool,
        dir: String,
        entry: FileEntry,
    },
    /// Open a remote file in the built-in editor.
    Edit {
        dir: String,
        name: String,
    },
    /// Choose local files (or folders) to upload into the remote folder.
    PickUpload {
        folders: bool,
    },
    SelectAll(bool),
    Select(bool, String, bool, bool),
    Sort(bool, SortKey),
    Transfer {
        from_remote: bool,
        dir: String,
        names: Vec<String>,
    },
    Rename {
        remote: bool,
        dir: String,
        name: String,
    },
    Delete {
        remote: bool,
        dir: String,
        names: Vec<String>,
    },
    Chmod {
        path: String,
        mode: u32,
    },
    ToggleHidden(bool),
    /// Switch the remote session to or from root (sudo).
    Root,
    RemoveTransfer(u64),
    /// Start a failed transfer again.
    Retry(u64),
    /// Open "Go to folder…".
    GoTo(bool),
    /// Transfer into a folder of the other list (files dropped on it): `into` is that
    /// folder's name in the other side's current folder.
    TransferInto {
        from_remote: bool,
        dir: String,
        names: Vec<String>,
        into: String,
    },
}

impl SftpPanel {
    pub fn new(opener: SftpOpener, start_dir: &str, local_dir: Option<&str>) -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        let local_cwd = local_dir
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(paths::home_dir)
            .display()
            .to_string();
        let mut panel = Self {
            session: Session::Opening(opener.open()),
            opener,
            is_root: false,
            reopening: None,
            events_tx,
            events_rx,
            local: SideState::default(),
            remote: SideState::default(),
            remote_seq: 0,
            transfers: Transfers::default(),
            dialog: None,
            focus_dialog: false,
            pending_uploads: Vec::new(),
            pending_remote: None,
            start_dir: start_dir.trim().to_string(),
            message: None,
            drag_label: None,
            menu_target: None,
            refresh_remote_pending: false,
            editor: None,
            reported_batch: 0,
            uid: {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            },
            show_activity: false,
            notice: None,
            host: String::new(),
            user: String::new(),
            address: String::new(),
            glyph: icons::HARD_DRIVES,
            label: None,
            maximized: false,
            active_remote: true,
            drop_dir: None,
            goto_dirs: (String::new(), Vec::new()),
            goto_seq: 0,
            #[cfg(debug_assertions)]
            dev_hover: Vec::new(),
        };
        panel.local_cd(&local_cwd);
        panel
    }

    /// The server's name and the user, for the headers and dialogs.
    pub fn with_labels(mut self, host: String, user: String) -> Self {
        self.host = host;
        self.user = user;
        self
    }

    /// The host's icon and environment label (the remote side's header), kept current.
    pub fn set_identity(
        &mut self,
        address: &str,
        glyph: &'static str,
        label: Option<crate::bookmarks::LabelColor>,
    ) {
        if self.address != address {
            self.address = address.to_string();
        }
        self.glyph = glyph;
        self.label = label;
    }

    /// The panel fills the tab, with the terminal shrunk to one line.
    pub fn maximized(&self) -> bool {
        self.maximized
    }

    pub fn set_maximized(&mut self, on: bool) {
        self.maximized = on;
    }

    /// Debug builds: a state to screenshot without a real sudo, a dialog, or a message.
    #[cfg(debug_assertions)]
    pub fn dev(&mut self, what: &str) {
        match what {
            "root" => self.is_root = true,
            "goto" => {
                self.dialog = Some(Dialog::GoTo {
                    remote: true,
                    text: "/var/log/".into(),
                    pick: None,
                })
            }
            "sudo" => {
                self.dialog = Some(Dialog::Sudo {
                    password: String::new(),
                    note: None,
                    show: false,
                })
            }
            "sudo-wrong" => {
                self.dialog = Some(Dialog::Sudo {
                    password: String::new(),
                    note: Some(t("Wrong password, try again.").into()),
                    show: false,
                })
            }
            "activity" => self.show_activity = true,
            "hover" => {
                self.dev_hover = vec!["screenshot-checkout.png".into(), "report-q3.pdf".into()]
            }
            "error" => self.error("permission denied"),
            "rename" | "chmod" | "delete" => {
                let mut names: Vec<String> = self.remote.selected.iter().cloned().collect();
                names.sort();
                let dir = self.remote.cwd.clone();
                let Some(first) = names.first().cloned() else {
                    return;
                };
                self.dialog = Some(match what {
                    "rename" => Dialog::Rename {
                        remote: true,
                        dir,
                        from: first.clone(),
                        to: first,
                    },
                    "chmod" => {
                        let mode = self
                            .remote
                            .entries
                            .iter()
                            .find(|e| e.name == first)
                            .and_then(|e| e.mode)
                            .unwrap_or(0o644);
                        Dialog::Chmod {
                            path: remote_join(&dir, &first),
                            original: mode,
                            mode: format!("{mode:o}"),
                        }
                    }
                    _ => {
                        let items = self.delete_items(true, &dir, &names);
                        Dialog::Delete {
                            remote: true,
                            dir,
                            names,
                            items,
                        }
                    }
                });
            }
            "replace" => {
                let mut names: Vec<String> = self.local.selected.iter().cloned().collect();
                names.sort();
                let sources: Vec<String> = names
                    .iter()
                    .map(|n| Path::new(&self.local.cwd).join(n).display().to_string())
                    .collect();
                let conflicts: Vec<String> = names
                    .iter()
                    .filter(|n| self.remote.entries.iter().any(|e| &e.name == *n))
                    .cloned()
                    .collect();
                let dest = self.remote.cwd.clone();
                let items = self.replace_items(true, &sources, &dest, &conflicts, None);
                self.dialog = Some(Dialog::Overwrite {
                    upload: true,
                    sources,
                    dest,
                    conflicts,
                    items,
                });
            }
            _ => {}
        }
        self.focus_dialog = true;
    }

    /// A remote file is open in the editor (it fills the tab's canvas).
    pub fn editor_open(&self) -> bool {
        self.editor.is_some()
    }

    pub fn sftp(&self) -> Option<Sftp> {
        match &self.session {
            Session::Ready(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn remote_cwd(&self) -> &str {
        &self.remote.cwd
    }

    pub fn local_cwd(&self) -> &str {
        &self.local.cwd
    }

    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some() || self.editor.is_some()
    }

    /// ⌘W: closes a dialog, or the editor (asking first if it has unsaved changes).
    pub fn close_dialog(&mut self) {
        if self.dialog.take().is_some() {
            return;
        }
        if self.editor.as_mut().is_some_and(FileEditor::request_close) {
            self.editor = None;
        }
    }

    /// The file open in the editor with unsaved changes, if any.
    pub fn unsaved_file(&self) -> Option<String> {
        self.editor
            .as_ref()
            .filter(|e| e.is_dirty())
            .map(|e| e.name().to_string())
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.session, Session::Ready(_))
    }

    /// The remote side runs as root (sudo).
    pub fn is_root(&self) -> bool {
        self.is_root
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), true));
        self.show_activity = true;
    }

    /// Uploads local files (e.g. dropped from Finder) into the current remote folder.
    pub fn upload_paths(
        &mut self,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        paths: Vec<PathBuf>,
    ) {
        match &self.session {
            Session::Failed(e) => {
                let e = e.clone();
                self.error(tf("SFTP unavailable: {}", &[&e]));
                return;
            }
            Session::Opening(_) => {
                self.pending_uploads.extend(paths);
                self.message = Some((t("Uploads will start when SFTP is ready.").into(), false));
                self.show_activity = true;
                return;
            }
            Session::Ready(_) => {}
        }
        if self.dialog.is_some() {
            self.pending_uploads.extend(paths);
            self.message = Some((
                t("Uploads will start after the open dialog is closed.").into(),
                false,
            ));
            self.show_activity = true;
            return;
        }
        if self.remote.cwd.is_empty() {
            if self.remote.loading {
                self.pending_uploads.extend(paths);
            } else {
                self.error(t("Open a remote folder first."));
            }
            return;
        }
        let sources: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
        let dest = self.remote.cwd.clone();
        self.upload_sources(rt, ctx, sources, dest);
    }

    /// Starts uploads of local paths into `dest`, asking before replacing existing entries.
    fn upload_sources(
        &mut self,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        sources: Vec<String>,
        dest: String,
    ) {
        if dest.is_empty() {
            self.error(t("Open a remote folder first."));
            return;
        }
        let conflicts: Vec<String> = sources
            .iter()
            .filter_map(|s| {
                Path::new(s)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .filter(|n| dest == self.remote.cwd && self.remote.entries.iter().any(|e| &e.name == n))
            .collect();
        if conflicts.is_empty() {
            for src in sources {
                self.start_upload(rt, ctx, PathBuf::from(src), &dest);
            }
        } else {
            let items = self.replace_items(true, &sources, &dest, &conflicts, None);
            self.dialog = Some(Dialog::Overwrite {
                upload: true,
                sources,
                dest,
                conflicts,
                items,
            });
        }
    }

    /// Starts downloads of remote paths into `dest`, asking before replacing existing entries.
    fn download_sources(
        &mut self,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        sources: Vec<String>,
        dest: String,
    ) {
        let conflicts: Vec<String> = sources
            .iter()
            .map(|s| entry::remote_name(s).to_string())
            .filter(|n| Path::new(&dest).join(n).exists())
            .collect();
        if conflicts.is_empty() {
            for src in sources {
                self.start_download(rt, ctx, src, Path::new(&dest));
            }
        } else {
            let items = self.replace_items(false, &sources, &dest, &conflicts, None);
            self.dialog = Some(Dialog::Overwrite {
                upload: false,
                sources,
                dest,
                conflicts,
                items,
            });
        }
    }

    fn local_cd(&mut self, path: &str) {
        let path = paths::expand_tilde(path.trim());
        match entry::list_local(&path) {
            Ok(mut entries) => {
                entry::sort_entries(&mut entries, self.local.sort, self.local.descending);
                self.local.set_listing(path.display().to_string(), entries);
            }
            Err(e) => self.local.error = Some(format!("{}: {e}", path.display())),
        }
    }

    /// A path typed into the local path field: `~` expands, relative paths are relative to
    /// the folder being shown.
    fn resolve_local_input(&self, input: &str) -> String {
        let input = input.trim();
        let p = paths::expand_tilde(input);
        if p.is_absolute() {
            p
        } else {
            Path::new(&self.local.cwd).join(p)
        }
        .display()
        .to_string()
    }

    /// A path typed into the remote path field: `~` means the home folder, relative paths are
    /// relative to the folder being shown.
    fn resolve_remote_input(&self, input: &str) -> String {
        let input = input.trim();
        if input == "~" {
            ".".into()
        } else if let Some(rest) = input.strip_prefix("~/") {
            format!("./{rest}")
        } else if input.starts_with('/') || self.remote.cwd.is_empty() {
            input.to_string()
        } else {
            remote_join(&self.remote.cwd, input)
        }
    }

    fn remote_cd(&mut self, rt: &tokio::runtime::Handle, ctx: &egui::Context, path: String) {
        let Some(sftp) = self.sftp() else { return };
        self.remote.loading = true;
        self.remote_seq += 1;
        let seq = self.remote_seq;
        let tx = self.events_tx.clone();
        let ctx = ctx.clone();
        rt.spawn(async move {
            let resolved = remote::canonicalize(&sftp, &path).await.unwrap_or(path);
            let result = remote::list(&sftp, &resolved).await;
            let _ = tx.send(Event::Listed {
                seq,
                path: resolved,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// Opens a new session on the same connection to switch root on (`Some`, with how to get
    /// past sudo) or off. The working session is kept until the new one is ready, so a failed
    /// switch never drops the working one.
    fn reopen(&mut self, root: Option<RootLogin>) {
        if self.reopening.is_some() {
            return;
        }
        let dir = self.remote.cwd.clone();
        self.reopening = Some(match root {
            Some(login) => Reopen::ToRoot(self.opener.open_root(login), dir),
            None => Reopen::ToUser(self.opener.open(), dir),
        });
        self.remote.loading = true;
    }

    /// Asks for the sudo password (`note`: why it is asked). With another dialog open, says so
    /// instead of replacing it.
    fn ask_sudo_password(&mut self, note: Option<&str>) {
        if self.dialog_open() {
            self.error(tf(
                "Could not switch to root: {}",
                &[note.unwrap_or(t("sudo needs a password."))],
            ));
            return;
        }
        self.dialog = Some(Dialog::Sudo {
            password: String::new(),
            note: note.map(str::to_string),
            show: false,
        });
        self.focus_dialog = true;
    }

    fn run_op(
        &mut self,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        op: impl FnOnce(
            Sftp,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
        > + Send
        + 'static,
    ) {
        let Some(sftp) = self.sftp() else { return };
        let tx = self.events_tx.clone();
        let ctx = ctx.clone();
        rt.spawn(async move {
            let result = op(sftp).await;
            let _ = tx.send(Event::OpDone {
                result,
                refresh_remote: true,
            });
            ctx.request_repaint();
        });
    }

    fn start_upload(
        &mut self,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        local: PathBuf,
        remote_dir: &str,
    ) {
        let Some(sftp) = self.sftp() else { return };
        let tx = self.events_tx.clone();
        let ctx2 = ctx.clone();
        let started = self.transfers.upload(rt, sftp, local, remote_dir, move || {
            let _ = tx.send(Event::OpDone {
                result: Ok(()),
                refresh_remote: true,
            });
            ctx2.request_repaint();
        });
        match started {
            Ok(_) => self.show_activity = true,
            Err(dest) => self.error(tf("{} is already being transferred.", &[&dest])),
        }
    }

    fn start_download(
        &mut self,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        remote_path: String,
        local_dir: &Path,
    ) {
        let Some(sftp) = self.sftp() else { return };
        let tx = self.events_tx.clone();
        let ctx2 = ctx.clone();
        let started = self
            .transfers
            .download(rt, sftp, remote_path, local_dir, move || {
                let _ = tx.send(Event::OpDone {
                    result: Ok(()),
                    refresh_remote: false,
                });
                ctx2.request_repaint();
            });
        match started {
            Ok(_) => self.show_activity = true,
            Err(dest) => self.error(tf("{} is already being transferred.", &[&dest])),
        }
    }

    /// Progress of the current transfer batch.
    pub fn transfer_summary(&self) -> Option<Summary> {
        self.transfers.summary()
    }

    /// A finished transfer batch to announce, once.
    pub fn take_notice(&mut self) -> Option<(String, bool)> {
        self.notice.take()
    }

    /// Processes finished operations and listings; also called while the panel is hidden, so
    /// transfers finishing in the background refresh the lists and are announced.
    /// Shows `path` in the remote list: the folder itself, or the folder a file is in.
    pub fn go_remote(&mut self, path: &str) {
        let path = path.trim();
        if !path.is_empty() {
            self.pending_remote = Some(path.to_string());
        }
    }

    /// Lists the folder of a path typed or selected elsewhere (its parent when it is a file).
    fn remote_go(&mut self, rt: &tokio::runtime::Handle, ctx: &egui::Context, path: String) {
        let Some(sftp) = self.sftp() else { return };
        let path = if path == "~" || path.starts_with("~/") {
            path.replacen('~', ".", 1)
        } else {
            path
        };
        self.remote.loading = true;
        self.remote_seq += 1;
        let seq = self.remote_seq;
        let tx = self.events_tx.clone();
        let ctx = ctx.clone();
        rt.spawn(async move {
            let mut resolved = remote::canonicalize(&sftp, &path).await.unwrap_or(path);
            if !remote::is_dir(&sftp, &resolved).await.unwrap_or(true)
                && let Some((parent, _)) = resolved.rsplit_once('/')
            {
                resolved = if parent.is_empty() {
                    "/".into()
                } else {
                    parent.to_string()
                };
            }
            let result = remote::list(&sftp, &resolved).await;
            let _ = tx.send(Event::Listed {
                seq,
                path: resolved,
                result,
            });
            ctx.request_repaint();
        });
    }

    pub fn poll(&mut self, rt: &tokio::runtime::Handle, ctx: &egui::Context) {
        if let Session::Opening(rx) = &mut self.session {
            match rx.try_recv() {
                Ok(Ok(sftp)) => {
                    self.session = Session::Ready(Arc::new(sftp));
                    let start = if self.start_dir.is_empty() {
                        ".".to_string()
                    } else {
                        self.start_dir.clone()
                    };
                    let start = if start == "~" || start.starts_with("~/") {
                        start.replacen('~', ".", 1)
                    } else {
                        start
                    };
                    self.remote_cd(rt, ctx, start);
                }
                Ok(Err(e)) => self.session = Session::Failed(e),
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.session = Session::Failed(t("Not connected.").into())
                }
            }
        }
        if self.sftp().is_some()
            && let Some(path) = self.pending_remote.take()
        {
            self.remote_go(rt, ctx, path);
        }
        // Switching root on or off: swap to the new session only once it is ready.
        let gone = || t("Not connected.").to_string();
        let reopened = match &mut self.reopening {
            None => None,
            Some(Reopen::ToRoot(rx, _)) => match rx.try_recv() {
                Ok(result) => Some((true, result)),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => {
                    Some((true, Err(RootError::Other(gone()))))
                }
            },
            Some(Reopen::ToUser(rx, _)) => match rx.try_recv() {
                Ok(result) => Some((false, result.map_err(RootError::Other))),
                Err(oneshot::error::TryRecvError::Empty) => None,
                Err(oneshot::error::TryRecvError::Closed) => {
                    Some((false, Err(RootError::Other(gone()))))
                }
            },
        };
        if let Some((to_root, result)) = reopened {
            let dir = match self.reopening.take() {
                Some(Reopen::ToRoot(_, dir) | Reopen::ToUser(_, dir)) => dir,
                None => String::new(),
            };
            self.remote.loading = false;
            match result {
                Ok(sftp) => {
                    self.session = Session::Ready(Arc::new(sftp));
                    self.is_root = to_root;
                    let dir = if dir.is_empty() { ".".to_string() } else { dir };
                    self.remote_cd(rt, ctx, dir);
                }
                Err(RootError::NeedsPassword) => self.ask_sudo_password(None),
                Err(RootError::Denied { login: true }) => self.ask_sudo_password(Some(t(
                    "sudo did not accept your login password. Enter the sudo password.",
                ))),
                Err(RootError::Denied { login: false }) => self.ask_sudo_password(Some(t(
                    "sudo did not accept the password (or does not allow this user).",
                ))),
                Err(RootError::Other(e)) => self.error(if to_root {
                    tf("Could not switch to root: {}", &[&e])
                } else {
                    tf("Could not switch back to your user: {}", &[&e])
                }),
            }
        }
        if matches!(self.session, Session::Failed(_)) {
            self.pending_uploads.clear();
        }
        let mut refresh_local = false;
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                Event::Listed { seq, path, result } if seq == self.remote_seq => {
                    self.remote.loading = false;
                    match result {
                        Ok(mut entries) => {
                            entry::sort_entries(
                                &mut entries,
                                self.remote.sort,
                                self.remote.descending,
                            );
                            if path != self.remote.cwd {
                                // A menu opened on the old listing must not act on the new one.
                                egui::Popup::close_all(ctx);
                            }
                            self.remote.set_listing(path, entries);
                        }
                        Err(e) if self.remote.cwd.is_empty() && path != "." => {
                            // The start folder can't be opened: fall back to the home folder.
                            self.error(tf(
                                "Could not open {} ({}); showing the home folder instead.",
                                &[&path, &e],
                            ));
                            self.remote_cd(rt, ctx, ".".into());
                        }
                        Err(e) => {
                            self.remote.error = Some(e);
                            self.remote.path_edit = self.remote.cwd.clone();
                        }
                    }
                }
                Event::Listed { .. } => {}
                Event::DirsListed { seq, dir, result } => {
                    if seq == self.goto_seq && self.goto_dirs.0 == dir {
                        let mut dirs = result.unwrap_or_default();
                        dirs.sort_by_key(|n| n.to_lowercase());
                        self.goto_dirs.1 = dirs;
                    }
                }
                Event::DestListed {
                    sources,
                    dest,
                    result,
                } => {
                    let existing = result.unwrap_or_default();
                    let conflicts: Vec<String> = sources
                        .iter()
                        .filter_map(|s| {
                            Path::new(s)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                        })
                        .filter(|n| existing.iter().any(|e| &e.name == n))
                        .collect();
                    if conflicts.is_empty() || self.dialog.is_some() {
                        for src in sources {
                            self.start_upload(rt, ctx, PathBuf::from(src), &dest);
                        }
                    } else {
                        let items =
                            self.replace_items(true, &sources, &dest, &conflicts, Some(&existing));
                        self.dialog = Some(Dialog::Overwrite {
                            upload: true,
                            sources,
                            dest,
                            conflicts,
                            items,
                        });
                    }
                }
                Event::OpDone {
                    result,
                    refresh_remote,
                } => {
                    if let Err(e) = result {
                        self.error(e);
                    }
                    self.refresh_remote_pending |= refresh_remote;
                    refresh_local = true;
                }
            }
        }
        // One refresh per frame however many operations finished. A navigation in flight
        // already brings a fresh listing, so it is not interrupted.
        if std::mem::take(&mut self.refresh_remote_pending)
            && !self.remote.loading
            && !self.remote.cwd.is_empty()
        {
            let cwd = self.remote.cwd.clone();
            self.remote_cd(rt, ctx, cwd);
        }
        if refresh_local {
            let cwd = self.local.cwd.clone();
            self.local_cd(&cwd);
        }
        if let Some(s) = self.transfers.summary()
            && !s.running()
            && s.batch != self.reported_batch
        {
            self.reported_batch = s.batch;
            self.notice = batch_notice(&s);
        }
        if self.is_ready()
            && self.dialog.is_none()
            && !self.remote.cwd.is_empty()
            && !self.pending_uploads.is_empty()
        {
            let paths = std::mem::take(&mut self.pending_uploads);
            self.upload_paths(rt, ctx, paths);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        rt: &tokio::runtime::Handle,
        input_enabled: bool,
    ) -> PanelOutput {
        let ctx = ui.ctx().clone();
        self.poll(rt, &ctx);
        let mut output = PanelOutput::None;
        if self.transfers.any_running() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let p = style::pal(ui);
        let panel = ui.max_rect();
        // The header (34 px): what this is and where, then maximize and close.
        let head = egui::Rect::from_min_size(panel.min, vec2(panel.width(), HEAD_H));
        if self.header_ui(ui, head) {
            output = PanelOutput::Close;
        }
        let full = egui::Rect::from_min_max(pos2(panel.left(), head.bottom()), panel.max);

        match &self.session {
            Session::Opening(_) => {
                let mut c = ui.new_child(egui::UiBuilder::new().max_rect(full));
                c.vertical_centered(|ui| {
                    ui.add_space((full.height() * 0.4).max(8.0));
                    ui.horizontal(|ui| {
                        ui.add_space((full.width() - 140.0) / 2.0);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        kit::spinner(ui, 14.0);
                        ui.label(RichText::new(t("Opening SFTP…")).color(p.text_muted));
                    });
                });
                return output;
            }
            Session::Failed(e) => {
                let e = e.clone();
                let mut c = ui.new_child(egui::UiBuilder::new().max_rect(full));
                c.add_space((full.height() * 0.25).max(8.0));
                kit::empty_state(
                    &mut c,
                    icons::WARNING_CIRCLE,
                    t("SFTP unavailable"),
                    &e,
                    |_| {},
                );
                return output;
            }
            Session::Ready(_) => {}
        }

        // The transfer strip along the bottom, the two lists side by side above it, split by
        // one hairline (no buttons between them: drag, the menu or Upload… transfer).
        let strip_h = self.strip_height();
        let lists = egui::Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - strip_h));
        let half = (lists.width() / 2.0).floor();
        let local_rect = egui::Rect::from_min_size(lists.min, vec2(half, lists.height()));
        let remote_rect =
            egui::Rect::from_min_max(pos2(local_rect.right() + 1.0, lists.top()), lists.max);
        self.drop_dir = None;
        let mut local_ui = ui.new_child(egui::UiBuilder::new().max_rect(local_rect));
        let local_actions = self.side_ui(&mut local_ui, false, input_enabled);
        let mut remote_ui = ui.new_child(egui::UiBuilder::new().max_rect(remote_rect));
        let remote_actions = self.side_ui(&mut remote_ui, true, input_enabled);
        ui.painter().vline(
            local_rect.right() + 0.5,
            lists.y_range(),
            Stroke::new(1.0, p.border),
        );
        for a in local_actions.into_iter().chain(remote_actions) {
            if let Some(out) = self.apply(a, rt, &ctx) {
                output = out;
            }
        }
        if strip_h > 0.0 {
            let strip =
                egui::Rect::from_min_max(pos2(full.left(), full.bottom() - strip_h), full.max);
            let mut strip_ui = ui.new_child(egui::UiBuilder::new().max_rect(strip));
            let actions = self.xfer_strip(&mut strip_ui, strip);
            for a in actions {
                self.apply(a, rt, &ctx);
            }
        }
        // Files dragged in from the file manager land in the remote folder that is open (macOS
        // does not say where they are dropped), so the whole panel is the target.
        #[allow(unused_mut)]
        let mut hovered = ctx.input(|i| hovered_names(&i.raw.hovered_files));
        #[cfg(debug_assertions)]
        if hovered.is_empty() {
            hovered = self.dev_hover.clone();
        }
        if !hovered.is_empty() {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new(("sftp-drop", self.uid)),
            ));
            paint_drop_layer(
                &painter,
                panel,
                &p,
                Some(&self.remote.cwd),
                &self.host,
                &hovered,
            );
        }
        self.drag_preview(&ctx);
        if self.show_activity && !self.transfers.items.is_empty() {
            self.activity_popup(&ctx, rt, panel);
        }
        self.dialog_ui(&ctx, rt);
        output
    }

    /// The editor of a remote file, filling `rect` (the tab's canvas). Returns true when it
    /// closed.
    pub fn show_editor(
        &mut self,
        ui: &mut Ui,
        rect: egui::Rect,
        rt: &tokio::runtime::Handle,
    ) -> bool {
        let ctx = ui.ctx().clone();
        self.poll(rt, &ctx);
        let sftp = self.sftp();
        let Some(editor) = self.editor.as_mut() else {
            return true;
        };
        let out = editor.show_in(ui, rect, rt, sftp);
        let path = editor.path.clone();
        match out {
            EditorOutput::Close { saved } => {
                self.editor = None;
                if saved {
                    self.apply(SideAction::Refresh(true), rt, &ctx);
                }
                true
            }
            EditorOutput::Download => {
                self.editor = None;
                let dir = remote_parent(&path);
                let name = entry::remote_name(&path).to_string();
                self.apply(
                    SideAction::Transfer {
                        from_remote: true,
                        dir,
                        names: vec![name],
                    },
                    rt,
                    &ctx,
                );
                true
            }
            EditorOutput::Open => false,
        }
    }

    /// The panel's header: the folder icon, "Files", the SFTP address; maximize and close on
    /// the right. Returns true when × was clicked.
    fn header_ui(&mut self, ui: &mut Ui, head: egui::Rect) -> bool {
        let p = style::pal(ui);
        let painter = ui.painter();
        painter.hline(
            head.x_range(),
            head.bottom() - 0.5,
            Stroke::new(1.0, p.line_soft),
        );
        let cy = head.center().y;
        let mut x = head.left() + 14.0;
        painter.text(
            pos2(x + 7.5, cy),
            Align2::CENTER_CENTER,
            icons::FOLDER_SIMPLE,
            style::icon_font(15.0),
            p.text_muted,
        );
        x += 15.0 + 8.0;
        let title =
            painter.layout_no_wrap(t("Files").to_string(), style::semibold(style::BODY), p.text);
        let tw = title.size().x;
        painter.galley(pos2(x, cy - title.size().y / 2.0), title, p.text);
        x += tw + 8.0;
        let close_r =
            egui::Rect::from_center_size(pos2(head.right() - 8.0 - 12.0, cy), vec2(24.0, 24.0));
        let max_r =
            egui::Rect::from_center_size(pos2(close_r.left() - 8.0 - 12.0, cy), vec2(24.0, 24.0));
        let address = format!("sftp · {}", self.address);
        painter
            .with_clip_rect(egui::Rect::from_min_max(
                pos2(x, head.top()),
                pos2(max_r.left() - 8.0, head.bottom()),
            ))
            .text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                address,
                style::mono(11.0),
                p.text_faint,
            );
        let max_tip = if self.maximized {
            t("Show terminal")
        } else {
            t("Fill the tab")
        };
        let mr = ui
            .interact(max_r, egui::Id::new(("sftp-max", self.uid)), Sense::click())
            .on_hover_text(max_tip);
        kit::paint_icon_button(
            ui,
            &mr,
            max_r,
            if self.maximized {
                icons::ARROWS_IN_SIMPLE
            } else {
                icons::ARROWS_OUT_SIMPLE
            },
            max_tip,
            self.maximized,
            false,
        );
        #[cfg(debug_assertions)]
        crate::devtest::record("sftp-maximize", max_r);
        if mr.clicked() {
            self.maximized = !self.maximized;
        }
        let cr = ui
            .interact(
                close_r,
                egui::Id::new(("sftp-close", self.uid)),
                Sense::click(),
            )
            .on_hover_text(t("Close"));
        kit::paint_icon_button(ui, &cr, close_r, icons::X, t("Close"), false, false);
        cr.clicked()
    }

    /// How tall the transfer strip is: one line while there are transfers or a message.
    fn strip_height(&self) -> f32 {
        if self.transfers.items.is_empty() && self.message.is_none() {
            0.0
        } else {
            STRIP_H
        }
    }

    /// One side: its toolbar (who, Up / Home / Refresh, New folder / Hidden, and on the remote
    /// side Root and Upload…), the root band, the path as a breadcrumb, the column titles and
    /// the files down to the bottom.
    fn side_ui(&mut self, ui: &mut Ui, is_remote: bool, input_enabled: bool) -> Vec<SideAction> {
        let mut actions = Vec::new();
        // The list's keys work while nothing else is asked (a dialog of the panel or the app).
        let keys_enabled = input_enabled && self.dialog.is_none();
        let uid = self.uid;
        let menu_target = self.menu_target.clone().filter(|m| m.remote == is_remote);
        let is_root_now = self.is_root && is_remote;
        let reopening = self.reopening.is_some();
        let host = self.host.clone();
        let user = self.user.clone();
        let glyph = self.glyph;
        let label = self.label;
        let active_side = self.active_remote == is_remote;
        let other_cwd = if is_remote {
            self.local.cwd.clone()
        } else {
            self.remote.cwd.clone()
        };
        let _ = other_cwd;
        let p = style::pal(ui);
        let dark = p.dark;
        let area = ui.max_rect();
        let mut activated = false;
        let side = if is_remote {
            &mut self.remote
        } else {
            &mut self.local
        };
        let show_hidden = side.show_hidden;
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

        // ---- Toolbar (40 px): who, the two button groups, Root and Upload….
        let (bar, _) = ui.allocate_exact_size(vec2(area.width(), 40.0), Sense::hover());
        let cy = bar.center().y;
        let painter = ui.painter().clone();
        let left = bar.left() + 12.0;
        let right_edge = bar.right() - 10.0;
        // The right part first, so the identity takes what is left.
        let mut rx = right_edge;
        let mut upload_rect = None;
        let mut root_rect = None;
        let g1_w = group_width(3);
        let g2_w = group_width(2);
        let groups_w = g1_w + 8.0 + g2_w;
        let lead_w = if is_remote { 20.0 } else { 16.0 };
        // A narrow side: Root and Upload… keep only their icons (with a tooltip).
        let uw_full = text_button_w(ui, t("Upload…"), true, true);
        let rw_full = text_button_w(ui, t("Root"), true, true);
        let compact = is_remote
            && bar.width() - 22.0
                < lead_w + 7.0 + 72.0 + 12.0 + groups_w + 8.0 + rw_full + 8.0 + uw_full;
        if is_remote {
            let uw = if compact { 26.0 } else { uw_full };
            let r = egui::Rect::from_min_max(pos2(rx - uw, cy - 13.0), pos2(rx, cy + 13.0));
            upload_rect = Some(r);
            rx = r.left() - if compact { 4.0 } else { 8.0 };
            let rw = if compact { 26.0 } else { rw_full };
            let r = egui::Rect::from_min_max(pos2(rx - rw, cy - 13.0), pos2(rx, cy + 13.0));
            root_rect = Some(r);
            rx = r.left() - 8.0;
        }
        let (who, sub) = if is_remote {
            (
                host.clone(),
                if is_root_now {
                    "root".to_string()
                } else {
                    user.clone()
                },
            )
        } else {
            (
                t("Local").to_string(),
                crate::platform::this_computer().to_string(),
            )
        };
        let name_g = painter.layout_no_wrap(who, style::semibold(style::BODY), p.text);
        let sub_g = painter.layout_no_wrap(sub, style::regular(style::SMALL), p.text_faint);
        let avail_who = (rx - left - groups_w - 8.0 - 4.0).max(lead_w);
        let full_who = lead_w + 7.0 + name_g.size().x + 7.0 + sub_g.size().x;
        let show_sub = full_who <= avail_who && !sub_g.is_empty();
        let who_w = if show_sub {
            full_who
        } else {
            (lead_w + 7.0 + name_g.size().x).min(avail_who)
        };
        let lead_c = pos2(left + lead_w / 2.0, cy);
        if is_remote {
            let r = egui::Rect::from_center_size(lead_c, vec2(20.0, 20.0));
            kit::paint_host_glyph(
                &painter,
                r,
                glyph,
                label.map(|l| style::label_color(l, dark)),
                None,
                p.canvas,
                &p,
            );
        } else {
            painter.text(
                lead_c,
                Align2::CENTER_CENTER,
                icons::LAPTOP,
                style::icon_font(16.0),
                p.text_muted,
            );
        }
        let name_x = left + lead_w + 7.0;
        let name_w = name_g.size().x;
        painter
            .with_clip_rect(egui::Rect::from_min_max(
                pos2(name_x, bar.top()),
                pos2(left + who_w, bar.bottom()),
            ))
            .galley(pos2(name_x, cy - name_g.size().y / 2.0), name_g, p.text);
        if show_sub {
            painter.galley(
                pos2(name_x + name_w + 7.0, cy - sub_g.size().y / 2.0),
                sub_g,
                p.text_faint,
            );
        }
        // Up, Home, Refresh | New folder, Hidden.
        let gx = left + who_w + 4.0 + 8.0;
        let g1 = egui::Rect::from_min_size(pos2(gx, cy - 13.0), vec2(g1_w, 26.0));
        let g2 = egui::Rect::from_min_size(pos2(g1.right() + 8.0, cy - 13.0), vec2(g2_w, 26.0));
        #[cfg(debug_assertions)]
        {
            let side_name = if is_remote { "remote" } else { "local" };
            for (i, n) in ["up", "home", "refresh"].iter().enumerate() {
                crate::devtest::record(
                    format!("{side_name}-{n}"),
                    egui::Rect::from_min_size(
                        pos2(g1.left() + 2.0 + 25.0 * i as f32, g1.top() + 2.0),
                        vec2(24.0, 22.0),
                    ),
                );
            }
            crate::devtest::record(
                format!("{side_name}-mkdir"),
                egui::Rect::from_min_size(pos2(g2.left() + 2.0, g2.top() + 2.0), vec2(24.0, 22.0)),
            );
            crate::devtest::record(
                format!("{side_name}-path"),
                egui::Rect::from_min_size(pos2(left, bar.bottom()), vec2(200.0, 28.0)),
            );
        }
        let r = tool_group(
            ui,
            g1,
            egui::Id::new(("sftp-g1", uid, is_remote)),
            &[
                (icons::ARROW_UP, t("Parent folder"), false, false),
                (icons::HOUSE, t("Home folder"), false, false),
                (icons::ARROW_CLOCKWISE, t("Refresh"), false, side.loading),
            ],
        );
        if r[0].clicked() {
            actions.push(SideAction::Up(is_remote));
        }
        if r[1].clicked() {
            actions.push(SideAction::Home(is_remote));
        }
        if r[2].clicked() {
            actions.push(SideAction::Refresh(is_remote));
        }
        let r = tool_group(
            ui,
            g2,
            egui::Id::new(("sftp-g2", uid, is_remote)),
            &[
                (icons::FOLDER_PLUS, t("New folder"), false, false),
                (
                    if show_hidden {
                        icons::EYE
                    } else {
                        icons::EYE_SLASH
                    },
                    if show_hidden {
                        t("Hide hidden files")
                    } else {
                        t("Show hidden files")
                    },
                    show_hidden,
                    false,
                ),
            ],
        );
        #[cfg(debug_assertions)]
        crate::devtest::record(
            if is_remote {
                "remote-hidden"
            } else {
                "local-hidden"
            },
            r[1].rect,
        );
        if r[0].clicked() {
            actions.push(SideAction::Mkdir(is_remote));
        }
        if r[1].clicked() {
            actions.push(SideAction::ToggleHidden(is_remote));
        }
        if let (Some(rr), Some(ur)) = (root_rect, upload_rect) {
            let root = ui
                .add_enabled_ui(!reopening, |ui| {
                    let resp =
                        ui.interact(rr, egui::Id::new(("sftp-root-btn", uid)), Sense::click());
                    paint_root_button(ui, &resp, rr, is_root_now, compact);
                    resp
                })
                .inner
                .on_hover_text(if is_root_now {
                    t("Running as root — click to go back to your user")
                } else {
                    t("Browse and transfer as root (sudo)")
                });
            #[cfg(debug_assertions)]
            crate::devtest::record("sftp-root", root.rect);
            if root.clicked() {
                actions.push(SideAction::Root);
            }
            let mut b = ui.new_child(egui::UiBuilder::new().max_rect(ur));
            let upload = if compact {
                kit::icon_button_ex(&mut b, icons::UPLOAD_SIMPLE, "", false, 26.0)
            } else {
                kit::button_ex(
                    &mut b,
                    kit::Kind::Secondary,
                    Some(icons::UPLOAD_SIMPLE),
                    t("Upload…"),
                    true,
                )
            }
            .on_hover_text(tf(
                "Choose files on {} to upload into this folder",
                &[crate::platform::this_computer()],
            ));
            #[cfg(debug_assertions)]
            crate::devtest::record("upload-button", upload.rect);
            egui::Popup::menu(&upload).show(|ui| {
                kit::menu_body(ui);
                ui.set_min_width(180.0);
                if kit::menu_item(ui, Some(icons::FILES), t("Files…"), None).clicked() {
                    actions.push(SideAction::PickUpload { folders: false });
                    ui.close();
                }
                if kit::menu_item(ui, Some(icons::FOLDERS), t("Folders…"), None).clicked() {
                    actions.push(SideAction::PickUpload { folders: true });
                    ui.close();
                }
            });
        }

        // ---- Root: what the side runs as, with the way back (30 px, then 6 below).
        if is_root_now {
            let (r, _) = ui.allocate_exact_size(vec2(area.width(), 36.0), Sense::hover());
            let band = egui::Rect::from_min_max(
                pos2(r.left() + 10.0, r.top()),
                pos2(r.right() - 10.0, r.top() + 30.0),
            );
            let painter = ui.painter();
            painter.rect_filled(
                band,
                CornerRadius::same(style::RADIUS),
                p.tone_soft(Tone::Root),
            );
            painter.rect_stroke(
                band,
                CornerRadius::same(style::RADIUS),
                Stroke::new(1.0, style::soft(p.root, 0.28)),
                StrokeKind::Inside,
            );
            painter.text(
                pos2(band.left() + 10.0 + 7.0, band.center().y),
                Align2::CENTER_CENTER,
                icons::SHIELD_WARNING,
                style::icon_font(14.0),
                p.root,
            );
            let back = tf(
                "Back to {}",
                &[if user.is_empty() {
                    t("your user")
                } else {
                    user.as_str()
                }],
            );
            let back_g = painter.layout_no_wrap(back, style::medium(style::SMALL), p.root);
            let back_r = egui::Rect::from_min_max(
                pos2(
                    band.right() - 6.0 - back_g.size().x - 18.0,
                    band.center().y - 11.0,
                ),
                pos2(band.right() - 6.0, band.center().y + 11.0),
            );
            let mut job = egui::text::LayoutJob::default();
            let f = |font: egui::FontId, color: egui::Color32| egui::TextFormat {
                font_id: font,
                color,
                ..Default::default()
            };
            job.append(
                t("Browsing as root (sudo)"),
                0.0,
                f(style::semibold(style::SMALL), p.root),
            );
            job.append(
                &format!(
                    " · {}",
                    tf(
                        "the terminal stays as {}",
                        &[if user.is_empty() {
                            t("your user")
                        } else {
                            user.as_str()
                        }]
                    )
                ),
                0.0,
                f(
                    style::regular(style::SMALL),
                    blend(p.root, p.text_muted, 0.3),
                ),
            );
            job.wrap.max_width = (back_r.left() - 8.0 - (band.left() + 30.0)).max(10.0);
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let g = painter.layout_job(job);
            painter.galley(
                pos2(band.left() + 30.0, band.center().y - g.size().y / 2.0),
                g,
                p.root,
            );
            let br = ui.interact(
                back_r,
                egui::Id::new(("sftp-root-back", uid)),
                Sense::click(),
            );
            if br.hovered() {
                ui.painter().rect_filled(
                    back_r,
                    CornerRadius::same(style::RADIUS),
                    p.tone_soft(Tone::Root),
                );
            }
            ui.painter()
                .galley(back_r.center() - back_g.size() / 2.0, back_g, p.root);
            if br.clicked() {
                actions.push(SideAction::Root);
            }
        }

        // ---- The path: a breadcrumb to click, or a field to type a path into (28, then 6).
        let (r, _) = ui.allocate_exact_size(vec2(area.width(), 34.0), Sense::hover());
        let path_r = egui::Rect::from_min_max(
            pos2(r.left() + 10.0, r.top()),
            pos2(r.right() - 10.0, r.top() + 28.0),
        );
        let edit_id = egui::Id::new(("sftp-path", uid, is_remote));
        if side.editing {
            // The same bar as the breadcrumb (icon, then the path where the crumbs start), with
            // a focus ring and a small "go" arrow on the right; Enter goes, Esc cancels.
            let cy = path_r.center().y;
            ui.painter()
                .rect_filled(path_r, CornerRadius::same(style::RADIUS), p.field);
            style::focus_halo(ui, path_r, style::RADIUS);
            ui.painter().text(
                pos2(path_r.left() + 8.0 + 7.0, cy),
                Align2::CENTER_CENTER,
                if is_remote {
                    icons::HARD_DRIVES
                } else {
                    icons::LAPTOP
                },
                style::icon_font(14.0),
                p.text_faint,
            );
            let go_r = egui::Rect::from_center_size(
                pos2(path_r.right() - 4.0 - 10.0, cy),
                vec2(20.0, 20.0),
            );
            let go = ui
                .interact(go_r, edit_id.with("go"), Sense::click())
                .on_hover_text(t("Go to this folder"));
            if go.hovered() {
                ui.painter()
                    .rect_filled(go_r, CornerRadius::same(5), p.hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            ui.painter().text(
                go_r.center(),
                Align2::CENTER_CENTER,
                icons::ARROW_ELBOW_DOWN_LEFT,
                style::icon_font(13.0),
                if go.hovered() { p.text } else { p.text_faint },
            );
            let field_r = egui::Rect::from_min_max(
                pos2(path_r.left() + 8.0 + 14.0 + 4.0, path_r.top()),
                pos2(go_r.left() - 4.0, path_r.bottom()),
            );
            let mut field_ui = ui.new_child(egui::UiBuilder::new().max_rect(field_r));
            let r = field_ui.add_sized(
                field_r.size(),
                egui::TextEdit::singleline(&mut side.path_edit)
                    .id(edit_id)
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(5, 0)))
                    .font(style::mono(12.0))
                    .vertical_align(egui::Align::Center),
            );
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.lost_focus() {
                side.editing = false;
                // Pressing the arrow takes the focus from the field before it counts as a click.
                let on_go = ui
                    .input(|i| i.pointer.interact_pos())
                    .is_some_and(|pos| go_r.contains(pos));
                if on_go || field_ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    actions.push(SideAction::Cd(is_remote, side.path_edit.clone()));
                } else {
                    side.path_edit = side.cwd.clone();
                }
            }
        } else {
            let crumb_resp = ui
                .interact(
                    path_r,
                    egui::Id::new(("sftp-crumbs", uid, is_remote)),
                    Sense::click(),
                )
                .on_hover_text(t("Click to type a path"));
            let hovered = crumb_resp.hovered();
            let painter = ui.painter();
            painter.rect_filled(
                path_r,
                CornerRadius::same(style::RADIUS),
                if dark {
                    p.hover
                } else {
                    style::soft(egui::Color32::BLACK, 0.04)
                },
            );
            if is_root_now {
                painter.rect_stroke(
                    path_r,
                    CornerRadius::same(style::RADIUS),
                    Stroke::new(1.0, style::soft(p.root, 0.30)),
                    StrokeKind::Inside,
                );
            }
            painter.text(
                pos2(path_r.left() + 8.0 + 7.0, path_r.center().y),
                Align2::CENTER_CENTER,
                if is_remote {
                    icons::HARD_DRIVES
                } else {
                    icons::LAPTOP
                },
                style::icon_font(14.0),
                p.text_faint,
            );
            // The hint takes room on the right only while the path is hovered.
            let hint_w = if hovered {
                let g = painter.layout_no_wrap(
                    t("click to type a path").to_string(),
                    style::regular(style::XS),
                    p.text_faint,
                );
                let w = g.size().x;
                painter.galley(
                    pos2(
                        path_r.right() - 8.0 - w,
                        path_r.center().y - g.size().y / 2.0,
                    ),
                    g,
                    p.text_faint,
                );
                w + 16.0
            } else {
                8.0
            };
            let segs = crumb_segments(&side.cwd, is_remote);
            let font = style::regular(style::SMALL);
            let font_on = style::medium(style::SMALL);
            let x0 = path_r.left() + 8.0 + 14.0 + 4.0;
            let avail = path_r.right() - hint_w - x0;
            let sep_w = 10.0 + 4.0;
            let widths: Vec<f32> = segs
                .iter()
                .enumerate()
                .map(|(i, (name, _))| {
                    let f = if i + 1 == segs.len() {
                        font_on.clone()
                    } else {
                        font.clone()
                    };
                    painter.layout_no_wrap(name.clone(), f, p.text).size().x + 10.0
                })
                .collect();
            let mut first = 0;
            let total = |from: usize| -> f32 {
                widths[from..].iter().sum::<f32>()
                    + sep_w * (segs.len() - from).saturating_sub(1) as f32
                    + if from > 0 { 14.0 + sep_w } else { 0.0 }
            };
            while first + 1 < segs.len() && total(first) > avail {
                first += 1;
            }
            let mut x = x0;
            if first > 0 {
                painter.text(
                    pos2(x + 5.0, path_r.center().y),
                    Align2::LEFT_CENTER,
                    "…",
                    font.clone(),
                    p.text_faint,
                );
                x += 14.0;
            }
            for (i, (name, path)) in segs.iter().enumerate().skip(first) {
                if i > first || first > 0 {
                    painter.text(
                        pos2(x + 7.0, path_r.center().y),
                        Align2::CENTER_CENTER,
                        icons::CARET_RIGHT,
                        style::icon_font(10.0),
                        p.text_ghost,
                    );
                    x += sep_w;
                }
                let last = i + 1 == segs.len();
                let w = widths[i].min((path_r.right() - hint_w - x).max(0.0));
                let seg =
                    egui::Rect::from_min_size(pos2(x, path_r.center().y - 10.0), vec2(w, 20.0));
                let r = ui.interact(
                    seg,
                    egui::Id::new(("crumb", uid, is_remote, i)),
                    Sense::click(),
                );
                if r.hovered() {
                    ui.painter()
                        .rect_filled(seg, CornerRadius::same(5), p.hover);
                }
                let (f, c) = if last {
                    (font_on.clone(), p.text)
                } else if r.hovered() {
                    (font.clone(), p.text)
                } else {
                    (font.clone(), p.text_muted)
                };
                ui.painter().with_clip_rect(seg).text(
                    pos2(seg.left() + 5.0, seg.center().y),
                    Align2::LEFT_CENTER,
                    name,
                    f,
                    c,
                );
                if r.clicked() {
                    actions.push(SideAction::Cd(is_remote, path.clone()));
                }
                x += w;
            }
            if crumb_resp.clicked() {
                side.editing = true;
                side.path_edit = side.cwd.clone();
            }
        }
        if let Some(err) = &side.error {
            let (r, _) = ui.allocate_exact_size(vec2(area.width(), 22.0), Sense::hover());
            ui.painter().text(
                pos2(r.left() + 12.0 + 7.0, r.center().y - 3.0),
                Align2::CENTER_CENTER,
                icons::WARNING_CIRCLE,
                style::icon_font(14.0),
                p.err,
            );
            ui.painter()
                .with_clip_rect(r.shrink2(vec2(12.0, 0.0)))
                .text(
                    pos2(r.left() + 12.0 + 20.0, r.center().y - 3.0),
                    Align2::LEFT_CENTER,
                    err,
                    style::regular(style::SMALL),
                    p.err,
                );
        }

        // ---- Column titles (28 px over a faint hairline).
        let (cols, _) = ui.allocate_exact_size(vec2(area.width(), 28.0), Sense::hover());
        ui.painter().hline(
            cols.x_range(),
            cols.bottom() - 0.5,
            Stroke::new(1.0, p.line_soft),
        );
        let head_cols = Columns::new(cols.left() + 18.0, cols.right() - 18.0, is_remote);
        let mut sort_clicked = None;
        let titles: [(&str, Option<SortKey>, bool); 4] = [
            (t("Name"), Some(SortKey::Name), false),
            (t("Size"), Some(SortKey::Size), true),
            (t("Modified"), Some(SortKey::Modified), false),
            (t("Permissions"), None, false),
        ];
        for (i, (title, key, right_align)) in titles.iter().enumerate() {
            if !head_cols.shown(i) {
                continue;
            }
            let (x0, x1) = head_cols.span(i);
            let cell = egui::Rect::from_min_max(pos2(x0, cols.top()), pos2(x1, cols.bottom()));
            let sorted = key.is_some_and(|k| k == side.sort);
            let r = ui.interact(
                cell,
                egui::Id::new(("sftp-col", uid, is_remote, i)),
                if key.is_some() {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            let color = if sorted || (key.is_some() && r.hovered()) {
                p.text
            } else {
                p.text_faint
            };
            let g = ui
                .painter()
                .layout_no_wrap(title.to_string(), style::medium(style::XS), color);
            let caret_w = if sorted { 4.0 + 10.0 } else { 0.0 };
            let tx = if *right_align {
                x1 - g.size().x - caret_w
            } else {
                x0
            };
            let gw = g.size().x;
            ui.painter()
                .galley(pos2(tx, cols.center().y - g.size().y / 2.0), g, color);
            if sorted {
                ui.painter().text(
                    pos2(tx + gw + 4.0 + 5.0, cols.center().y),
                    Align2::CENTER_CENTER,
                    if side.descending {
                        icons::CARET_DOWN
                    } else {
                        icons::CARET_UP
                    },
                    style::icon_font(10.0),
                    color,
                );
            }
            if r.clicked()
                && let Some(k) = key
            {
                sort_clicked = Some(*k);
            }
        }
        if let Some(k) = sort_clicked {
            actions.push(SideAction::Sort(is_remote, k));
        }

        // ---- The files, down to the bottom (rows 27 px, 6 px in from the sides, 4 above).
        let row_h = style::row_h() - 1.0;
        let visible: Vec<usize> = side
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| show_hidden || !e.is_hidden())
            .map(|(i, _)| i)
            .collect();
        let list_rect = egui::Rect::from_min_max(pos2(area.left(), ui.cursor().top()), area.max);
        // A click in the list gives it the keyboard (Esc gives it back to the terminal).
        let list_id = egui::Id::new(("sftp-list-keys", uid, is_remote));
        let list_bg = ui.interact(list_rect, list_id, Sense::click());
        if list_bg.clicked() || list_bg.secondary_clicked() {
            ui.memory_mut(|m| m.request_focus(list_id));
            activated = true;
        }
        list_bg.context_menu(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(220.0);
            if kit::menu_item(ui, Some(icons::FOLDER_PLUS), t("New folder"), None).clicked() {
                actions.push(SideAction::Mkdir(is_remote));
                ui.close();
            }
            if kit::menu_item(
                ui,
                Some(icons::ARROW_BEND_DOWN_RIGHT),
                t("Go to folder…"),
                Some(if cfg!(target_os = "macos") {
                    "⇧⌘G"
                } else {
                    "Ctrl+Shift+G"
                }),
            )
            .clicked()
            {
                actions.push(SideAction::GoTo(is_remote));
                ui.close();
            }
            let hidden_label = if show_hidden {
                t("Hide hidden files")
            } else {
                t("Show hidden files")
            };
            if kit::menu_item(
                ui,
                Some(if show_hidden {
                    icons::EYE_SLASH
                } else {
                    icons::EYE
                }),
                hidden_label,
                None,
            )
            .clicked()
            {
                actions.push(SideAction::ToggleHidden(is_remote));
                ui.close();
            }
            kit::menu_sep(ui);
            if kit::menu_item(ui, Some(icons::ARROW_CLOCKWISE), t("Refresh"), None).clicked() {
                actions.push(SideAction::Refresh(is_remote));
                ui.close();
            }
            if kit::menu_item(
                ui,
                Some(icons::SELECTION_ALL),
                t("Select All"),
                Some(if cfg!(target_os = "macos") {
                    "⌘A"
                } else {
                    "Ctrl+A"
                }),
            )
            .clicked()
            {
                actions.push(SideAction::SelectAll(is_remote));
                ui.close();
            }
        });
        let payload = egui::DragAndDrop::payload::<DragFiles>(ui.ctx());
        let drag_in = payload.as_ref().is_some_and(|d| d.from_remote != is_remote);
        let over_list = list_bg.contains_pointer();
        let mut drop_here: Option<String> = None;
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        let row_cols = Columns::new(list_rect.left() + 12.0, list_rect.right() - 12.0, is_remote);
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 6,
                right: 6,
                top: 4,
                bottom: 0,
            })
            .show(&mut list_ui, |ui| {
                ui.set_min_size(vec2(list_rect.width() - 12.0, list_rect.height() - 4.0));
                if visible.is_empty() && !side.loading {
                    ui.add_space((list_rect.height() * 0.18).max(4.0));
                    let body = if is_remote {
                        t("Drop files here or use Upload.")
                    } else {
                        t("Nothing to show here.")
                    };
                    kit::empty_state(
                        ui,
                        icons::FOLDER_OPEN,
                        t("This folder is empty"),
                        body,
                        |_| {},
                    );
                    return;
                }
                kit::show_rows(
                    egui::ScrollArea::vertical()
                        .id_salt(("sftp-list", uid, is_remote))
                        .auto_shrink([false, false]),
                    ui,
                    row_h,
                    visible.len(),
                    |ui, range| {
                        for row in range {
                            let e = &side.entries[visible[row]];
                            let selected = side.selected.contains(&e.name);
                            let (rect, resp) = ui.allocate_exact_size(
                                vec2(ui.available_width(), row_h),
                                Sense::click_and_drag(),
                            );
                            let drop_row = drag_in && e.is_dir && resp.contains_pointer();
                            if drop_row {
                                drop_here = Some(e.name.clone());
                            }
                            let painter = ui.painter();
                            let radius = CornerRadius::same(style::RADIUS);
                            if drop_row {
                                painter.rect_filled(rect, radius, p.accent_softer);
                                painter.rect_stroke(
                                    rect,
                                    radius,
                                    Stroke::new(1.5, p.accent_line),
                                    StrokeKind::Inside,
                                );
                            } else if selected {
                                painter.rect_filled(
                                    rect,
                                    radius,
                                    if active_side {
                                        p.accent_tint
                                    } else {
                                        p.accent_soft
                                    },
                                );
                            } else if resp.hovered() {
                                painter.rect_filled(rect, radius, p.hover);
                            }
                            paint_file_row(painter, rect, e, &row_cols, is_remote, selected, &p);
                            #[cfg(debug_assertions)]
                            crate::devtest::record(
                                format!(
                                    "{}-row:{}",
                                    if is_remote { "remote" } else { "local" },
                                    e.name
                                ),
                                rect,
                            );
                            // The names a drag or menu acts on: the selection if this row is
                            // part of it, otherwise just this row.
                            let names_here = || -> Vec<String> {
                                if side.selected.contains(&e.name) {
                                    let mut v: Vec<String> =
                                        side.selected.iter().cloned().collect();
                                    v.sort();
                                    v
                                } else {
                                    vec![e.name.clone()]
                                }
                            };
                            if resp.clicked()
                                || resp.double_clicked()
                                || resp.secondary_clicked()
                                || resp.drag_started()
                            {
                                resp.ctx.memory_mut(|m| m.request_focus(list_id));
                                activated = true;
                            }
                            if resp.drag_started() {
                                // The row under the pointer leads (its card is on top).
                                let mut names = names_here();
                                if let Some(i) = names.iter().position(|n| *n == e.name) {
                                    let lead = names.remove(i);
                                    names.insert(0, lead);
                                }
                                let first = names
                                    .first()
                                    .and_then(|n| side.entries.iter().find(|x| &x.name == n));
                                actions.push(SideAction::DragStarted(names[0].clone()));
                                egui::DragAndDrop::set_payload(
                                    &resp.ctx,
                                    DragFiles {
                                        from_remote: is_remote,
                                        dir: side.cwd.clone(),
                                        first_kind: first.map_or(icons::FileKind::Other, |f| {
                                            icons::FileKind::of(
                                                &f.name,
                                                f.is_dir,
                                                f.is_symlink,
                                                f.mode.is_some_and(|m| m & 0o111 != 0),
                                            )
                                        }),
                                        names,
                                    },
                                );
                            }
                            if resp.double_clicked() {
                                // A remote file opens in the editor; folders open, and local
                                // files upload.
                                actions.push(if is_remote && !e.is_dir {
                                    SideAction::Edit {
                                        dir: side.cwd.clone(),
                                        name: e.name.clone(),
                                    }
                                } else {
                                    SideAction::Open {
                                        remote: is_remote,
                                        dir: side.cwd.clone(),
                                        entry: e.clone(),
                                    }
                                });
                            } else if resp.clicked() {
                                let mods = ui_mods(&resp);
                                actions.push(SideAction::Select(
                                    is_remote,
                                    e.name.clone(),
                                    mods.0,
                                    mods.1,
                                ));
                            }
                            if resp.secondary_clicked() {
                                actions.push(SideAction::MenuOpened(MenuTarget {
                                    remote: is_remote,
                                    dir: side.cwd.clone(),
                                    names: names_here(),
                                    entry: e.clone(),
                                }));
                            }
                            resp.context_menu(|ui| {
                                kit::menu_body(ui);
                                ui.set_min_width(226.0);
                                // Act on what was right-clicked, even if the listing changed
                                // since.
                                let target = menu_target.clone().unwrap_or_else(|| MenuTarget {
                                    remote: is_remote,
                                    dir: side.cwd.clone(),
                                    names: names_here(),
                                    entry: e.clone(),
                                });
                                file_menu(ui, &target, is_remote, &mut actions);
                            });
                        }
                    },
                );
            });
        // Files dragged from the other side: onto a folder of this list, or into the folder it
        // shows (the whole list lights up).
        if drag_in && over_list && drop_here.is_none() {
            let r = list_rect.shrink(4.0);
            ui.painter()
                .rect_filled(r, CornerRadius::same(style::RADIUS_MD), p.accent_softer);
            ui.painter().rect_stroke(
                r,
                CornerRadius::same(style::RADIUS_MD),
                Stroke::new(1.5, p.accent_line),
                StrokeKind::Inside,
            );
        }
        let dropped = list_bg.dnd_release_payload::<DragFiles>();
        // The keys of the list that has the keyboard.
        let list_focused = ui.memory(|m| m.has_focus(list_id));
        if list_focused {
            activated = true;
            // Arrows and Tab stay here instead of moving the keyboard to another widget.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    list_id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }
        if list_focused && keys_enabled && !side.editing {
            use super::keys::FileKey;
            let mut names: Vec<String> = side.selected.iter().cloned().collect();
            names.sort();
            let one = match names.as_slice() {
                [name] => side.entries.iter().find(|e| &e.name == name).cloned(),
                _ => None,
            };
            let transfer_key = if is_remote {
                FileKey::Download
            } else {
                FileKey::Upload
            };
            let (open, rename, delete, copy_path, transfer, escape, goto) = ui.input_mut(|i| {
                (
                    FileKey::Open.take(i),
                    FileKey::Rename.take(i),
                    FileKey::Delete.take(i),
                    FileKey::CopyPath.take(i),
                    transfer_key.take(i),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                    i.consume_key(
                        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                        egui::Key::G,
                    ),
                )
            });
            let dir = side.cwd.clone();
            if goto {
                actions.push(SideAction::GoTo(is_remote));
            }
            if let Some(e) = one.clone().filter(|_| open) {
                // Like a double click, but a local file is not uploaded by Enter.
                if e.is_dir {
                    actions.push(SideAction::Open {
                        remote: is_remote,
                        dir: dir.clone(),
                        entry: e,
                    });
                } else if is_remote {
                    actions.push(SideAction::Edit {
                        dir: dir.clone(),
                        name: e.name,
                    });
                }
            } else if let Some(e) = one.filter(|_| rename) {
                actions.push(SideAction::Rename {
                    remote: is_remote,
                    dir: dir.clone(),
                    name: e.name,
                });
            } else if delete && !names.is_empty() {
                actions.push(SideAction::Delete {
                    remote: is_remote,
                    dir: dir.clone(),
                    names: names.clone(),
                });
            } else if transfer && !names.is_empty() {
                actions.push(SideAction::Transfer {
                    from_remote: is_remote,
                    dir: dir.clone(),
                    names: names.clone(),
                });
            }
            if copy_path && !names.is_empty() {
                let paths: Vec<String> = names
                    .iter()
                    .map(|n| {
                        if is_remote {
                            remote_join(&dir, n)
                        } else {
                            Path::new(&dir).join(n).display().to_string()
                        }
                    })
                    .collect();
                ui.ctx().copy_text(paths.join("\n"));
            }
            if escape {
                ui.memory_mut(|m| m.surrender_focus(list_id));
            }
        }
        // ⌘A over the list (or in it) selects everything shown.
        if (over_list || list_focused)
            && !side.editing
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A))
        {
            actions.push(SideAction::SelectAll(is_remote));
        }
        if let Some(drop) = dropped
            && drop.from_remote != is_remote
        {
            match &drop_here {
                Some(into) => actions.push(SideAction::TransferInto {
                    from_remote: drop.from_remote,
                    dir: drop.dir.clone(),
                    names: drop.names.clone(),
                    into: into.clone(),
                }),
                None => actions.push(SideAction::Transfer {
                    from_remote: drop.from_remote,
                    dir: drop.dir.clone(),
                    names: drop.names.clone(),
                }),
            }
        }
        if drag_in && over_list {
            self.drop_dir = Some((is_remote, drop_here.unwrap_or_default()));
        }
        if activated {
            self.active_remote = is_remote;
        }
        actions
    }

    fn apply(
        &mut self,
        action: SideAction,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
    ) -> Option<PanelOutput> {
        match action {
            SideAction::DragStarted(label) => self.drag_label = Some(label),
            SideAction::GoTo(remote) => {
                let side = if remote { &self.remote } else { &self.local };
                let mut text = side.cwd.clone();
                if !text.ends_with('/') {
                    text.push('/');
                }
                self.goto_dirs = (String::new(), Vec::new());
                self.dialog = Some(Dialog::GoTo {
                    remote,
                    text,
                    pick: None,
                });
                self.focus_dialog = true;
            }
            SideAction::TransferInto {
                from_remote: true,
                dir,
                names,
                into,
            } => {
                let sources: Vec<String> = names.iter().map(|n| remote_join(&dir, n)).collect();
                let dest = Path::new(&self.local.cwd).join(into).display().to_string();
                self.download_sources(rt, ctx, sources, dest);
            }
            SideAction::TransferInto {
                from_remote: false,
                dir,
                names,
                into,
            } => {
                // Ask before replacing anything in that folder: list it first.
                let sources: Vec<String> = names
                    .iter()
                    .map(|n| Path::new(&dir).join(n).display().to_string())
                    .collect();
                let dest = remote_join(&self.remote.cwd, &into);
                if let Some(sftp) = self.sftp() {
                    let (tx, ctx2) = (self.events_tx.clone(), ctx.clone());
                    rt.spawn(async move {
                        let result = remote::list(&sftp, &dest).await;
                        let _ = tx.send(Event::DestListed {
                            sources,
                            dest,
                            result,
                        });
                        ctx2.request_repaint();
                    });
                }
            }
            SideAction::RemoveTransfer(id) => self.transfers.items.retain(|t| t.id != id),
            SideAction::Retry(id) => {
                if let Some(tr) = self.transfers.items.iter().find(|t| t.id == id).cloned() {
                    self.transfers.items.retain(|t| t.id != id);
                    match tr.direction {
                        Direction::Upload => {
                            let dir = remote_parent(&tr.remote);
                            self.start_upload(rt, ctx, tr.local.clone(), &dir);
                        }
                        Direction::Download => {
                            if let Some(dir) = tr.local.parent() {
                                let dir = dir.to_path_buf();
                                self.start_download(rt, ctx, tr.remote.clone(), &dir);
                            }
                        }
                    }
                }
            }
            SideAction::MenuOpened(target) => {
                let side = if target.remote {
                    &mut self.remote
                } else {
                    &mut self.local
                };
                if !side.selected.contains(&target.entry.name) {
                    side.selected = HashSet::from([target.entry.name.clone()]);
                    side.anchor = Some(target.entry.name.clone());
                }
                self.menu_target = Some(target);
            }
            SideAction::Root => {
                // Passwordless sudo, or the login password; the password is asked for only
                // when neither gets past sudo.
                self.reopen((!self.is_root).then_some(RootLogin::Auto));
            }
            SideAction::ToggleHidden(remote) => {
                let side = if remote {
                    &mut self.remote
                } else {
                    &mut self.local
                };
                side.show_hidden = !side.show_hidden;
                if !side.show_hidden {
                    side.selected.retain(|n| !n.starts_with('.'));
                }
            }
            SideAction::Up(true) => {
                if !self.remote.cwd.is_empty() {
                    let p = remote_parent(&self.remote.cwd);
                    self.remote_cd(rt, ctx, p);
                }
            }
            SideAction::Up(false) => {
                if let Some(p) = Path::new(&self.local.cwd)
                    .parent()
                    .map(|p| p.display().to_string())
                {
                    self.local_cd(&p);
                    return Some(PanelOutput::LocalDir(self.local.cwd.clone()));
                }
            }
            SideAction::Home(true) => self.remote_cd(rt, ctx, ".".into()),
            SideAction::Home(false) => {
                self.local_cd("~");
                return Some(PanelOutput::LocalDir(self.local.cwd.clone()));
            }
            SideAction::PickUpload { folders } => {
                let picker = rfd::FileDialog::new()
                    .set_title(tf("Upload to {}", &[&self.remote.cwd]))
                    .set_directory(&self.local.cwd);
                let picked = if folders {
                    picker.pick_folders()
                } else {
                    picker.pick_files()
                };
                if let Some(paths) = picked.filter(|p| !p.is_empty()) {
                    self.upload_paths(rt, ctx, paths);
                }
            }
            SideAction::SelectAll(remote) => {
                let side = if remote {
                    &mut self.remote
                } else {
                    &mut self.local
                };
                side.selected = side
                    .visible(side.show_hidden)
                    .map(|e| e.name.clone())
                    .collect();
                side.anchor = None;
            }
            SideAction::Edit { dir, name } => match self.sftp() {
                Some(sftp) => {
                    self.editor = Some(
                        FileEditor::open(rt, ctx, sftp, remote_join(&dir, &name))
                            .with_host(self.host.clone()),
                    );
                }
                None => self.error(t("Not connected.")),
            },
            SideAction::Refresh(true) => {
                if !self.remote.cwd.is_empty() {
                    let cwd = self.remote.cwd.clone();
                    self.remote_cd(rt, ctx, cwd);
                }
            }
            SideAction::Refresh(false) => {
                let cwd = self.local.cwd.clone();
                self.local_cd(&cwd);
            }
            SideAction::Cd(true, p) => {
                let target = self.resolve_remote_input(&p);
                self.remote_cd(rt, ctx, target);
            }
            SideAction::Cd(false, p) => {
                let target = self.resolve_local_input(&p);
                self.local_cd(&target);
                return Some(PanelOutput::LocalDir(self.local.cwd.clone()));
            }
            SideAction::Mkdir(remote) => {
                let dir = if remote {
                    self.remote.cwd.clone()
                } else {
                    self.local.cwd.clone()
                };
                if dir.is_empty() {
                    self.error(t("Open a remote folder first."));
                } else {
                    self.dialog = Some(Dialog::Mkdir {
                        remote,
                        dir,
                        name: String::new(),
                    });
                    self.focus_dialog = true;
                }
            }
            SideAction::Open {
                remote: true,
                dir,
                entry,
            } if entry.is_dir => {
                self.remote_cd(rt, ctx, remote_join(&dir, &entry.name));
            }
            SideAction::Open {
                remote: false,
                dir,
                entry,
            } if entry.is_dir => {
                let p = Path::new(&dir).join(&entry.name).display().to_string();
                self.local_cd(&p);
                return Some(PanelOutput::LocalDir(self.local.cwd.clone()));
            }
            SideAction::Open { remote, dir, entry } => {
                return self.apply(
                    SideAction::Transfer {
                        from_remote: remote,
                        dir,
                        names: vec![entry.name],
                    },
                    rt,
                    ctx,
                );
            }
            SideAction::Select(remote, name, toggle, range) => {
                let side = if remote {
                    &mut self.remote
                } else {
                    &mut self.local
                };
                let names: Vec<String> = side
                    .visible(side.show_hidden)
                    .map(|e| e.name.clone())
                    .collect();
                let anchor_pos = side
                    .anchor
                    .as_ref()
                    .and_then(|a| names.iter().position(|n| n == a));
                match (range, anchor_pos, names.iter().position(|n| *n == name)) {
                    (true, Some(a), Some(b)) => {
                        side.selected = names[a.min(b)..=a.max(b)].iter().cloned().collect();
                    }
                    _ if toggle => {
                        if !side.selected.remove(&name) {
                            side.selected.insert(name.clone());
                        }
                        side.anchor = Some(name);
                    }
                    _ => {
                        side.selected = HashSet::from([name.clone()]);
                        side.anchor = Some(name);
                    }
                }
            }
            SideAction::Sort(remote, key) => {
                let side = if remote {
                    &mut self.remote
                } else {
                    &mut self.local
                };
                if side.sort == key {
                    side.descending = !side.descending;
                } else {
                    side.sort = key;
                    side.descending = false;
                }
                entry::sort_entries(&mut side.entries, side.sort, side.descending);
            }
            SideAction::Transfer {
                from_remote: true,
                dir,
                names,
            } => {
                let sources: Vec<String> = names.iter().map(|n| remote_join(&dir, n)).collect();
                let dest = self.local.cwd.clone();
                self.download_sources(rt, ctx, sources, dest);
            }
            SideAction::Transfer {
                from_remote: false,
                dir,
                names,
            } => {
                let sources: Vec<String> = names
                    .iter()
                    .map(|n| Path::new(&dir).join(n).display().to_string())
                    .collect();
                let dest = self.remote.cwd.clone();
                self.upload_sources(rt, ctx, sources, dest);
            }
            SideAction::Rename { remote, dir, name } => {
                self.dialog = Some(Dialog::Rename {
                    remote,
                    dir,
                    from: name.clone(),
                    to: name,
                });
                self.focus_dialog = true;
            }
            SideAction::Delete { remote, dir, names } => {
                let items = self.delete_items(remote, &dir, &names);
                self.dialog = Some(Dialog::Delete {
                    remote,
                    dir,
                    names,
                    items,
                })
            }
            SideAction::Chmod { path, mode } => {
                self.dialog = Some(Dialog::Chmod {
                    path,
                    original: mode & 0o7777,
                    mode: format!("{:o}", mode & 0o7777),
                });
                self.focus_dialog = true;
            }
        }
        None
    }

    fn drag_preview(&mut self, ctx: &egui::Context) {
        let Some(drag) = egui::DragAndDrop::payload::<DragFiles>(ctx) else {
            self.drag_label = None;
            return;
        };
        let Some(pos) = ctx.pointer_interact_pos() else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("sftp-drag"),
        ));
        let visuals = ctx.global_style().visuals.clone();
        let p = style::palette(visuals.dark_mode);
        let n = drag.names.len();
        let origin = pos + vec2(10.0, 12.0);
        let size = vec2(200.0, 34.0);
        let radius = CornerRadius::same(style::RADIUS_MD);
        // A stack of cards: more than one file shows two more behind the first.
        for k in (1..n.min(3)).rev() {
            let r = egui::Rect::from_min_size(origin + vec2(4.0, 4.0) * k as f32, size);
            let alpha = if k == 1 { 0.8 } else { 0.55 };
            painter.add(visuals.popup_shadow.as_shape(r, radius));
            painter.rect_filled(r, radius, style::soft(p.raised, alpha));
        }
        let card = egui::Rect::from_min_size(origin, size);
        painter.add(visuals.popup_shadow.as_shape(card, radius));
        painter.rect(
            card,
            radius,
            p.raised,
            visuals.window_stroke,
            StrokeKind::Inside,
        );
        icons::paint_file_icon(
            &painter,
            pos2(card.left() + 10.0 + 8.0, card.center().y),
            16.0,
            drag.first_kind,
            &p,
        );
        let mut name_right = card.right() - 10.0;
        if n > 1 {
            let g = painter.layout_no_wrap(
                n.to_string(),
                style::semibold(style::XS),
                egui::Color32::WHITE,
            );
            let w = (g.size().x + 12.0).max(20.0);
            let badge = egui::Rect::from_min_size(
                pos2(card.right() - 10.0 - w, card.center().y - 10.0),
                vec2(w, 20.0),
            );
            painter.rect_filled(badge, CornerRadius::same(10), p.accent);
            painter.galley(badge.center() - g.size() / 2.0, g, egui::Color32::WHITE);
            name_right = badge.left() - 8.0;
        }
        let name = drag.names.first().cloned().unwrap_or_default();
        let g = painter.layout_job(single_line(
            &name,
            style::medium(style::BODY),
            p.text,
            name_right - (card.left() + 36.0),
        ));
        painter.galley(
            pos2(card.left() + 36.0, card.center().y - g.size().y / 2.0),
            g,
            p.text,
        );
        // Where the files go, under the cards.
        if let Some((remote_side, into)) = &self.drop_dir
            && *remote_side != drag.from_remote
        {
            let (icon, text) = if *remote_side {
                let dest = if into.is_empty() {
                    self.remote.cwd.clone()
                } else {
                    remote_join(&self.remote.cwd, into)
                };
                (
                    icons::UPLOAD_SIMPLE,
                    (t("Upload to {}"), short_dir(&dest, true)),
                )
            } else {
                let dest = if into.is_empty() {
                    self.local.cwd.clone()
                } else {
                    Path::new(&self.local.cwd).join(into).display().to_string()
                };
                (
                    icons::DOWNLOAD_SIMPLE,
                    (t("Download to {}"), short_dir(&dest, false)),
                )
            };
            let white = egui::Color32::WHITE;
            let job = styled_tf(
                text.0,
                &[(
                    text.1,
                    egui::TextFormat {
                        font_id: style::mono(11.5),
                        color: white,
                        ..Default::default()
                    },
                )],
                egui::TextFormat {
                    font_id: style::medium(style::SMALL),
                    color: white,
                    ..Default::default()
                },
            );
            let g = painter.layout_job(job);
            let tip = egui::Rect::from_min_size(
                origin + vec2(6.0, size.y + 14.0),
                vec2(9.0 + 13.0 + 6.0 + g.size().x + 9.0, 24.0),
            );
            painter.add(visuals.popup_shadow.as_shape(tip, CornerRadius::same(6)));
            painter.rect_filled(tip, CornerRadius::same(6), p.accent);
            painter.text(
                pos2(tip.left() + 9.0 + 6.5, tip.center().y),
                Align2::CENTER_CENTER,
                icon,
                style::icon_font(13.0),
                white,
            );
            painter.galley(
                pos2(
                    tip.left() + 9.0 + 13.0 + 6.0,
                    tip.center().y - g.size().y / 2.0,
                ),
                g,
                white,
            );
        }
    }

    /// The transfer strip along the bottom of the panel (36 px): the batch's progress with
    /// Cancel all, or how it ended, or the latest message; "Show transfers" opens the list.
    fn xfer_strip(&mut self, ui: &mut Ui, rect: egui::Rect) -> Vec<SideAction> {
        let mut actions = Vec::new();
        let p = style::pal(ui);
        let dark = p.dark;
        ui.painter().rect_filled(rect, 0.0, p.canvas);
        ui.painter()
            .hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
        let summary = self.transfers.summary();
        let running = summary.as_ref().filter(|s| s.running()).cloned();
        let has_items = !self.transfers.items.is_empty();
        let cy = rect.center().y;
        let lead = pos2(rect.left() + 14.0 + 7.0, cy);
        let painter = ui.painter().clone();
        let (title, detail) = match (&running, &summary, &self.message) {
            (Some(s), _, _) => {
                style::progress_ring(&painter, lead, 7.0, s.fraction(), dark);
                (summary_title(s), summary_meta(s))
            }
            (None, Some(_), _) if has_items => {
                let o = self.outcome();
                let (glyph, color) = if o.failed > 0 {
                    ("!", p.err)
                } else {
                    (icons::CHECK, p.ok)
                };
                icons::paint_badge_circle(&painter, lead, 16.0, glyph, color, p.canvas);
                (o.title(), self.finished_meta())
            }
            (_, _, Some((msg, err))) => {
                let color = if *err { p.err } else { p.text_muted };
                if *err {
                    icons::paint_badge_circle(&painter, lead, 16.0, "!", color, p.canvas);
                } else {
                    painter.text(
                        lead,
                        Align2::CENTER_CENTER,
                        icons::INFO,
                        style::icon_font(15.0),
                        color,
                    );
                }
                (msg.clone(), String::new())
            }
            _ => (t("Transfers").to_string(), String::new()),
        };
        // From the right: × (or Show transfers), Cancel all, the bar.
        let mut right = rect.right() - 8.0;
        let mut row = |w: f32| {
            let r = egui::Rect::from_min_max(pos2(right - w, cy - 13.0), pos2(right, cy + 13.0));
            right = r.left() - 6.0;
            r
        };
        if running.is_none() {
            let r = row(20.0);
            let r = egui::Rect::from_center_size(r.center(), vec2(20.0, 20.0));
            let x = ui
                .interact(r, egui::Id::new(("sftp-strip-x", self.uid)), Sense::click())
                .on_hover_text(t("Dismiss"));
            kit::paint_icon_button(ui, &x, r, icons::X, t("Dismiss"), false, false);
            if x.clicked() {
                self.message = None;
                self.transfers.clear_finished();
                self.show_activity = false;
            }
        }
        if has_items {
            let w = text_button_w(ui, t("Show transfers"), true, false);
            let mut b = ui.new_child(egui::UiBuilder::new().max_rect(row(w)));
            let label = if self.show_activity {
                t("Hide transfers")
            } else {
                t("Show transfers")
            };
            if kit::button_ex(&mut b, kit::Kind::Ghost, None, label, true).clicked() {
                self.show_activity = !self.show_activity;
            }
        }
        if let Some(s) = &running {
            let w = text_button_w(ui, t("Cancel all"), true, false);
            let mut b = ui.new_child(egui::UiBuilder::new().max_rect(row(w)));
            if kit::button_ex(&mut b, kit::Kind::Ghost, None, t("Cancel all"), true).clicked() {
                self.transfers.cancel_all();
            }
            right -= 4.0;
            let w = (right - (rect.left() + 300.0)).clamp(0.0, 180.0);
            if w > 40.0 {
                let bar = egui::Rect::from_min_size(pos2(right - w, cy - 2.0), vec2(w, 4.0));
                paint_track(&painter, bar, s.fraction(), p.accent, &p);
                right = bar.left() - 10.0;
            }
        }
        let x = rect.left() + 14.0 + 14.0 + 10.0;
        let tg = painter.layout_job(single_line(
            &title,
            style::semibold(style::SMALL),
            p.text,
            (right - x).max(10.0),
        ));
        let tw = tg.size().x;
        painter.galley(pos2(x, cy - tg.size().y / 2.0), tg, p.text);
        if !detail.is_empty() {
            let dx = x + tw + 10.0;
            if right - dx > 30.0 {
                let dg = painter.layout_job(single_line(
                    &detail,
                    style::regular(style::SMALL),
                    p.text_faint,
                    right - dx,
                ));
                painter.galley(pos2(dx, cy - dg.size().y / 2.0), dg, p.text_faint);
            }
        }
        if running.is_none() && has_items {
            // "just now" turns into "1 min ago".
            ui.ctx().request_repaint_after(Duration::from_secs(30));
        }
        let _ = &mut actions;
        actions
    }

    /// How the transfers in the list ended, all batches together.
    fn outcome(&self) -> Outcome {
        let mut o = Outcome::default();
        for tr in &self.transfers.items {
            match tr.state() {
                TransferState::Done => {
                    o.done += tr
                        .files_done
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .max(1) as usize
                }
                TransferState::Failed(_) => o.failed += 1,
                TransferState::Cancelled => o.cancelled += 1,
                _ => {}
            }
            match tr.direction {
                Direction::Upload => o.uploads = true,
                Direction::Download => o.downloads = true,
            }
        }
        o
    }

    /// What finished: the first names, the size, and when ("nginx.conf, deploy.sh · 3.1 KB ·
    /// just now").
    fn finished_meta(&self) -> String {
        let done: Vec<&Transfer> = self
            .transfers
            .items
            .iter()
            .map(|t| t.as_ref())
            .filter(|t| matches!(t.state(), TransferState::Done))
            .collect();
        if done.is_empty() {
            return String::new();
        }
        let mut names: Vec<String> = done.iter().take(2).map(|t| t.name.clone()).collect();
        if done.len() > 2 {
            names.push(tf("and {} more", &[&(done.len() - 2).to_string()]));
        }
        let bytes: u64 = done
            .iter()
            .map(|t| t.total.load(std::sync::atomic::Ordering::Relaxed))
            .sum();
        let mut parts = vec![names.join(", "), entry::human_size(bytes)];
        if let Some(at) = done.iter().filter_map(|t| t.finished_at()).max() {
            parts.push(ago(at.elapsed()));
        }
        parts.join(" · ")
    }

    /// The transfers list as a popover above the strip.
    fn activity_popup(
        &mut self,
        ctx: &egui::Context,
        rt: &tokio::runtime::Handle,
        panel: egui::Rect,
    ) {
        let anchor = pos2(panel.right() - 10.0, panel.bottom() - STRIP_H - 6.0);
        let area = egui::Area::new(egui::Id::new(("sftp-activity", self.uid)))
            .order(egui::Order::Foreground)
            .pivot(Align2::RIGHT_BOTTOM)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                kit::popover_frame(ui)
                    .show(ui, |ui| self.transfers_popover(ui, rt))
                    .inner
            });
        let hide = area.inner;
        let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
            && ctx
                .pointer_interact_pos()
                .is_some_and(|pos| !area.response.rect.contains(pos))
            && !egui::Popup::is_any_open(ctx);
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if hide || escape || (clicked_outside && !self.strip_clicked(ctx, panel)) {
            self.show_activity = false;
        }
    }

    /// The pointer was pressed on the strip (whose button toggles the popover itself).
    fn strip_clicked(&self, ctx: &egui::Context, panel: egui::Rect) -> bool {
        let strip =
            egui::Rect::from_min_max(pos2(panel.left(), panel.bottom() - STRIP_H), panel.max);
        ctx.pointer_interact_pos()
            .is_some_and(|p| strip.contains(p))
    }

    /// The transfers of this tab (500 px wide): the batch, a row per transfer, Clear finished
    /// and Hide. Also shown from the Files button in the title row. Returns true when Hide was
    /// clicked.
    pub fn transfers_popover(&mut self, ui: &mut Ui, rt: &tokio::runtime::Handle) -> bool {
        let ctx = ui.ctx().clone();
        let p = style::pal(ui);
        let mut hide = false;
        let mut actions = Vec::new();
        ui.set_width(POP_W);
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let summary = self.transfers.summary();
        let running = summary.as_ref().filter(|s| s.running()).cloned();
        // Header: what, how far, how fast.
        let (head, _) = ui.allocate_exact_size(vec2(POP_W, 58.0), Sense::hover());
        let outcome = self.outcome();
        // Redesign 3.1: no icon tile; the title starts at the edge.
        let tile =
            egui::Rect::from_min_size(pos2(head.left() + 14.0, head.top() + 14.0), vec2(0.0, 32.0));
        let (title, sub) = match (&running, &summary) {
            (Some(s), _) => (summary_title(s), summary_meta(s)),
            (None, Some(_)) => (t("Transfers").to_string(), outcome.detail()),
            _ => (t("Transfers").to_string(), String::new()),
        };
        let mut right = head.right() - 14.0;
        if running.is_some() {
            let w = text_button_w(ui, t("Cancel all"), true, false);
            let r = egui::Rect::from_min_max(
                pos2(right - w, tile.top() - 1.0),
                pos2(right, tile.top() + 25.0),
            );
            let mut b = ui.new_child(egui::UiBuilder::new().max_rect(r));
            if kit::button_ex(&mut b, kit::Kind::Ghost, None, t("Cancel all"), true).clicked() {
                self.transfers.cancel_all();
            }
            right = r.left() - 8.0;
        }
        let tx = tile.left();
        let painter = ui.painter();
        let g = painter.layout_job(single_line(
            &title,
            style::semibold(14.0),
            p.text,
            right - tx,
        ));
        painter.galley(pos2(tx, tile.top() + 1.0), g, p.text);
        let g = painter.layout_job(single_line(
            &sub,
            style::regular(style::SMALL),
            p.text_faint,
            right - tx,
        ));
        painter.galley(pos2(tx, tile.top() + 20.0), g, p.text_faint);
        if let Some(s) = summary.as_ref() {
            let (bar, _) = ui.allocate_exact_size(vec2(POP_W, 17.0), Sense::hover());
            let track = egui::Rect::from_min_size(
                pos2(bar.left() + 14.0, bar.top()),
                vec2(POP_W - 28.0, 5.0),
            );
            let color = if s.running() {
                p.accent
            } else if outcome.failed > 0 {
                p.err
            } else {
                p.ok
            };
            paint_track(
                ui.painter(),
                track,
                if s.running() { s.fraction() } else { 1.0 },
                color,
                &p,
            );
        }
        // The rows: running first, then failed, finished and waiting.
        let (top, _) = ui.allocate_exact_size(vec2(POP_W, 6.0), Sense::hover());
        ui.painter()
            .hline(top.x_range(), top.top() + 0.5, Stroke::new(1.0, p.border));
        let mut order: Vec<&Transfer> = self.transfers.items.iter().map(|t| t.as_ref()).collect();
        order.sort_by_key(|tr| {
            let rank = match tr.state() {
                TransferState::Running => 0,
                TransferState::Failed(_) => 1,
                TransferState::Done | TransferState::Cancelled => 2,
                TransferState::Queued => 3,
            };
            (rank, std::cmp::Reverse(tr.id))
        });
        let message = self.message.clone();
        let is_root = self.is_root;
        let rows = order.len() + usize::from(message.is_some());
        let list_h = (rows as f32 * (XFER_ROW_H + 1.0)).min(6.0 * (XFER_ROW_H + 1.0));
        let mut dismiss = false;
        ui.allocate_ui(vec2(POP_W, list_h), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("sftp-transfers-list", self.uid))
                .max_height(list_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    if let Some((msg, is_err)) = &message
                        && message_row(ui, msg, *is_err, self.uid)
                    {
                        dismiss = true;
                    }
                    for tr in order {
                        match transfer_row(ui, tr, is_root) {
                            Some(RowAction::Remove(id)) => {
                                actions.push(SideAction::RemoveTransfer(id))
                            }
                            Some(RowAction::Retry(id)) => actions.push(SideAction::Retry(id)),
                            None => {}
                        }
                    }
                });
        });
        if dismiss {
            self.message = None;
        }
        let (gap, _) = ui.allocate_exact_size(vec2(POP_W, 6.0), Sense::hover());
        let _ = gap;
        // Footer.
        let (foot, _) = ui.allocate_exact_size(vec2(POP_W, 42.0), Sense::hover());
        let painter = ui.painter();
        painter.hline(foot.x_range(), foot.top() + 0.5, Stroke::new(1.0, p.border));
        let cy = foot.center().y;
        let mut right = foot.right() - 10.0;
        let mut button = |ui: &mut Ui, label: &str| {
            let w = text_button_w(ui, label, true, false);
            let r = egui::Rect::from_min_max(pos2(right - w, cy - 13.0), pos2(right, cy + 13.0));
            right = r.left() - 8.0;
            let mut b = ui.new_child(egui::UiBuilder::new().max_rect(r));
            kit::button_ex(&mut b, kit::Kind::Ghost, None, label, true).clicked()
        };
        if button(ui, t("Hide")) {
            hide = true;
        }
        if button(ui, t("Clear finished")) {
            self.transfers.clear_finished();
        }
        let note = if running.is_some() {
            t("You'll get a notification when it's done.")
        } else {
            ""
        };
        if !note.is_empty() {
            let painter = ui.painter();
            painter.text(
                pos2(foot.left() + 14.0 + 7.0, cy),
                Align2::CENTER_CENTER,
                icons::INFO,
                style::icon_font(14.0),
                p.text_faint,
            );
            let g = painter.layout_job(single_line(
                note,
                style::regular(style::SMALL),
                p.text_faint,
                right - (foot.left() + 36.0),
            ));
            painter.galley(
                pos2(foot.left() + 36.0, cy - g.size().y / 2.0),
                g,
                p.text_faint,
            );
        }
        for a in actions {
            self.apply(a, rt, &ctx);
        }
        hide
    }

    /// The items a Delete dialog lists (at most 8).
    fn delete_items(&self, remote: bool, dir: &str, names: &[String]) -> Vec<DialogItem> {
        let side = if remote { &self.remote } else { &self.local };
        let same_dir = side.cwd == dir;
        names
            .iter()
            .take(8)
            .map(|n| {
                let e = same_dir
                    .then(|| side.entries.iter().find(|e| &e.name == n))
                    .flatten();
                DialogItem {
                    name: n.clone(),
                    kind: e.map_or(icons::FileKind::of(n, false, false, false), entry_kind),
                    meta: match e {
                        Some(e) if e.is_dir => t("folder").to_string(),
                        Some(e) => entry::human_size(e.size),
                        None => String::new(),
                    },
                    old: String::new(),
                }
            })
            .collect()
    }

    /// The items a Replace dialog lists: each conflict's new size and date, then the existing
    /// one's. `existing` are the entries of the destination when it isn't the open folder.
    fn replace_items(
        &self,
        upload: bool,
        sources: &[String],
        dest: &str,
        conflicts: &[String],
        existing: Option<&[FileEntry]>,
    ) -> Vec<DialogItem> {
        let describe = |size: u64, secs: Option<i64>| match secs {
            Some(s) => format!("{}, {}", entry::human_size(size), short_day(s)),
            None => entry::human_size(size),
        };
        conflicts
            .iter()
            .take(8)
            .map(|name| {
                let src = sources.iter().find(|s| {
                    if upload {
                        Path::new(s)
                            .file_name()
                            .is_some_and(|n| n.to_string_lossy() == *name)
                    } else {
                        entry::remote_name(s) == name
                    }
                });
                let local_meta = |path: &Path| -> Option<(u64, Option<i64>, bool)> {
                    let m = std::fs::metadata(path).ok()?;
                    Some((
                        m.len(),
                        m.modified().ok().and_then(entry::unix_secs),
                        m.is_dir(),
                    ))
                };
                let remote_meta = |dir: &str, n: &str| -> Option<(u64, Option<i64>, bool)> {
                    let list: &[FileEntry] = match existing {
                        Some(l) if upload => l,
                        _ if self.remote.cwd == dir => &self.remote.entries,
                        _ => return None,
                    };
                    list.iter()
                        .find(|e| e.name == n)
                        .map(|e| (e.size, e.modified, e.is_dir))
                };
                let (new, old) = if upload {
                    (
                        src.and_then(|s| local_meta(Path::new(s))),
                        remote_meta(dest, name),
                    )
                } else {
                    (
                        src.and_then(|s| remote_meta(&remote_parent(s), name)),
                        local_meta(&Path::new(dest).join(name)),
                    )
                };
                let is_dir = new.is_some_and(|m| m.2) || old.is_some_and(|m| m.2);
                DialogItem {
                    name: name.clone(),
                    kind: icons::FileKind::of(name, is_dir, false, false),
                    meta: new.map_or(String::new(), |m| {
                        if m.2 {
                            t("folder").to_string()
                        } else {
                            describe(m.0, m.1)
                        }
                    }),
                    old: old.map_or(String::new(), |m| {
                        if m.2 {
                            t("folder").to_string()
                        } else {
                            describe(m.0, m.1)
                        }
                    }),
                }
            })
            .collect()
    }

    fn dialog_ui(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Handle) {
        if self.dialog.is_none() {
            return;
        }
        // "Go to folder…" suggestions follow the folder typed so far.
        if let Some(Dialog::GoTo { remote, text, .. }) = &self.dialog {
            let (remote, parent) = (*remote, goto_parent(text));
            if self.goto_dirs.0 != parent {
                self.goto_dirs = (parent.clone(), Vec::new());
                if remote {
                    if let Some(sftp) = self.sftp() {
                        self.goto_seq += 1;
                        let (seq, tx, ctx2) = (self.goto_seq, self.events_tx.clone(), ctx.clone());
                        let dir = self.resolve_remote_input(&parent);
                        rt.spawn(async move {
                            let result = remote::list(&sftp, &dir).await.map(|entries| {
                                entries
                                    .into_iter()
                                    .filter(|e| e.is_dir)
                                    .map(|e| e.name)
                                    .collect()
                            });
                            let _ = tx.send(Event::DirsListed {
                                seq,
                                dir: parent,
                                result,
                            });
                            ctx2.request_repaint();
                        });
                    }
                } else {
                    let dir = self.resolve_local_input(&parent);
                    if let Ok(entries) = entry::list_local(Path::new(&dir)) {
                        self.goto_dirs.1 = entries
                            .into_iter()
                            .filter(|e| e.is_dir)
                            .map(|e| e.name)
                            .collect();
                        self.goto_dirs.1.sort_by_key(|n| n.to_lowercase());
                    }
                }
            }
        }
        let goto_dirs = self.goto_dirs.1.clone();
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut close = false;
        let mut confirmed = false;
        let mut skip_existing = false;
        let mut goto_open: Option<String> = None;
        let focus = std::mem::take(&mut self.focus_dialog);
        let host = self.host.clone();
        let user = self.user.clone();
        let where_ = |remote: bool| {
            if remote {
                host.clone()
            } else {
                crate::platform::this_computer().to_string()
            }
        };
        // Whether a name is taken in a folder (renaming onto another file, a new folder).
        let taken = |remote: bool, dir: &str, from: Option<&str>, name: &str| -> bool {
            let name = name.trim();
            if name.is_empty() || Some(name) == from {
                return false;
            }
            if remote {
                self.remote.cwd == dir && self.remote.entries.iter().any(|e| e.name == name)
            } else {
                let to = Path::new(dir).join(name);
                to.exists() && !from.is_some_and(|f| same_local_file(&Path::new(dir).join(f), &to))
            }
        };
        let modal = style::modal(ctx, egui::Id::new("sftp-dialog")).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            let enter = |ui: &Ui, r: &egui::Response| {
                r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
            };
            let mut ok_enabled = true;
            match dialog {
                Dialog::Mkdir { name, dir, remote } => {
                    ui.set_width(380.0);
                    let sub = tf("in {} on {}", &[&short_dir(dir, *remote), &where_(*remote)]);
                    dialog_head(ui, t("New folder"), Some(&sub));
                    let bad = taken(*remote, dir, None, name);
                    ok_enabled = !name.trim().is_empty() && !bad;
                    dialog_body(ui, |ui| {
                        let r = ui.add(
                            style::input(name)
                                .hint_text(t("Folder name"))
                                .error(bad),
                        );
                        if focus {
                            r.request_focus();
                        }
                        if bad {
                            error_line(ui, &tf("{} already exists.", &[name.trim()]));
                        }
                        if enter(ui, &r) && ok_enabled {
                            confirmed = true;
                        }
                    });
                }
                Dialog::Rename { to, from, dir, remote } => {
                    ui.set_width(380.0);
                    let sub = tf("in {} on {}", &[&short_dir(dir, *remote), &where_(*remote)]);
                    dialog_head(ui, t("Rename"), Some(&sub));
                    let bad = taken(*remote, dir, Some(from), to);
                    ok_enabled = !to.trim().is_empty() && !bad;
                    dialog_body(ui, |ui| {
                        let id = egui::Id::new(("sftp-rename", dir.as_str(), from.as_str()));
                        let r = ui.add(style::input(to).id(id).error(bad));
                        if focus {
                            r.request_focus();
                            // The name without its extension is selected, so typing keeps it.
                            if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
                                let stem = stem_chars(to);
                                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(egui::text::CharIndex(0)),
                                    egui::text::CCursor::new(egui::text::CharIndex(stem)),
                                )));
                                state.store(ui.ctx(), id);
                            }
                        }
                        if bad {
                            error_line(ui, &tf("{} already exists.", &[to.trim()]));
                        } else {
                            ui.add_space(8.0);
                            style::hint(ui, t("The name is selected without its extension."));
                        }
                        if enter(ui, &r) && ok_enabled {
                            confirmed = true;
                        }
                    });
                }
                Dialog::Delete {
                    names,
                    remote,
                    dir,
                    items,
                } => {
                    ui.set_width(400.0);
                    let title = match names.as_slice() {
                        [one] => tf("Delete “{}” on {}?", &[one, &where_(*remote)]),
                        _ => tf(
                            "Delete {} items on {}?",
                            &[&names.len().to_string(), &where_(*remote)],
                        ),
                    };
                    // Folders take everything in them: say so only when one may be among them.
                    let folders = items.len() < names.len()
                        || items
                            .iter()
                            .any(|i| i.kind == icons::FileKind::Folder || i.meta.is_empty());
                    dialog_head(ui, &title,
                        Some(&tf("in {}", &[&short_dir(dir, *remote)])),
                    );
                    dialog_body(ui, |ui| {
                        item_list(ui, items, names.len());
                        ui.add_space(12.0);
                        kit::callout(
                            ui,
                            icons::WARNING,
                            Tone::Warn,
                            if folders {
                                t("Folders are deleted with everything in them. This can't be undone.")
                            } else {
                                t("This can't be undone.")
                            },
                        );
                    });
                }
                Dialog::Chmod { path, mode, .. } => {
                    ui.set_width(400.0);
                    let name = entry::remote_name(path).to_string();
                    let sub = tf("{} on {}", &[&name, &host]);
                    dialog_head(ui, t("Permissions"), Some(&sub));
                    let bad = entry::parse_mode(mode).is_none();
                    ok_enabled = !bad;
                    dialog_body(ui, |ui| {
                        permission_matrix(ui, mode);
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let (r, _) = ui.allocate_exact_size(vec2(44.0, style::CONTROL_H), Sense::hover());
                            ui.painter().text(
                                r.left_center(),
                                Align2::LEFT_CENTER,
                                t("Octal"),
                                style::medium(style::SMALL),
                                p.text_muted,
                            );
                            let r = ui.add(style::input(mode).desired_width(84.0).mono().error(bad));
                            if enter(ui, &r) && !bad {
                                confirmed = true;
                            }
                            ui.label(
                                RichText::new(
                                    entry::parse_mode(mode)
                                        .map(entry::mode_string)
                                        .unwrap_or_else(|| "?".into()),
                                )
                                .font(style::mono(12.0))
                                .color(p.text_faint),
                            );
                        });
                        if bad {
                            error_line(ui, t("Invalid permissions; use octal like 644 or 755."));
                        }
                    });
                }
                Dialog::Overwrite {
                    conflicts,
                    upload,
                    items,
                    ..
                } => {
                    ui.set_width(420.0);
                    let title = match conflicts.as_slice() {
                        [one] => tf("Replace “{}”?", &[one]),
                        _ => tf("Replace {} existing items?", &[&conflicts.len().to_string()]),
                    };
                    let sub = tf("The files on {} will be overwritten.", &[&where_(*upload)]);
                    dialog_head(ui, &title, Some(&sub));
                    dialog_body(ui, |ui| {
                        item_list(ui, items, conflicts.len());
                        ui.add_space(10.0);
                        style::hint(
                            ui,
                            tf(
                                "New size and date first, the one on {} after the arrow.",
                                &[&if *upload {
                                    t("the server").to_string()
                                } else {
                                    crate::platform::this_computer().to_string()
                                }],
                            ),
                        );
                    });
                }
                Dialog::Sudo { password, note, show } => {
                    ui.set_width(400.0);
                    let sub = tf("sudo on {} needs a password.", &[&host]);
                    dialog_head(ui, t("sudo password"), Some(&sub));
                    ok_enabled = !password.is_empty();
                    dialog_body(ui, |ui| {
                        let bad = note.is_some() && password.is_empty();
                        let r = ui
                            .horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let r = ui.add(
                                    style::input(password)
                                        .password(!*show)
                                        .hint_text(t("sudo password"))
                                        .error(bad),
                                );
                                let eye = egui::Rect::from_center_size(
                                    pos2(r.rect.right() - 6.0 - 10.0, r.rect.center().y),
                                    vec2(20.0, 20.0),
                                );
                                let er = ui.interact(eye, egui::Id::new("sftp-sudo-eye"), Sense::click());
                                kit::paint_icon_button(
                                    ui,
                                    &er,
                                    eye,
                                    if *show { icons::EYE_SLASH } else { icons::EYE },
                                    if *show { t("Hide") } else { t("Show") },
                                    false,
                                    false,
                                );
                                if er.clicked() {
                                    *show = !*show;
                                }
                                r
                            })
                            .inner;
                        if focus {
                            r.request_focus();
                        }
                        if enter(ui, &r) && !password.is_empty() {
                            confirmed = true;
                        }
                        if let Some(n) = note.as_deref().filter(|_| password.is_empty()) {
                            error_line(ui, n);
                        }
                        ui.add_space(12.0);
                        let who = if user.is_empty() {
                            t("your user").to_string()
                        } else {
                            user.clone()
                        };
                        kit::callout(
                            ui,
                            icons::INFO,
                            Tone::Root,
                            &tf(
                                "Only the file panel runs as root; the terminal stays as {}. Nothing is saved.",
                                &[&who],
                            ),
                        );
                    });
                }
                Dialog::GoTo { remote, text, pick } => {
                    ui.set_width(400.0);
                    let sub = tf("on {}", &[&where_(*remote)]);
                    dialog_head(ui, t("Go to folder…"),
                        Some(&sub),
                    );
                    let parent = goto_parent(text);
                    let prefix = text[parent.len()..].to_lowercase();
                    let matches: Vec<String> = goto_dirs
                        .iter()
                        .filter(|n| {
                            n.to_lowercase().starts_with(&prefix)
                                && (prefix.starts_with('.') || !n.starts_with('.'))
                        })
                        .take(6)
                        .map(|n| format!("{parent}{n}"))
                        .collect();
                    if matches.is_empty() || prefix.is_empty() {
                        *pick = None;
                    } else if pick.is_none_or(|i| i >= matches.len()) {
                        *pick = Some(0);
                    }
                    let (down, up) = ui.input_mut(|i| {
                        (
                            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                            i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                        )
                    });
                    if !matches.is_empty() {
                        if down {
                            *pick = Some(pick.map_or(0, |i| (i + 1).min(matches.len() - 1)));
                        }
                        if up {
                            *pick = pick.and_then(|i| i.checked_sub(1));
                        }
                    }
                    dialog_body(ui, |ui| {
                        let id = egui::Id::new("sftp-goto");
                        let r = ui.add(style::input(text).id(id).mono());
                        if focus {
                            r.request_focus();
                            if let Some(mut state) = egui::text_edit::TextEditState::load(ui.ctx(), id) {
                                let end = egui::text::CCursor::new(egui::text::CharIndex(text.chars().count()));
                                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                                state.store(ui.ctx(), id);
                            }
                        }
                        if enter(ui, &r) {
                            goto_open = Some(match pick {
                                Some(i) => matches[*i].clone(),
                                None => text.clone(),
                            });
                        }
                        if !matches.is_empty() {
                            ui.add_space(8.0);
                            if let Some(i) = suggestion_list(ui, &matches, *pick) {
                                goto_open = Some(matches[i].clone());
                            }
                        }
                    });
                }
            }
            let (ok_label, kind) = match dialog {
                Dialog::Delete { .. } => (t("Delete"), kit::Kind::Danger),
                Dialog::Overwrite { .. } => (t("Replace"), kit::Kind::Primary),
                Dialog::Mkdir { .. } => (t("Create"), kit::Kind::Primary),
                Dialog::Rename { .. } => (t("Rename"), kit::Kind::Primary),
                Dialog::Chmod { .. } => (t("Apply"), kit::Kind::Primary),
                Dialog::Sudo { .. } => (t("Continue"), kit::Kind::Primary),
                Dialog::GoTo { .. } => (t("Open"), kit::Kind::Primary),
            };
            let enter_hint = matches!(
                dialog,
                Dialog::Mkdir { .. } | Dialog::Rename { .. } | Dialog::Chmod { .. }
            );
            let sudo = matches!(dialog, Dialog::Sudo { .. });
            let overwrite = matches!(dialog, Dialog::Overwrite { .. });
            let goto_text = match dialog {
                Dialog::GoTo { text, .. } => Some(text.clone()),
                _ => None,
            };
            kit::dialog_footer(
                ui,
                |_| {},
                |ui| {
                    let ok = ui
                        .add_enabled_ui(ok_enabled, |ui| {
                            if sudo {
                                root_button_filled(ui, ok_label)
                            } else {
                                kit::button_kbd(
                                    ui,
                                    kind,
                                    None,
                                    ok_label,
                                    enter_hint.then_some("↩"),
                                    false,
                                    false,
                                )
                            }
                        })
                        .inner;
                    if ok.clicked() {
                        match &goto_text {
                            Some(text) => goto_open = Some(text.clone()),
                            None => confirmed = true,
                        }
                    }
                    if overwrite && kit::secondary(ui, None, t("Skip existing")).clicked() {
                        skip_existing = true;
                    }
                    if kit::secondary(ui, None, t("Cancel")).clicked() {
                        close = true;
                    }
                },
            );
        });
        if modal.should_close() {
            close = true;
        }
        if let Some(path) = goto_open {
            let remote = matches!(self.dialog, Some(Dialog::GoTo { remote: true, .. }));
            self.dialog = None;
            self.apply(SideAction::Cd(remote, path), rt, ctx);
        } else if skip_existing
            && let Some(Dialog::Overwrite {
                upload,
                sources,
                dest,
                conflicts,
                ..
            }) = self.dialog.take()
        {
            let keep: Vec<String> = sources
                .into_iter()
                .filter(|s| {
                    let name = if upload {
                        Path::new(s)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    } else {
                        entry::remote_name(s).to_string()
                    };
                    !conflicts.contains(&name)
                })
                .collect();
            for src in keep {
                if upload {
                    self.start_upload(rt, ctx, PathBuf::from(src), &dest);
                } else {
                    self.start_download(rt, ctx, src, Path::new(&dest));
                }
            }
        } else if confirmed {
            let dialog = self.dialog.take().unwrap();
            self.confirm(dialog, rt, ctx);
        } else if close {
            self.dialog = None;
        }
    }

    fn confirm(&mut self, dialog: Dialog, rt: &tokio::runtime::Handle, ctx: &egui::Context) {
        match dialog {
            Dialog::Mkdir {
                remote: true,
                dir,
                name,
            } if !name.trim().is_empty() => {
                let path = remote_join(&dir, name.trim());
                self.run_op(rt, ctx, move |s| {
                    Box::pin(async move { remote::mkdir(&s, &path).await })
                });
            }
            Dialog::Mkdir {
                remote: false,
                dir,
                name,
            } if !name.trim().is_empty() => {
                let path = Path::new(&dir).join(name.trim());
                if let Err(e) = std::fs::create_dir(&path) {
                    self.error(format!("{}: {e}", path.display()));
                }
                let cwd = self.local.cwd.clone();
                self.local_cd(&cwd);
            }
            Dialog::Rename {
                remote: true,
                dir,
                from,
                to,
            } if !to.trim().is_empty() && to != from => {
                let a = remote_join(&dir, &from);
                let b = remote_join(&dir, to.trim());
                self.run_op(rt, ctx, move |s| {
                    Box::pin(async move { remote::rename(&s, &a, &b).await })
                });
            }
            Dialog::Rename {
                remote: false,
                dir,
                from,
                to,
            } if !to.trim().is_empty() && to != from => {
                let a = Path::new(&dir).join(&from);
                let b = Path::new(&dir).join(to.trim());
                // On a case-insensitive disk "readme" → "README" is the same file: allowed.
                let same_file = same_local_file(&a, &b);
                if b.exists() && !same_file {
                    self.error(tf("{} already exists.", &[&b.display().to_string()]));
                } else if let Err(e) = std::fs::rename(&a, &b) {
                    self.error(format!("{}: {e}", a.display()));
                }
                let cwd = self.local.cwd.clone();
                self.local_cd(&cwd);
            }
            Dialog::Delete {
                remote: true,
                dir,
                names,
                ..
            } => {
                let paths: Vec<String> = names.iter().map(|n| remote_join(&dir, n)).collect();
                self.remote.selected.clear();
                self.run_op(rt, ctx, move |s| {
                    Box::pin(async move {
                        for p in paths {
                            remote::remove(&s, &p).await?;
                        }
                        Ok(())
                    })
                });
            }
            Dialog::Delete {
                remote: false,
                dir,
                names,
                ..
            } => {
                for n in names {
                    let p = Path::new(&dir).join(&n);
                    let r = if p.is_dir() && !p.is_symlink() {
                        std::fs::remove_dir_all(&p)
                    } else {
                        std::fs::remove_file(&p)
                    };
                    if let Err(e) = r {
                        self.error(format!("{}: {e}", p.display()));
                    }
                }
                self.local.selected.clear();
                let cwd = self.local.cwd.clone();
                self.local_cd(&cwd);
            }
            Dialog::Chmod {
                path,
                original,
                mode,
            } => match entry::parse_mode(&mode) {
                Some(m) if m == original => {}
                Some(m) => self.run_op(rt, ctx, move |s| {
                    Box::pin(async move { remote::chmod(&s, &path, m).await })
                }),
                None => self.error(t("Invalid permissions; use octal like 644 or 755.")),
            },
            Dialog::Overwrite {
                upload: true,
                sources,
                dest,
                ..
            } => {
                for src in sources {
                    self.start_upload(rt, ctx, PathBuf::from(src), &dest);
                }
            }
            Dialog::Overwrite {
                upload: false,
                sources,
                dest,
                ..
            } => {
                for src in sources {
                    self.start_download(rt, ctx, src, Path::new(&dest));
                }
            }
            // A box of spaces is a real password.
            Dialog::Sudo { password, .. } if !password.is_empty() => {
                self.reopen(Some(RootLogin::Password(password)));
            }
            _ => {}
        }
    }
}

/// Height of the panel's header.
const HEAD_H: f32 = 34.0;
/// Height of the transfer strip.
const STRIP_H: f32 = 36.0;
/// Width of the transfers popover.
const POP_W: f32 = 500.0;
/// Height of a row in the transfers popover.
const XFER_ROW_H: f32 = 40.0;

/// How the listed transfers ended.
#[derive(Default)]
struct Outcome {
    done: usize,
    failed: usize,
    cancelled: usize,
    uploads: bool,
    downloads: bool,
}

impl Outcome {
    /// "Uploaded 2 files." or, with failures, "Uploaded 1 file. 1 transfer failed."
    fn title(&self) -> String {
        done_text(
            self.uploads,
            self.downloads,
            self.done as u64,
            self.failed as u64,
        )
    }

    /// "4 done · 1 failed".
    fn detail(&self) -> String {
        let mut parts = vec![tf("{} done", &[&self.done.to_string()])];
        if self.failed > 0 {
            parts.push(tf("{} failed", &[&self.failed.to_string()]));
        }
        if self.cancelled > 0 {
            parts.push(tf("{} cancelled", &[&self.cancelled.to_string()]));
        }
        parts.join(" · ")
    }
}

/// What a click in a transfer row asks for.
enum RowAction {
    Remove(u64),
    Retry(u64),
}

/// A modification time that fits its column: "Today 16:05", "Yesterday", "Nov 2 16:05",
/// "2025-10-31".
fn short_time(secs: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(at) = Local.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if at.date_naive() == now.date_naive() {
        return tf("Today {}", &[&at.format("%H:%M").to_string()]);
    }
    if now.date_naive().pred_opt() == Some(at.date_naive()) {
        return t("Yesterday").to_string();
    }
    if at.year() == now.year() {
        return format!("{} {} {}", month(at.month0()), at.day(), at.format("%H:%M"));
    }
    at.format("%Y-%m-%d").to_string()
}

/// A day: "today", "Nov 2", "2025-10-31" (the Replace dialog).
fn short_day(secs: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(at) = Local.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if at.date_naive() == now.date_naive() {
        t("today").to_string()
    } else if at.year() == now.year() {
        format!("{} {}", month(at.month0()), at.day())
    } else {
        at.format("%Y-%m-%d").to_string()
    }
}

fn month(m0: u32) -> &'static str {
    [
        t("Jan"),
        t("Feb"),
        t("Mar"),
        t("Apr"),
        t("May"),
        t("Jun"),
        t("Jul"),
        t("Aug"),
        t("Sep"),
        t("Oct"),
        t("Nov"),
        t("Dec"),
    ][m0.min(11) as usize]
}

/// "just now", "4 min ago", "2 h ago".
fn ago(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        t("just now").to_string()
    } else if secs < 3600 {
        tf("{} min ago", &[&(secs / 60).to_string()])
    } else {
        tf("{} h ago", &[&(secs / 3600).to_string()])
    }
}

/// The columns of a file list between `left` and `right`: Name takes what Size (62, or 56 on
/// the remote side), Modified (92) and Permissions (72, remote) leave, 12 px apart.
struct Columns {
    spans: [(f32, f32); 4],
    /// Which columns fit: in a narrow side Permissions goes first, then Modified, so the names
    /// keep at least `NAME_MIN`.
    shown: [bool; 4],
}

impl Columns {
    fn new(left: f32, right: f32, remote: bool) -> Self {
        const GAP: f32 = 12.0;
        const NAME_MIN: f32 = 130.0;
        const MOD_W: f32 = 92.0;
        const PERM_W: f32 = 72.0;
        let size_w = if remote { 56.0 } else { 62.0 };
        let width = right - left;
        let with_mod = NAME_MIN + GAP + size_w + GAP + MOD_W;
        let show_mod = width >= with_mod;
        let show_perm = remote && width >= with_mod + GAP + PERM_W;
        let mut spans = [(0.0, 0.0); 4];
        let mut x = right;
        if show_perm {
            spans[3] = (x - PERM_W, x);
            x -= PERM_W + GAP;
        }
        if show_mod {
            spans[2] = (x - MOD_W, x);
            x -= MOD_W + GAP;
        }
        spans[1] = (x - size_w, x);
        x -= size_w + GAP;
        spans[0] = (left, x.max(left + 40.0));
        Self {
            spans,
            shown: [true, true, show_mod, show_perm],
        }
    }

    fn span(&self, i: usize) -> (f32, f32) {
        self.spans[i]
    }

    fn shown(&self, i: usize) -> bool {
        self.shown[i]
    }
}

/// The kind of a listed entry (its icon and colour).
fn entry_kind(e: &FileEntry) -> icons::FileKind {
    icons::FileKind::of(
        &e.name,
        e.is_dir && !e.is_symlink,
        e.is_symlink,
        e.mode.is_some_and(|m| m & 0o111 != 0),
    )
}

/// One file row's content: the type-coloured icon, the name (with a link's target), size,
/// modification time and, on the server, permissions.
fn paint_file_row(
    painter: &egui::Painter,
    rect: egui::Rect,
    e: &FileEntry,
    cols: &Columns,
    remote: bool,
    selected: bool,
    p: &style::Palette,
) {
    let cy = rect.center().y;
    let (n0, n1) = cols.span(0);
    icons::paint_file_icon(painter, pos2(n0 + 8.0, cy), 16.0, entry_kind(e), p);
    let meta = if selected { p.text_muted } else { p.text_faint };
    let name_x = n0 + 24.0;
    let clip = egui::Rect::from_min_max(pos2(name_x, rect.top()), pos2(n1, rect.bottom()));
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &e.name,
        0.0,
        egui::TextFormat {
            font_id: style::regular(style::BODY),
            color: p.text,
            ..Default::default()
        },
    );
    if let Some(target) = &e.link_target {
        job.append(
            &format!("→ {target}"),
            6.0,
            egui::TextFormat {
                font_id: style::regular(11.5),
                color: meta,
                ..Default::default()
            },
        );
    }
    job.wrap.max_width = (n1 - name_x).max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let g = painter.layout_job(job);
    painter
        .with_clip_rect(clip)
        .galley(pos2(name_x, cy - g.size().y / 2.0), g, p.text);
    let small = style::regular(style::SMALL);
    if !e.is_dir {
        let (_, s1) = cols.span(1);
        painter.text(
            pos2(s1, cy),
            Align2::RIGHT_CENTER,
            entry::human_size(e.size),
            small.clone(),
            meta,
        );
    }
    if cols.shown(2) {
        let (m0, m1) = cols.span(2);
        painter
            .with_clip_rect(egui::Rect::from_min_max(
                pos2(m0, rect.top()),
                pos2(m1, rect.bottom()),
            ))
            .text(
                pos2(m0, cy),
                Align2::LEFT_CENTER,
                e.modified.map(short_time).unwrap_or_default(),
                small,
                meta,
            );
    }
    if remote && cols.shown(3) {
        let (r0, _) = cols.span(3);
        painter.text(
            pos2(r0, cy),
            Align2::LEFT_CENTER,
            e.mode.map(entry::mode_string).unwrap_or_default(),
            style::mono(11.0),
            meta,
        );
    }
}

/// One line of text cut with "…" to `width`.
fn single_line(
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap.max_width = width.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

/// A translated template (`"Upload to {}"`) with each `{}` replaced by an argument in its own
/// format, the rest in `base`.
fn styled_tf(
    template: &str,
    args: &[(String, egui::TextFormat)],
    base: egui::TextFormat,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut rest = template;
    let mut args = args.iter();
    while let Some(pos) = rest.find("{}") {
        if pos > 0 {
            job.append(&rest[..pos], 0.0, base.clone());
        }
        if let Some((text, format)) = args.next() {
            job.append(text, 0.0, format.clone());
        }
        rest = &rest[pos + 2..];
    }
    if !rest.is_empty() {
        job.append(rest, 0.0, base);
    }
    job
}

/// A progress bar on a quiet track.
fn paint_track(
    painter: &egui::Painter,
    rect: egui::Rect,
    fraction: f32,
    color: egui::Color32,
    p: &style::Palette,
) {
    let radius = CornerRadius::same((rect.height() / 2.0).ceil() as u8);
    painter.rect_filled(rect, radius, p.subtle);
    let mut done = rect;
    done.set_width(rect.width() * fraction.clamp(0.0, 1.0));
    if done.width() > 0.5 {
        painter.rect_filled(done, radius, color);
    }
}

/// A dialog's header: the title (16 px semibold) and where it acts (Redesign 3.1: no icon
/// tile).
fn dialog_head(ui: &mut Ui, title: &str, sub: Option<&str>) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 18,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
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
                    if let Some(s) = sub {
                        ui.add(
                            egui::Label::new(
                                RichText::new(s)
                                    .font(style::regular(style::BODY))
                                    .color(p.text_muted),
                            )
                            .wrap(),
                        );
                    }
                });
            });
        });
}

/// A dialog's body: 14 × 20 above and beside, 18 below.
fn dialog_body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 14,
            bottom: 18,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui)
        })
        .inner
}

/// A validation error under a field.
fn error_line(ui: &mut Ui, text: &str) {
    let p = style::pal(ui);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        kit::icon(ui, icons::WARNING_CIRCLE, 14.0, p.err);
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .font(style::regular(style::SMALL))
                    .color(p.err),
            )
            .wrap(),
        );
    });
}

/// The items of a Delete or Replace dialog in a well, "and N more" below them.
fn item_list(ui: &mut Ui, items: &[DialogItem], total: usize) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for it in items {
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
                let painter = ui.painter();
                icons::paint_file_icon(
                    painter,
                    pos2(r.left() + 8.0 + 8.0, r.center().y),
                    16.0,
                    it.kind,
                    &p,
                );
                // The detail on the right: the new one, then the existing one fainter.
                let mut job = egui::text::LayoutJob::default();
                let small = |color| egui::TextFormat {
                    font_id: style::regular(style::SMALL),
                    color,
                    ..Default::default()
                };
                job.append(&it.meta, 0.0, small(p.text_faint));
                if !it.old.is_empty() {
                    job.append(&format!("← {}", it.old), 6.0, small(p.text_ghost));
                }
                let meta = painter.layout_job(job);
                let mw = meta.size().x;
                painter.galley(
                    pos2(r.right() - 8.0 - mw, r.center().y - meta.size().y / 2.0),
                    meta,
                    p.text_faint,
                );
                let name_x = r.left() + 8.0 + 16.0 + 9.0;
                let g = painter.layout_job(single_line(
                    &it.name,
                    style::regular(style::BODY),
                    p.text,
                    r.right() - 8.0 - mw - 10.0 - name_x,
                ));
                painter.galley(pos2(name_x, r.center().y - g.size().y / 2.0), g, p.text);
            }
            if total > items.len() {
                let (r, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
                ui.painter().text(
                    pos2(r.left() + 8.0, r.center().y),
                    Align2::LEFT_CENTER,
                    tf("and {} more", &[&(total - items.len()).to_string()]),
                    style::regular(style::SMALL),
                    p.text_faint,
                );
            }
        });
}

/// Read / Write / Execute for Owner, Group and Others, kept in step with the octal field.
fn permission_matrix(ui: &mut Ui, mode: &mut String) {
    let p = style::pal(ui);
    let mut bits = entry::parse_mode(mode).unwrap_or(0);
    let width = ui.available_width();
    let label_w = width - 3.0 * 64.0;
    let (head, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    for (i, h) in [t("Read"), t("Write"), t("Execute")].iter().enumerate() {
        ui.painter().text(
            pos2(
                head.left() + label_w + 32.0 + 64.0 * i as f32,
                head.top() + 8.0,
            ),
            Align2::CENTER_CENTER,
            h,
            style::medium(style::XS),
            p.text_faint,
        );
    }
    for (who, shift) in [(t("Owner"), 6), (t("Group"), 3), (t("Others"), 0)] {
        let (row, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
        ui.painter().text(
            pos2(row.left(), row.center().y),
            Align2::LEFT_CENTER,
            who,
            style::regular(style::BODY),
            p.text,
        );
        for (i, bit) in [4u32, 2, 1].into_iter().enumerate() {
            let mask = bit << shift;
            let mut on = bits & mask != 0;
            let cell = egui::Rect::from_center_size(
                pos2(
                    row.left() + label_w + 32.0 + 64.0 * i as f32,
                    row.center().y,
                ),
                vec2(16.0, row.height()),
            );
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(cell));
            if style::checkbox(&mut c, &mut on, "").changed() {
                bits = if on { bits | mask } else { bits & !mask };
                *mode = format!("{bits:o}");
            }
        }
    }
}

/// Folder suggestions under the "Go to folder…" field; returns a clicked one.
fn suggestion_list(ui: &mut Ui, matches: &[String], pick: Option<usize>) -> Option<usize> {
    let p = style::pal(ui);
    let mut clicked = None;
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (i, m) in matches.iter().enumerate() {
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                let painter = ui.painter();
                if pick == Some(i) {
                    painter.rect_filled(r, CornerRadius::same(style::RADIUS), p.accent_soft);
                } else if resp.hovered() {
                    painter.rect_filled(r, CornerRadius::same(style::RADIUS), p.hover);
                }
                icons::paint_file_icon(
                    painter,
                    pos2(r.left() + 8.0 + 8.0, r.center().y),
                    16.0,
                    icons::FileKind::Folder,
                    &p,
                );
                painter.with_clip_rect(r.shrink2(vec2(8.0, 0.0))).text(
                    pos2(r.left() + 8.0 + 16.0 + 9.0, r.center().y),
                    Align2::LEFT_CENTER,
                    m,
                    style::mono(12.0),
                    p.text,
                );
                if pick == Some(i) {
                    painter.text(
                        pos2(r.right() - 8.0, r.center().y),
                        Align2::RIGHT_CENTER,
                        "↩",
                        style::regular(style::SMALL),
                        p.text_faint,
                    );
                }
                if resp.clicked() {
                    clicked = Some(i);
                }
            }
        });
    clicked
}

/// The folder part of a typed path, with its trailing slash ("/var/log/" of "/var/log/ng").
fn goto_parent(text: &str) -> String {
    match text.rfind('/') {
        Some(i) => text[..=i].to_string(),
        None => String::new(),
    }
}

/// How many characters of a file name come before its extension (all of them for folders,
/// dot-files and names without one).
fn stem_chars(name: &str) -> usize {
    match name.rfind('.') {
        Some(i) if i > 0 => name[..i].chars().count(),
        _ => name.chars().count(),
    }
}

/// A folder for a dialog's subtitle: `~` for the home folder, and only its last folders when
/// it is long ("…/remote/srv/app").
fn short_dir(dir: &str, remote: bool) -> String {
    let dir = if remote { dir.to_string() } else { tilde(dir) };
    const MAX: usize = 40;
    if dir.chars().count() <= MAX {
        return dir;
    }
    let sep = if dir.contains('\\') && !dir.contains('/') {
        '\\'
    } else {
        '/'
    };
    let mut tail = String::new();
    for part in dir.rsplit(sep).filter(|p| !p.is_empty()) {
        let next = if tail.is_empty() {
            part.to_string()
        } else {
            format!("{part}{sep}{tail}")
        };
        if next.chars().count() > MAX - 2 && !tail.is_empty() {
            break;
        }
        tail = next;
    }
    format!("…{sep}{tail}")
}

/// A local path with the home folder written as `~`.
fn tilde(path: &str) -> String {
    let home = paths::home_dir().display().to_string();
    match path.strip_prefix(&home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') || rest.starts_with('\\') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// The breadcrumb of a folder: each segment's name and the path up to it. Remote paths start
/// at `/`; local folders under the home folder start at it ("ilham › projects").
fn crumb_segments(cwd: &str, remote: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if remote || !cfg!(windows) {
        let home = (!remote).then(|| paths::home_dir().display().to_string());
        let under_home = home
            .as_deref()
            .filter(|h| !h.is_empty() && (cwd == *h || cwd.starts_with(&format!("{h}/"))));
        let (mut path, rest) = match under_home {
            Some(h) => {
                let name = Path::new(h)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "~".into());
                out.push((name, h.to_string()));
                (h.to_string(), &cwd[h.len()..])
            }
            None => {
                if cwd.starts_with('/') {
                    out.push(("/".to_string(), "/".to_string()));
                }
                (String::new(), cwd)
            }
        };
        for part in rest.split('/').filter(|s| !s.is_empty()) {
            path.push('/');
            path.push_str(part);
            out.push((part.to_string(), path.clone()));
        }
        if out.is_empty() {
            out.push(("/".to_string(), "/".to_string()));
        }
    } else {
        let mut acc = PathBuf::new();
        for comp in Path::new(cwd).components() {
            acc.push(comp.as_os_str());
            let name = comp
                .as_os_str()
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_string();
            if !name.is_empty() {
                out.push((name, acc.display().to_string()));
            }
        }
    }
    out
}

/// The names of files being dragged in from the file manager.
pub fn hovered_names(files: &[egui::HoveredFile]) -> Vec<String> {
    files
        .iter()
        .filter_map(|f| f.path.as_ref())
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}

/// The layer shown while files from the file manager hover the window: one honest target, the
/// remote folder that is open (macOS doesn't say where they will be dropped), and the files.
pub fn paint_drop_layer(
    painter: &egui::Painter,
    rect: egui::Rect,
    p: &style::Palette,
    folder: Option<&str>,
    host: &str,
    names: &[String],
) {
    painter.rect_filled(rect, 0.0, style::soft(p.canvas, 0.93));
    let zone = rect.shrink(10.0);
    painter.rect_filled(zone, CornerRadius::same(style::RADIUS_LG), p.accent_softer);
    kit::dashed_rect(
        painter,
        zone,
        style::RADIUS_LG as f32,
        Stroke::new(1.5, p.accent_line),
    );
    let chips = !names.is_empty();
    let block_h = 56.0 + 10.0 + 22.0 + 10.0 + 18.0 + if chips { 16.0 + 22.0 } else { 0.0 };
    let mut y = zone.center().y - block_h / 2.0;
    let cx = zone.center().x;
    let tile = egui::Rect::from_center_size(pos2(cx, y + 28.0), vec2(56.0, 56.0));
    let shadow = egui::Shadow {
        offset: [0, 10],
        blur: 30,
        spread: 0,
        color: style::soft(p.accent, 0.4),
    };
    painter.add(shadow.as_shape(tile, CornerRadius::same(16)));
    painter.rect_filled(tile, CornerRadius::same(16), p.accent);
    painter.text(
        tile.center(),
        Align2::CENTER_CENTER,
        icons::UPLOAD_SIMPLE,
        style::icon_font(28.0),
        egui::Color32::WHITE,
    );
    y += 56.0 + 10.0;
    painter.text(
        pos2(cx, y + 11.0),
        Align2::CENTER_CENTER,
        t("Drop to upload"),
        style::semibold(18.0),
        p.text,
    );
    y += 22.0 + 10.0;
    let base = egui::TextFormat {
        font_id: style::regular(style::BODY),
        color: p.text_muted,
        ..Default::default()
    };
    let job = match folder.filter(|f| !f.is_empty()) {
        Some(f) => styled_tf(
            t("to {} on {}"),
            &[
                (
                    short_dir(f, true),
                    egui::TextFormat {
                        font_id: style::mono(12.0),
                        color: p.text,
                        ..Default::default()
                    },
                ),
                (
                    host.to_string(),
                    egui::TextFormat {
                        font_id: style::semibold(style::BODY),
                        color: p.text,
                        ..Default::default()
                    },
                ),
            ],
            base,
        ),
        None => {
            egui::text::LayoutJob::single_section(tf("to the remote folder on {}", &[host]), base)
        }
    };
    let g = painter.layout_job(job);
    painter.galley(
        pos2(cx - g.size().x / 2.0, y + 9.0 - g.size().y / 2.0),
        g,
        p.text_muted,
    );
    y += 18.0 + 16.0;
    if chips {
        let mut parts: Vec<(Option<icons::FileKind>, String)> = names
            .iter()
            .take(3)
            .map(|n| (Some(icons::FileKind::of(n, false, false, false)), n.clone()))
            .collect();
        if names.len() > 3 {
            parts.push((None, tf("and {} more", &[&(names.len() - 3).to_string()])));
        }
        let galleys: Vec<_> = parts
            .iter()
            .map(|(_, n)| {
                painter.layout_no_wrap(n.clone(), style::regular(style::SMALL), p.text_muted)
            })
            .collect();
        let widths: Vec<f32> = parts
            .iter()
            .zip(&galleys)
            .map(|((k, _), g)| g.size().x + 16.0 + if k.is_some() { 12.0 + 5.0 } else { 0.0 })
            .collect();
        let total: f32 = widths.iter().sum::<f32>() + 6.0 * (widths.len() - 1) as f32;
        let mut x = cx - total / 2.0;
        for (((kind, _), g), w) in parts.iter().zip(galleys).zip(widths) {
            let chip = egui::Rect::from_min_size(pos2(x, y), vec2(w, 22.0));
            painter.rect_filled(chip, CornerRadius::same(11), p.hover);
            let mut tx = chip.left() + 8.0;
            if let Some(k) = kind {
                let (icon, _) = k.icon(p);
                painter.text(
                    pos2(tx + 6.0, chip.center().y),
                    Align2::CENTER_CENTER,
                    icon,
                    style::icon_font(12.0),
                    p.text_muted,
                );
                tx += 12.0 + 5.0;
            }
            painter.galley(
                pos2(tx, chip.center().y - g.size().y / 2.0),
                g,
                p.text_muted,
            );
            x += w + 6.0;
        }
    }
}

/// The width a small button with an icon and `text` takes.
fn text_button_w(ui: &Ui, text: &str, small: bool, icon: bool) -> f32 {
    let size = if small { style::SMALL } else { style::BODY };
    let g =
        ui.painter()
            .layout_no_wrap(text.to_string(), style::medium(size), egui::Color32::WHITE);
    g.size().x + 2.0 * if small { 9.0 } else { 12.0 } + if icon { 14.0 + 5.0 } else { 0.0 }
}

/// The width of a group of `n` toolbar buttons (24 px each, 1 apart, 2 px inside).
fn group_width(n: usize) -> f32 {
    2.0 + 24.0 * n as f32 + (n.saturating_sub(1)) as f32 + 2.0
}

/// A group of small icon buttons on one quiet track: (icon, tooltip, on, busy). A busy
/// button shows a spinner.
fn tool_group(
    ui: &mut Ui,
    rect: egui::Rect,
    id: egui::Id,
    items: &[(&str, &str, bool, bool)],
) -> Vec<egui::Response> {
    let p = style::pal(ui);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(style::RADIUS), p.hover);
    let enabled = ui.is_enabled();
    items
        .iter()
        .enumerate()
        .map(|(i, (icon, tip, on, busy))| {
            let r = egui::Rect::from_min_size(
                pos2(rect.left() + 2.0 + 25.0 * i as f32, rect.top() + 2.0),
                vec2(24.0, 22.0),
            );
            let resp = ui
                .interact(r, id.with(i), Sense::click())
                .on_hover_text(*tip);
            resp.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, *on, *tip)
            });
            let hovered = enabled && resp.hovered();
            let painter = ui.painter();
            if *on {
                painter.rect_filled(r, CornerRadius::same(4), p.accent_soft);
            } else if enabled && resp.is_pointer_button_down_on() {
                painter.rect_filled(r, CornerRadius::same(4), p.subtle);
            } else if hovered {
                painter.rect_filled(r, CornerRadius::same(4), p.hover);
            }
            if *busy {
                kit::paint_spinner(ui, r.center(), 6.0);
            } else {
                painter.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    *icon,
                    style::icon_font(14.0),
                    if *on || hovered { p.text } else { p.text_muted },
                );
            }
            resp
        })
        .collect()
}

/// The Root toggle: a quiet button at rest, root-tinted with a ring while the side runs as
/// root.
fn paint_root_button(ui: &Ui, resp: &egui::Response, rect: egui::Rect, on: bool, compact: bool) {
    let p = style::pal(ui);
    let label = t("Root");
    let hovered = resp.hovered() && ui.is_enabled();
    let (fill, color) = if on {
        (p.tone_soft(Tone::Root), p.root)
    } else if hovered {
        (p.hover, p.text)
    } else {
        (egui::Color32::TRANSPARENT, p.text_muted)
    };
    let color = if ui.is_enabled() {
        color
    } else {
        color.gamma_multiply(0.45)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(style::RADIUS), fill);
    if on {
        painter.rect_stroke(
            rect,
            CornerRadius::same(style::RADIUS),
            Stroke::new(1.0, style::soft(p.root, 0.3)),
            StrokeKind::Inside,
        );
    }
    let g = painter.layout_no_wrap(label.to_string(), style::medium(style::SMALL), color);
    let w = if compact {
        14.0
    } else {
        14.0 + 5.0 + g.size().x
    };
    let x = rect.center().x - w / 2.0;
    painter.text(
        pos2(x + 7.0, rect.center().y),
        Align2::CENTER_CENTER,
        if on {
            icons::SHIELD_CHECK
        } else {
            icons::SHIELD
        },
        style::icon_font(14.0),
        color,
    );
    if !compact {
        painter.galley(pos2(x + 19.0, rect.center().y - g.size().y / 2.0), g, color);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
}

/// The confirming button of the sudo dialog: filled in the root colour.
fn root_button_filled(ui: &mut Ui, label: &str) -> egui::Response {
    let p = style::pal(ui);
    let g = ui.painter().layout_no_wrap(
        label.to_string(),
        style::medium(style::BODY),
        egui::Color32::WHITE,
    );
    let (rect, resp) =
        ui.allocate_exact_size(vec2(g.size().x + 24.0, style::CONTROL_H), Sense::click());
    let enabled = ui.is_enabled();
    let fill = if enabled && resp.hovered() {
        p.root.gamma_multiply(0.92)
    } else {
        p.root
    };
    let fill = if enabled {
        fill
    } else {
        fill.gamma_multiply(0.45)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(style::RADIUS), fill);
    ui.painter()
        .galley(rect.center() - g.size() / 2.0, g, egui::Color32::WHITE);
    resp
}

/// The right-click menu of a file: open or edit, transfer, then rename, permissions and copy
/// path, and delete at the bottom.
fn file_menu(ui: &mut Ui, target: &MenuTarget, is_remote: bool, actions: &mut Vec<SideAction>) {
    use super::keys::FileKey;
    if is_remote
        && !target.entry.is_dir
        && target.names.len() == 1
        && kit::menu_item(
            ui,
            Some(icons::PENCIL_SIMPLE),
            t("Edit"),
            Some(FileKey::Open.label()),
        )
        .clicked()
    {
        actions.push(SideAction::Edit {
            dir: target.dir.clone(),
            name: target.entry.name.clone(),
        });
        ui.close();
    }
    if target.entry.is_dir
        && kit::menu_item(
            ui,
            Some(icons::FOLDER_OPEN),
            t("Open"),
            Some(FileKey::Open.label()),
        )
        .clicked()
    {
        actions.push(SideAction::Open {
            remote: is_remote,
            dir: target.dir.clone(),
            entry: target.entry.clone(),
        });
        ui.close();
    }
    let (label, icon, key) = if is_remote {
        (t("Download"), icons::DOWNLOAD_SIMPLE, FileKey::Download)
    } else {
        (t("Upload"), icons::UPLOAD_SIMPLE, FileKey::Upload)
    };
    if kit::menu_item(ui, Some(icon), label, Some(key.label())).clicked() {
        actions.push(SideAction::Transfer {
            from_remote: is_remote,
            dir: target.dir.clone(),
            names: target.names.clone(),
        });
        ui.close();
    }
    kit::menu_sep(ui);
    let one = target.names.len() == 1;
    if ui
        .add_enabled_ui(one, |ui| {
            kit::menu_item(
                ui,
                Some(icons::CURSOR_TEXT),
                t("Rename…"),
                Some(FileKey::Rename.label()),
            )
        })
        .inner
        .clicked()
    {
        actions.push(SideAction::Rename {
            remote: is_remote,
            dir: target.dir.clone(),
            name: target.entry.name.clone(),
        });
        ui.close();
    }
    if is_remote {
        let can_chmod = one && target.entry.mode.is_some();
        if ui
            .add_enabled_ui(can_chmod, |ui| {
                kit::menu_item(ui, Some(icons::LOCK_KEY), t("Permissions…"), None)
            })
            .inner
            .clicked()
        {
            actions.push(SideAction::Chmod {
                path: remote_join(&target.dir, &target.entry.name),
                mode: target.entry.mode.unwrap_or(0),
            });
            ui.close();
        }
    }
    if kit::menu_item(
        ui,
        Some(icons::COPY),
        t("Copy Path"),
        Some(FileKey::CopyPath.label()),
    )
    .clicked()
    {
        let paths: Vec<String> = target
            .names
            .iter()
            .map(|name| {
                if is_remote {
                    remote_join(&target.dir, name)
                } else {
                    Path::new(&target.dir).join(name).display().to_string()
                }
            })
            .collect();
        ui.ctx().copy_text(paths.join("\n"));
        ui.close();
    }
    if !is_remote
        && kit::menu_item(
            ui,
            Some(icons::ARROW_SQUARE_OUT),
            crate::platform::reveal_label(),
            None,
        )
        .clicked()
    {
        crate::platform::reveal(&Path::new(&target.dir).join(&target.entry.name));
        ui.close();
    }
    kit::menu_sep(ui);
    if kit::menu_item_ex(
        ui,
        Some(icons::TRASH),
        t("Delete…"),
        Some(FileKey::Delete.label()),
        true,
        false,
    )
    .clicked()
    {
        actions.push(SideAction::Delete {
            remote: is_remote,
            dir: target.dir.clone(),
            names: target.names.clone(),
        });
        ui.close();
    }
}

/// A message in the transfers list (an error or a note), with × to dismiss it. Returns true
/// when dismissed.
fn message_row(ui: &mut Ui, msg: &str, is_err: bool, uid: u64) -> bool {
    let p = style::pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(POP_W - 12.0, XFER_ROW_H), Sense::hover());
    let painter = ui.painter();
    let lead = pos2(rect.left() + 8.0 + 9.0, rect.center().y);
    if is_err {
        icons::paint_badge_circle(painter, lead, 16.0, "!", p.err, p.raised);
    } else {
        painter.text(
            lead,
            Align2::CENTER_CENTER,
            icons::INFO,
            style::icon_font(15.0),
            p.text_muted,
        );
    }
    let x_rect = egui::Rect::from_center_size(
        pos2(rect.right() - 8.0 - 11.0, rect.center().y),
        vec2(20.0, 20.0),
    );
    let g = painter.layout_job(single_line(
        msg,
        style::regular(style::SMALL),
        if is_err { p.err } else { p.text_muted },
        x_rect.left() - 8.0 - (lead.x + 19.0),
    ));
    painter.galley(
        pos2(lead.x + 19.0, rect.center().y - g.size().y / 2.0),
        g,
        p.text,
    );
    let x = ui
        .interact(x_rect, egui::Id::new(("sftp-msg-x", uid)), Sense::click())
        .on_hover_text(t("Dismiss"));
    kit::paint_icon_button(ui, &x, x_rect, icons::X, t("Dismiss"), false, false);
    x.clicked()
}

/// One transfer (40 px): its state, the name and where to, progress or how it ended, the size
/// (or Retry), and × to cancel it.
fn transfer_row(ui: &mut Ui, tr: &Transfer, is_root: bool) -> Option<RowAction> {
    let mut out = None;
    let p = style::pal(ui);
    let state = tr.state();
    let done = tr.done.load(std::sync::atomic::Ordering::Relaxed);
    let total = tr.total.load(std::sync::atomic::Ordering::Relaxed);
    let skipped = tr.skipped.lock().clone();
    let (rect, resp) = ui.allocate_exact_size(vec2(POP_W - 12.0, XFER_ROW_H), Sense::hover());
    let failed = matches!(state, TransferState::Failed(_));
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), p.hover);
    }
    // Columns: 18 | name | 110 | 80 | 22, 10 apart, 8 in from the sides.
    let inner = rect.shrink2(vec2(8.0, 0.0));
    let x_lead = inner.left();
    let x_close = inner.right() - 22.0;
    let x_act = x_close - 10.0 - 80.0;
    let x_mid = x_act - 10.0 - 110.0;
    let x_name = x_lead + 18.0 + 10.0;
    let name_w = x_mid - 10.0 - x_name;
    let cy = rect.center().y;
    let lead = pos2(x_lead + 9.0, cy);
    match &state {
        TransferState::Done if skipped.is_empty() => {
            icons::paint_badge_circle(painter, lead, 16.0, icons::CHECK, p.ok, p.raised)
        }
        TransferState::Done => {
            icons::paint_badge_circle(painter, lead, 16.0, "!", p.warn, p.raised)
        }
        TransferState::Failed(_) => {
            icons::paint_badge_circle(painter, lead, 16.0, "!", p.err, p.raised)
        }
        TransferState::Queued => {
            painter.text(
                lead,
                Align2::CENTER_CENTER,
                icons::CLOCK,
                style::icon_font(15.0),
                p.text_faint,
            );
        }
        TransferState::Cancelled => {
            painter.text(
                lead,
                Align2::CENTER_CENTER,
                icons::X_CIRCLE,
                style::icon_font(15.0),
                p.text_faint,
            );
        }
        TransferState::Running => style::progress_ring(painter, lead, 7.0, tr.fraction(), p.dark),
    }
    let name_g = painter.layout_job(single_line(
        &tr.name,
        style::medium(style::BODY),
        p.text,
        name_w,
    ));
    painter.galley(
        pos2(x_name, cy - 8.0 - name_g.size().y / 2.0 + 1.0),
        name_g,
        p.text,
    );
    let dir = match tr.direction {
        Direction::Upload => remote_parent(&tr.remote),
        Direction::Download => tr
            .local
            .parent()
            .map(|d| tilde(&d.display().to_string()))
            .unwrap_or_default(),
    };
    let (sub, sub_color) = match &state {
        TransferState::Failed(e) => {
            // "…/nginx.conf: permission denied": the path is in the row already.
            let mut e = capitalized(e.rsplit_once(": ").map_or(e.as_str(), |(_, m)| m));
            if tr.direction == Direction::Upload
                && !is_root
                && e.to_lowercase().contains("permission denied")
            {
                e = format!("{e} — {}", t("try Root"));
            }
            (e, p.err)
        }
        _ => (
            tf(
                "to {}",
                &[&short_dir(&dir, tr.direction == Direction::Upload)],
            ),
            p.text_faint,
        ),
    };
    let sub_g = painter.layout_job(single_line(&sub, style::regular(11.5), sub_color, name_w));
    painter.galley(
        pos2(x_name, cy + 8.0 - sub_g.size().y / 2.0),
        sub_g,
        sub_color,
    );
    let (from, to) = match tr.direction {
        Direction::Upload => (tr.local.display().to_string(), tr.remote.clone()),
        Direction::Download => (tr.remote.clone(), tr.local.display().to_string()),
    };
    let name_rect = egui::Rect::from_min_max(
        pos2(x_name, rect.top()),
        pos2(x_name + name_w, rect.bottom()),
    );
    let tip = match &state {
        TransferState::Failed(e) => format!("{from}\n→ {to}\n\n{e}"),
        _ => format!("{from}\n→ {to}"),
    };
    ui.interact(name_rect, egui::Id::new(("tr-name", tr.id)), Sense::hover())
        .on_hover_text(tip);
    let painter = ui.painter();
    let small = style::regular(style::SMALL);
    match &state {
        TransferState::Running => {
            let bar = egui::Rect::from_min_size(pos2(x_mid, cy - 9.0), vec2(110.0, 3.0));
            paint_track(painter, bar, tr.fraction(), p.accent, &p);
            painter.text(
                pos2(x_mid, cy + 6.0),
                Align2::LEFT_CENTER,
                format!(
                    "{}% · {}/s",
                    (tr.fraction() * 100.0).floor() as u32,
                    entry::human_size(tr.speed() as u64)
                ),
                small.clone(),
                p.text_faint,
            );
        }
        TransferState::Queued => {
            painter.text(
                pos2(x_mid, cy),
                Align2::LEFT_CENTER,
                t("Waiting…"),
                small.clone(),
                p.text_faint,
            );
        }
        TransferState::Done if skipped.is_empty() => {
            painter.text(
                pos2(x_mid, cy),
                Align2::LEFT_CENTER,
                t("Done"),
                small.clone(),
                p.text_faint,
            );
        }
        TransferState::Done => {
            painter.text(
                pos2(x_mid, cy),
                Align2::LEFT_CENTER,
                tf("{} skipped", &[&skipped.len().to_string()]),
                small.clone(),
                p.warn,
            );
            let r = egui::Rect::from_min_size(pos2(x_mid, rect.top()), vec2(110.0, rect.height()));
            ui.interact(r, egui::Id::new(("tr-skip", tr.id)), Sense::hover())
                .on_hover_text(
                    skipped
                        .iter()
                        .take(20)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
        }
        TransferState::Cancelled => {
            painter.text(
                pos2(x_mid, cy),
                Align2::LEFT_CENTER,
                t("Cancelled"),
                small.clone(),
                p.text_faint,
            );
        }
        TransferState::Failed(_) => {}
    }
    if failed {
        let g =
            ui.painter()
                .layout_no_wrap(t("Retry").to_string(), style::medium(style::SMALL), p.err);
        let r = egui::Rect::from_min_size(
            pos2(x_act + 80.0 - g.size().x - 12.0, cy - 11.0),
            vec2(g.size().x + 12.0, 22.0),
        );
        let retry = ui.interact(r, egui::Id::new(("tr-retry", tr.id)), Sense::click());
        if retry.hovered() {
            ui.painter()
                .rect_filled(r, CornerRadius::same(style::RADIUS), p.hover);
        }
        ui.painter().galley(r.center() - g.size() / 2.0, g, p.err);
        if retry.clicked() {
            out = Some(RowAction::Retry(tr.id));
        }
    } else {
        let size = match &state {
            TransferState::Running | TransferState::Queued | TransferState::Done if total > 0 => {
                entry::human_size(total)
            }
            _ => String::new(),
        };
        let _ = done;
        ui.painter().text(
            pos2(x_act + 80.0, cy),
            Align2::RIGHT_CENTER,
            size,
            small,
            p.text_faint,
        );
    }
    if !tr.is_finished() {
        let x_rect = egui::Rect::from_center_size(pos2(x_close + 11.0, cy), vec2(20.0, 20.0));
        let x = ui
            .interact(x_rect, egui::Id::new(("tr-x", tr.id)), Sense::click())
            .on_hover_text(t("Cancel"));
        kit::paint_icon_button(ui, &x, x_rect, icons::X, t("Cancel"), false, false);
        if x.clicked() {
            tr.cancel();
        }
    }
    let _ = RowAction::Remove;
    out
}

/// "This Mac" (the platform's name for this computer, starting a label); also "Permission
/// denied" from "permission denied".
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// "Uploading 7 files" (files inside folders count one by one).
pub fn summary_title(s: &Summary) -> String {
    let n = s.files_total;
    match (s.uploads > 0, s.downloads > 0) {
        (true, false) => tn("Uploading {} file", "Uploading {} files", n, &[]),
        (false, true) => tn("Downloading {} file", "Downloading {} files", n, &[]),
        _ => tn("Transferring {} file", "Transferring {} files", n, &[]),
    }
}

/// "Uploaded 2 files.", "Uploaded 1 file. 1 transfer failed." or, when nothing got through,
/// only "2 transfers failed."
fn done_text(uploads: bool, downloads: bool, done: u64, failed: u64) -> String {
    let failed_text = tn("{} transfer failed.", "{} transfers failed.", failed, &[]);
    if failed > 0 && done == 0 {
        return failed_text;
    }
    let mut text = match (uploads, downloads) {
        (true, false) => tn("Uploaded {} file.", "Uploaded {} files.", done, &[]),
        (false, true) => tn("Downloaded {} file.", "Downloaded {} files.", done, &[]),
        _ => tn("Transferred {} file.", "Transferred {} files.", done, &[]),
    };
    if failed > 0 {
        text.push(' ');
        text.push_str(&failed_text);
    }
    text
}

/// "56% · 236 of 419 MB · 12.4 MB/s · about 1 min left"
pub fn summary_meta(s: &Summary) -> String {
    let done = entry::human_size(s.bytes_done);
    let total = entry::human_size(s.bytes_total);
    // One unit for both when they share it: "236 of 419 MB".
    let done = match (done.rsplit_once(' '), total.rsplit_once(' ')) {
        (Some((n, u1)), Some((_, u2))) if u1 == u2 => n.to_string(),
        _ => done,
    };
    let mut text = format!(
        "{}% · {} · {}/s",
        (s.fraction() * 100.0).floor() as u32,
        tf("{} of {}", &[&done, &total]),
        entry::human_size(s.speed as u64)
    );
    if let Some(eta) = s.eta() {
        let secs = eta.as_secs();
        let left = if secs >= 3600 {
            tf(
                "{} h {} min",
                &[&(secs / 3600).to_string(), &(secs % 3600 / 60).to_string()],
            )
        } else if secs >= 60 {
            tf("{} min", &[&secs.div_ceil(60).to_string()])
        } else {
            tf("{} s", &[&secs.max(1).to_string()])
        };
        text.push_str(&format!(" · {}", tf("about {} left", &[&left])));
    }
    text
}

/// "3 done · 45% · 1.2 GB / 2.6 GB · 12.3 MB/s · about 20s left"
pub fn summary_detail(s: &Summary) -> String {
    summary_meta(s)
}

/// What to say when a batch has finished (nothing if it was all cancelled).
fn batch_notice(s: &Summary) -> Option<(String, bool)> {
    let text = done_text(
        s.uploads > 0,
        s.downloads > 0,
        s.files_done,
        s.failed as u64,
    );
    if s.failed > 0 {
        return Some((text, true));
    }
    (s.done > 0).then_some((text, false))
}

/// Whether two paths name one file (on a case-insensitive disk "readme" and "README" do).
fn same_local_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(ma), Ok(mb)) => ma.dev() == mb.dev() && ma.ino() == mb.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

fn ui_mods(resp: &egui::Response) -> (bool, bool) {
    let m = resp.ctx.input(|i| i.modifiers);
    (m.command, m.shift)
}

/// `color` mixed towards `other` by `amount` (0–1).
fn blend(color: egui::Color32, other: egui::Color32, amount: f32) -> egui::Color32 {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    egui::Color32::from_rgb(
        mix(color.r(), other.r()),
        mix(color.g(), other.g()),
        mix(color.b(), other.b()),
    )
}

// ---------------------------------------------------------------------------------------------
// The panel in its tab: docked under the terminal, or filling the tab
// ---------------------------------------------------------------------------------------------

/// Height of the terminal's one-line strip while the panel fills the tab.
pub const TERM_LINE_H: f32 = 34.0;

/// Where a tab's body goes: (the terminal, the Files panel, the terminal's one-line strip when
/// the panel fills the tab). `split` is the panel's share of the height when docked.
pub fn files_layout(
    body: egui::Rect,
    show: bool,
    maximized: bool,
    split: f32,
) -> (egui::Rect, Option<egui::Rect>, Option<egui::Rect>) {
    if !show {
        return (body, None, None);
    }
    if maximized {
        let line = egui::Rect::from_min_size(body.min, vec2(body.width(), TERM_LINE_H));
        let panel = egui::Rect::from_min_max(pos2(body.left(), line.bottom()), body.max);
        return (line, Some(panel), Some(line));
    }
    let split_y = (body.top() + body.height() * (1.0 - split)).round();
    (
        egui::Rect::from_min_max(body.min, pos2(body.right(), split_y)),
        Some(egui::Rect::from_min_max(
            pos2(body.left(), split_y),
            body.max,
        )),
        None,
    )
}

/// The top edge of the docked panel: a hairline with a grip in the middle (the divider that
/// resizes it is the caller's).
pub fn paint_panel_edge(
    painter: &egui::Painter,
    rect: egui::Rect,
    active: bool,
    p: &style::Palette,
) {
    painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
    let grip =
        egui::Rect::from_center_size(pos2(rect.center().x, rect.top() - 0.5), vec2(36.0, 5.0));
    painter.rect_filled(
        grip,
        CornerRadius::same(3),
        if active { p.accent } else { p.border_strong },
    );
}

/// The terminal shrunk to one line while the panel fills the tab: the line the cursor is on
/// (the prompt, in the terminal's colours) and "Show terminal". Returns true when that was
/// clicked.
pub fn terminal_line(
    ui: &mut Ui,
    rect: egui::Rect,
    shared: Option<&crate::term::core::TermShared>,
    scheme: &crate::term::scheme::Scheme,
    fill: egui::Color32,
) -> bool {
    let p = style::pal(ui);
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, fill);
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, p.border),
    );
    let cy = rect.center().y;
    painter.text(
        pos2(rect.left() + 14.0 + 7.5, cy),
        Align2::CENTER_CENTER,
        icons::TERMINAL_WINDOW,
        style::icon_font(15.0),
        p.text_muted,
    );
    // From the right: Show terminal ⇧⌘F.
    let label = t("Show terminal");
    let keys = super::keys::Shortcut::Sftp.label();
    let g = ui
        .painter()
        .layout_no_wrap(label.to_string(), style::medium(style::SMALL), p.text);
    let kg = ui
        .painter()
        .layout_no_wrap(keys.to_string(), style::regular(style::XS), p.text);
    let w = 9.0 + 14.0 + 5.0 + g.size().x + 5.0 + (kg.size().x + 10.0).max(20.0) + 9.0;
    let br = egui::Rect::from_min_max(
        pos2(rect.right() - 8.0 - w, cy - 13.0),
        pos2(rect.right() - 8.0, cy + 13.0),
    );
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(br));
    let clicked = kit::button_kbd(
        &mut b,
        kit::Kind::Ghost,
        Some(icons::CARET_DOWN),
        label,
        Some(&keys),
        true,
        false,
    )
    .clicked();
    // The prompt line, cut off before the button.
    let x = rect.left() + 14.0 + 15.0 + 10.0;
    let max_w = br.left() - 10.0 - x;
    if let Some(shared) = shared {
        let job = cursor_line(shared, scheme, max_w);
        let galley = ui.painter().layout_job(job);
        let gw = galley.size().x;
        ui.painter().galley(
            pos2(x, cy - galley.size().y / 2.0),
            galley,
            scheme.foreground,
        );
        if gw + 8.0 < max_w {
            let cur = egui::Rect::from_min_size(pos2(x + gw, cy - 6.5), vec2(7.0, 13.0));
            ui.painter()
                .rect_filled(cur, 0.0, scheme.cursor.gamma_multiply(0.8));
        }
    }
    clicked
}

/// The terminal's cursor line up to the cursor, in the cells' colours (mono 12 px).
fn cursor_line(
    shared: &crate::term::core::TermShared,
    scheme: &crate::term::scheme::Scheme,
    max_w: f32,
) -> egui::text::LayoutJob {
    use alacritty_terminal::index::Column;
    use alacritty_terminal::term::cell::Flags;
    use alacritty_terminal::vte::ansi::{Color, NamedColor};
    let resolve = |c: Color| -> egui::Color32 {
        match c {
            Color::Spec(rgb) => egui::Color32::from_rgb(rgb.r, rgb.g, rgb.b),
            Color::Named(n) => {
                let i = n as usize;
                if i < 16 {
                    scheme.ansi[i]
                } else if n == NamedColor::Background {
                    scheme.background
                } else {
                    scheme.foreground
                }
            }
            Color::Indexed(i) if i < 16 => scheme.ansi[i as usize],
            Color::Indexed(i) => crate::term::scheme::xterm_256(i),
        }
    };
    let font = style::mono(12.0);
    let mut job = egui::text::LayoutJob::default();
    let term = shared.term.lock();
    let grid = term.grid();
    let point = grid.cursor.point;
    let row = &grid[point.line];
    let mut run = String::new();
    let mut run_color = scheme.foreground;
    for x in 0..point.column.0 {
        let cell = &row[Column(x)];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let color = resolve(cell.fg);
        if color != run_color && !run.is_empty() {
            job.append(
                &std::mem::take(&mut run),
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: run_color,
                    ..Default::default()
                },
            );
        }
        run_color = color;
        run.push(cell.c);
    }
    drop(term);
    if !run.is_empty() {
        job.append(
            &run,
            0.0,
            egui::TextFormat {
                font_id: font,
                color: run_color,
                ..Default::default()
            },
        );
    }
    job.wrap.max_width = max_w.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}
