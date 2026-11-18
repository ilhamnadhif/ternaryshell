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
use crate::ssh::types::{
    ConnError, ConnState, HostKeyStatus, ProbeReason, Prompt, SecretAnswer, SecretPromptKind,
};
use crate::ui::actions::Action;
use crate::ui::keys::Shortcut as S;
use crate::ui::layout::{self, SplitDir};
use crate::ui::sftp_panel::PanelOutput;
use crate::ui::style::{self, Tone};
use crate::ui::{icons, kit, terminal};

mod cards;

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
    /// When it failed.
    failed_at: Option<Instant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
                        track.failed_at = Some(Instant::now());
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

    /// The canvas card: its fill and ring, the session or the Start page, the status strip,
    /// and a 2 px line in the session's environment label colour along its top edge.
    pub(super) fn canvas(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let dark = ui.visuals().dark_mode;
        let card = ui.max_rect();
        let radius = CornerRadius::same(super::chrome::CANVAS_R);
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
                color: Color32::from_black_alpha(10),
            };
            ui.painter().add(shadow.as_shape(card, radius));
        }
        ui.painter().rect_filled(card, radius, fill);
        if self.tabs.is_empty() {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(card));
            self.start_page(&mut child);
        } else {
            let status =
                egui::Rect::from_min_max(pos2(card.left(), card.bottom() - STATUS_H), card.max);
            let body = egui::Rect::from_min_max(card.min, pos2(card.right(), status.top()));
            self.session(ui, body, fill);
            self.status_strip(ui, status, fill);
            if let Some(tab) = self.tabs.get(self.active) {
                let saved = self
                    .store
                    .bookmark(tab.bookmark.id)
                    .unwrap_or(&tab.bookmark);
                if let Some(l) = self.store.label_of(saved) {
                    let edge =
                        egui::Rect::from_min_max(card.min, pos2(card.right(), card.top() + 2.0));
                    ui.painter().rect_filled(
                        edge,
                        0.0,
                        style::label_color(l.color, dark).gamma_multiply(0.9),
                    );
                }
            }
        }
        let painter = ui.painter();
        kit::round_corners(painter, card, super::chrome::CANVAS_R as f32, p.chrome);
        painter.rect_stroke(
            card,
            radius,
            Stroke::new(1.0, p.card_ring),
            StrokeKind::Outside,
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
            if let Some(action) = resp.action {
                self.actions.push(action);
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
                ui.painter().rect_filled(rect, 0.0, style::soft(fill, 0.38));
            }
            // A pane's × shows while the pointer is over the pane.
            if multi && !modal_open && ui.rect_contains_pointer(rect) {
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
                self.progress_card(&ctx, area, Some(e.clone()));
            }
            ConnState::Closed(error) => {
                // The output stays readable (and scrollable) under the bar, dimmed.
                ui.painter().rect_filled(area, 0.0, style::soft(fill, 0.65));
                self.lost_bar(&ctx, area, error.as_ref());
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

    /// The stops on the way to `bookmark` (this Mac, each jump host, the server), for the route.
    fn route_stops(&self, bookmark: &crate::bookmarks::Bookmark, dark: bool) -> Vec<cards::Hop> {
        let chain: Vec<crate::bookmarks::Bookmark> = self
            .store
            .jump_chain(bookmark)
            .map(|c| c.into_iter().cloned().collect())
            .unwrap_or_default();
        let hop = |b: &crate::bookmarks::Bookmark| cards::Hop {
            name: b.display_name(),
            icon: kit::host_icon(b.host_icon()),
            label: self
                .store
                .label_of(b)
                .map(|l| style::label_color(l.color, dark)),
            this_mac: false,
        };
        let mut stops = vec![cards::Hop {
            name: t("this Mac").to_string(),
            icon: icons::LAPTOP,
            label: None,
            this_mac: true,
        }];
        stops.extend(chain.iter().map(hop));
        stops.push(hop(bookmark));
        stops
    }

    /// The card while connecting (or after it failed): the route through the jump hosts, the
    /// steps with how long each took, and when it failed, what to check.
    fn progress_card(&mut self, ctx: &egui::Context, area: egui::Rect, failure: Option<ConnError>) {
        let tab = &self.tabs[self.active];
        let tab_id = tab.id;
        let bookmark = tab.bookmark.clone();
        let dark = ctx.global_style().visuals.dark_mode;
        let (labels, jumps) = step_labels(&self.store, &bookmark);
        let track = self.conn_steps.get(&tab_id);
        let marks: Vec<(usize, Instant)> = track.map(|t| t.marks.clone()).unwrap_or_default();
        let failed_at = track.and_then(|t| t.failed_at);
        let now_step = marks.last().map_or(0, |m| m.0);
        let failed = failure
            .as_ref()
            .map(|_| track.and_then(|t| t.failed).unwrap_or(now_step));
        let label = self.store.label_of(&bookmark).cloned();
        let label_color = label.as_ref().map(|l| style::label_color(l.color, dark));
        let stops = (!jumps.is_empty()).then(|| self.route_stops(&bookmark, dark));
        let checks = failure
            .as_ref()
            .map(|e| cards::things_to_check(e, &bookmark))
            .unwrap_or_default();
        let name = bookmark.display_name();
        let address = super::chrome::address_with_port(&bookmark);
        let sub_rest = match jumps.last() {
            Some(j) => {
                let full = tf("{} · through {}", &[&address, j]);
                full.strip_prefix(address.as_str())
                    .map(str::to_string)
                    .unwrap_or_default()
            }
            None => String::new(),
        };
        let mut cancel = false;
        let mut retry = false;
        let mut edit = false;
        let mut copy = false;
        card_area(ctx, ("progress", tab_id), area, 468.0, false, |ui| {
            let p = style::pal(ui);
            match &failure {
                Some(e) => cards::head(
                    ui,
                    0.0,
                    |ui| {
                        cards::tile(ui, icons::WARNING_OCTAGON, Tone::Err, 44.0);
                    },
                    &tf("Couldn't connect to {}", &[&name]),
                    |ui| cards::sub_text(ui, &e.message()),
                    |_| {},
                ),
                None => cards::head(
                    ui,
                    if label.is_some() { 72.0 } else { 0.0 },
                    |ui| {
                        kit::host_glyph(
                            ui,
                            40.0,
                            kit::host_icon(bookmark.host_icon()),
                            label_color,
                            None,
                            p.surface,
                        );
                    },
                    &tf("Connecting to {}", &[&name]),
                    |ui| cards::sub_address(ui, &address, &sub_rest),
                    |ui| {
                        if let (Some(l), Some(c)) = (&label, label_color)
                            && !l.tag().is_empty()
                        {
                            kit::env_tag(ui, &l.tag(), c, true);
                        }
                    },
                ),
            }
            cards::body(ui, |ui| {
                if let Some(stops) = &stops {
                    cards::route(ui, stops, |i| match failed {
                        Some(f) if i < f => cards::WireState::Done,
                        Some(_) => cards::WireState::Pending,
                        None if i < now_step => cards::WireState::Done,
                        None if i == now_step => cards::WireState::Now,
                        None => cards::WireState::Pending,
                    });
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut prev = None;
                    for (i, label) in labels.iter().enumerate() {
                        let st = match failed {
                            Some(f) if i < f => StepState::Done,
                            Some(f) if i == f => StepState::Failed,
                            Some(_) => StepState::Pending,
                            None if i < now_step => StepState::Done,
                            None if i == now_step => StepState::Now,
                            None => StepState::Pending,
                        };
                        let start = marks.iter().find(|m| m.0 == i).map(|m| m.1);
                        let meta = match st {
                            StepState::Done => start
                                .zip(marks.iter().find(|m| m.0 > i).map(|m| m.1))
                                .map(|(a, b)| ms(b - a)),
                            StepState::Failed => start.zip(failed_at).map(|(a, b)| ms(b - a)),
                            _ => None,
                        };
                        // The port of the server, beside the step that failed to reach it.
                        let note = (st == StepState::Failed && i == jumps.len())
                            .then(|| tf("port {}", &[&bookmark.port.to_string()]));
                        cards::step(ui, st, label, note.as_deref(), meta.as_deref(), prev);
                        prev = Some(st);
                    }
                });
                if !checks.is_empty() && cards::checklist(ui, &checks) {
                    edit = true;
                }
            });
            cards::foot(
                ui,
                |ui| {
                    if failure.is_some() {
                        if kit::ghost(ui, Some(icons::COPY), t("Copy Error")).clicked() {
                            copy = true;
                        }
                    } else {
                        kit::kbd(ui, "esc");
                        cards::hint(ui, t("to cancel"));
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
                            false,
                            false,
                        )
                        .clicked()
                        {
                            retry = true;
                        }
                        if kit::secondary(ui, None, t("Edit Connection…")).clicked() {
                            edit = true;
                        }
                    } else if cards::ghost_sm(ui, None, t("Edit Connection…")).clicked() {
                        edit = true;
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
        if copy && let Some(e) = &failure {
            ctx.copy_text(e.message());
        }
    }

    /// The bar over a session whose connection is gone: what happened, Reconnect, Close Tab.
    fn lost_bar(&mut self, ctx: &egui::Context, area: egui::Rect, error: Option<&ConnError>) {
        let tab = &self.tabs[self.active];
        let tab_id = tab.id;
        let name = tab.bookmark.display_name();
        let (title, icon, tone) = match error {
            Some(ConnError::NotResponding(ProbeReason::NetworkChanged)) => {
                (t("Connection lost"), icons::WIFI_SLASH, Tone::Err)
            }
            Some(_) => (t("Connection lost"), icons::PLUGS, Tone::Err),
            None => (t("Disconnected"), icons::PLUGS, Tone::Neutral),
        };
        let detail = cards::lost_cause(error, &name);
        let width = 640.0_f32.min(area.width() - 32.0).max(320.0);
        let mut reconnect = false;
        let mut close = false;
        egui::Area::new(egui::Id::new(("lost", tab_id)))
            .fixed_pos(pos2(area.center().x, area.top() + 14.0))
            .pivot(Align2::CENTER_TOP)
            .order(egui::Order::Middle)
            .constrain_to(area)
            .show(ctx, |ui| {
                let p = style::pal(ui);
                egui::Frame::NONE
                    .fill(p.raised)
                    .corner_radius(CornerRadius::same(12))
                    .shadow(egui::Shadow {
                        offset: [0, 14],
                        blur: 40,
                        spread: 0,
                        color: Color32::from_black_alpha(if p.dark { 115 } else { 36 }),
                    })
                    .stroke(Stroke::new(
                        1.0,
                        if tone == Tone::Err {
                            style::soft(p.err, 0.4)
                        } else {
                            p.border_strong
                        },
                    ))
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 10,
                        top: 10,
                        bottom: 10,
                    })
                    .show(ui, |ui| {
                        ui.set_width(width - 24.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            cards::tile(ui, icon, tone, 32.0);
                            let buttons = ui.ctx().fonts_mut(|f| {
                                f.layout_no_wrap(
                                    format!("{}{}", t("Reconnect"), t("Close Tab")),
                                    style::medium(style::SMALL),
                                    Color32::WHITE,
                                )
                                .size()
                                .x
                            }) + 90.0;
                            let text_w = (ui.available_width() - buttons - 12.0).max(120.0);
                            ui.vertical(|ui| {
                                ui.set_width(text_w);
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(
                                    RichText::new(title)
                                        .font(style::semibold(style::BODY))
                                        .color(p.text),
                                );
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&detail)
                                            .font(style::regular(style::SMALL))
                                            .color(p.text_faint),
                                    )
                                    .truncate(),
                                )
                                .on_hover_text(&detail);
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    if kit::button_kbd(
                                        ui,
                                        kit::Kind::Primary,
                                        None,
                                        t("Reconnect"),
                                        Some(&S::Reconnect.label()),
                                        true,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        reconnect = true;
                                    }
                                    if cards::ghost_sm(ui, None, t("Close Tab")).clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                    });
            });
        if reconnect {
            self.actions.push(Action::Reconnect);
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
        let (_, jumps) = step_labels(&self.store, &bookmark);
        let via = self.tabs[active].conn.shared.lock().via.clone();
        let mut answer: Option<Option<SecretAnswer>> = None;
        let mut hostkey_answer: Option<bool> = None;
        let mut close_tab = false;
        let tab = &mut self.tabs[active];
        match &view {
            PromptView::HostKey {
                host,
                port,
                algorithm,
                fingerprint,
                changed: false,
                ..
            } => {
                let name = via.clone().unwrap_or_else(|| bookmark.display_name());
                card_area(ctx, ("hostkey", tab_id), area, 560.0, false, |ui| {
                    let p = style::pal(ui);
                    cards::head(
                        ui,
                        0.0,
                        |ui| {
                            cards::tile(ui, icons::FINGERPRINT, Tone::Accent, 44.0);
                        },
                        t("Verify host key"),
                        |ui| {
                            cards::sub_text(
                                ui,
                                &format!(
                                    "{}. {}",
                                    tf("First time connecting to {}", &[&name]),
                                    t("Check the fingerprint before trusting this server.")
                                ),
                            )
                        },
                        |_| {},
                    );
                    cards::body(ui, |ui| {
                        let art_w = 122.0;
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 12.0;
                            let left_w = ui.available_width() - art_w - 12.0;
                            ui.vertical(|ui| {
                                ui.set_width(left_w);
                                ui.spacing_mut().item_spacing.y = 10.0;
                                kv_rows(
                                    ui,
                                    &[
                                        (t("Host"), format!("{host}:{port}"), true),
                                        (t("Key type"), key_type(algorithm), false),
                                    ],
                                );
                                cards::fingerprint(ui, fingerprint, cards::Fp::Plain);
                                if kit::secondary_sm(ui, Some(icons::COPY), t("Copy fingerprint"))
                                    .clicked()
                                {
                                    ui.ctx().copy_text(fingerprint.clone());
                                }
                            });
                            cards::randomart_box(ui, fingerprint, &cards::art_title(algorithm));
                        });
                        compare_callout(ui, &cards::compare_command(algorithm), &p);
                    });
                    cards::foot(
                        ui,
                        |ui| {
                            kit::icon(ui, icons::INFO, 14.0, p.text_faint);
                            cards::hint(ui, t("Saved to TernaryShell's known_hosts"));
                        },
                        |ui| {
                            // No Enter shortcut: a key meant for the terminal must never trust a
                            // host key. The decision takes a click.
                            if kit::button_ex(
                                ui,
                                kit::Kind::Primary,
                                Some(icons::SHIELD_CHECK),
                                t("Trust and Connect"),
                                false,
                            )
                            .clicked()
                            {
                                hostkey_answer = Some(true);
                            }
                            if kit::button_kbd(
                                ui,
                                kit::Kind::Secondary,
                                None,
                                t("Cancel"),
                                Some("esc"),
                                false,
                                false,
                            )
                            .clicked()
                            {
                                hostkey_answer = Some(false);
                            }
                        },
                    );
                });
            }
            PromptView::HostKey {
                algorithm,
                fingerprint,
                known,
                ..
            } => {
                let name = via.clone().unwrap_or_else(|| bookmark.display_name());
                let checked_id = egui::Id::new(("hostkey-checked", tab_id, fingerprint.as_str()));
                let mut checked: bool = ctx.data(|d| d.get_temp(checked_id)).unwrap_or(false);
                let saved: Vec<(String, String)> = known
                    .iter()
                    .map(|k| match k.split_once(char::is_whitespace) {
                        Some((alg, fp)) => (key_type(alg), fp.trim().to_string()),
                        None => (String::new(), k.clone()),
                    })
                    .collect();
                let new_type = key_type(algorithm);
                card_area(ctx, ("hostkey", tab_id), area, 560.0, true, |ui| {
                    let p = style::pal(ui);
                    cards::head(
                        ui,
                        0.0,
                        |ui| {
                            cards::tile(ui, icons::SHIELD_WARNING, Tone::Err, 44.0);
                        },
                        &tf("The host key of {} has changed", &[&name]),
                        |ui| {
                            cards::sub_text(
                                ui,
                                t(
                                    "Someone may be intercepting this connection. TernaryShell did not log in.",
                                ),
                            )
                        },
                        |_| {},
                    );
                    cards::body(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 8.0;
                            for (kind, fp) in &saved {
                                key_heading(ui, t("Saved"), kind, p.text_faint);
                                cards::fingerprint(ui, fp, cards::Fp::Old);
                            }
                            ui.add_space(4.0);
                            let different = saved.iter().all(|(k, _)| *k != new_type);
                            let what = if different && !saved.is_empty() {
                                tf("{} · different type", &[&new_type])
                            } else {
                                new_type.clone()
                            };
                            key_heading(ui, t("Now"), &what, p.err);
                            cards::fingerprint(ui, fingerprint, cards::Fp::Danger);
                        });
                        kit::callout(
                            ui,
                            icons::INFO,
                            Tone::Neutral,
                            t(
                                "Expected after a server rebuild or an OS reinstall. If you didn't expect it, close the tab and ask the server's admin.",
                            ),
                        );
                        if style::checkbox(
                            ui,
                            &mut checked,
                            RichText::new(t("I checked the new key with the server's admin"))
                                .font(style::regular(12.5)),
                        )
                        .changed()
                        {
                            ui.data_mut(|d| d.insert_temp(checked_id, checked));
                        }
                    });
                    cards::foot(
                        ui,
                        |ui| {
                            if cards::ghost_sm(ui, Some(icons::COPY), t("Copy fingerprint"))
                                .clicked()
                            {
                                ui.ctx().copy_text(fingerprint.clone());
                            }
                        },
                        |ui| {
                            if kit::button_kbd(
                                ui,
                                kit::Kind::Primary,
                                None,
                                t("Close Tab"),
                                Some("esc"),
                                false,
                                false,
                            )
                            .clicked()
                            {
                                hostkey_answer = Some(false);
                                close_tab = true;
                            }
                            let accept = ui
                                .add_enabled_ui(checked, |ui| {
                                    kit::button_ex(
                                        ui,
                                        kit::Kind::DangerQuiet,
                                        None,
                                        t("Accept New Key"),
                                        false,
                                    )
                                })
                                .inner;
                            if accept.clicked() && checked {
                                hostkey_answer = Some(true);
                            }
                        },
                    );
                });
            }
            PromptView::Secret {
                kind,
                message,
                echo,
                can_save,
                retry,
            } => {
                let through = match &via {
                    Some(v) => Some(v.clone()),
                    None => jumps.last().cloned(),
                };
                let (icon, title, sub_mono, sub_rest, go) = match kind {
                    SecretPromptKind::Password => {
                        let (user, host) = message
                            .rsplit_once('@')
                            .map(|(u, h)| (u.to_string(), h.to_string()))
                            .unwrap_or_else(|| (message.clone(), bookmark.host.clone()));
                        let rest = match &through {
                            Some(j) => {
                                let full = tf("{} · through {}", &[&host, j]);
                                full.strip_prefix(host.as_str())
                                    .map(str::to_string)
                                    .unwrap_or_default()
                            }
                            None => String::new(),
                        };
                        (
                            icons::PASSWORD,
                            tf("Password for {}", &[&user]),
                            host,
                            rest,
                            t("Connect"),
                        )
                    }
                    SecretPromptKind::Passphrase => {
                        let full = tf("{} is protected by a passphrase", &[message]);
                        let (mono, rest) = match full.strip_prefix(message.as_str()) {
                            Some(rest) => (message.clone(), rest.to_string()),
                            None => (String::new(), full),
                        };
                        (
                            icons::KEY,
                            t("Key passphrase").to_string(),
                            mono,
                            rest,
                            t("Unlock"),
                        )
                    }
                    SecretPromptKind::Interactive => (
                        icons::SHIELD_CHECK,
                        t("Verification").to_string(),
                        String::new(),
                        t("The server asks").to_string(),
                        t("Send"),
                    ),
                };
                let code =
                    *kind == SecretPromptKind::Interactive && cards::is_code_prompt(message, *echo);
                let settled = tab
                    .prompt
                    .shown_at
                    .is_some_and(|t| t.elapsed() >= Duration::from_millis(300));
                let enter = ctx.input(|i| {
                    i.events.iter().any(|e| matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, repeat: false, modifiers, .. } if modifiers.is_none()))
                });
                let prompt = &mut tab.prompt;
                card_area(ctx, ("secret", tab_id), area, 360.0, false, |ui| {
                    let p = style::pal(ui);
                    cards::head(
                        ui,
                        0.0,
                        |ui| {
                            cards::tile(ui, icon, Tone::Accent, 32.0);
                        },
                        &title,
                        |ui| {
                            if sub_mono.is_empty() {
                                cards::sub_text(ui, &sub_rest);
                            } else {
                                cards::sub_address(ui, &sub_mono, &sub_rest);
                            }
                        },
                        |_| {},
                    );
                    cards::body(ui, |ui| {
                        if *kind == SecretPromptKind::Interactive {
                            egui::Frame::NONE
                                .fill(p.well)
                                .stroke(Stroke::new(1.0, p.border))
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(egui::Margin::symmetric(10, 8))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(message.as_str())
                                                .font(style::mono(12.0))
                                                .color(p.text),
                                        )
                                        .wrap(),
                                    );
                                });
                        }
                        let r = if code {
                            cards::code_boxes(
                                ui,
                                &mut prompt.text,
                                egui::Id::new(("prompt-field", tab_id)),
                            )
                        } else {
                            secret_field(
                                ui,
                                &mut prompt.text,
                                *echo,
                                *retry,
                                *kind == SecretPromptKind::Interactive,
                                ("prompt-field", tab_id),
                                match kind {
                                    SecretPromptKind::Passphrase => t("Passphrase"),
                                    SecretPromptKind::Password => t("Password"),
                                    SecretPromptKind::Interactive => "",
                                },
                            )
                            .0
                        };
                        if code {
                            style::hint(ui, t("Sent automatically after the 6th digit."));
                        }
                        if *retry {
                            ui.add_space(-4.0);
                            ui.spacing_mut().interact_size.y = 16.0;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                kit::icon(ui, icons::WARNING_CIRCLE, 13.0, p.err);
                                style::hint_tone(
                                    ui,
                                    if *kind == SecretPromptKind::Passphrase {
                                        t("Wrong passphrase, try again.")
                                    } else {
                                        t("Wrong password, try again.")
                                    },
                                    Tone::Err,
                                );
                            });
                        }
                        // A one-time code is never worth saving.
                        if *can_save && !code {
                            let label = if *kind == SecretPromptKind::Passphrase {
                                tf("Save passphrase in {}", &[crate::platform::keychain()])
                            } else {
                                tf("Save in {}", &[crate::platform::keychain()])
                            };
                            if style::checkbox(
                                ui,
                                &mut prompt.save,
                                RichText::new(label).font(style::regular(12.5)),
                            )
                            .changed()
                            {
                                r.request_focus();
                            }
                        }
                        let nothing_focused = ui.memory(|m| m.focused().is_none());
                        let complete = code && prompt.text.len() == cards::CODE_LEN;
                        let submit = settled
                            && ((enter && (r.lost_focus() || r.has_focus() || nothing_focused))
                                || (complete && r.changed()));
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
                    cards::foot(
                        ui,
                        |ui| {
                            kit::kbd(ui, "esc");
                        },
                        |ui| {
                            if kit::button_kbd(
                                ui,
                                kit::Kind::Primary,
                                None,
                                go,
                                Some("↩"),
                                false,
                                false,
                            )
                            .clicked()
                            {
                                answer = Some(Some(SecretAnswer {
                                    value: std::mem::take(&mut prompt.text),
                                    save: prompt.save,
                                }));
                            }
                        },
                    );
                });
            }
        }
        // Only Esc dismisses (a stray click beside the card must not cancel the login). On a
        // changed host key Esc is "Close Tab".
        let escape = on_top && ctx.input(|i| i.key_pressed(Key::Escape));
        let tab = &mut self.tabs[active];
        if escape && tab.prompt.armed && answer.is_none() && hostkey_answer.is_none() {
            match view {
                PromptView::HostKey { changed, .. } => {
                    hostkey_answer = Some(false);
                    close_tab = changed;
                }
                PromptView::Secret { .. } => answer = Some(None),
            }
        }
        if answer.is_some() || hostkey_answer.is_some() {
            tab.answer_prompt(hostkey_answer.unwrap_or(false), answer.flatten());
            self.focus_target = None;
            // The Enter that answered must not repeat into the terminal.
            self.swallow_repeats = true;
            ctx.request_repaint();
            if close_tab {
                self.actions.push(Action::CloseTab(tab_id));
            }
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
    danger: bool,
    add: impl FnOnce(&mut Ui),
) {
    let width = width.min(area.width() - 32.0).max(280.0);
    egui::Area::new(egui::Id::new(id))
        .fixed_pos(area.center())
        .pivot(Align2::CENTER_CENTER)
        .order(egui::Order::Middle)
        .constrain_to(area.shrink(8.0))
        .show(ctx, |ui| {
            let p = style::pal(ui);
            cards::frame(&p, danger).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                add(ui);
            });
        });
}

