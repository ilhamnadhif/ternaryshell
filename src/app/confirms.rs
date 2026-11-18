//! The confirmations the app asks (close a tab, delete, quit, reconnect, restart to update),
//! with what each one affects spelled out row by row.

use std::time::Duration;

use uuid::Uuid;

use super::TernaryShellApp;
use crate::bookmarks::Auth;
use crate::i18n::{t, tf};
use crate::paths;
use crate::sftp::entry::human_size;
use crate::ui::actions::Action;
use crate::ui::confirm::{ConfirmExtra, ConfirmRow, GroupTree, HostChip};
use crate::ui::dialogs::{self, ConfirmAction, Dialog};
use crate::ui::style::Tone;
use crate::ui::tabs::{Tab, cwd_from_title};
use crate::ui::{icons, kit};

/// The key cap of Quit: pressing the shortcut again quits.
pub fn quit_keys() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘Q"
    } else {
        "Ctrl+Shift+Q"
    }
}

/// A program running in the foreground, from a terminal title (`vim nginx.conf`), not the
/// `user@host: ~/dir` or shell name that shells set when idle.
pub fn running_program(title: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() || title.contains('@') || cwd_from_title(title).is_some() {
        return None;
    }
    let first = title
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('-');
    let shell = matches!(
        first.rsplit('/').next().unwrap_or(first),
        "bash" | "zsh" | "sh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "login" | "ash"
    );
    (!shell && title.len() <= 80).then(|| title.to_string())
}

/// The file listing the connections open when an update restarted the app.
fn reopen_file() -> std::path::PathBuf {
    paths::data_dir().join("reopen.json")
}

/// Saved connections to open again after an update restarted the app (once, if recent).
pub fn reopen_after_update() -> Vec<Action> {
    let path = reopen_file();
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(900));
    let data = fresh.then(|| std::fs::read(&path).ok()).flatten();
    let _ = std::fs::remove_file(&path);
    data.and_then(|d| serde_json::from_slice::<Vec<Uuid>>(&d).ok())
        .unwrap_or_default()
        .into_iter()
        .map(Action::Connect)
        .collect()
}

impl TernaryShellApp {
    /// Remembers the open saved connections so the updated app opens them again.
    pub(super) fn remember_for_reopen(&self) {
        let ids: Vec<Uuid> = self
            .tabs
            .iter()
            .map(|t| t.bookmark.id)
            .filter(|id| self.store.bookmark(*id).is_some())
            .collect();
        if !ids.is_empty()
            && let Ok(json) = serde_json::to_vec(&ids)
        {
            let _ = paths::write_private(&reopen_file(), &json);
        }
    }

    /// The label and address under a confirmation's title.
    fn sub_of(&self, b: &crate::bookmarks::Bookmark) -> ConfirmExtra {
        ConfirmExtra {
            sub: Some((self.store.label_of(b).cloned(), b.address())),
            ..Default::default()
        }
    }

    fn chip(&self, tab: &Tab) -> HostChip {
        HostChip {
            name: tab.title(),
            icon: kit::host_icon(tab.bookmark.host_icon()),
            color: self.store.label_of(&tab.bookmark).map(|l| l.color),
        }
    }

