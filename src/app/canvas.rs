//! The canvas card: the active session's panes and SFTP panel, and the connection lifecycle
//! drawn inside the pane (progress, host key, password, failure, lost connection) instead of in
//! dialogs over the whole window.

use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Key, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::{STATUS_H, TernaryShellApp};
use crate::bookmarks::Auth;
use crate::i18n::{t, tf};
use crate::ssh::types::{ConnState, HostKeyStatus, Prompt, SecretAnswer, SecretPromptKind};
use crate::ui::actions::Action;
use crate::ui::keys::Shortcut as S;
use crate::ui::layout::{self, SplitDir};
use crate::ui::sftp_panel::PanelOutput;
use crate::ui::style::{self, Tone};
use crate::ui::{icons, kit, terminal};

/// Snapshot of a pending prompt, copied out of the connection lock for drawing.
enum PromptView {
    HostKey {
        host: String,
        port: u16,
        algorithm: String,
        fingerprint: String,
        changed: bool,
        known: Vec<String>,
    },
    Secret {
        kind: SecretPromptKind,
        message: String,
        echo: bool,
        can_save: bool,
        retry: bool,
    },
}

/// The steps of making a connection, and when each was reached.
#[derive(Default)]
pub struct StepTrack {
    /// (step index, when it was reached), in order.
    marks: Vec<(usize, Instant)>,
    /// The step that failed, when the connection ended before its session.
    failed: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StepState {
    Done,
    Now,
    Pending,
    Failed,
}

/// The connection's steps: each jump host, the server, its host key, the login, the shell.
fn step_labels(
    app_store: &crate::bookmarks::BookmarkStore,
    b: &crate::bookmarks::Bookmark,
) -> (Vec<String>, Vec<String>) {
    let jumps: Vec<String> = app_store
        .jump_chain(b)
        .map(|c| c.iter().map(|j| j.display_name()).collect())
        .unwrap_or_default();
    let mut labels: Vec<String> = jumps.iter().map(|j| tf("Reach {}", &[j])).collect();
    let name = b.display_name();
    labels.push(match jumps.last() {
        Some(j) => tf("Connect to {} through {}", &[&name, j]),
        None => tf("Connect to {}", &[&name]),
    });
    labels.push(t("Verify host key").to_string());
    labels.push(match &b.auth {
        Auth::Password => t("Log in with password").to_string(),
        Auth::Key { .. } => t("Log in with key file").to_string(),
        Auth::KeychainKey => t("Log in with pasted key").to_string(),
        Auth::Agent { .. } => t("Log in with SSH agent").to_string(),
    });
    labels.push(t("Open shell").to_string());
    (labels, jumps)
}

/// "ssh-ed25519" → "ED25519".
fn key_type(algorithm: &str) -> String {
    let a = algorithm.to_ascii_lowercase();
    if a.contains("ed25519") {
        "ED25519".into()
    } else if a.contains("ecdsa") {
        if a.contains("384") {
            "ECDSA P-384".into()
        } else if a.contains("521") {
            "ECDSA P-521".into()
        } else {
            "ECDSA P-256".into()
        }
    } else if a.contains("rsa") {
        "RSA".into()
    } else {
        algorithm.to_string()
    }
}

fn ms(d: Duration) -> String {
    if d.as_secs() >= 10 {
        format!("{} s", d.as_secs())
    } else if d.as_millis() >= 1000 {
        format!("{:.1} s", d.as_secs_f32())
    } else {
        format!("{} ms", d.as_millis().max(1))
    }
}

impl TernaryShellApp {
    /// Follows each connection through its steps (for the progress card).
    pub(super) fn track_steps(&mut self) {
        for tab in &self.tabs {
            let (state, via, prompt_hostkey) = {
                let s = tab.conn.shared.lock();
                (
                    s.state.clone(),
                    s.via.clone(),
                    matches!(s.prompt, Some(Prompt::HostKey { .. })),
                )
            };
            let (_, jumps) = step_labels(&self.store, &tab.bookmark);
            let hops = jumps.len();
            let track = self.conn_steps.entry(tab.id).or_default();
            let step = match (&state, &via) {
                (ConnState::Connecting | ConnState::Authenticating, Some(v)) => {
                    jumps.iter().position(|j| j == v).unwrap_or(0)
                }
                (ConnState::Connecting, None) if prompt_hostkey => hops + 1,
                (ConnState::Connecting, None) => hops,
                (ConnState::Authenticating, None) => hops + 2,
                (ConnState::Connected, _) => hops + 3,
                (ConnState::Closed(_), _) => {
                    if !matches!(state, ConnState::Closed(None)) && track.failed.is_none() {
                        track.failed = track.marks.last().map(|m| m.0);
                    }
                    continue;
                }
            };
            // A new attempt (reconnect) starts over.
            let restarted = track
                .marks
                .last()
                .is_some_and(|m| step < m.0 && m.0 >= hops + 3)
                || (track.failed.is_some() && step <= hops);
            if restarted {
                *track = StepTrack::default();
            }
            if track.marks.last().is_none_or(|m| m.0 != step) {
                // Steps skipped over (no host key question) count as done at once.
                track.marks.push((step, Instant::now()));
            }
        }
        let ids: Vec<u64> = self.tabs.iter().map(|t| t.id).collect();
        self.conn_steps.retain(|id, _| ids.contains(id));
    }

    /// The canvas card: its fill and ring, the session or the Start page, the status strip.
    pub(super) fn canvas(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let dark = ui.visuals().dark_mode;
        let card = ui.max_rect();
        let fill = if self.tabs.is_empty() {
            p.canvas
        } else {
            self.scheme(ui.ctx()).background
        };
        if !dark {
            let shadow = egui::Shadow {
                offset: [0, 1],
                blur: 3,
                spread: 0,
                color: Color32::from_black_alpha(13),
            };
            ui.painter()
                .add(shadow.as_shape(card, CornerRadius::same(style::RADIUS_LG)));
        }
        ui.painter()
            .rect_filled(card, CornerRadius::same(style::RADIUS_LG), fill);
        if self.tabs.is_empty() {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(card));
            self.start_page(&mut child);
        } else {
            let status =
                egui::Rect::from_min_max(pos2(card.left(), card.bottom() - STATUS_H), card.max);
            let body = egui::Rect::from_min_max(card.min, pos2(card.right(), status.top()));
            self.session(ui, body, fill);
            self.status_strip(ui, status, fill);
        }
        let painter = ui.painter();
        kit::round_corners(painter, card, style::RADIUS_LG as f32, p.chrome);
        painter.rect_stroke(
            card,
            CornerRadius::same(style::RADIUS_LG),
            Stroke::new(1.0, p.card_ring),
            StrokeKind::Inside,
        );
    }

