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

use crate::i18n::{t, tf};
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
}

/// What a right-click menu acts on, captured when the menu opens.
#[derive(Clone)]
struct MenuTarget {
    remote: bool,
    dir: String,
    names: Vec<String>,
    entry: FileEntry,
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
    },
    /// Switch the remote session to root via sudo, with a password the user types. Asked only
    /// when sudo wants a password that isn't known (or rejected the one tried); `note` says why.
    Sudo {
        password: String,
        note: Option<String>,
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
}

/// Something the app should do after the panel was drawn.
pub enum PanelOutput {
    None,
    /// Remember this local folder for next time.
    LocalDir(String),
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
    /// Type a path instead of the breadcrumb.
    EditPath(bool),
    RemoveTransfer(u64),
    /// Start a failed transfer again.
    Retry(u64),
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
            self.dialog = Some(Dialog::Overwrite {
                upload: true,
                sources,
                dest,
                conflicts,
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
            self.dialog = Some(Dialog::Overwrite {
                upload: false,
                sources,
                dest,
                conflicts,
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
        let full = ui.max_rect();

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

        // The transfer strip along the bottom, the two lists side by side above it.
        let strip_h = self.strip_height();
        let lists = egui::Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - strip_h));
        let gutter_w = 40.0;
        let half = ((lists.width() - gutter_w) / 2.0).floor();
        let local_rect = egui::Rect::from_min_size(lists.min, vec2(half, lists.height()));
        let gutter = egui::Rect::from_min_size(
            pos2(local_rect.right(), lists.top()),
            vec2(gutter_w, lists.height()),
        );
        let remote_rect = egui::Rect::from_min_max(pos2(gutter.right(), lists.top()), lists.max);
        let mut local_ui = ui.new_child(egui::UiBuilder::new().max_rect(local_rect));
        let local_actions = self.side_ui(&mut local_ui, false, input_enabled);
        let mut remote_ui = ui.new_child(egui::UiBuilder::new().max_rect(remote_rect));
        let remote_actions = self.side_ui(&mut remote_ui, true, input_enabled);
        // The middle: send the selection across with one click.
        let mut mid = Vec::new();
        {
            let painter = ui.painter();
            painter.vline(
                gutter.left() + 0.5,
                gutter.y_range(),
                Stroke::new(1.0, p.line_soft),
            );
            painter.vline(
                gutter.right() - 0.5,
                gutter.y_range(),
                Stroke::new(1.0, p.line_soft),
            );
            let c = gutter.center();
            let up = egui::Rect::from_center_size(c - vec2(0.0, 17.0), vec2(28.0, 28.0));
            let down = egui::Rect::from_center_size(c + vec2(0.0, 17.0), vec2(28.0, 28.0));
            for (rect, icon, from_remote, tip) in [
                (up, icons::ARROW_RIGHT, false, t("Upload selected")),
                (down, icons::ARROW_LEFT, true, t("Download selected")),
            ] {
                let side = if from_remote {
                    &self.remote
                } else {
                    &self.local
                };
                let go = !side.selected.is_empty();
                #[cfg(debug_assertions)]
                crate::devtest::record(
                    if from_remote {
                        "sftp-download-selected"
                    } else {
                        "sftp-upload-selected"
                    },
                    rect,
                );
                let resp = ui
                    .interact(
                        rect,
                        egui::Id::new(("sftp-across", self.uid, from_remote)),
                        if go { Sense::click() } else { Sense::hover() },
                    )
                    .on_hover_text(tip);
                let painter = ui.painter();
                let (fill, stroke, color) = if go {
                    (
                        if resp.hovered() {
                            p.accent_tint
                        } else {
                            p.accent_softer
                        },
                        p.accent_line,
                        p.accent_text,
                    )
                } else {
                    (p.canvas, p.border, p.text_faint)
                };
                painter.rect(
                    rect,
                    CornerRadius::same(style::RADIUS),
                    fill,
                    Stroke::new(1.0, stroke),
                    StrokeKind::Inside,
                );
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    icon,
                    style::icon_font(16.0),
                    color,
                );
                if go && resp.clicked() {
                    let mut names: Vec<String> = side.selected.iter().cloned().collect();
                    names.sort();
                    mid.push(SideAction::Transfer {
                        from_remote,
                        dir: side.cwd.clone(),
                        names,
                    });
                }
            }
        }
        for a in local_actions.into_iter().chain(remote_actions).chain(mid) {
            if let Some(out) = self.apply(a, rt, &ctx) {
                output = out;
            }
        }
        // Files dragged in from the file manager land in the remote folder.
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new(("sftp-drop", self.uid)),
            ));
            let to = tf("to {} on {}", &[&self.remote.cwd, &self.host]);
            kit::drop_zone(&painter, remote_rect.shrink(6.0), &p, &to);
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
        self.drag_preview(&ctx);
        self.dialog_ui(&ctx, rt);
        let sftp = self.sftp();
        let editor_out = self
            .editor
            .as_mut()
            .map(|e| (e.show(&ctx, rt, sftp), e.path.clone()));
        match editor_out {
            Some((EditorOutput::Close { saved }, _)) => {
                self.editor = None;
                if saved {
                    self.apply(SideAction::Refresh(true), rt, &ctx);
                }
            }
            Some((EditorOutput::Download, path)) => {
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
            }
            Some((EditorOutput::Open, _)) | None => {}
        }
        output
    }

    /// How tall the transfer strip is: none without transfers or messages, one line closed, up
    /// to four rows open.
    fn strip_height(&self) -> f32 {
        let items = self.transfers.items.len() + usize::from(self.message.is_some());
        if items == 0 {
            return 0.0;
        }
        if !self.show_activity {
            return 40.0;
        }
        40.0 + items.min(4) as f32 * TRANSFER_ROW_H + 8.0
    }

    /// One side: its header (who, up, the path as a breadcrumb, more, and on the remote side
    /// Root and Upload), the column titles, the files, and the count.
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
        let p = style::pal(ui);
        let area = ui.max_rect();
        if is_root_now {
            ui.painter().rect_filled(area, 0.0, p.tone_soft(Tone::Root));
        }
        let side = if is_remote {
            &mut self.remote
        } else {
            &mut self.local
        };
        let show_hidden = side.show_hidden;
        ui.spacing_mut().item_spacing = vec2(6.0, 0.0);

        // Header (40 px).
        let (head, _) = ui.allocate_exact_size(vec2(area.width(), 40.0), Sense::hover());
        let mut h = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(head.left() + 12.0, head.top()),
                    pos2(head.right() - 8.0, head.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        h.spacing_mut().item_spacing.x = 6.0;
        let (who_icon, who) = if is_remote {
            (icons::HARD_DRIVES, host.clone())
        } else {
            (icons::LAPTOP, capitalized(crate::platform::this_computer()))
        };
        kit::icon(&mut h, who_icon, 15.0, p.text_muted);
        h.label(
            RichText::new(&who)
                .font(style::semibold(style::SMALL))
                .color(p.text),
        );
        h.add_space(4.0);
        if kit::icon_button_ex(&mut h, icons::ARROW_UP, t("Parent folder"), false, 22.0).clicked() {
            actions.push(SideAction::Up(is_remote));
        }
        // The right part first, so the breadcrumb takes what is left.
        let mut right_w = 22.0 + 6.0;
        if is_remote {
            right_w += text_button_w(&h, t("Root"), true, true)
                + 6.0
                + text_button_w(&h, t("Upload"), true, true)
                + 6.0;
        }
        let crumbs_w = (h.available_width() - right_w).max(60.0);
        let (crumbs, crumb_resp) = h.allocate_exact_size(vec2(crumbs_w, 28.0), Sense::click());
        let crumbs_hover = crumb_resp.hovered();
        // The path: segments to click, or a field to type a path into.
        let edit_id = egui::Id::new(("sftp-path", uid, is_remote));
        if side.editing {
            let mut field_ui = h.new_child(egui::UiBuilder::new().max_rect(crumbs));
            let r = field_ui.add(
                style::input(&mut side.path_edit)
                    .id(edit_id)
                    .desired_width(crumbs.width())
                    .margin(egui::Margin::symmetric(8, 6))
                    .mono(),
            );
            if !r.has_focus() && !r.lost_focus() {
                r.request_focus();
            }
            if r.lost_focus() {
                side.editing = false;
                if field_ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    actions.push(SideAction::Cd(is_remote, side.path_edit.clone()));
                } else {
                    side.path_edit = side.cwd.clone();
                }
            }
        } else {
            let painter = h.painter();
            painter.rect_filled(
                crumbs,
                CornerRadius::same(style::RADIUS),
                if crumbs_hover { p.subtle } else { p.hover },
            );
            // Home, then the segments; the leading ones give way when the path is long.
            let home = egui::Rect::from_center_size(
                pos2(crumbs.left() + 8.0 + 7.5, crumbs.center().y),
                vec2(18.0, 18.0),
            );
            let home_r = h
                .interact(
                    home,
                    egui::Id::new(("crumb-home", uid, is_remote)),
                    Sense::click(),
                )
                .on_hover_text(t("Home folder"));
            h.painter().text(
                home.center(),
                Align2::CENTER_CENTER,
                icons::HOUSE,
                style::icon_font(15.0),
                if home_r.hovered() {
                    p.text
                } else {
                    p.text_faint
                },
            );
            if home_r.clicked() {
                actions.push(SideAction::Home(is_remote));
            }
            let refresh = egui::Rect::from_center_size(
                pos2(crumbs.right() - 4.0 - 9.0, crumbs.center().y),
                vec2(18.0, 18.0),
            );
            if side.loading {
                kit::paint_spinner(&h, refresh.center(), 6.0);
            } else {
                let rr = h
                    .interact(
                        refresh,
                        egui::Id::new(("crumb-refresh", uid, is_remote)),
                        Sense::click(),
                    )
                    .on_hover_text(t("Refresh"));
                kit::paint_icon_button(
                    &h,
                    &rr,
                    refresh,
                    icons::ARROWS_CLOCKWISE,
                    t("Refresh"),
                    false,
                    false,
                );
                if rr.clicked() {
                    actions.push(SideAction::Refresh(is_remote));
                }
            }
            let segs = path_segments(&side.cwd, is_remote);
            let font = style::regular(style::SMALL);
            let avail = refresh.left() - 4.0 - (home.right() + 6.0);
            let sep_w = 14.0;
            let widths: Vec<f32> = segs
                .iter()
                .map(|(name, _)| {
                    h.painter()
                        .layout_no_wrap(name.clone(), font.clone(), p.text)
                        .size()
                        .x
                        + 8.0
                })
                .collect();
            let mut first = 0;
            while first + 1 < segs.len()
                && widths[first..].iter().sum::<f32>() + sep_w * (segs.len() - first) as f32 + 20.0
                    > avail
            {
                first += 1;
            }
            let mut x = home.right() + 6.0;
            if first > 0 {
                h.painter().text(
                    pos2(x, crumbs.center().y),
                    Align2::LEFT_CENTER,
                    "…",
                    font.clone(),
                    p.text_faint,
                );
                x += 14.0;
            }
            for (i, (name, path)) in segs.iter().enumerate().skip(first) {
                h.painter().text(
                    pos2(x + 3.0, crumbs.center().y),
                    Align2::CENTER_CENTER,
                    icons::CARET_RIGHT,
                    style::icon_font(10.0),
                    p.text_faint,
                );
                x += sep_w - 4.0;
                let last = i + 1 == segs.len();
                let w = widths[i].min((refresh.left() - 4.0 - x).max(0.0));
                let seg =
                    egui::Rect::from_min_size(pos2(x, crumbs.center().y - 10.0), vec2(w, 20.0));
                let r = h.interact(
                    seg,
                    egui::Id::new(("crumb", uid, is_remote, i)),
                    Sense::click(),
                );
                if r.hovered() {
                    h.painter().rect_filled(seg, CornerRadius::same(4), p.hover);
                }
                let (f, c) = if last {
                    (style::medium(style::SMALL), p.text)
                } else {
                    (font.clone(), p.text_muted)
                };
                h.painter().with_clip_rect(seg).text(
                    pos2(seg.left() + 4.0, seg.center().y),
                    Align2::LEFT_CENTER,
                    name,
                    f,
                    c,
                );
                if r.clicked() {
                    actions.push(SideAction::Cd(is_remote, path.clone()));
                }
                x += w + 2.0;
            }
            if crumb_resp.clicked() {
                side.editing = true;
                side.path_edit = side.cwd.clone();
            }
            crumb_resp.on_hover_text(t("Click to type a path"));
        }
        let more = kit::icon_button_ex(&mut h, icons::DOTS_THREE, t("More"), false, 22.0);
        egui::Popup::menu(&more).show(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(220.0);
            if kit::menu_item(ui, Some(icons::FOLDER_PLUS), t("New folder"), None).clicked() {
                actions.push(SideAction::Mkdir(is_remote));
                ui.close();
            }
            let hidden_label = if show_hidden {
                t("Hide hidden files")
            } else {
                t("Show hidden files")
            };
            let r = kit::menu_item(
                ui,
                Some(if show_hidden {
                    icons::EYE_SLASH
                } else {
                    icons::EYE
                }),
                hidden_label,
                None,
            );
            #[cfg(debug_assertions)]
            crate::devtest::record(
                if is_remote {
                    "remote-hidden"
                } else {
                    "local-hidden"
                },
                r.rect,
            );
            if r.clicked() {
                actions.push(SideAction::ToggleHidden(is_remote));
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::FOLDER_OPEN), t("Go to folder…"), None).clicked() {
                actions.push(SideAction::EditPath(is_remote));
                ui.close();
            }
            kit::menu_sep(ui);
            if kit::menu_item(ui, Some(icons::LIST), t("Select All"), None).clicked() {
                actions.push(SideAction::SelectAll(is_remote));
                ui.close();
            }
        });
        if is_remote {
            let root = h
                .add_enabled_ui(!reopening, |ui| root_button(ui, is_root_now))
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
            let upload = kit::button_ex(
                &mut h,
                kit::Kind::Secondary,
                Some(icons::UPLOAD_SIMPLE),
                t("Upload"),
                true,
            )
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
        let side = if is_remote {
            &mut self.remote
        } else {
            &mut self.local
        };
        // Root: a reminder of what the side runs as, with the way back.
        if is_root_now {
            let (r, _) = ui.allocate_exact_size(vec2(area.width(), 30.0), Sense::hover());
            let mut b = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        pos2(r.left() + 12.0, r.top()),
                        pos2(r.right() - 8.0, r.bottom() - 8.0),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            b.spacing_mut().item_spacing.x = 8.0;
            kit::icon(&mut b, icons::SHIELD_WARNING, 14.0, p.root);
            let mut job = egui::text::LayoutJob::default();
            let f = |font: egui::FontId| egui::TextFormat {
                font_id: font,
                color: p.root,
                ..Default::default()
            };
            job.append(t("Browsing as"), 0.0, f(style::regular(style::SMALL)));
            job.append(" root ", 0.0, f(style::semibold(style::SMALL)));
            job.append(
                t("via sudo — changes affect system files."),
                0.0,
                f(style::regular(style::SMALL)),
            );
            b.add(egui::Label::new(job).truncate());
            b.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let back = tf(
                    "Back to {}",
                    &[if user.is_empty() {
                        t("your user")
                    } else {
                        user.as_str()
                    }],
                );
                let g =
                    ui.painter()
                        .layout_no_wrap(back.clone(), style::medium(style::SMALL), p.root);
                let (rect, resp) = ui
                    .allocate_exact_size(vec2(g.size().x + 18.0, style::TOOLBAR_H), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(style::RADIUS),
                        p.tone_soft(Tone::Root),
                    );
                }
                ui.painter()
                    .galley(rect.center() - g.size() / 2.0, g, p.root);
                if resp.clicked() {
                    actions.push(SideAction::Root);
                }
            });
        }
        if let Some(err) = &side.error {
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                kit::icon(ui, icons::WARNING_CIRCLE, 14.0, p.err);
                ui.add(
                    egui::Label::new(
                        RichText::new(err)
                            .font(style::regular(style::SMALL))
                            .color(p.err),
                    )
                    .truncate(),
                );
            });
            ui.add_space(6.0);
        }

        // Column titles (26 px between faint hairlines).
        let widths = column_widths(area.width() - 24.0, is_remote);
        let (cols, _) = ui.allocate_exact_size(vec2(area.width(), 26.0), Sense::hover());
        ui.painter().hline(
            cols.x_range(),
            cols.top() + 0.5,
            Stroke::new(1.0, p.line_soft),
        );
        ui.painter().hline(
            cols.x_range(),
            cols.bottom() - 0.5,
            Stroke::new(1.0, p.line_soft),
        );
        let mut x = cols.left() + 12.0;
        let mut sort_clicked = None;
        let titles: Vec<(&str, Option<SortKey>)> = if is_remote {
            vec![
                (t("Name"), Some(SortKey::Name)),
                (t("Size"), Some(SortKey::Size)),
                (t("Modified"), Some(SortKey::Modified)),
                (t("Permissions"), None),
            ]
        } else {
            vec![
                (t("Name"), Some(SortKey::Name)),
                (t("Size"), Some(SortKey::Size)),
                (t("Modified"), Some(SortKey::Modified)),
            ]
        };
        for (i, (title, key)) in titles.iter().enumerate() {
            let sorted = key.is_some_and(|k| k == side.sort);
            let label = if sorted {
                format!(
                    "{title} {}",
                    if side.descending {
                        icons::CARET_DOWN
                    } else {
                        icons::CARET_UP
                    }
                )
            } else {
                title.to_string()
            };
            let cell =
                egui::Rect::from_min_size(pos2(x, cols.top()), vec2(widths[i], cols.height()));
            let color = if sorted { p.text_muted } else { p.text_faint };
            if let Some(k) = key {
                let r = ui.interact(
                    cell,
                    egui::Id::new(("sftp-col", uid, is_remote, i)),
                    Sense::click(),
                );
                let c = if r.hovered() { p.text } else { color };
                ui.painter().text(
                    pos2(cell.left(), cell.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    style::medium(style::XS),
                    c,
                );
                if r.clicked() {
                    sort_clicked = Some(*k);
                }
            } else {
                ui.painter().text(
                    pos2(cell.left(), cell.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    style::medium(style::XS),
                    color,
                );
            }
            x += widths[i];
        }
        if let Some(k) = sort_clicked {
            actions.push(SideAction::Sort(is_remote, k));
        }

        // The files (28 px rows inside 4 × 6 padding), then the count.
        let visible: Vec<usize> = side
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| show_hidden || !e.is_hidden())
            .map(|(i, _)| i)
            .collect();
        let foot_h = 26.0;
        let list_rect = egui::Rect::from_min_max(
            pos2(area.left(), ui.cursor().top()),
            pos2(area.right(), area.bottom() - foot_h),
        );
        // A click in the list gives it the keyboard (Esc gives it back to the terminal).
        let list_id = egui::Id::new(("sftp-list-keys", uid, is_remote));
        let list_bg = ui.interact(list_rect, list_id, Sense::click());
        if list_bg.clicked() || list_bg.secondary_clicked() {
            ui.memory_mut(|m| m.request_focus(list_id));
        }
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        let (_, dropped, list_hovered) = file_drop_zone(&mut list_ui, list_rect, is_remote, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(6, 4))
                .show(ui, |ui| {
                    ui.set_min_size(vec2(list_rect.width() - 12.0, list_rect.height() - 8.0));
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
                        ROW_H,
                        visible.len(),
                        |ui, range| {
                            for row in range {
                                let e = &side.entries[visible[row]];
                                let selected = side.selected.contains(&e.name);
                                let anchor = side.anchor.as_deref() == Some(e.name.as_str());
                                let (rect, resp) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), ROW_H),
                                    Sense::click_and_drag(),
                                );
                                let painter = ui.painter();
                                if selected {
                                    painter.rect_filled(
                                        rect,
                                        CornerRadius::same(style::RADIUS),
                                        p.accent_soft,
                                    );
                                    if anchor && !side.selected.is_empty() {
                                        painter.rect_stroke(
                                            rect,
                                            CornerRadius::same(style::RADIUS),
                                            Stroke::new(1.0, p.accent_line),
                                            StrokeKind::Inside,
                                        );
                                    }
                                } else if resp.hovered() {
                                    painter.rect_filled(
                                        rect,
                                        CornerRadius::same(style::RADIUS),
                                        p.hover,
                                    );
                                }
                                let mut x = rect.left() + 6.0;
                                let (icon, color) =
                                    icons::file_icon(&e.name, e.is_dir, e.is_symlink, &p);
                                let color = if e.is_dir { color } else { p.text_faint };
                                painter.text(
                                    pos2(x + 8.0, rect.center().y),
                                    Align2::CENTER_CENTER,
                                    icon,
                                    style::icon_font(16.0),
                                    color,
                                );
                                let name_clip = egui::Rect::from_min_max(
                                    pos2(x + 24.0, rect.top()),
                                    pos2(rect.left() + 6.0 + widths[0] - 8.0, rect.bottom()),
                                );
                                painter.with_clip_rect(name_clip).text(
                                    pos2(x + 24.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    &e.name,
                                    style::regular(style::BODY),
                                    p.text,
                                );
                                x = rect.left() + 6.0 + widths[0];
                                if !e.is_dir {
                                    painter.text(
                                        pos2(x, rect.center().y),
                                        Align2::LEFT_CENTER,
                                        entry::human_size(e.size),
                                        style::regular(style::SMALL),
                                        p.text_faint,
                                    );
                                }
                                x += widths[1];
                                let time = e.modified.map(short_time).unwrap_or_default();
                                painter
                                    .with_clip_rect(egui::Rect::from_min_max(
                                        pos2(x, rect.top()),
                                        pos2(x + widths[2] - 6.0, rect.bottom()),
                                    ))
                                    .text(
                                        pos2(x, rect.center().y),
                                        Align2::LEFT_CENTER,
                                        time,
                                        style::regular(style::SMALL),
                                        p.text_faint,
                                    );
                                x += widths[2];
                                if is_remote {
                                    let mode = e.mode.map(entry::mode_string).unwrap_or_default();
                                    painter.text(
                                        pos2(x, rect.center().y),
                                        Align2::LEFT_CENTER,
                                        mode,
                                        style::mono(11.5),
                                        p.text_faint,
                                    );
                                }
                                #[cfg(debug_assertions)]
                                crate::devtest::record(
                                    format!(
                                        "{}-row:{}",
                                        if is_remote { "remote" } else { "local" },
                                        e.name
                                    ),
                                    rect,
                                );
                                // The names a drag or menu acts on: the selection if this row is part
                                // of it, otherwise just this row.
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
                                }
                                if resp.drag_started() {
                                    let names = names_here();
                                    actions.push(SideAction::DragStarted(if names.len() == 1 {
                                        names[0].clone()
                                    } else {
                                        tf("{} items", &[&names.len().to_string()])
                                    }));
                                    egui::DragAndDrop::set_payload(
                                        &resp.ctx,
                                        DragFiles {
                                            from_remote: is_remote,
                                            dir: side.cwd.clone(),
                                            names,
                                        },
                                    );
                                }
                                if resp.double_clicked() {
                                    // A remote file opens in the editor; folders open, and local files
                                    // upload.
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
                                    ui.set_min_width(230.0);
                                    // Act on what was right-clicked, even if the listing changed since.
                                    let target =
                                        menu_target.clone().unwrap_or_else(|| MenuTarget {
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
        });
        // The keys of the list that has the keyboard.
        let list_focused = ui.memory(|m| m.has_focus(list_id));
        if list_focused {
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
            let (open, rename, delete, copy_path, transfer, escape) = ui.input_mut(|i| {
                (
                    FileKey::Open.take(i),
                    FileKey::Rename.take(i),
                    FileKey::Delete.take(i),
                    FileKey::CopyPath.take(i),
                    transfer_key.take(i),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            let dir = side.cwd.clone();
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
        if (list_hovered || list_focused)
            && !side.editing
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A))
        {
            actions.push(SideAction::SelectAll(is_remote));
        }
        if let Some(drop) = dropped
            && drop.from_remote != is_remote
        {
            actions.push(SideAction::Transfer {
                from_remote: drop.from_remote,
                dir: drop.dir.clone(),
                names: drop.names.clone(),
            });
        }
        // The count.
        let foot = egui::Rect::from_min_max(pos2(area.left(), area.bottom() - foot_h), area.max);
        ui.painter().hline(
            foot.x_range(),
            foot.top() + 0.5,
            Stroke::new(1.0, p.line_soft),
        );
        let count = tf("{} items", &[&visible.len().to_string()]);
        let text = if visible.is_empty() && !side.loading {
            t("Empty folder").to_string()
        } else {
            count
        };
        ui.painter().text(
            pos2(foot.left() + 12.0, foot.center().y),
            Align2::LEFT_CENTER,
            text,
            style::regular(style::XS),
            p.text_faint,
        );
        let _ = input_enabled;
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
            SideAction::EditPath(remote) => {
                let side = if remote {
                    &mut self.remote
                } else {
                    &mut self.local
                };
                side.editing = true;
                side.path_edit = side.cwd.clone();
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
                self.dialog = Some(Dialog::Delete { remote, dir, names })
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
        if !egui::DragAndDrop::has_payload_of_type::<DragFiles>(ctx) {
            self.drag_label = None;
            return;
        }
        let (Some(label), Some(pos)) = (&self.drag_label, ctx.pointer_interact_pos()) else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("sftp-drag"),
        ));
        let visuals = ctx.global_style().visuals.clone();
        let p = style::palette(visuals.dark_mode);
        let galley = painter.layout_no_wrap(
            format!("{}  {label}", icons::FILES),
            style::medium(13.0),
            p.text,
        );
        let r = egui::Rect::from_min_size(
            pos + egui::vec2(14.0, 6.0),
            galley.size() + egui::vec2(18.0, 10.0),
        );
        painter.add(
            visuals
                .popup_shadow
                .as_shape(r, egui::CornerRadius::same(6)),
        );
        painter.rect(
            r,
            egui::CornerRadius::same(6),
            p.surface,
            egui::Stroke::new(1.0, p.border_strong),
            egui::StrokeKind::Inside,
        );
        painter.galley(r.min + egui::vec2(9.0, 5.0), galley, p.text);
    }

    /// The transfer strip along the bottom of the panel: one line with the batch's progress
    /// (and Cancel all), and when opened, a row per transfer and the latest message.
    fn xfer_strip(&mut self, ui: &mut Ui, rect: egui::Rect) -> Vec<SideAction> {
        let mut actions = Vec::new();
        let p = style::pal(ui);
        let dark = ui.visuals().dark_mode;
        ui.painter().rect_filled(rect, 0.0, p.canvas);
        ui.painter()
            .hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
        let summary = self.transfers.summary();
        let running = summary.as_ref().filter(|s| s.running()).cloned();
        let has_items = !self.transfers.items.is_empty();
        let head = egui::Rect::from_min_size(rect.min, vec2(rect.width(), 40.0));
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(head.left() + 14.0, head.top()),
                    pos2(head.right() - 10.0, head.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 12.0;
        let (lead, _) = row.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        let (title, detail) = match (&running, &summary, &self.message) {
            (Some(s), _, _) => {
                style::progress_ring(row.painter(), lead.center(), 7.0, s.fraction(), dark);
                (summary_title(s), summary_detail(s))
            }
            (None, Some(s), _) if has_items => {
                let (icon, color) = if s.failed > 0 {
                    (icons::X_CIRCLE, p.err)
                } else {
                    (icons::CHECK_CIRCLE, p.ok)
                };
                row.painter().text(
                    lead.center(),
                    Align2::CENTER_CENTER,
                    icon,
                    style::icon_font(15.0),
                    color,
                );
                (t("Transfers").to_string(), finished_detail(s))
            }
            (_, _, Some((msg, err))) => {
                let (icon, color) = if *err {
                    (icons::WARNING_CIRCLE, p.err)
                } else {
                    (icons::INFO, p.text_muted)
                };
                row.painter().text(
                    lead.center(),
                    Align2::CENTER_CENTER,
                    icon,
                    style::icon_font(15.0),
                    color,
                );
                (msg.clone(), String::new())
            }
            _ => (t("Transfers").to_string(), String::new()),
        };
        row.label(
            RichText::new(&title)
                .font(style::medium(style::BODY))
                .color(p.text),
        );
        let open = self.show_activity;
        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let caret = kit::icon_button_ex(
                ui,
                if open {
                    icons::CARET_DOWN
                } else {
                    icons::CARET_UP
                },
                if open { t("Hide") } else { t("Details") },
                false,
                22.0,
            );
            if caret.clicked() {
                self.show_activity = !open;
            }
            if running.is_some() {
                if kit::ghost(ui, None, t("Cancel all")).clicked() {
                    self.transfers.cancel_all();
                }
            } else if has_items {
                if kit::ghost(ui, Some(icons::BROOM), t("Clear"))
                    .on_hover_text(t("Remove finished transfers from the list"))
                    .clicked()
                {
                    self.transfers.clear_finished();
                }
            } else if self.message.is_some() && kit::ghost(ui, None, t("Dismiss")).clicked() {
                self.message = None;
            }
            if let (Some(s), false) = (&running, open) {
                let w = ui.available_width().clamp(0.0, 220.0);
                if w > 40.0 {
                    let (r, _) = ui.allocate_exact_size(vec2(w, 4.0), Sense::hover());
                    kit::paint_bar(ui.painter(), r, s.fraction(), Tone::Accent, &p);
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if !detail.is_empty() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(&detail)
                                .font(style::regular(style::SMALL))
                                .color(p.text_muted),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&detail);
                }
            });
        });
        if !open {
            return actions;
        }
        let list = egui::Rect::from_min_max(
            pos2(rect.left() + 8.0, head.bottom()),
            pos2(rect.right() - 8.0, rect.bottom() - 8.0),
        );
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        egui::ScrollArea::vertical()
            .id_salt(("sftp-transfers-list", self.uid))
            .auto_shrink([false, false])
            .show(&mut list_ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if let Some((msg, is_err)) = self.message.clone()
                    && has_items
                {
                    let (r, _) = ui.allocate_exact_size(
                        vec2(ui.available_width(), TRANSFER_ROW_H),
                        Sense::hover(),
                    );
                    let (icon, color) = if is_err {
                        (icons::WARNING_CIRCLE, p.err)
                    } else {
                        (icons::INFO, p.text_muted)
                    };
                    ui.painter().text(
                        pos2(r.left() + 6.0 + 10.0, r.center().y),
                        Align2::CENTER_CENTER,
                        icon,
                        style::icon_font(14.0),
                        color,
                    );
                    let x_rect = egui::Rect::from_center_size(
                        pos2(r.right() - 6.0 - 11.0, r.center().y),
                        vec2(18.0, 18.0),
                    );
                    ui.painter()
                        .with_clip_rect(egui::Rect::from_min_max(
                            r.min,
                            pos2(x_rect.left() - 8.0, r.bottom()),
                        ))
                        .text(
                            pos2(r.left() + 6.0 + 30.0, r.center().y),
                            Align2::LEFT_CENTER,
                            &msg,
                            style::regular(style::SMALL),
                            color,
                        );
                    let x = ui
                        .interact(
                            x_rect,
                            egui::Id::new(("sftp-msg-x", self.uid)),
                            Sense::click(),
                        )
                        .on_hover_text(t("Dismiss"));
                    kit::paint_icon_button(ui, &x, x_rect, icons::X, t("Dismiss"), false, false);
                    if x.clicked() {
                        self.message = None;
                    }
                }
                for tr in self.transfers.items.iter().rev() {
                    match transfer_row(ui, tr) {
                        Some(RowAction::Remove(id)) => actions.push(SideAction::RemoveTransfer(id)),
                        Some(RowAction::Retry(id)) => actions.push(SideAction::Retry(id)),
                        None => {}
                    }
                }
            });
        actions
    }

    fn dialog_ui(&mut self, ctx: &egui::Context, rt: &tokio::runtime::Handle) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let mut close = false;
        let mut confirmed = false;
        let mut skip_existing = false;
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
        let modal = style::modal(ctx, egui::Id::new("sftp-dialog")).show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            let enter = |ui: &Ui, r: &egui::Response| r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            match dialog {
                Dialog::Mkdir { name, dir, .. } => {
                    ui.set_width(380.0);
                    kit::dialog_header(ui, Some((icons::FOLDER_PLUS, Tone::Neutral)), t("New folder"), Some(&tf("in {}", &[dir])), false);
                    kit::dialog_body(ui, |ui| {
                        let r = ui.add(style::input(name).hint_text(t("Folder name")));
                        if focus {
                            r.request_focus();
                        }
                        if enter(ui, &r) {
                            confirmed = true;
                        }
                    });
                }
                Dialog::Rename { to, from, .. } => {
                    ui.set_width(380.0);
                    kit::dialog_header(ui, Some((icons::CURSOR_TEXT, Tone::Neutral)), t("Rename"), Some(from), false);
                    kit::dialog_body(ui, |ui| {
                        let r = ui.add(style::input(to).mono());
                        if focus {
                            r.request_focus();
                        }
                        if enter(ui, &r) {
                            confirmed = true;
                        }
                    });
                }
                Dialog::Delete { names, remote, dir } => {
                    ui.set_width(400.0);
                    let title = tf("Delete {} item(s) on {}?", &[&names.len().to_string(), &where_(*remote)]);
                    kit::dialog_header(ui, Some((icons::TRASH, Tone::Err)), &title, None, false);
                    let entries = if *remote { &self.remote } else { &self.local };
                    let same_dir = entries.cwd == *dir;
                    kit::dialog_body(ui, |ui| {
                        egui::Frame::NONE
                            .fill(p.card)
                            .stroke(Stroke::new(1.0, p.border))
                            .corner_radius(CornerRadius::same(style::RADIUS_LG))
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 4.0;
                                for n in names.iter().take(8) {
                                    let e = same_dir.then(|| entries.entries.iter().find(|e| &e.name == n)).flatten();
                                    let is_dir = e.is_some_and(|e| e.is_dir);
                                    let (icon, _) = icons::file_icon(n, is_dir, e.is_some_and(|e| e.is_symlink), &p);
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 8.0;
                                        kit::icon(ui, icon, 14.0, p.text_muted);
                                        ui.label(RichText::new(n).font(style::regular(12.5)).color(p.text));
                                        if is_dir {
                                            ui.label(RichText::new(format!("· {}", t("folder"))).font(style::regular(12.5)).color(p.text_faint));
                                        }
                                    });
                                }
                                if names.len() > 8 {
                                    ui.label(RichText::new(tf("and {} more", &[&(names.len() - 8).to_string()])).font(style::regular(12.5)).color(p.text_faint));
                                }
                            });
                        ui.add_space(10.0);
                        ui.add(egui::Label::new(t("Folders are deleted with everything in them. This can't be undone.")).wrap());
                    });
                }
                Dialog::Chmod { path, mode, .. } => {
                    ui.set_width(360.0);
                    let name = entry::remote_name(path).to_string();
                    kit::dialog_header(ui, Some((icons::LOCK_KEY, Tone::Neutral)), t("Permissions"), Some(&name), false);
                    kit::dialog_body(ui, |ui| {
                        let mut bits = entry::parse_mode(mode).unwrap_or(0);
                        egui::Grid::new("chmod").num_columns(4).min_col_width(64.0).spacing([6.0, 8.0]).show(ui, |ui| {
                            ui.allocate_exact_size(vec2(80.0 - 6.0, 16.0), Sense::hover());
                            for h in [t("Read"), t("Write"), t("Execute")] {
                                ui.vertical_centered(|ui| ui.label(RichText::new(h).font(style::regular(12.5)).color(p.text_faint)));
                            }
                            ui.end_row();
                            for (who, shift) in [(t("Owner"), 6), (t("Group"), 3), (t("Others"), 0)] {
                                ui.label(RichText::new(who).font(style::regular(12.5)).color(p.text));
                                for bit in [4u32, 2, 1] {
                                    let mask = bit << shift;
                                    let mut on = bits & mask != 0;
                                    ui.vertical_centered(|ui| {
                                        if style::checkbox(ui, &mut on, "").changed() {
                                            bits = if on { bits | mask } else { bits & !mask };
                                            *mode = format!("{:o}", bits);
                                        }
                                    });
                                }
                                ui.end_row();
                            }
                        });
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let bad = entry::parse_mode(mode).is_none();
                            ui.add(style::input(mode).desired_width(90.0).mono().error(bad));
                            ui.label(
                                RichText::new(entry::parse_mode(mode).map(entry::mode_string).unwrap_or_else(|| "?".into()))
                                    .font(style::mono(style::MONO))
                                    .color(p.text_muted),
                            );
                        });
                    });
                }
                Dialog::Overwrite { conflicts, dest, upload, .. } => {
                    ui.set_width(420.0);
                    let title = tf("Replace {} existing item(s)?", &[&conflicts.len().to_string()]);
                    let sub = tf("in {} on {}", &[dest, &where_(*upload)]);
                    kit::dialog_header(ui, Some((icons::COPY, Tone::Warn)), &title, Some(&sub), false);
                    kit::dialog_body(ui, |ui| {
                        for n in conflicts.iter().take(8) {
                            ui.label(RichText::new(n).font(style::mono(12.0)).color(p.text));
                        }
                        if conflicts.len() > 8 {
                            ui.label(RichText::new(tf("and {} more", &[&(conflicts.len() - 8).to_string()])).font(style::regular(12.0)).color(p.text_faint));
                        }
                        ui.add_space(8.0);
                        let msg = if *upload { t("The files on the server will be overwritten.").to_string() } else { tf("The files on {} will be overwritten.", &[crate::platform::this_computer()]) };
                        ui.label(msg);
                    });
                }
                Dialog::Sudo { password, note } => {
                    ui.set_width(400.0);
                    let sub = note.clone().unwrap_or_else(|| tf("sudo on {} needs a password", &[&host]));
                    kit::dialog_header(ui, Some((icons::SHIELD_CHECK, Tone::Root)), t("Browse as root"), Some(&sub), false);
                    kit::dialog_body(ui, |ui| {
                        let bad = note.is_some() && password.is_empty();
                        let r = ui.add(style::input(password).password(true).hint_text(t("sudo password")).error(bad));
                        if focus {
                            r.request_focus();
                        }
                        if enter(ui, &r) && !password.is_empty() {
                            confirmed = true;
                        }
                        ui.add_space(8.0);
                        let who = if user.is_empty() { t("your user").to_string() } else { user.clone() };
                        style::hint(ui, tf("Only the file panel runs as root; the terminal stays as {}. Nothing is saved.", &[&who]));
                    });
                }
            }
            let (ok_label, danger) = match dialog {
                Dialog::Delete { .. } => (t("Delete"), true),
                Dialog::Overwrite { .. } => (t("Replace"), false),
                Dialog::Mkdir { .. } => (t("Create"), false),
                Dialog::Rename { .. } => (t("Rename"), false),
                Dialog::Chmod { .. } => (t("Apply"), false),
                Dialog::Sudo { .. } => (t("Continue"), false),
            };
            // sudo is asked without a password first: here one is needed.
            let ok_enabled = !matches!(dialog, Dialog::Sudo { password, .. } if password.is_empty());
            let overwrite = matches!(dialog, Dialog::Overwrite { .. });
            kit::dialog_footer(ui, |_| {}, |ui| {
                let ok = ui.add_enabled_ui(ok_enabled, |ui| if danger { kit::danger(ui, ok_label) } else { kit::primary(ui, ok_label) }).inner;
                if ok.clicked() {
                    confirmed = true;
                }
                if overwrite && kit::secondary(ui, None, t("Skip existing")).clicked() {
                    skip_existing = true;
                }
                if kit::secondary(ui, None, t("Cancel")).clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }
        if skip_existing
            && let Some(Dialog::Overwrite {
                upload,
                sources,
                dest,
                conflicts,
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

const TRANSFER_ROW_H: f32 = 30.0;
/// Height of a file row.
const ROW_H: f32 = 28.0;

/// What a click in a transfer row asks for.
enum RowAction {
    Remove(u64),
    Retry(u64),
}

/// "This Mac" (the platform's name for this computer, starting a label).
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// A modification time that fits its column: "Today 16:05", "Nov 2 16:05", "2025-10-31".
fn short_time(secs: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(at) = Local.timestamp_opt(secs, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if at.date_naive() == now.date_naive() {
        return tf("Today {}", &[&at.format("%H:%M").to_string()]);
    }
    if at.year() == now.year() {
        let months = [
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
        ];
        return format!(
            "{} {} {}",
            months[at.month0() as usize],
            at.day(),
            at.format("%H:%M")
        );
    }
    at.format("%Y-%m-%d").to_string()
}

/// The widths of the columns (name takes what the others leave).
fn column_widths(width: f32, remote: bool) -> Vec<f32> {
    let fixed: f32 = if remote {
        70.0 + 100.0 + 82.0
    } else {
        70.0 + 100.0
    };
    let name = (width - fixed).max(120.0);
    if remote {
        vec![name, 70.0, 100.0, 82.0]
    } else {
        vec![name, 70.0, 100.0]
    }
}

/// The breadcrumb of a folder: each segment's name and the path up to it.
fn path_segments(cwd: &str, remote: bool) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if remote || !cfg!(windows) {
        let mut path = String::new();
        for part in cwd.split('/').filter(|s| !s.is_empty()) {
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

/// The width a small button with an icon and `text` takes.
fn text_button_w(ui: &Ui, text: &str, small: bool, icon: bool) -> f32 {
    let size = if small { style::SMALL } else { style::BODY };
    let g =
        ui.painter()
            .layout_no_wrap(text.to_string(), style::medium(size), egui::Color32::WHITE);
    g.size().x + 2.0 * if small { 9.0 } else { 12.0 } + if icon { 14.0 + 5.0 } else { 0.0 }
}

/// The Root toggle: quiet at rest, root-tinted while the side runs as root.
fn root_button(ui: &mut Ui, on: bool) -> egui::Response {
    let p = style::pal(ui);
    let label = t("Root");
    let g = ui.painter().layout_no_wrap(
        label.to_string(),
        style::medium(style::SMALL),
        egui::Color32::PLACEHOLDER,
    );
    let w = 9.0 + 14.0 + 5.0 + g.size().x + 9.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, style::TOOLBAR_H), Sense::click());
    let hovered = resp.hovered() && ui.is_enabled();
    let (fill, color) = if on {
        (p.tone_soft(Tone::Root), p.root)
    } else if hovered {
        (p.hover, p.text)
    } else {
        (egui::Color32::TRANSPARENT, p.text_muted)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(style::RADIUS), fill);
    painter.text(
        pos2(rect.left() + 9.0 + 7.0, rect.center().y),
        Align2::CENTER_CENTER,
        icons::SHIELD_CHECK,
        style::icon_font(14.0),
        color,
    );
    painter.galley(
        pos2(
            rect.left() + 9.0 + 14.0 + 5.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        color,
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, label));
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
        Some(icons::CLIPBOARD),
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

/// One transfer (30 px): its direction or outcome, the name, a bar or its state, sizes (or
/// Retry), and × to cancel or remove it.
fn transfer_row(ui: &mut Ui, tr: &Transfer) -> Option<RowAction> {
    let mut out = None;
    let p = style::pal(ui);
    let state = tr.state();
    let done = tr.done.load(std::sync::atomic::Ordering::Relaxed);
    let total = tr.total.load(std::sync::atomic::Ordering::Relaxed);
    let skipped = tr.skipped.lock().clone();
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), TRANSFER_ROW_H), Sense::hover());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(style::RADIUS), p.hover);
    }
    let inner = rect.shrink2(vec2(6.0, 0.0));
    let gap = 10.0;
    let flex = inner.width() - 20.0 - 120.0 - 22.0 - 4.0 * gap;
    let name_w = flex * 1.6 / 2.6;
    let status_w = flex - name_w;
    let x_icon = inner.left();
    let x_name = x_icon + 20.0 + gap;
    let x_status = x_name + name_w + gap;
    let x_meta = x_status + status_w + gap;
    let x_close = x_meta + 120.0 + gap;
    let cy = rect.center().y;
    let painter = ui.painter();
    let (icon, color) = match (&state, tr.direction) {
        (TransferState::Done, _) => (icons::CHECK_CIRCLE, p.ok),
        (TransferState::Failed(_), _) => (icons::X_CIRCLE, p.err),
        (_, Direction::Upload) => (icons::ARROW_UP, p.accent_text),
        (_, Direction::Download) => (icons::ARROW_DOWN, p.ok),
    };
    painter.text(
        pos2(x_icon + 10.0, cy),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(14.0),
        color,
    );
    let name_rect = egui::Rect::from_min_max(
        pos2(x_name, rect.top()),
        pos2(x_name + name_w, rect.bottom()),
    );
    painter.with_clip_rect(name_rect).text(
        pos2(x_name, cy),
        Align2::LEFT_CENTER,
        &tr.name,
        style::regular(style::BODY),
        p.text,
    );
    let (from, to) = match tr.direction {
        Direction::Upload => (tr.local.display().to_string(), tr.remote.clone()),
        Direction::Download => (tr.remote.clone(), tr.local.display().to_string()),
    };
    ui.interact(name_rect, egui::Id::new(("tr-name", tr.id)), Sense::hover())
        .on_hover_text(format!("{from}\n→ {to}"));
    let status_rect = egui::Rect::from_min_max(
        pos2(x_status, rect.top()),
        pos2(x_status + status_w, rect.bottom()),
    );
    let painter = ui.painter();
    let small = style::regular(style::SMALL);
    match &state {
        TransferState::Running => {
            let bar = egui::Rect::from_min_size(pos2(x_status, cy - 2.0), vec2(status_w, 4.0));
            kit::paint_bar(painter, bar, tr.fraction(), Tone::Accent, &p);
        }
        TransferState::Queued => {
            painter.text(
                pos2(x_status, cy),
                Align2::LEFT_CENTER,
                t("Waiting…"),
                small.clone(),
                p.text_faint,
            );
        }
        TransferState::Done if skipped.is_empty() => {
            painter.text(
                pos2(x_status, cy),
                Align2::LEFT_CENTER,
                t("Done"),
                small.clone(),
                p.text_faint,
            );
        }
        TransferState::Done => {
            painter.text(
                pos2(x_status, cy),
                Align2::LEFT_CENTER,
                tf("{} skipped", &[&skipped.len().to_string()]),
                small.clone(),
                p.warn,
            );
            ui.interact(
                status_rect,
                egui::Id::new(("tr-skip", tr.id)),
                Sense::hover(),
            )
            .on_hover_text(
                skipped
                    .iter()
                    .take(20)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        TransferState::Failed(e) => {
            painter.with_clip_rect(status_rect).text(
                pos2(x_status, cy),
                Align2::LEFT_CENTER,
                e,
                small.clone(),
                p.err,
            );
            ui.interact(
                status_rect,
                egui::Id::new(("tr-err", tr.id)),
                Sense::hover(),
            )
            .on_hover_text(e.as_str());
        }
        TransferState::Cancelled => {
            painter.text(
                pos2(x_status, cy),
                Align2::LEFT_CENTER,
                t("Cancelled"),
                small.clone(),
                p.text_faint,
            );
        }
    }
    let meta_rect = egui::Rect::from_min_size(pos2(x_meta, rect.top()), vec2(120.0, rect.height()));
    match &state {
        TransferState::Failed(_) => {
            let g = ui.painter().layout_no_wrap(
                t("Retry").to_string(),
                style::medium(style::SMALL),
                p.accent_text,
            );
            let r = egui::Rect::from_min_size(
                pos2(meta_rect.right() - g.size().x - 8.0, cy - 10.0),
                vec2(g.size().x + 8.0, 20.0),
            );
            let retry = ui.interact(r, egui::Id::new(("tr-retry", tr.id)), Sense::click());
            if retry.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(4), p.hover);
            }
            ui.painter()
                .galley(r.center() - g.size() / 2.0, g, p.accent_text);
            if retry.clicked() {
                out = Some(RowAction::Retry(tr.id));
            }
        }
        _ => {
            let meta = match &state {
                TransferState::Running => {
                    format!("{} / {}", entry::human_size(done), entry::human_size(total))
                }
                TransferState::Done => entry::human_size(total),
                _ => String::new(),
            };
            ui.painter().text(
                pos2(meta_rect.right(), cy),
                Align2::RIGHT_CENTER,
                meta,
                small,
                p.text_faint,
            );
        }
    }
    let x_rect = egui::Rect::from_center_size(pos2(x_close + 11.0, cy), vec2(18.0, 18.0));
    let tip = if tr.is_finished() {
        t("Remove")
    } else {
        t("Cancel")
    };
    let x = ui
        .interact(x_rect, egui::Id::new(("tr-x", tr.id)), Sense::click())
        .on_hover_text(tip);
    kit::paint_icon_button(ui, &x, x_rect, icons::X, tip, false, false);
    if x.clicked() {
        if tr.is_finished() {
            out = Some(RowAction::Remove(tr.id));
        } else {
            tr.cancel();
        }
    }
    out
}

