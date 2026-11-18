//! The confirmations the app asks (close a tab, delete, quit, reconnect, restart to update),
//! with what each one affects said in plain sentences, with real numbers.

use std::time::Duration;

use uuid::Uuid;

use super::TernaryShellApp;
use crate::bookmarks::Auth;
use crate::i18n::{t, tf};
use crate::paths;
use crate::sftp::entry::human_size;
use crate::ui::actions::Action;
use crate::ui::confirm::{ConfirmExtra, ConfirmRow};
use crate::ui::dialogs::{self, ConfirmAction, Dialog};
use crate::ui::tabs::{Tab, cwd_from_title};

/// Clauses as one sentence: "A.", "A, and B.", "A, B, and C."
fn sentence(clauses: &[String]) -> String {
    match clauses {
        [] => String::new(),
        [a] => format!("{a}."),
        [rest @ .., last] => tf("{}, and {}.", &[&rest.join(", "), last]),
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

    /// "Close web-01?" when it is connected, transfers files or has unsaved edits.
    pub(super) fn close_tab_dialog(&self, tab: &Tab) -> Dialog {
        let transfers = tab.running_transfers();
        let unsaved = tab.unsaved_file();
        let other = transfers > 0 || unsaved.is_some();
        let mut clauses = Vec::new();
        if tab.is_active() {
            clauses.push(
                match tab
                    .panes
                    .iter()
                    .filter(|p| p.is_live())
                    .filter_map(|p| p.title())
                    .find_map(|title| running_program(&title))
                {
                    Some(program) => tf("The session is still running `{}`", &[&program]),
                    None => t("The session is still running").to_string(),
                },
            );
        }
        if transfers > 0 {
            clauses.push(self.transfers_clause(tab, transfers));
        }
        if let Some(file) = &unsaved {
            clauses.push(tf("{} has unsaved changes", &[file]));
        }
        // Only the plain "it's connected" question can be turned off; lost transfers and
        // edits are always asked about.
        dialogs::confirm_rows(
            &tf("Close {}?", &[&tab.title()]),
            sentence(&clauses),
            Vec::new(),
            t("Close Tab"),
            ConfirmAction::CloseTab(tab.id),
            ConfirmExtra::default(),
            !other,
        )
    }

    /// "2 transfers (1.2 GB) will be cancelled".
    fn transfers_clause(&self, tab: &Tab, transfers: usize) -> String {
        let n = transfers.to_string();
        let size = tab
            .transfer_progress()
            .filter(|s| s.bytes_total > 0)
            .map(|s| human_size(s.bytes_total));
        match (transfers, size) {
            (1, Some(size)) => tf("1 transfer ({}) will be cancelled", &[&size]),
            (1, None) => t("1 transfer will be cancelled").to_string(),
            (_, Some(size)) => tf("{} transfers ({}) will be cancelled", &[&n, &size]),
            (_, None) => tf("{} transfers will be cancelled", &[&n]),
        }
    }

    /// "Delete “web-02”?": what goes, and that the server is not touched.
    pub(super) fn delete_bookmark_dialog(&self, id: Uuid) -> Option<Dialog> {
        let b = self.store.bookmark(id)?;
        let here = crate::platform::this_computer();
        let goes = match &b.auth {
            Auth::Password => tf(
                "The connection and its saved password are removed from {}.",
                &[here],
            ),
            Auth::Key { .. } => tf(
                "The connection and its saved passphrase are removed from {}.",
                &[here],
            ),
            Auth::KeychainKey => tf(
                "The connection and its pasted key are removed from {}.",
                &[here],
            ),
            Auth::Agent { .. } => tf("The connection is removed from {}.", &[here]),
        };
        Some(dialogs::confirm_rows(
            &tf("Delete “{}”?", &[&b.display_name()]),
            format!(
                "{goes} {}",
                t("The server and your keys in `~/.ssh` are not touched.")
            ),
            Vec::new(),
            t("Delete"),
            ConfirmAction::DeleteBookmark(id),
            ConfirmExtra::default(),
            false,
        ))
    }

    /// "Delete the group?": only the group goes, its contents move up one level.
    pub(super) fn delete_group_dialog(&self, id: Uuid) -> Option<Dialog> {
        let g = self.store.group(id)?;
        let count = self.store.bookmarks_in(Some(id)).count();
        let subgroups = self.store.child_groups(Some(id)).count();
        let message = match (count, subgroups) {
            (0, 0) => t("The group is empty. Nothing else is deleted.").to_string(),
            (1, 0) => t("Only the group goes. Its connection moves up one level.").to_string(),
            (n, 0) => tf(
                "Only the group goes. Its {} connections move up one level.",
                &[&n.to_string()],
            ),
            (0, _) => t("Only the group goes. Its subgroups move up one level.").to_string(),
            (1, _) => t("Only the group goes. Its connection and subgroups move up one level.")
                .to_string(),
            (n, _) => tf(
                "Only the group goes. Its {} connections and subgroups move up one level.",
                &[&n.to_string()],
            ),
        };
        Some(dialogs::confirm_rows(
            &tf("Delete the group “{}”?", &[&g.name]),
            message,
            Vec::new(),
            t("Delete Group"),
            ConfirmAction::DeleteGroup(id),
            ConfirmExtra::default(),
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
            const SHOWN: usize = 5;
            let mut names: Vec<String> = live.iter().take(SHOWN).map(|t| t.title()).collect();
            if live.len() > SHOWN {
                names.push("…".to_string());
            }
            let names = names.join(", ");
            rows.push(ConfirmRow::new(if live.len() == 1 {
                tf("1 session: {}", &[&names])
            } else {
                tf("{} sessions: {}", &[&live.len().to_string(), &names])
            }));
        }
        if transfers > 0 {
            let (done, total) = self
                .tabs
                .iter()
                .filter_map(Tab::transfer_progress)
                .fold((0u64, 0u64), |(d, t), s| {
                    (d + s.bytes_done, t + s.bytes_total)
                });
            let n = transfers.to_string();
            let pct =
                (total > 0).then(|| format!("{}", (done as f64 / total as f64 * 100.0).floor()));
            rows.push(ConfirmRow::new(match (transfers, pct) {
                (1, Some(pct)) => tf("1 transfer at {}% (partial files are removed)", &[&pct]),
                (1, None) => t("1 transfer (partial files are removed)").to_string(),
                (_, Some(pct)) => tf(
                    "{} transfers at {}% (partial files are removed)",
                    &[&n, &pct],
                ),
                (_, None) => tf("{} transfers (partial files are removed)", &[&n]),
            }));
        }
        for (i, tab) in self.tabs.iter().enumerate() {
            if let Some(file) = tab.unsaved_file() {
                rows.push(
                    ConfirmRow::new(tf("{} on {}, not saved", &[&file, &tab.title()]))
                        .link(t("Review"), Action::SelectTab(i)),
                );
            }
        }
        (!rows.is_empty()).then_some(rows)
    }

    pub(super) fn quit_dialog(&self, rows: Vec<ConfirmRow>) -> Dialog {
        dialogs::confirm_rows(
            t("Quit TernaryShell?"),
            t("These will be closed:").to_string(),
            rows,
            t("Quit"),
            ConfirmAction::Quit,
            ConfirmExtra { width: Some(440.0) },
            false,
        )
    }

    pub(super) fn restart_dialog(&self, rows: Vec<ConfirmRow>) -> Dialog {
        dialogs::confirm_rows(
            t("Restart to update?"),
            format!(
                "{} {}",
                t("TernaryShell restarts and opens your connections again."),
                t("These will be closed:")
            ),
            rows,
            t("Restart"),
            ConfirmAction::RestartForUpdate,
            ConfirmExtra { width: Some(440.0) },
            false,
        )
    }

    /// "Reconnect web-01?" while something runs in it.
    pub(super) fn reconnect_dialog(&self, tab: &Tab) -> Dialog {
        let mut message = match tab.panes.len() {
            0 | 1 => t("The session is closed and opened again. Scrollback is kept.").to_string(),
            2 => t("Both panes get a new shell. Scrollback is kept.").to_string(),
            n => tf(
                "All {} panes get a new shell. Scrollback is kept.",
                &[&n.to_string()],
            ),
        };
        let transfers = tab.running_transfers();
        if transfers > 0 {
            message = format!("{message} {}.", self.transfers_clause(tab, transfers));
        }
        dialogs::confirm_rows(
            &tf("Reconnect {}?", &[&tab.title()]),
            message,
            Vec::new(),
            t("Reconnect"),
            ConfirmAction::Reconnect(tab.id),
            ConfirmExtra::default(),
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
                    Toast::new(Kind::Ok, "Downloaded 4 files.")
                        .detail("Into the local folder `~/Downloads/web-01`")
                        .button(
                            crate::platform::reveal_label(),
                            Do::Reveal(paths::data_dir().to_path_buf()),
                        ),
                );
                self.toasts.add(
                    Toast::new(Kind::Warn, "db-primary: port forwarding")
                        .detail("Port 6379 is in use on this Mac, so 6379 → redis.internal isn't listening.")
                        .button(t("Edit forwards…"), Do::Run(Action::Settings))
                        .quiet_button(t("Retry"), Do::Run(Action::Settings))
                        .sticky(),
                );
                self.toasts.add(
                    Toast::new(Kind::Info, "Exported to connections.json")
                        .detail("9 connections in ~/Downloads. Passwords and keys are included as plain text; delete it once it's imported.")
                        .button(crate::platform::reveal_label(), Do::Reveal(paths::data_dir().to_path_buf())),
                );
                self.toasts.add(
                    Toast::new(Kind::Error, "Couldn't connect to worker-staging")
                        .detail("Connection timed out after 15 s. The host may be down or a firewall drops SSH.")
                        .button(t("Retry"), Do::Run(Action::Reconnect))
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