    /// The active tab: its panes over the optional SFTP panel, and the connection overlays.
    fn session(&mut self, ui: &mut Ui, body: egui::Rect, fill: Color32) {
        let ctx = ui.ctx().clone();
        let p = style::pal(ui);
        let cell = self.cell_size(&ctx);
        let scheme = self.scheme(&ctx);
        let modal_open = self.modal_open();
        let rt = self.rt.handle().clone();
        let fonts_choice = crate::ui::fonts::effective_choice(&self.settings.font);
        let term_style = terminal::TermStyle {
            font_size: self.settings.font_size,
            cell,
            scheme,
            real_bold: crate::ui::fonts::has_real_face(&fonts_choice, true, false),
            real_italic: crate::ui::fonts::has_real_face(&fonts_choice, false, true),
            option_as_meta: self.settings.option_as_meta,
            copy_on_select: self.settings.copy_on_select,
            cursor_blink: self.settings.cursor_blink,
        };
        let active = self.active;
        let tab = &mut self.tabs[active];
        tab.clear_activity();
        let state = tab.state();
        let (has_prompt, checking, was_connected) = {
            let s = tab.conn.shared.lock();
            let checking = s
                .checking_since
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(1500));
            (s.prompt.is_some(), checking, s.connected_at.is_some())
        };

        // Terminal over the optional SFTP panel.
        let (term_rect, sftp_rect) = if tab.show_sftp {
            let split_y = (body.top() + body.height() * (1.0 - tab.sftp_split)).round();
            (
                egui::Rect::from_min_max(body.min, pos2(body.right(), split_y)),
                Some(egui::Rect::from_min_max(
                    pos2(body.left(), split_y),
                    body.max,
                )),
            )
        } else {
            (body, None)
        };

        // A split pane whose shell exited closes by itself; the last pane stays.
        if tab.panes.len() > 1 {
            let ended: Vec<_> = tab
                .panes
                .iter()
                .filter(|p| {
                    matches!(
                        *p.status.lock(),
                        crate::ssh::session::PaneStatus::Exited(Some(_))
                    )
                })
                .map(|p| p.id)
                .collect();
            for id in ended {
                if tab.panes.len() > 1 {
                    tab.close_pane(id);
                }
            }
        }
        if self.swallow_repeats {
            if ctx.input(|i| i.keys_down.is_empty()) {
                self.swallow_repeats = false;
            } else {
                ctx.input_mut(|i| {
                    i.events
                        .retain(|e| !matches!(e, egui::Event::Key { repeat: true, .. }))
                });
            }
        }
        // Keyboard focus follows the active pane (after a split, a tab switch, ⌘[ / ⌘]).
        let want_focus = (tab.id, tab.focused);
        if self.focus_target != Some(want_focus) && !modal_open && !has_prompt {
            ctx.memory_mut(|m| m.request_focus(terminal::pane_widget_id(tab.focused)));
            self.focus_target = Some(want_focus);
        }