/// Label / value rows (12 px; the labels in a 70 px column, faint).
fn kv_rows(ui: &mut Ui, rows: &[(&str, String, bool)]) {
    let p = style::pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 7.0;
        for (k, v, mono) in rows {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
            ui.painter().text(
                pos2(r.left(), r.center().y),
                Align2::LEFT_CENTER,
                *k,
                style::regular(style::SMALL),
                p.text_faint,
            );
            ui.painter().text(
                pos2(r.left() + 80.0, r.center().y),
                Align2::LEFT_CENTER,
                v,
                if *mono {
                    style::mono(11.5)
                } else {
                    style::regular(style::SMALL)
                },
                p.text,
            );
        }
    });
}

/// "SAVED  ED25519 · trusted…": an overline (in `color`) and a faint note.
fn key_heading(ui: &mut Ui, overline: &str, note: &str, color: Color32) {
    let p = style::pal(ui);
    ui.spacing_mut().interact_size.y = 17.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let (r, _) = ui.allocate_exact_size(vec2(60.0, 16.0), Sense::hover());
        let g =
            ui.painter()
                .layout_no_wrap(overline.to_uppercase(), style::semibold(style::XS), color);
        ui.painter()
            .galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, color);
        ui.label(
            RichText::new(note)
                .font(style::regular(style::SMALL))
                .color(p.text_faint),
        );
    });
}