    /// "Close web-01?" when it is connected, transfers files or has unsaved edits.
    pub(super) fn close_tab_dialog(&self, tab: &Tab) -> Dialog {
        let transfers = tab.running_transfers();
        let unsaved = tab.unsaved_file();
        let other = transfers > 0 || unsaved.is_some();
        let mut rows = Vec::new();
        if tab.is_active() {
            let mut row = ConfirmRow::new(icons::TERMINAL, t("The session is still running"))
                .tone(if other { Tone::Warn } else { Tone::Neutral });
            if let Some(program) = tab
                .panes
                .iter()
                .filter(|p| p.is_live())
                .filter_map(|p| p.title())
                .find_map(|title| running_program(&title))
            {
                row = row.detail(tf("Running `{}`", &[&program]));
            }
            rows.push(row);
        }
        if transfers > 0 {
            rows.push(self.transfers_row(tab, transfers, Tone::Err));
        }
        if let Some(file) = &unsaved {
            rows.push(
                ConfirmRow::new(icons::PENCIL_SIMPLE, tf("{} has unsaved changes", &[file]))
                    .tone(Tone::Err),
            );
        }
        let extra = ConfirmExtra {
            icon: Some((
                icons::TERMINAL_WINDOW,
                if other { Tone::Warn } else { Tone::Neutral },
            )),
            escape: Some(t("keep it open").to_string()),
            ..self.sub_of(&tab.bookmark)
        };
        // Only the plain "it's connected" question can be turned off; lost transfers and
        // edits are always asked about.
        dialogs::confirm_rows(
            &tf("Close {}?", &[&tab.title()]),
            String::new(),
            rows,
            t("Close Tab"),
            ConfirmAction::CloseTab(tab.id),
            extra,
            !other,
        )
    }

    fn transfers_row(&self, tab: &Tab, transfers: usize, tone: Tone) -> ConfirmRow {
        let mut row = ConfirmRow::new(
            icons::ARROWS_DOWN_UP,
            tf(
                "{} transfer(s) will be cancelled",
                &[&transfers.to_string()],
            ),
        )
        .tone(tone)
        .detail(t("Partial files are removed, on the server too."));
        if let Some(s) = tab.transfer_progress()
            && s.bytes_total > 0
        {
            row = row.right(human_size(s.bytes_total));
        }
        row
    }

    /// "Delete “web-02”?": what goes, and that the server is not touched.
    pub(super) fn delete_bookmark_dialog(&self, id: Uuid) -> Option<Dialog> {
        let b = self.store.bookmark(id)?;
        let mut rows = Vec::new();
        let settings = if b.forwards.is_empty() {
            t("Connection settings and tags").to_string()
        } else {
            tf(
                "Connection settings, tags and {} forward(s)",
                &[&b.forwards.len().to_string()],
            )
        };
        rows.push(
            ConfirmRow::new(icons::HARD_DRIVES, settings)
                .tone(Tone::Err)
                .right(t("removed")),
        );
        let keychain = crate::platform::keychain();
        let secret = match &b.auth {
            Auth::Password => Some(tf("Saved password in {}", &[keychain])),
            Auth::Key { .. } => Some(tf("Saved passphrase in {}", &[keychain])),
            Auth::KeychainKey => Some(tf("Pasted key and passphrase in {}", &[keychain])),
            Auth::Agent { .. } => None,
        };
        if let Some(s) = secret {
            rows.push(
                ConfirmRow::new(icons::KEY, s)
                    .tone(Tone::Err)
                    .right(t("removed")),
            );
        }
        rows.push(
            ConfirmRow::new(
                icons::SHIELD_CHECK,
                t("The server, its files and `~/.ssh` keys"),
            )
            .tone(Tone::Ok)
            .right(t("not touched")),
        );
        Some(dialogs::confirm_rows(
            &tf("Delete “{}”?", &[&b.display_name()]),
            tf(
                "The connection and its saved password are removed from {}. The server isn't touched.",
                &[crate::platform::this_computer()],
            ),
            rows,
            t("Delete"),
            ConfirmAction::DeleteBookmark(id),
            self.sub_of(b),
            false,
        ))
    }