        let (pane_rects, dividers) = tab.layout.arrange(term_rect);
        let multi = pane_rects.len() > 1;
        let tab_id = tab.id;
        let mut new_focus = None;
        let mut sent_input = false;
        let mut close_pane = None;
        for (pane_index, (pane_id, rect)) in pane_rects.into_iter().enumerate() {
            #[cfg(debug_assertions)]
            crate::devtest::record(format!("pane:{pane_index}"), rect);
            let _ = pane_index;
            let focused_pane = tab.focused == pane_id;
            let Some(pane) = tab.pane_mut(pane_id) else {
                continue;
            };
            let alive = pane.is_live();
            // 12 px above and below the text, 16 px beside it (the terminal pads 10).
            let inner = rect.shrink2(vec2(6.0, 2.0));
            let resp = terminal::show(
                ui,
                inner,
                &mut pane.view,
                &pane.shared,
                &term_style,
                focused_pane,
                !modal_open && !has_prompt,
                alive,
            );
            if !resp.input.is_empty() {
                pane.send(resp.input);
                sent_input = true;
            }
            if let Some((size, px_w, px_h)) = resp.resized {
                let _ = pane.input.send(crate::ssh::session::PaneInput::Resize {
                    cols: size.cols as u32,
                    rows: size.rows as u32,
                    px_w,
                    px_h,
                });
            }
            if let Some(text) = pane.shared.notes.lock().clipboard.take() {
                ctx.copy_text(text);
            }
            if resp.reconnect {
                self.actions.push(Action::RestartPane(pane_id));
            }
            // An ended shell: why, and what Enter / ⌘R do, under its last output (a dropped
            // connection has its own bar instead).
            if !alive
                && state.is_connected()
                && matches!(
                    *pane.status.lock(),
                    crate::ssh::session::PaneStatus::Exited(_)
                )
                && let (Some(at), Some(hint)) =
                    (resp.cursor_cell, pane.shared.notes.lock().ended.clone())
            {
                session_ended_hint(ui, inner, at, &hint);
            }
            if resp.activated {
                new_focus = Some(pane_id);
            }
            if let Some(dir) = resp.split {
                new_focus = Some(pane_id);
                self.actions.push(match dir {
                    SplitDir::Horizontal => Action::SplitRight,
                    SplitDir::Vertical => Action::SplitDown,
                });
            }
            // The panes that don't have the keyboard are dimmed.
            if multi && !focused_pane {
                ui.painter().rect_filled(rect, 0.0, style::soft(fill, 0.42));
            }
            if multi && !modal_open {
                let close_rect = egui::Rect::from_min_size(
                    pos2(rect.right() - 8.0 - 22.0, rect.top() + 8.0),
                    vec2(22.0, 22.0),
                );
                let close = ui
                    .interact(
                        close_rect,
                        egui::Id::new(("pane-close", tab_id, pane_id)),
                        Sense::click(),
                    )
                    .on_hover_text(format!("{} ({})", t("Close pane"), S::Close.label()));
                ui.painter()
                    .rect_filled(close_rect, CornerRadius::same(style::RADIUS), fill);
                let painter = ui.painter();
                if close.hovered() {
                    painter.rect_filled(close_rect, CornerRadius::same(style::RADIUS), p.hover);
                }
                painter.text(
                    close_rect.center(),
                    Align2::CENTER_CENTER,
                    icons::X,
                    style::icon_font(14.0),
                    if close.hovered() {
                        p.text
                    } else {
                        p.text_muted.gamma_multiply(0.7)
                    },
                );
                #[cfg(debug_assertions)]
                crate::devtest::record(format!("pane-close:{pane_index}"), close_rect);
                if close.clicked() {
                    close_pane = Some(pane_id);
                }
            }
            let bell = pane.shared.notes.lock().bell_at;
            if let Some(at) = bell {
                let age = at.elapsed().as_secs_f32();
                if age < 0.25 {
                    ui.painter().rect_filled(
                        rect,
                        0.0,
                        Color32::from_white_alpha(((0.25 - age) * 120.0) as u8),
                    );
                    ctx.request_repaint_after(Duration::from_millis(30));
                }
            }
        }
        if let Some(f) = new_focus {
            tab.focused = f;
        }
        if sent_input {
            tab.note_input();
        }
        if let Some(id) = close_pane {
            tab.close_pane(id);
        }
        for (i, d) in dividers.iter().enumerate() {
            let hit = d.rect.expand2(match d.dir {
                SplitDir::Horizontal => vec2(1.0, 0.0),
                SplitDir::Vertical => vec2(0.0, 1.0),
            });
            let resp = ui.interact(hit, egui::Id::new(("divider", tab.id, i)), Sense::drag());
            let color = if resp.hovered() || resp.dragged() {
                p.accent
            } else {
                p.border
            };
            ui.painter().rect_filled(d.rect, 0.0, fill);
            match d.dir {
                SplitDir::Horizontal => {
                    ui.painter().vline(
                        d.rect.center().x,
                        d.rect.y_range(),
                        Stroke::new(1.0, color),
                    );
                }
                SplitDir::Vertical => {
                    ui.painter().hline(
                        d.rect.x_range(),
                        d.rect.center().y,
                        Stroke::new(1.0, color),
                    );
                }
            }
            if resp.hovered() || resp.dragged() {
                ctx.set_cursor_icon(match d.dir {
                    SplitDir::Horizontal => egui::CursorIcon::ResizeHorizontal,
                    SplitDir::Vertical => egui::CursorIcon::ResizeVertical,
                });
            }
            if resp.dragged()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                tab.layout.set_ratio(&d.path, layout::ratio_at(d, pos));
            }
        }

        if let Some(rect) = sftp_rect {
            // The panel's top edge: a hairline with a grip, dragged to resize.
            let div = egui::Rect::from_min_max(
                pos2(body.left(), rect.top() - 3.0),
                pos2(body.right(), rect.top() + 3.0),
            );
            let resp = ui.interact(div, egui::Id::new(("sftp-divider", tab.id)), Sense::drag());
            #[cfg(debug_assertions)]
            crate::devtest::record("sftp-divider", div);
            if resp.hovered() || resp.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeVertical);
            }
            if resp.dragged()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                tab.sftp_split = (1.0 - (pos.y - body.top()) / body.height()).clamp(0.2, 0.85);
                ctx.request_repaint();
            }
            let last = self.settings.last_local_dir.clone();
            if tab.sftp.is_none() && tab.state().is_connected() {
                tab.sftp = Some(tab.new_sftp_panel(last.as_deref()));
            }
            ui.painter().rect_filled(rect, 0.0, p.canvas);
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            match &mut tab.sftp {
                Some(panel) => {
                    if let PanelOutput::LocalDir(dir) = panel.show(&mut child, &rt, !modal_open) {
                        self.settings.last_local_dir = Some(dir);
                    }
                }
                None => {
                    child.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new(t("SFTP opens once the connection is ready."))
                                .color(p.text_faint),
                        )
                    });
                }
            }
            let painter = ui.painter();
            painter.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
            let grip = egui::Rect::from_center_size(
                pos2(rect.center().x, rect.top() + 1.0),
                vec2(36.0, 4.0),
            );
            painter.rect_filled(
                grip,
                CornerRadius::same(2),
                if resp.hovered() || resp.dragged() {
                    p.accent
                } else {
                    style::soft(p.border_strong, 0.7)
                },
            );
        }

        // Files dragged from Finder with the panel closed: the whole session is the target.
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering && state.is_connected() && sftp_rect.is_none() {
            let host = tab.bookmark.display_name();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("drop-overlay"),
            ));
            kit::drop_zone(
                &painter,
                term_rect.shrink(6.0),
                &p,
                &tf("to the remote folder on {}", &[&host]),
            );
        }

        // The connection lifecycle, over the panes.
        self.connection_overlay(
            ui,
            term_rect,
            fill,
            state,
            has_prompt,
            checking,
            was_connected,
        );
    }

    /// What the connection is doing, drawn in the pane: a progress card while it connects, the
    /// host key or login question, why it failed, or a bar to reconnect once it is gone.
    #[allow(clippy::too_many_arguments)]
    fn connection_overlay(
        &mut self,
        ui: &mut Ui,
        area: egui::Rect,
        fill: Color32,
        state: ConnState,
        has_prompt: bool,
        checking: bool,
        was_connected: bool,
    ) {
        let ctx = ui.ctx().clone();
        let p = style::pal(ui);
        if has_prompt {
            ui.painter().rect_filled(area, 0.0, style::soft(fill, 0.72));
            self.prompt_card(&ctx, area);
            return;
        }
        match &state {
            ConnState::Connecting | ConnState::Authenticating => {
                ui.painter().rect_filled(area, 0.0, style::soft(fill, 0.72));
                self.progress_card(&ctx, area, None);
            }
            ConnState::Closed(Some(e)) if !was_connected => {
                ui.painter().rect_filled(area, 0.0, style::soft(fill, 0.72));
                let message = e.message();
                self.progress_card(&ctx, area, Some(message));
            }
            ConnState::Closed(error) => {
                ui.painter().rect_filled(area, 0.0, style::soft(fill, 0.45));
                let (title, detail) = match error {
                    Some(e) => (t("Connection lost"), e.message()),
                    None => (
                        t("Disconnected"),
                        t("The connection was closed.").to_string(),
                    ),
                };
                self.lost_bar(&ctx, area, title, &detail);
            }
            ConnState::Connected if checking => {
                let address = self.tabs[self.active].bookmark.display_name();
                egui::Area::new(egui::Id::new(("checking", self.tabs[self.active].id)))
                    .fixed_pos(pos2(area.center().x, area.top() + 14.0))
                    .pivot(Align2::CENTER_TOP)
                    .order(egui::Order::Middle)
                    .show(&ctx, |ui| {
                        egui::Frame::NONE
                            .fill(p.raised)
                            .corner_radius(CornerRadius::same(14))
                            .shadow(ui.visuals().popup_shadow)
                            .stroke(ui.visuals().window_stroke)
                            .inner_margin(egui::Margin::symmetric(12, 0))
                            .show(ui, |ui| {
                                ui.set_height(28.0);
                                ui.horizontal_centered(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    kit::spinner(ui, 12.0);
                                    ui.label(
                                        RichText::new(tf(
                                            "Checking the connection to {}…",
                                            &[&address],
                                        ))
                                        .font(style::regular(style::SMALL))
                                        .color(p.text_muted),
                                    );
                                });
                            });
                    });
            }
            ConnState::Connected => {}
        }
    }

    /// The card while connecting (or after it failed): the steps, each with how long it took.
    fn progress_card(&mut self, ctx: &egui::Context, area: egui::Rect, failure: Option<String>) {
        let tab = &self.tabs[self.active];
        let tab_id = tab.id;
        let bookmark = tab.bookmark.clone();
        let (labels, _) = step_labels(&self.store, &bookmark);
        let track = self.conn_steps.get(&tab_id);
        let marks: Vec<(usize, Instant)> = track.map(|t| t.marks.clone()).unwrap_or_default();
        let failed = track.and_then(|t| t.failed);
        let now_step = marks.last().map_or(0, |m| m.0);
        let shell = labels.len() - 1;
        let mut cancel = false;
        let mut retry = false;
        let mut edit = false;
        let mut close = false;
        let title = if failure.is_some() {
            tf("Couldn't connect to {}", &[&bookmark.display_name()])
        } else {
            tf("Connecting to {}", &[&bookmark.display_name()])
        };
        let address = super::chrome::address_with_port(&bookmark);
        card_area(ctx, ("progress", tab_id), area, 452.0, |ui| {
            let p = style::pal(ui);
            card_head(
                ui,
                if failure.is_some() {
                    icons::WARNING_CIRCLE
                } else {
                    icons::PLUGS
                },
                if failure.is_some() {
                    Tone::Err
                } else {
                    Tone::Neutral
                },
                &title,
                Some((&address, true)),
            );
            card_body(ui, |ui| {
                for (i, label) in labels.iter().enumerate() {
                    if i == shell && failure.is_none() && now_step < shell {
                        // "Open shell" waits until the login is done.
                    }
                    let st = if failure.is_some() && Some(i) == failed.or(Some(now_step)) {
                        StepState::Failed
                    } else if i < now_step {
                        StepState::Done
                    } else if i == now_step && failure.is_none() {
                        StepState::Now
                    } else {
                        StepState::Pending
                    };
                    let meta = if st == StepState::Done {
                        let start = marks.iter().find(|m| m.0 == i).map(|m| m.1);
                        let end = marks.iter().find(|m| m.0 > i).map(|m| m.1);
                        match (start, end) {
                            (Some(a), Some(b)) => Some(ms(b - a)),
                            (None, Some(_)) => None,
                            _ => None,
                        }
                    } else {
                        None
                    };
                    step_row(ui, st, label, meta.as_deref());
                }
                if let Some(message) = &failure {
                    ui.add_space(8.0);
                    kit::callout(ui, icons::WARNING, Tone::Err, message);
                }
            });
            card_foot(
                ui,
                |ui| {
                    if failure.is_some() {
                        if kit::ghost(ui, Some(icons::PENCIL_SIMPLE), t("Edit Connection…"))
                            .clicked()
                        {
                            edit = true;
                        }
                    } else {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        kit::kbd(ui, "esc");
                        ui.label(
                            RichText::new(t("to cancel"))
                                .font(style::regular(style::SMALL))
                                .color(p.text_faint),
                        );
                    }
                },
                |ui| {
                    if failure.is_some() {
                        if kit::button_kbd(
                            ui,
                            kit::Kind::Primary,
                            None,
                            t("Try Again"),
                            Some(&S::Reconnect.label()),
                            true,
                            false,
                        )
                        .clicked()
                        {
                            retry = true;
                        }
                        if kit::secondary_sm(ui, None, t("Close Tab")).clicked() {
                            close = true;
                        }
                    } else if kit::secondary_sm(ui, None, t("Cancel")).clicked() {
                        cancel = true;
                    }
                },
            );
        });
        if failure.is_none() && ctx.input(|i| i.key_pressed(Key::Escape)) && !self.modal_open() {
            cancel = true;
        }
        if cancel {
            self.actions.push(Action::CancelConnect(tab_id));
        }
        if retry {
            self.actions.push(Action::Reconnect);
        }
        if edit {
            self.actions.push(Action::EditBookmark(bookmark.id));
        }
        if close {
            self.actions.push(Action::CloseTab(tab_id));
        }
    }

    /// The bar over a session whose connection is gone: what happened, Reconnect, and more.
    fn lost_bar(&mut self, ctx: &egui::Context, area: egui::Rect, title: &str, detail: &str) {
        let tab = &self.tabs[self.active];
        let (tab_id, bookmark_id) = (tab.id, tab.bookmark.id);
        let mut reconnect = false;
        let mut edit = false;
        let mut close = false;
        let mut copy = false;
        egui::Area::new(egui::Id::new(("lost", tab_id)))
            .fixed_pos(pos2(area.center().x, area.top() + 14.0))
            .pivot(Align2::CENTER_TOP)
            .order(egui::Order::Middle)
            .constrain_to(area)
            .show(ctx, |ui| {
                let p = style::pal(ui);
                egui::Frame::NONE
                    .fill(p.raised)
                    .corner_radius(CornerRadius::same(style::RADIUS_LG))
                    .shadow(ui.visuals().popup_shadow)
                    .stroke(ui.visuals().window_stroke)
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 8,
                        top: 8,
                        bottom: 8,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            kit::icon(ui, icons::PLUGS, 17.0, p.err);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(
                                    RichText::new(title)
                                        .font(style::medium(style::BODY))
                                        .color(p.text),
                                );
                                ui.set_max_width((area.width() - 260.0).clamp(160.0, 460.0));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(detail)
                                            .font(style::regular(style::SMALL))
                                            .color(p.text_muted),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(detail);
                            });
                            ui.add_space(8.0);
                            if kit::button_ex(
                                ui,
                                kit::Kind::Primary,
                                Some(icons::ARROWS_CLOCKWISE),
                                t("Reconnect"),
                                true,
                            )
                            .on_hover_text(S::Reconnect.label())
                            .clicked()
                            {
                                reconnect = true;
                            }
                            let more =
                                kit::icon_button_ex(ui, icons::DOTS_THREE, t("More"), false, 22.0);
                            egui::Popup::menu(&more).show(|ui| {
                                kit::menu_body(ui);
                                ui.set_min_width(200.0);
                                if kit::menu_item(
                                    ui,
                                    Some(icons::PENCIL_SIMPLE),
                                    t("Edit Connection…"),
                                    None,
                                )
                                .clicked()
                                {
                                    edit = true;
                                    ui.close();
                                }
                                if kit::menu_item(ui, Some(icons::COPY), t("Copy Error"), None)
                                    .clicked()
                                {
                                    copy = true;
                                    ui.close();
                                }
                                kit::menu_sep(ui);
                                if kit::menu_item(
                                    ui,
                                    Some(icons::X),
                                    t("Close Tab"),
                                    Some(&S::Close.label()),
                                )
                                .clicked()
                                {
                                    close = true;
                                    ui.close();
                                }
                            });
                        });
                    });
            });
        if reconnect {
            self.actions.push(Action::Reconnect);
        }
        if edit {
            self.actions.push(Action::EditBookmark(bookmark_id));
        }
        if copy {
            ctx.copy_text(detail.to_string());
        }
        if close {
            self.actions.push(Action::CloseTab(tab_id));
        }
    }

    /// The host key or login question, in a card in the pane. Only this tab waits: the rest of
    /// the window stays usable. A host key is only ever accepted with a click.
    fn prompt_card(&mut self, ctx: &egui::Context, area: egui::Rect) {
        let active = self.active;
        let view = {
            let tab = &self.tabs[active];
            let s = tab.conn.shared.lock();
            match &s.prompt {
                Some(Prompt::HostKey {
                    host,
                    port,
                    algorithm,
                    fingerprint,
                    status,
                    known,
                    ..
                }) => PromptView::HostKey {
                    host: host.clone(),
                    port: *port,
                    algorithm: algorithm.clone(),
                    fingerprint: fingerprint.clone(),
                    changed: *status == HostKeyStatus::Changed,
                    known: known.clone(),
                },
                Some(Prompt::Secret {
                    kind,
                    message,
                    echo,
                    can_save,
                    retry,
                    ..
                }) => PromptView::Secret {
                    kind: *kind,
                    message: message.clone(),
                    echo: *echo,
                    can_save: *can_save,
                    retry: *retry,
                },
                None => return,
            }
        };
        // A palette or dialog opened over the window gets the keys until it closes.
        let on_top = ctx.memory(|m| m.top_modal_layer().is_none());
        let tab_id = self.tabs[active].id;
        let bookmark = self.tabs[active].bookmark.clone();
        let (labels, jumps) = step_labels(&self.store, &bookmark);
        let marks: Vec<(usize, Instant)> = self
            .conn_steps
            .get(&tab_id)
            .map(|t| t.marks.clone())
            .unwrap_or_default();
        let now_step = marks.last().map_or(0, |m| m.0);
        let via = self.tabs[active].conn.shared.lock().via.clone();
        let mut answer: Option<Option<SecretAnswer>> = None;
        let mut hostkey_answer: Option<bool> = None;
        let tab = &mut self.tabs[active];
        match &view {
            PromptView::HostKey {
                host,
                port,
                algorithm,
                fingerprint,
                changed,
                known,
            } => {
                let name = via.clone().unwrap_or_else(|| bookmark.display_name());
                card_area(
                    ctx,
                    ("hostkey", tab_id),
                    area,
                    if *changed { 480.0 } else { 452.0 },
                    |ui| {
                        let p = style::pal(ui);
                        if *changed {
                            let r = ui.max_rect();
                            let bar = egui::Rect::from_min_size(r.min, vec2(r.width(), 4.0));
                            ui.painter().rect_filled(
                                bar,
                                CornerRadius {
                                    nw: style::RADIUS_XL,
                                    ne: style::RADIUS_XL,
                                    sw: 0,
                                    se: 0,
                                },
                                p.err,
                            );
                            ui.add_space(4.0);
                            card_head(
                                ui,
                                icons::SHIELD_WARNING,
                                Tone::Err,
                                &tf("The host key of {} has changed", &[&name]),
                                Some((t("Someone may be intercepting this connection."), false)),
                            );
                        } else {
                            card_head(
                                ui,
                                icons::FINGERPRINT,
                                Tone::Neutral,
                                &tf("First time connecting to {}", &[&name]),
                                Some((
                                    t("Check the fingerprint before trusting this server."),
                                    false,
                                )),
                            );
                        }
                        card_body(ui, |ui| {
                            if *changed {
                                kit::callout(
                                    ui,
                                    icons::WARNING,
                                    Tone::Err,
                                    t(
                                        "The server sent a different key than the one saved for it. Only continue if you know why it changed — for example, the server was reinstalled.",
                                    ),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    RichText::new(tf("New key · {}", &[&key_type(algorithm)]))
                                        .font(style::medium(style::SMALL))
                                        .color(p.text_muted),
                                );
                                fingerprint_box(ui, fingerprint, false);
                                if !known.is_empty() {
                                    ui.add_space(2.0);
                                    ui.label(
                                        RichText::new(t("Saved key"))
                                            .font(style::medium(style::SMALL))
                                            .color(p.text_muted),
                                    );
                                    for k in known {
                                        fingerprint_box(ui, k, true);
                                    }
                                }
                            } else {
                                let hop_step = jumps
                                    .iter()
                                    .position(|j| Some(j) == via.as_ref())
                                    .unwrap_or(jumps.len() + 1);
                                for (i, label) in labels.iter().enumerate().take(hop_step + 1) {
                                    let st = if i < now_step.min(hop_step)
                                        || (i < hop_step && via.is_none())
                                    {
                                        StepState::Done
                                    } else {
                                        StepState::Now
                                    };
                                    let meta = if st == StepState::Done {
                                        let start = marks.iter().find(|m| m.0 == i).map(|m| m.1);
                                        let end = marks.iter().find(|m| m.0 > i).map(|m| m.1);
                                        start.zip(end).map(|(a, b)| ms(b - a))
                                    } else {
                                        None
                                    };
                                    let label = if via.is_some() && i == hop_step {
                                        t("Verify host key").to_string()
                                    } else {
                                        label.clone()
                                    };
                                    step_row(ui, st, &label, meta.as_deref());
                                }
                                ui.add_space(12.0);
                                for (k, v, mono) in [
                                    (t("Host"), format!("{host}:{port}"), true),
                                    (t("Key type"), key_type(algorithm), false),
                                ] {
                                    let (r, _) = ui.allocate_exact_size(
                                        vec2(ui.available_width(), 20.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().text(
                                        pos2(r.left(), r.center().y),
                                        Align2::LEFT_CENTER,
                                        k,
                                        style::regular(12.5),
                                        p.text_muted,
                                    );
                                    let font = if mono {
                                        style::mono(12.5)
                                    } else {
                                        style::regular(12.5)
                                    };
                                    ui.painter().text(
                                        pos2(r.left() + 96.0 + 14.0, r.center().y),
                                        Align2::LEFT_CENTER,
                                        v,
                                        font,
                                        p.text,
                                    );
                                    ui.add_space(8.0);
                                }
                                ui.add_space(8.0);
                                fingerprint_box(ui, fingerprint, false);
                                ui.add_space(8.0);
                                if kit::ghost(ui, Some(icons::COPY), t("Copy fingerprint"))
                                    .clicked()
                                {
                                    ui.ctx().copy_text(fingerprint.clone());
                                }
                            }
                        });
                        if *changed {
                            let mut accept = false;
                            let mut refuse = false;
                            card_foot(
                                ui,
                                |ui| {
                                    accept = kit::button_ex(
                                        ui,
                                        kit::Kind::DangerQuiet,
                                        None,
                                        t("Accept New Key"),
                                        true,
                                    )
                                    .clicked();
                                },
                                |ui| {
                                    refuse = kit::button_ex(
                                        ui,
                                        kit::Kind::Primary,
                                        None,
                                        t("Cancel"),
                                        true,
                                    )
                                    .clicked();
                                },
                            );
                            if accept {
                                hostkey_answer = Some(true);
                            } else if refuse {
                                hostkey_answer = Some(false);
                            }
                        } else {
                            card_foot(
                                ui,
                                |ui| {
                                    ui.label(
                                        RichText::new(t("Saved to TernaryShell's known_hosts"))
                                            .font(style::regular(style::SMALL))
                                            .color(p.text_faint),
                                    );
                                },
                                |ui| {
                                    // No Enter shortcut: a key meant for the terminal must never trust a
                                    // host key. The decision takes a click.
                                    if kit::button_ex(
                                        ui,
                                        kit::Kind::Primary,
                                        None,
                                        t("Trust and Connect"),
                                        true,
                                    )
                                    .clicked()
                                    {
                                        hostkey_answer = Some(true);
                                    }
                                    if kit::secondary_sm(ui, None, t("Cancel")).clicked() {
                                        hostkey_answer = Some(false);
                                    }
                                },
                            );
                        }
                    },
                );
            }
            PromptView::Secret {
                kind,
                message,
                echo,
                can_save,
                retry,
            } => {
                let (icon, title, sub, go) = match kind {
                    SecretPromptKind::Password => (
                        icons::LOCK_KEY,
                        tf("Password for {}", &[message]),
                        match &via {
                            Some(v) => tf("{} · through {}", &[&bookmark.host, v]),
                            None => match jumps.last() {
                                Some(j) => tf("{} · through {}", &[&bookmark.host, j]),
                                None => bookmark.host.clone(),
                            },
                        },
                        t("Connect"),
                    ),
                    SecretPromptKind::Passphrase => (
                        icons::KEY,
                        t("Key passphrase").to_string(),
                        tf("{} is protected by a passphrase", &[message]),
                        t("Unlock"),
                    ),
                    SecretPromptKind::Interactive => (
                        icons::SHIELD_CHECK,
                        t("Verification").to_string(),
                        t("The server asks").to_string(),
                        t("Send"),
                    ),
                };
                let settled = tab
                    .prompt
                    .shown_at
                    .is_some_and(|t| t.elapsed() >= Duration::from_millis(300));
                let enter = ctx.input(|i| {
                    i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, repeat: false, modifiers, .. } if modifiers.is_none()))
                });
                let prompt = &mut tab.prompt;
                card_area(ctx, ("secret", tab_id), area, 420.0, |ui| {
                    let p = style::pal(ui);
                    card_head(ui, icon, Tone::Neutral, &title, Some((&sub, false)));
                    card_body(ui, |ui| {
                        if *kind == SecretPromptKind::Interactive {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(message.as_str())
                                        .font(style::mono(style::MONO))
                                        .color(p.text),
                                )
                                .wrap(),
                            );
                            ui.add_space(10.0);
                        }
                        let (r, shown_toggle) = secret_field(
                            ui,
                            &mut prompt.text,
                            *echo,
                            *retry,
                            *kind == SecretPromptKind::Interactive,
                            ("prompt-field", tab_id),
                        );
                        let _ = shown_toggle;
                        if *retry {
                            ui.add_space(6.0);
                            style::hint_tone(
                                ui,
                                if *kind == SecretPromptKind::Passphrase {
                                    t("Wrong passphrase, try again.")
                                } else {
                                    t("Wrong password, try again.")
                                },
                                Tone::Err,
                            );
                        }
                        if *can_save {
                            ui.add_space(12.0);
                            let label = if *kind == SecretPromptKind::Passphrase {
                                tf("Save passphrase in {}", &[crate::platform::keychain()])
                            } else {
                                tf("Save in {}", &[crate::platform::keychain()])
                            };
                            if style::checkbox(ui, &mut prompt.save, label).changed() {
                                r.request_focus();
                            }
                        }
                        let nothing_focused = ui.memory(|m| m.focused().is_none());
                        let submit = settled
                            && enter
                            && (r.lost_focus() || r.has_focus() || nothing_focused);
                        if on_top && !submit && (!prompt.focus || nothing_focused || r.lost_focus())
                        {
                            r.request_focus();
                            prompt.focus = true;
                        }
                        if submit && on_top {
                            answer = Some(Some(SecretAnswer {
                                value: std::mem::take(&mut prompt.text),
                                save: prompt.save,
                            }));
                        }
                    });
                    card_foot(
                        ui,
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            kit::kbd(ui, "↩");
                            ui.label(
                                RichText::new(t("to connect ·"))
                                    .font(style::regular(style::SMALL))
                                    .color(p.text_faint),
                            );
                            kit::kbd(ui, "esc");
                            ui.label(
                                RichText::new(t("to cancel"))
                                    .font(style::regular(style::SMALL))
                                    .color(p.text_faint),
                            );
                        },
                        |ui| {
                            if kit::button_ex(ui, kit::Kind::Primary, None, go, true).clicked() {
                                answer = Some(Some(SecretAnswer {
                                    value: std::mem::take(&mut prompt.text),
                                    save: prompt.save,
                                }));
                            }
                            if kit::secondary_sm(ui, None, t("Cancel")).clicked() {
                                answer = Some(None);
                            }
                        },
                    );
                });
            }
        }
        // Only Esc dismisses (a stray click beside the card must not cancel the login).
        let escape = on_top && ctx.input(|i| i.key_pressed(Key::Escape));
        let tab = &mut self.tabs[active];
        if escape && tab.prompt.armed && answer.is_none() && hostkey_answer.is_none() {
            match view {
                PromptView::HostKey { .. } => hostkey_answer = Some(false),
                PromptView::Secret { .. } => answer = Some(None),
            }
        }
        if answer.is_some() || hostkey_answer.is_some() {
            tab.answer_prompt(hostkey_answer.unwrap_or(false), answer.flatten());
            self.focus_target = None;
            // The Enter that answered must not repeat into the terminal.
            self.swallow_repeats = true;
            ctx.request_repaint();
        } else if !on_top {
            tab.prompt.focus = false;
            tab.prompt.armed = false;
            tab.prompt.shown_at = None;
            ctx.request_repaint_after(Duration::from_millis(150));
        } else {
            tab.prompt.armed = true;
            if tab.prompt.shown_at.is_none() {
                tab.prompt.shown_at = Some(Instant::now());
                ctx.request_repaint_after(Duration::from_millis(320));
            }
        }
    }
}