/// "Compare on the server: ssh-keygen -lf …" with a copy button, on the accent wash.
fn compare_callout(ui: &mut Ui, command: &str, p: &style::Palette) {
    egui::Frame::NONE
        .fill(p.accent_softer)
        .corner_radius(CornerRadius::same(style::RADIUS_MD))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().interact_size.y = 20.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                kit::icon(ui, icons::TERMINAL_WINDOW, 16.0, p.accent_text);
                let mut job = egui::text::LayoutJob::default();
                job.append(
                    t("Compare on the server:"),
                    0.0,
                    egui::TextFormat {
                        font_id: style::regular(style::SMALL),
                        color: p.text_muted,
                        valign: egui::Align::Center,
                        ..Default::default()
                    },
                );
                job.append(
                    command,
                    6.0,
                    egui::TextFormat {
                        font_id: style::mono(11.5),
                        color: p.text,
                        valign: egui::Align::Center,
                        ..Default::default()
                    },
                );
                let w = ui.available_width() - 30.0;
                ui.allocate_ui(vec2(w, 20.0), |ui| {
                    ui.set_width(w);
                    ui.add(egui::Label::new(job).wrap());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::icon_button_ex(ui, icons::COPY, t("Copy"), false, 20.0).clicked() {
                        ui.ctx().copy_text(command.to_string());
                    }
                });
            });
        });
}