/// "4 done · 1 failed" for a finished batch.
fn finished_detail(s: &Summary) -> String {
    let mut parts = vec![tf("{} done", &[&s.done.to_string()])];
    if s.failed > 0 {
        parts.push(tf("{} failed", &[&s.failed.to_string()]));
    }
    if s.cancelled > 0 {
        parts.push(tf("{} cancelled", &[&s.cancelled.to_string()]));
    }
    parts.join(" · ")
}

/// "Uploading 7 file(s)" (files inside folders count one by one).
pub fn summary_title(s: &Summary) -> String {
    let n = s.files_total.to_string();
    match (s.uploads > 0, s.downloads > 0) {
        (true, false) => tf("Uploading {} file(s)", &[&n]),
        (false, true) => tf("Downloading {} file(s)", &[&n]),
        _ => tf("Transferring {} file(s)", &[&n]),
    }
}

/// "3 done · 45% · 1.2 GB / 2.6 GB · 12.3 MB/s · about 20s left"
pub fn summary_detail(s: &Summary) -> String {
    let mut text = String::new();
    if s.files_total > 1 {
        text.push_str(&tf("{} done", &[&s.files_done.to_string()]));
        text.push_str(" · ");
    }
    text.push_str(&format!(
        "{}% · {} / {} · {}/s",
        (s.fraction() * 100.0).floor() as u32,
        entry::human_size(s.bytes_done),
        entry::human_size(s.bytes_total),
        entry::human_size(s.speed as u64)
    ));
    if let Some(eta) = s.eta() {
        let secs = eta.as_secs();
        let left = if secs >= 3600 {
            format!("{}h {}m", secs / 3600, secs % 3600 / 60)
        } else if secs >= 60 {
            format!("{}m {}s", secs / 60, secs % 60)
        } else {
            format!("{}s", secs.max(1))
        };
        text.push_str(&format!(" · {}", tf("about {} left", &[&left])));
    }
    text
}