// ----- the card ---------------------------------------------------------------------------

/// A connection card centered in `area`, in its own layer above the panes.
fn card_area(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    area: egui::Rect,
    width: f32,
    add: impl FnOnce(&mut Ui),
) {
    egui::Area::new(egui::Id::new(id))
        .fixed_pos(area.center())
        .pivot(Align2::CENTER_CENTER)
        .order(egui::Order::Middle)
        .constrain_to(area.shrink(8.0))
        .show(ctx, |ui| {
            let p = style::pal(ui);
            egui::Frame::NONE
                .fill(p.surface)
                .corner_radius(CornerRadius::same(style::RADIUS_XL))
                .shadow(ui.visuals().window_shadow)
                .stroke(Stroke::new(
                    1.0,
                    if p.dark {
                        Color32::from_white_alpha(15)
                    } else {
                        Color32::from_black_alpha(20)
                    },
                ))
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    add(ui);
                });
        });
}

/// The card's header: an icon tile, the title and a line under it (mono for addresses).
fn card_head(ui: &mut Ui, icon: &str, tone: Tone, title: &str, sub: Option<(&str, bool)>) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 18,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                kit::icon_tile(ui, icon, tone, 36.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let color = if tone == Tone::Err { p.err } else { p.text };
                    ui.add(
                        egui::Label::new(
                            RichText::new(title)
                                .font(style::semibold(style::TITLE))
                                .color(color),
                        )
                        .wrap(),
                    );
                    if let Some((s, mono)) = sub {
                        let font = if mono {
                            style::mono(style::SMALL)
                        } else {
                            style::regular(style::SMALL)
                        };
                        ui.add(
                            egui::Label::new(RichText::new(s).font(font).color(p.text_muted))
                                .wrap(),
                        );
                    }
                });
            });
        });
}