    /// "Delete group?": the group and what moves up one level, before and after.
    pub(super) fn delete_group_dialog(&self, id: Uuid) -> Option<Dialog> {
        let g = self.store.group(id)?;
        const SHOWN: usize = 6;
        let mut items: Vec<(String, &'static str, bool)> = self
            .store
            .child_groups(Some(id))
            .map(|c| (c.name.clone(), icons::FOLDER_SIMPLE, true))
            .collect();
        items.extend(
            self.store
                .bookmarks_in(Some(id))
                .map(|b| (b.display_name(), kit::host_icon(b.host_icon()), false)),
        );
        let more = items.len().saturating_sub(SHOWN);
        items.truncate(SHOWN);
        let count = self.store.bookmarks_in(Some(id)).count();
        let tree = GroupTree {
            name: g.name.clone(),
            color: self.store.group_label(Some(id)).map(|l| l.color),
            parent: g.parent.map(|p| self.store.group_path(Some(p))),
            items,
            more,
        };
        Some(dialogs::confirm_rows(
            &tf("Delete group “{}”?", &[&g.name]),
            tf(
                "Its {} connection(s) and subgroups move up one level. Nothing else is deleted.",
                &[&count.to_string()],
            ),
            Vec::new(),
            t("Delete Group"),
            ConfirmAction::DeleteGroup(id),
            ConfirmExtra {
                tree: (!tree.items.is_empty()).then_some(tree),
                ..Default::default()
            },
            false,
        ))
    }

    /// What quitting (or restarting to update) would interrupt, if the user should be asked
    /// first. Running file transfers always ask; open sessions only when the setting says so.
    pub(super) fn quit_rows(&self) -> Option<Vec<ConfirmRow>> {
        let live: Vec<&Tab> = self.tabs.iter().filter(|t| t.is_active()).collect();
        let transfers: usize = self.tabs.iter().map(Tab::running_transfers).sum();
        let mut rows = Vec::new();
        if !live.is_empty() && (self.settings.confirm_close_connected || transfers > 0) {
            rows.push(
                ConfirmRow::new(
                    icons::TERMINAL_WINDOW,
                    tf("{} open session(s)", &[&live.len().to_string()]),
                )
                .hosts(live.iter().take(6).map(|t| self.chip(t)).collect()),
            );
        }
        if transfers > 0 {
            let (done, total) = self
                .tabs
                .iter()
                .filter_map(Tab::transfer_progress)
                .fold((0u64, 0u64), |(d, t), s| {
                    (d + s.bytes_done, t + s.bytes_total)
                });
            let mut row = ConfirmRow::new(
                icons::ARROWS_DOWN_UP,
                tf(
                    "{} transfer(s) will be cancelled",
                    &[&transfers.to_string()],
                ),
            )
            .tone(Tone::Warn)
            .detail(t("Partial files are removed, on the server too."));
            if total > 0 {
                row = row.right(format!("{}%", (done as f64 / total as f64 * 100.0).floor()));
            }
            rows.push(row);
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            if let Some(file) = tab.unsaved_file() {
                rows.push(
                    ConfirmRow::new(
                        icons::PENCIL_SIMPLE,
                        tf("{} on {} has unsaved changes", &[&file, &tab.title()]),
                    )
                    .tone(Tone::Err)
                    .link(t("Review"), Action::SelectTab(i)),
                );
            }
        }
        (!rows.is_empty()).then_some(rows)
    }

    pub(super) fn quit_dialog(&self, rows: Vec<ConfirmRow>) -> Dialog {
        dialogs::confirm_rows(
            t("Quit TernaryShell?"),
            String::new(),
            rows,
            t("Quit"),
            ConfirmAction::Quit,
            ConfirmExtra {
                keys: Some(quit_keys().to_string()),
                width: Some(440.0),
                ..Default::default()
            },
            false,
        )
    }

    pub(super) fn restart_dialog(&self, rows: Vec<ConfirmRow>) -> Dialog {
        dialogs::confirm_rows(
            t("Restart to update?"),
            t("TernaryShell restarts and opens your connections again.").to_string(),
            rows,
            t("Restart"),
            ConfirmAction::RestartForUpdate,
            ConfirmExtra {
                width: Some(440.0),
                ..Default::default()
            },
            false,
        )
    }

    /// "Reconnect web-01?" while something runs in it.
    pub(super) fn reconnect_dialog(&self, tab: &Tab) -> Dialog {
        let mut rows = Vec::new();
        let panes = tab.panes.len();
        if panes > 1 {
            rows.push(
                ConfirmRow::new(
                    icons::SQUARE_SPLIT_HORIZONTAL,
                    tf("{} panes", &[&panes.to_string()]),
                )
                .detail(t("Each gets a new shell")),
            );
        }
        let transfers = tab.running_transfers();
        if transfers > 0 {
            rows.push(self.transfers_row(tab, transfers, Tone::Err));
        }
        dialogs::confirm_rows(
            &tf("Reconnect {}?", &[&tab.title()]),
            t("The running sessions are closed and opened again. Scrollback is kept.").to_string(),
            rows,
            t("Reconnect"),
            ConfirmAction::Reconnect(tab.id),
            self.sub_of(&tab.bookmark),
            false,
        )
    }
}

#[cfg(debug_assertions)]
impl TernaryShellApp {
    /// `dg <what>` in a debug script: shows one of this area's dialogs, toasts or states.
    pub(super) fn dev_dialogs(&mut self, _ctx: &eframe::egui::Context, what: &str) {
        use crate::ui::toasts::{Do, Kind, Toast};
        let id_of = |app: &Self, name: &str| {
            app.store
                .bookmarks
                .iter()
                .find(|b| b.display_name() == name)
                .map(|b| b.id)
        };
        match what {
            "close" => {
                if let Some(tab) = self.tabs.get(self.active) {
                    self.dialog = Some(self.close_tab_dialog(tab));
                }
            }
            "delete" => {
                if let Some(id) =
                    id_of(self, "web-02").or(self.store.bookmarks.first().map(|b| b.id))
                {
                    self.dialog = self.delete_bookmark_dialog(id);
                }
            }
            "group" => {
                if let Some(g) = self.store.groups.first() {
                    self.dialog = self.delete_group_dialog(g.id);
                }
            }
            "quit" => {
                let rows = self.quit_rows().unwrap_or_default();
                self.dialog = Some(self.quit_dialog(rows));
            }
            "reconnect" => {
                if let Some(tab) = self.tabs.get(self.active) {
                    self.dialog = Some(self.reconnect_dialog(tab));
                }
            }
            "about" => self.about = Some(Default::default()),
            "toasts" => {
                self.toasts.add(
                    Toast::new(Kind::Ok, "Downloaded 4 file(s).")
                        .detail("Into the local folder `~/Downloads/web-01`")
                        .button(
                            crate::platform::reveal_label(),
                            Do::Reveal(paths::data_dir().to_path_buf()),
                        ),
                );
                self.toasts.add(
                    Toast::new(Kind::Warn, "db-primary: port forwarding")
                        .icon(icons::PLUGS, None)
                        .detail("Port 6379 is in use on this Mac, so 6379 → redis.internal isn't listening.")
                        .button(t("Edit forwards…"), Do::Run(Action::Settings))
                        .quiet_button(t("Retry"), Do::Run(Action::Settings))
                        .sticky(),
                );
                self.toasts.add(
                    Toast::new(Kind::Info, "Exported to connections.json")
                        .icon(icons::EXPORT, None)
                        .detail("9 connections in ~/Downloads. Passwords and keys are included as plain text; delete it once it's imported.")
                        .button(crate::platform::reveal_label(), Do::Reveal(paths::data_dir().to_path_buf())),
                );
                self.toasts.add(
                    Toast::new(Kind::Error, "Couldn't connect to worker-staging")
                        .detail("Connection timed out after 15 s. The host may be down or a firewall drops SSH.")
                        .button(t("Retry"), Do::Run(Action::Reconnect))
                        .keys("⌘R")
                        .quiet_button(t("Copy Error"), Do::Copy("timed out".into())),
                );
            }
            other => self
                .updater
                .dev_state(other.strip_prefix("update-").unwrap_or(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::running_program;

    #[test]
    fn programs_from_titles() {
        assert_eq!(
            running_program("vim nginx.conf").as_deref(),
            Some("vim nginx.conf")
        );
        assert_eq!(
            running_program("tail -f access.log").as_deref(),
            Some("tail -f access.log")
        );
        assert_eq!(running_program("deploy@web-01: ~/app"), None);
        assert_eq!(running_program("-bash"), None);
        assert_eq!(running_program("zsh"), None);
        assert_eq!(running_program("/bin/zsh"), None);
        assert_eq!(running_program(""), None);
    }
}