/// What to say when a batch has finished (nothing if it was all cancelled).
fn batch_notice(s: &Summary) -> Option<(String, bool)> {
    let files = s.files_done.to_string();
    let mut text = match (s.uploads > 0, s.downloads > 0) {
        (true, false) => tf("Uploaded {} file(s).", &[&files]),
        (false, true) => tf("Downloaded {} file(s).", &[&files]),
        _ => tf("Transferred {} file(s).", &[&files]),
    };
    if s.failed > 0 {
        text.push(' ');
        text.push_str(&tf("{} transfer(s) failed.", &[&s.failed.to_string()]));
        return Some((text, true));
    }
    (s.done > 0).then_some((text, false))
}

/// The list's surface, which takes files dragged from the other side: an accent outline while
/// such a drag hovers it. Returns what `add_contents` returned, the files dropped, and whether
/// the pointer is over it.
fn file_drop_zone<R>(
    ui: &mut Ui,
    rect: egui::Rect,
    is_remote: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> (R, Option<Arc<DragFiles>>, bool) {
    let p = style::pal(ui);
    let response = ui.interact(rect, ui.id().with(("drop-zone", is_remote)), Sense::hover());
    // `contains_pointer`, not `hovered`: nothing counts as hovered while something is dragged.
    let target = response.contains_pointer()
        && egui::DragAndDrop::payload::<DragFiles>(ui.ctx())
            .is_some_and(|d| d.from_remote != is_remote);
    if target {
        ui.painter().rect_filled(
            rect.shrink(4.0),
            CornerRadius::same(style::RADIUS_MD),
            p.accent_softer,
        );
    }
    let inner = add_contents(ui);
    if target {
        ui.painter().rect_stroke(
            rect.shrink(4.0),
            CornerRadius::same(style::RADIUS_MD),
            Stroke::new(1.5, p.accent),
            StrokeKind::Inside,
        );
    }
    (
        inner,
        response.dnd_release_payload::<DragFiles>(),
        response.contains_pointer(),
    )
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