fn card_body(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 4,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
}

/// The card's footer: a hairline, a hint or quiet action on the left, buttons on the right.
fn card_foot(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    let p = style::pal(ui);
    let top = ui.cursor().top();
    ui.painter().hline(
        ui.max_rect().x_range(),
        top + 0.5,
        Stroke::new(1.0, p.border),
    );
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_height(style::TOOLBAR_H);
                left(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    right(ui);
                });
            });
        });
}

/// One step (30 px): an 18 px circle (✓ done, spinning now, empty pending, ✗ failed), the
/// label, and how long it took on the right.
fn step_row(ui: &mut Ui, state: StepState, label: &str, meta: Option<&str>) {
    let p = style::pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let c = pos2(rect.left() + 9.0, rect.center().y);
    let painter = ui.painter();
    match state {
        StepState::Done => {
            painter.circle_filled(c, 9.0, p.tone_soft(Tone::Ok));
            painter.text(
                c,
                Align2::CENTER_CENTER,
                icons::CHECK,
                style::icon_font(11.0),
                p.ok,
            );
        }
        StepState::Failed => {
            painter.circle_filled(c, 9.0, p.tone_soft(Tone::Err));
            painter.text(
                c,
                Align2::CENTER_CENTER,
                icons::X,
                style::icon_font(11.0),
                p.err,
            );
        }
        StepState::Pending => {
            painter.circle_stroke(c, 8.25, Stroke::new(1.5, p.border_strong));
        }
        StepState::Now => {
            let time = ui.input(|i| i.time) as f32;
            let start = time * 5.0;
            let points: Vec<egui::Pos2> = (0..=24)
                .map(|i| {
                    let a = start + i as f32 / 24.0 * std::f32::consts::TAU * 0.75;
                    c + vec2(a.cos(), a.sin()) * 8.25
                })
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(1.5, p.accent)));
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
    }
    let (font, color) = match state {
        StepState::Done => (style::regular(style::BODY), p.text_muted),
        StepState::Now => (style::medium(style::BODY), p.text),
        StepState::Pending => (style::regular(style::BODY), p.text_faint),
        StepState::Failed => (style::regular(style::BODY), p.err),
    };
    let painter = ui.painter();
    let meta_w = meta.map_or(0.0, |m| {
        let g = painter.layout_no_wrap(m.to_string(), style::regular(style::SMALL), p.text_faint);
        let w = g.size().x;
        painter.galley(
            pos2(rect.right() - w, rect.center().y - g.size().y / 2.0),
            g,
            p.text_faint,
        );
        w + 10.0
    });
    let clip = egui::Rect::from_min_max(rect.min, pos2(rect.right() - meta_w, rect.bottom()));
    painter.with_clip_rect(clip).text(
        pos2(rect.left() + 28.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        color,
    );
}