/// The secret field of a prompt with a show/hide eye (passwords); returns its response.
fn secret_field(
    ui: &mut Ui,
    text: &mut String,
    echo: bool,
    error: bool,
    mono: bool,
    id: impl std::hash::Hash + std::fmt::Debug,
    hint: &str,
) -> (egui::Response, bool) {
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
    if !hint.is_empty() {
        field = field.hint_text(hint);
    }
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
        if resp.clicked() {
            shown = !shown;
            toggled = true;
            ui.data_mut(|d| d.insert_temp(shown_id, shown));
            r.request_focus();
        }
    }
    (r, toggled)
}

/// What a pane whose shell ended says, in a card under its last line: "Session ended", why,
/// and what ↩, ⌘R and ⌘W do there. Drawn only (the keys do the work), clipped to the pane.
fn session_ended_hint(ui: &Ui, pane: egui::Rect, at: egui::Rect, headline: &str) {
    let p = style::pal(ui);
    let painter = ui.painter().with_clip_rect(pane.shrink(2.0));
    // "exit code 1" or "signal 9" when the headline names one (in any language: the number is
    // the only digits in it).
    let number: String = headline.chars().filter(|c| c.is_ascii_digit()).collect();
    let mut detail = t("the server ended the session").to_string();
    if !number.is_empty() {
        let signal = tf("Session ended by signal {}.", &[&number]) == headline;
        detail.push_str(" · ");
        detail.push_str(&if signal {
            tf("signal {}", &[&number])
        } else {
            tf("exit code {}", &[&number])
        });
    }
    let left = pane.left() + 10.0;
    let right = (pane.right() - 10.0).min(left + 520.0);
    // Under the last line, or pinned to the bottom of the pane when the output fills it.
    let top = (at.top() + 4.0).min(pane.bottom() - 84.0 - 10.0);
    let rect = egui::Rect::from_min_max(pos2(left, top), pos2(right, top + 84.0));
    painter.rect(
        rect,
        CornerRadius::same(10),
        p.surface,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    let tile_rect = egui::Rect::from_min_size(rect.min + vec2(14.0, 12.0), vec2(32.0, 32.0));
    cards::paint_tile(&painter, tile_rect, icons::POWER, Tone::Neutral, &p);
    let x = tile_rect.right() + 10.0;
    painter.text(
        pos2(x, tile_rect.top() + 8.0),
        Align2::LEFT_CENTER,
        t("Session ended"),
        style::semibold(style::BODY),
        p.text,
    );
    painter.text(
        pos2(x, tile_rect.bottom() - 7.0),
        Align2::LEFT_CENTER,
        detail,
        style::regular(style::SMALL),
        p.text_faint,
    );
    let y = tile_rect.bottom() + 10.0 + 9.0;
    let mut x = rect.left() + 14.0;
    let close = S::Close.label();
    let reconnect = S::Reconnect.label();
    for (key, what) in [
        ("↩", t("new shell")),
        (reconnect.as_str(), t("reconnect tab")),
        (close.as_str(), t("close pane")),
    ] {
        x += kit::paint_kbd_at(&painter, pos2(x, y), key, &p) + 6.0;
        let g =
            painter.layout_no_wrap(what.to_string(), style::regular(style::SMALL), p.text_faint);
        let w = g.size().x;
        painter.galley(pos2(x, y - g.size().y / 2.0), g, p.text_faint);
        x += w + 14.0;
    }
}