/// A fingerprint in a mono box (selectable); a saved one that no longer matches is struck out.
fn fingerprint_box(ui: &mut Ui, text: &str, old: bool) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .fill(p.field)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut rt = RichText::new(text).font(style::mono(12.0)).color(if old {
                p.text_faint
            } else {
                p.text
            });
            if old {
                rt = rt.strikethrough();
            }
            ui.add(egui::Label::new(rt).wrap().selectable(true));
        });
    ui.add_space(6.0);
}

/// The secret field of a prompt with a show/hide eye (passwords); returns its response.
fn secret_field(
    ui: &mut Ui,
    text: &mut String,
    echo: bool,
    error: bool,
    mono: bool,
    id: impl std::hash::Hash + std::fmt::Debug,
) -> (egui::Response, bool) {
    let p = style::pal(ui);
    let id = egui::Id::new(id);
    let shown_id = id.with("shown");
    let mut shown: bool = ui.data(|d| d.get_temp(shown_id)).unwrap_or(false);
    let width = ui.available_width();
    let mut field = style::input(text)
        .id(id)
        .password(!echo && !shown)
        .desired_width(width)
        .margin(egui::Margin {
            left: 10,
            right: if echo { 10 } else { 30 },
            top: 7,
            bottom: 7,
        })
        .error(error);
    if mono {
        field = field.mono();
    }
    let r = ui.add(field);
    let mut toggled = false;
    if !echo {
        let eye = egui::Rect::from_center_size(
            pos2(r.rect.right() - 15.0, r.rect.center().y),
            vec2(18.0, 18.0),
        );
        let resp = ui
            .interact(eye, id.with("eye"), Sense::click())
            .on_hover_text(if shown { t("Hide") } else { t("Show") });
        kit::paint_icon_button(
            ui,
            &resp,
            eye,
            if shown { icons::EYE_SLASH } else { icons::EYE },
            "",
            false,
            false,
        );
        let _ = p;
        if resp.clicked() {
            shown = !shown;
            toggled = true;
            ui.data_mut(|d| d.insert_temp(shown_id, shown));
            r.request_focus();
        }
    }
    (r, toggled)
}

/// "Session ended. Press ↩ for a new shell, or ⌘R to reconnect." in a soft chip at `at` (the
/// line under the ended session's output), clipped to the pane.
fn session_ended_hint(ui: &Ui, pane: egui::Rect, at: egui::Rect, headline: &str) {
    let p = style::pal(ui);
    let painter = ui.painter().with_clip_rect(pane.shrink(2.0));
    let font = style::regular(style::SMALL);
    let enter = "↩";
    let reconnect = S::Reconnect.label();
    // "Press {} for a new shell, or {} to reconnect." around the two key caps.
    let template = t("Press {} for a new shell, or {} to reconnect.");
    let mut parts = template.splitn(3, "{}");
    let (a, b, c) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    );
    let text = |s: &str| painter.layout_no_wrap(s.to_string(), font.clone(), p.text_muted);
    let kbd_w = |k: &str| (text(k).size().x + 10.0).max(20.0);
    let pieces = [
        text(&format!("{headline} {}", a.trim_end())),
        text(b.trim()),
        text(c.trim_start()),
    ];
    let gap = 6.0;
    let width = 10.0
        + 14.0
        + 10.0
        + pieces.iter().map(|g| g.size().x).sum::<f32>()
        + kbd_w(enter)
        + kbd_w(&reconnect)
        + gap * 4.0
        + 10.0;
    let rect = egui::Rect::from_min_size(pos2(at.left(), at.top() + 2.0), vec2(width, 28.0));
    painter.rect_filled(rect, CornerRadius::same(style::RADIUS_MD), p.hover);
    let y = rect.center().y;
    let mut x = rect.left() + 10.0;
    painter.text(
        pos2(x + 7.0, y),
        Align2::CENTER_CENTER,
        icons::TERMINAL,
        style::icon_font(14.0),
        p.text_muted,
    );
    x += 14.0 + 10.0;
    let [first, middle, last] = pieces;
    for (galley, key) in [
        (first, Some(enter)),
        (middle, Some(reconnect.as_str())),
        (last, None),
    ] {
        let w = galley.size().x;
        painter.galley(pos2(x, y - galley.size().y / 2.0), galley, p.text_muted);
        x += w + gap;
        if let Some(key) = key {
            x += kit::paint_kbd_at(&painter, pos2(x, y), key, &p) + gap;
        }
    }
}
