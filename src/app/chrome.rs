//! The window frame: the sidebar's top row, the title row (tabs and the session toolbar), the
//! canvas card with its status strip, and the Start and Welcome pages shown with no tab open.

use std::collections::HashMap;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use uuid::Uuid;

use super::{TITLE_H, TernaryShellApp};
use crate::bookmarks::transfer::ExportScope;
use crate::i18n::{t, tf};
use crate::settings::ThemeMode;
use crate::ssh::types::ConnState;
use crate::ui::actions::Action;
use crate::ui::keys::Shortcut as S;
use crate::ui::style::{self, Tone};
use crate::ui::tabs::Tab;
use crate::ui::{icons, kit};

/// Room left of the title row's first control for the traffic lights (macOS).
pub const TRAFFIC_W: f32 = 78.0;
/// Space around the canvas card.
pub const GUTTER: f32 = 8.0;
/// Corner radius of the canvas card.
pub const CANVAS_R: u8 = 10;

/// What a tab's status dot says.
pub fn tab_status(tab: &Tab, p: &style::Palette) -> (Color32, &'static str, bool) {
    match tab.state() {
        ConnState::Connected if tab.panes.iter().any(|p| p.is_live()) => {
            (p.ok, t("Connected"), true)
        }
        ConnState::Connected => (p.text_faint, t("Session ended"), false),
        ConnState::Connecting | ConnState::Authenticating => (p.warn, t("Connecting…"), true),
        ConnState::Closed(Some(_)) => (p.err, t("Disconnected"), true),
        ConnState::Closed(None) => (p.text_faint, t("Disconnected"), false),
    }
}

/// "2 min ago", "Yesterday", "Mon", "Oct 30".
pub fn ago(at: i64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let now = chrono::Utc::now().timestamp();
    let secs = (now - at).max(0);
    if secs < 60 {
        return t("Just now").to_string();
    }
    if secs < 3600 {
        return tf("{} min ago", &[&(secs / 60).to_string()]);
    }
    let Some(then) = Local.timestamp_opt(at, 0).single() else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let day = then.date_naive();
    if day == today {
        return tf("{} h ago", &[&(secs / 3600).to_string()]);
    }
    let days = (today - day).num_days();
    if days == 1 {
        return t("Yesterday").to_string();
    }
    if days < 7 {
        let names = [
            t("Mon"),
            t("Tue"),
            t("Wed"),
            t("Thu"),
            t("Fri"),
            t("Sat"),
            t("Sun"),
        ];
        return names[then.weekday().num_days_from_monday() as usize].to_string();
    }
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
    format!("{} {}", months[then.month0() as usize], then.day())
}

impl TernaryShellApp {
    // ----- sidebar ------------------------------------------------------------------------

    /// The sidebar: its top row (the traffic lights' room and the hide button on macOS, the ☰
    /// menu elsewhere), then the filter, the tree and the footer.
    pub(super) fn sidebar_panel(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let (top, top_resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), TITLE_H), Sense::click_and_drag());
        self.window_drag(ui, &top_resp);
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(top.shrink2(vec2(0.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 0.0;
        if cfg!(target_os = "macos") {
            row.add_space(if self.fullscreen { 10.0 } else { TRAFFIC_W });
        } else {
            // Windows and Linux: the app's icon and name lead the row (☰ sits with the
            // session tools, before the window's own buttons).
            row.add_space(12.0);
            let (logo, _) = row.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            icons::paint_logo(row.painter(), logo);
            row.add_space(6.0);
            row.label(
                RichText::new("TernaryShell")
                    .font(style::semibold(style::SMALL))
                    .color(p.text),
            );
        }
        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            let hide = kit::icon_button_ex(
                ui,
                icons::SIDEBAR_SIMPLE,
                &format!("{} ({})", t("Hide Sidebar"), S::Sidebar.label()),
                false,
                24.0,
            );
            if hide.clicked() {
                self.actions.push(Action::ToggleSidebar);
            }
        });
        let live = self.live_map();
        let mut open = HashMap::<Uuid, usize>::new();
        for tab in &self.tabs {
            *open.entry(tab.bookmark.id).or_default() += 1;
        }
        let input_enabled = !self.modal_open();
        let (actions, moved) = self
            .sidebar
            .show(ui, &self.store, &live, &open, input_enabled);
        self.actions.extend(actions);
        if let Some(mv) = moved {
            self.apply_move(mv);
        }
    }

    /// Dragging an empty part of the title row moves the window; a double click zooms it, as
    /// the title bar does (macOS: the content runs under the title bar).
    pub(super) fn window_drag(&mut self, ui: &Ui, resp: &egui::Response) {
        if !cfg!(target_os = "macos") {
            return;
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if resp.double_clicked() {
            let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
    }

    // ----- title row ----------------------------------------------------------------------

    /// The row above the canvas: the tabs, then the session toolbar on the right.
    pub(super) fn title_row(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let bar = ui.max_rect();
        let bg = ui.interact(bar, egui::Id::new("title-row-bg"), Sense::click_and_drag());
        self.window_drag(ui, &bg);

        // Leading controls when the sidebar is hidden: the traffic lights' room, the menu
        // (Windows, Linux) and the button that shows the sidebar.
        let mut lead = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        lead.spacing_mut().item_spacing.x = 0.0;
        if self.settings.sidebar_visible {
            lead.add_space(6.0);
        } else {
            // macOS: room for the traffic lights (none in full screen).
            lead.add_space(if cfg!(target_os = "macos") && !self.fullscreen {
                90.0
            } else {
                10.0
            });
            if kit::icon_button_ex(
                &mut lead,
                icons::SIDEBAR_SIMPLE,
                &format!("{} ({})", t("Show Sidebar"), S::Sidebar.label()),
                false,
                24.0,
            )
            .clicked()
            {
                self.actions.push(Action::ToggleSidebar);
            }
            lead.add_space(4.0);
        }
        let tabs_left = lead.min_rect().right().max(bar.left());

        // The toolbar on the right first, so the tabs know how much room is left.
        let mut tools = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(tabs_left, bar.top()),
                    pos2(bar.right() - 10.0, bar.bottom()),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        self.session_tools(&mut tools);
        let tools_left = tools.min_rect().left().min(bar.right() - 10.0);
        let tabs_rect = egui::Rect::from_min_max(
            pos2(tabs_left, bar.top()),
            pos2(tools_left - 8.0, bar.bottom()),
        );
        self.tab_strip(ui, tabs_rect, &p);
    }

    fn tab_strip(&mut self, ui: &mut Ui, area: egui::Rect, p: &style::Palette) {
        if self.tabs.is_empty() {
            return;
        }
        let dark = ui.visuals().dark_mode;
        let mut drag_target = None;
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let mut rects = Vec::with_capacity(self.tabs.len());
        // While the tabs overflow (known from the last frame) the ⌄ +N button gets its own
        // room after them instead of covering the last one.
        let overflow_id = egui::Id::new("tab-strip-overflow");
        let was_overflowing: bool = ui.data(|d| d.get_temp(overflow_id)).unwrap_or(false);
        let strip_w = if was_overflowing {
            area.width() - 32.0
        } else {
            area.width()
        };
        let out = egui::ScrollArea::horizontal()
            .id_salt("tab-strip")
            .max_width(strip_w)
            .max_height(TITLE_H)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut child, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for i in 0..self.tabs.len() {
                        let (rect, drop) = self.tab_pill(ui, i, p, dark);
                        rects.push(rect);
                        if let Some(t) = drop {
                            drag_target = Some(t);
                        }
                    }
                    if tab_add_button(
                        ui,
                        &format!("{} ({})", t("New Tab"), S::QuickConnect.label()),
                    )
                    .clicked()
                    {
                        self.actions.push(Action::QuickConnect);
                    }
                });
            });
        // Tabs that don't fit scroll: the edge that hides more fades out, and a ⌄ +N button
        // lists every tab.
        let (view, offset) = (out.inner_rect, out.state.offset.x);
        if offset > 0.5 {
            kit::fade(
                ui.painter(),
                egui::Rect::from_min_size(view.left_top(), vec2(36.0, view.height())),
                p.chrome,
                false,
            );
        }
        let overflow = out.content_size.x - view.width() - offset > 0.5;
        let overflowing = overflow || offset > 0.5;
        if overflowing != was_overflowing {
            ui.data_mut(|d| d.insert_temp(overflow_id, overflowing));
            ui.ctx().request_repaint();
        }
        if overflowing {
            let list = egui::Rect::from_center_size(
                pos2(view.right() + 6.0 + 13.0, view.center().y),
                vec2(26.0, 26.0),
            );
            let hidden = rects
                .iter()
                .filter(|r| r.right() > view.right() + 0.5 || r.left() < view.left() - 0.5)
                .count();
            if overflow {
                kit::fade(
                    ui.painter(),
                    egui::Rect::from_min_size(
                        pos2(view.right() - 48.0, view.top()),
                        vec2(48.0, view.height()),
                    ),
                    p.chrome,
                    true,
                );
            }
            let resp = ui.interact(list, egui::Id::new("tab-list"), Sense::click());
            let painter = ui.painter();
            painter.rect_filled(
                list,
                CornerRadius::same(6),
                if resp.hovered() { p.subtle } else { p.hover },
            );
            painter.text(
                list.center() + vec2(-1.0, 1.0),
                Align2::CENTER_CENTER,
                icons::CARET_DOWN,
                style::icon_font(14.0),
                if resp.hovered() { p.text } else { p.text_muted },
            );
            if hidden > 0 {
                painter.text(
                    pos2(list.right() - 1.0, list.top() + 2.0),
                    Align2::RIGHT_TOP,
                    format!("+{hidden}"),
                    style::semibold(9.0),
                    p.text_muted,
                );
            }
            #[cfg(debug_assertions)]
            crate::devtest::record("tab-list", list);
            let resp = resp.on_hover_text(t("All Tabs"));
            egui::Popup::menu(&resp).show(|ui| {
                kit::menu_body(ui);
                ui.set_min_width(240.0);
                for (i, tab) in self.tabs.iter().enumerate() {
                    let (dot, _, _) = tab_status(tab, p);
                    let r = kit::menu_item(ui, None, &tab.title(), None);
                    ui.painter().circle_filled(
                        pos2(r.rect.left() + 16.0, r.rect.center().y),
                        3.5,
                        dot,
                    );
                    if r.clicked() {
                        self.actions.push(Action::SelectTab(i));
                        ui.close();
                    }
                }
            });
        }
        if let Some((from, to)) = drag_target {
            self.actions.push(Action::MoveTab { from, to });
        }
    }

    /// One tab: a 30 px pill (the active one raised on the canvas colour with a hairline ring),
    /// its state on the left, the title, × (or the key of a waiting prompt) on the right, and a
    /// 2 px line in its environment label's colour along the bottom. Returns its rect and a
    /// drop.
    fn tab_pill(
        &mut self,
        ui: &mut Ui,
        i: usize,
        p: &style::Palette,
        dark: bool,
    ) -> (egui::Rect, Option<(u64, u64)>) {
        let active = i == self.active;
        let tab = &self.tabs[i];
        let tab_id = tab.id;
        let (dot, _, _) = tab_status(tab, p);
        let waiting = tab.conn.shared.lock().prompt.is_some();
        let activity = !active && tab.has_activity();
        let connecting = matches!(
            tab.state(),
            ConnState::Connecting | ConnState::Authenticating
        ) && !waiting;
        let progress = tab.transfer_progress();
        let title = tab.title();
        let saved = self
            .store
            .bookmark(tab.bookmark.id)
            .unwrap_or(&tab.bookmark);
        let label = self.store.label_of(saved).cloned();
        let label_color = label.as_ref().map(|l| style::label_color(l.color, dark));
        let galley = ui.painter().layout_no_wrap(
            title.clone(),
            style::medium(style::BODY),
            Color32::PLACEHOLDER,
        );
        let lead_w = if progress.is_some() {
            14.0
        } else if connecting {
            11.0
        } else {
            7.0
        };
        let width =
            (10.0 + lead_w + 7.0 + galley.size().x + 7.0 + 16.0 + 8.0).clamp(TAB_MIN_W, 200.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click_and_drag());
        // A tab that just became active scrolls into view.
        if active {
            let shown_id = egui::Id::new("tab-strip-shown");
            if ui.data(|d| d.get_temp::<u64>(shown_id)) != Some(tab_id) {
                ui.data_mut(|d| d.insert_temp(shown_id, tab_id));
                ui.scroll_to_rect(rect.expand2(vec2(8.0, 0.0)), None);
            }
        }
        let hovered = ui.rect_contains_pointer(rect);
        let painter = ui.painter();
        let radius = CornerRadius::same(style::RADIUS_MD);
        if active {
            painter.rect_filled(
                rect.translate(vec2(0.0, 1.0)).expand(0.5),
                radius,
                Color32::from_black_alpha(if dark { 64 } else { 14 }),
            );
            painter.rect_filled(rect, radius, p.tab_on);
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, p.border),
                StrokeKind::Outside,
            );
        } else if hovered {
            painter.rect_filled(rect, radius, p.hover);
        }
        let cy = rect.center().y;
        let lead_c = pos2(rect.left() + 10.0 + lead_w / 2.0, cy);
        if let Some(pr) = &progress {
            style::progress_ring(painter, lead_c, 7.0, pr.fraction(), dark);
        } else if connecting {
            kit::paint_spinner(ui, lead_c, 5.5);
        } else {
            painter.circle_filled(lead_c, 3.5, dot);
        }
        let text_color = if active || hovered || activity {
            p.text
        } else {
            p.text_muted
        };
        let text_x = rect.left() + 10.0 + lead_w + 7.0;
        let clip = egui::Rect::from_min_max(
            pos2(text_x, rect.top()),
            pos2(rect.right() - 8.0 - 16.0 - 4.0, rect.bottom()),
        );
        painter.with_clip_rect(clip).galley(
            pos2(text_x, cy - galley.size().y / 2.0 - 1.0),
            galley,
            text_color,
        );
        if let Some(c) = label_color {
            let line = egui::Rect::from_min_max(
                pos2(rect.left() + 10.0, rect.bottom() - 2.0),
                pos2(rect.right() - 10.0, rect.bottom()),
            );
            painter.rect_filled(
                line,
                CornerRadius {
                    nw: 2,
                    ne: 2,
                    sw: 0,
                    se: 0,
                },
                if active { c } else { c.gamma_multiply(0.55) },
            );
        }
        // × on the active or hovered tab; the waiting key instead while a prompt waits.
        let close_rect =
            egui::Rect::from_center_size(pos2(rect.right() - 8.0 - 8.0, cy), vec2(16.0, 16.0));
        let close = ui.interact(
            close_rect,
            egui::Id::new(("tab-close", tab_id)),
            Sense::click(),
        );
        let painter = ui.painter();
        if waiting && !close.hovered() {
            painter.text(
                close_rect.center(),
                Align2::CENTER_CENTER,
                icons::KEY,
                style::icon_font(12.0),
                p.accent_text,
            );
        } else if hovered || active {
            if close.hovered() {
                painter.rect_filled(close_rect, CornerRadius::same(4), p.hover);
            }
            painter.text(
                close_rect.center(),
                Align2::CENTER_CENTER,
                icons::X,
                style::icon_font(11.0),
                if close.hovered() {
                    p.text
                } else {
                    p.text_faint
                },
            );
        } else if activity {
            painter.circle_filled(pos2(rect.right() - 16.0, cy), 3.0, p.accent);
        }
        #[cfg(debug_assertions)]
        {
            crate::devtest::record(format!("tab:{i}"), rect);
            crate::devtest::record(format!("tab-close:{i}"), close_rect);
        }
        if close.clicked() || resp.middle_clicked() {
            self.actions.push(Action::CloseTab(tab_id));
        } else if resp.clicked() {
            self.actions.push(Action::SelectTab(i));
        }
        let mut hover = tab.bookmark.address();
        if let Some(via) = self
            .store
            .jump_chain(saved)
            .ok()
            .and_then(|c| c.last().map(|b| b.display_name()))
        {
            hover.push_str(" · ");
            hover.push_str(&tf("via {}", &[&via]));
        }
        if waiting {
            hover.push('\n');
            hover.push_str(t("Waiting for you"));
        }
        let resp = resp.on_hover_text(hover);
        resp.dnd_set_drag_payload(tab_id);
        let mut target = None;
        if let Some(from) = resp.dnd_release_payload::<u64>() {
            target = Some((*from, tab_id));
        }
        let bookmark_id = tab.bookmark.id;
        let own_label = saved.label.clone();
        let group_label = self.store.group_label(saved.group).cloned();
        let group_name = saved
            .group
            .and_then(|g| self.store.group(g))
            .map(|g| g.name.clone());
        let several = self.tabs.len() > 1;
        resp.context_menu(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(250.0);
            let mut chosen: Vec<Action> = Vec::new();
            let mut item =
                |ui: &mut Ui, icon: &str, label: &str, sc: Option<S>, actions: &[Action]| {
                    if kit::menu_item(ui, Some(icon), label, sc.map(|s| s.label()).as_deref())
                        .clicked()
                    {
                        chosen.extend_from_slice(actions);
                        ui.close();
                    }
                };
            item(
                ui,
                icons::PLUS,
                t("New Tab"),
                Some(S::NewTab),
                &[Action::SelectTab(i), Action::NewTab],
            );
            item(
                ui,
                icons::COPY,
                t("Duplicate Tab"),
                None,
                &[Action::DuplicateTab(tab_id)],
            );
            item(
                ui,
                icons::ARROWS_CLOCKWISE,
                t("Reconnect"),
                Some(S::Reconnect),
                &[Action::SelectTab(i), Action::Reconnect],
            );
            kit::menu_sep(ui);
            item(
                ui,
                icons::SQUARE_SPLIT_HORIZONTAL,
                t("Split Right"),
                Some(S::SplitRight),
                &[Action::SelectTab(i), Action::SplitRight],
            );
            item(
                ui,
                icons::SQUARE_SPLIT_VERTICAL,
                t("Split Down"),
                Some(S::SplitDown),
                &[Action::SelectTab(i), Action::SplitDown],
            );
            kit::menu_sep(ui);
            item(
                ui,
                icons::PENCIL_SIMPLE,
                t("Edit Connection…"),
                None,
                &[Action::EditBookmark(bookmark_id)],
            );
            kit::submenu(ui, Some(icons::TAG), t("Label"), |ui| {
                if let Some(label) = label_menu(
                    ui,
                    own_label.as_ref(),
                    group_label.as_ref().zip(group_name.as_deref()),
                ) {
                    chosen.push(Action::SetBookmarkLabel {
                        id: bookmark_id,
                        label,
                    });
                }
            });
            kit::menu_sep(ui);
            ui.add_enabled_ui(several, |ui| {
                if kit::menu_item(ui, Some(icons::X_SQUARE), t("Close Other Tabs"), None).clicked()
                {
                    chosen.push(Action::CloseOtherTabs(tab_id));
                    ui.close();
                }
            });
            if kit::menu_item(ui, Some(icons::X), t("Close Tab"), Some(&S::Close.label())).clicked()
            {
                chosen.push(Action::CloseTab(tab_id));
                ui.close();
            }
            self.actions.extend(chosen);
        });
        (rect, target)
    }

    /// The session toolbar, right to left: ☰ (Windows and Linux), Files, split, quick
    /// commands, and the update pill.
    fn session_tools(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.x = 2.0;
        let has_tab = !self.tabs.is_empty();
        if !cfg!(target_os = "macos") {
            self.app_menu(ui);
            if has_tab {
                toolbar_sep(ui);
            }
        }
        if has_tab {
            // The Files toggle; while files move it shows how far along they are.
            let files_on = self.tabs.get(self.active).is_some_and(|t| t.show_sftp);
            let progress = self.tabs.get(self.active).and_then(Tab::transfer_progress);
            let hover = match &progress {
                Some(pr) => format!(
                    "{}\n{}",
                    crate::ui::sftp_panel::summary_title(pr),
                    crate::ui::sftp_panel::summary_detail(pr)
                ),
                None => format!("{} ({})", t("Files Panel"), S::Sftp.label()),
            };
            let fraction = progress.as_ref().map(|pr| pr.fraction());
            let files = files_toggle(ui, files_on, fraction).on_hover_text(hover);
            #[cfg(debug_assertions)]
            crate::devtest::record("sftp-button", files.rect);
            if files.clicked() {
                self.actions.push(Action::ToggleSftp);
            }
            toolbar_sep(ui);
            let split_id = egui::Id::new("split-menu");
            let split_open = egui::Popup::is_id_open(ui.ctx(), split_id);
            let split = small_icon_button(
                ui,
                icons::SQUARE_SPLIT_HORIZONTAL,
                &format!("{} ({})", t("Split Right"), S::SplitRight.label()),
                split_open,
            );
            egui::Popup::menu(&split).id(split_id).show(|ui| {
                kit::menu_body(ui);
                ui.set_min_width(230.0);
                let items = [
                    (
                        icons::SQUARE_SPLIT_HORIZONTAL,
                        t("Split Right"),
                        S::SplitRight,
                        Action::SplitRight,
                    ),
                    (
                        icons::SQUARE_SPLIT_VERTICAL,
                        t("Split Down"),
                        S::SplitDown,
                        Action::SplitDown,
                    ),
                ];
                for (icon, label, sc, action) in items {
                    if kit::menu_item(ui, Some(icon), label, Some(&sc.label())).clicked() {
                        self.actions.push(action);
                        ui.close();
                    }
                }
                kit::menu_sep(ui);
                let items = [
                    (
                        icons::ARROW_RIGHT,
                        t("Next Pane"),
                        S::NextPane,
                        Action::NextPane,
                    ),
                    (
                        icons::ARROW_LEFT,
                        t("Previous Pane"),
                        S::PrevPane,
                        Action::PrevPane,
                    ),
                ];
                for (icon, label, sc, action) in items {
                    if kit::menu_item(ui, Some(icon), label, Some(&sc.label())).clicked() {
                        self.actions.push(action);
                        ui.close();
                    }
                }
            });
            let quick_id = egui::Id::new("quick-commands");
            let quick_open = egui::Popup::is_id_open(ui.ctx(), quick_id);
            let quick = small_icon_button(ui, icons::LIGHTNING, t("Quick Commands"), quick_open);
            #[cfg(debug_assertions)]
            crate::devtest::record("quick-button", quick.rect);
            self.quick_commands_popover(&quick, quick_id);
        }
        if self.updater.toolbar_button(ui) {
            ui.add_space(6.0);
        }
    }

    /// The ⚡ popover (340 px): a filter, then each command over a mono preview of what it
    /// sends, ▶ when it runs at once and ✎ when it is pasted first, its ⌃n key, and a legend.
    fn quick_commands_popover(&mut self, button: &egui::Response, id: egui::Id) {
        let mut chosen = None;
        let mut edit = false;
        let filter_id = egui::Id::new("quick-filter");
        let popup = egui::Popup::menu(button)
            .id(id)
            .width(340.0)
            .gap(5.0)
            .frame(kit::popover_frame_ctx(&button.ctx).inner_margin(egui::Margin::same(6)))
            .show(|ui| {
                kit::menu_body(ui);
                ui.set_width(328.0);
                let p = style::pal(ui);
                let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.allocate_ui(vec2(320.0, 28.0), |ui| {
                        let r = kit::quiet_search(
                            ui,
                            filter_id.with("field"),
                            &mut filter,
                            t("Search commands"),
                            None,
                        );
                        if ui.memory(|m| m.focused().is_none()) {
                            r.request_focus();
                        }
                    });
                });
                ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
                ui.add_space(6.0);
                let q = filter.to_lowercase();
                let list: Vec<(usize, &crate::settings::QuickCommand)> = self
                    .settings
                    .quick_commands
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        q.is_empty()
                            || c.name.to_lowercase().contains(&q)
                            || c.command.to_lowercase().contains(&q)
                    })
                    .collect();
                if list.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.add_space(8.0);
                        ui.label(RichText::new(t("No quick commands")).color(p.text_faint));
                    });
                    ui.add_space(6.0);
                }
                for (i, qc) in list {
                    if quick_row(ui, i, qc).clicked() {
                        chosen = Some(qc.id);
                        ui.close();
                    }
                }
                kit::menu_sep(ui);
                // The legend, and Edit… on the right.
                let (rect, _) = ui.allocate_exact_size(vec2(328.0, 24.0), Sense::hover());
                let painter = ui.painter();
                let mut x = rect.left() + 8.0;
                let cy = rect.center().y - 1.0;
                for (icon, text) in [
                    (icons::PLAY, t("runs at once")),
                    (icons::PENCIL_SIMPLE_LINE, t("pasted first")),
                ] {
                    painter.text(
                        pos2(x + 7.0, cy),
                        Align2::CENTER_CENTER,
                        icon,
                        style::icon_font(14.0),
                        p.text_faint,
                    );
                    x += 14.0 + 10.0;
                    let g = painter.layout_no_wrap(
                        text.to_string(),
                        style::regular(style::SMALL),
                        p.text_faint,
                    );
                    let w = g.size().x;
                    painter.galley(pos2(x, cy - g.size().y / 2.0), g, p.text_faint);
                    x += w + 18.0;
                }
                let g = painter.layout_no_wrap(
                    t("Edit…").to_string(),
                    style::regular(style::SMALL),
                    p.accent_text,
                );
                let er = egui::Rect::from_min_size(
                    pos2(rect.right() - 8.0 - g.size().x, cy - g.size().y / 2.0),
                    g.size(),
                )
                .expand(3.0);
                let resp = ui
                    .interact(er, filter_id.with("edit"), Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                ui.painter().galley(
                    er.min + vec2(3.0, 3.0),
                    g,
                    if resp.hovered() {
                        p.accent
                    } else {
                        p.accent_text
                    },
                );
                if resp.clicked() {
                    edit = true;
                    ui.close();
                }
            });
        if popup.is_none() {
            button.ctx.data_mut(|d| d.remove::<String>(filter_id));
        }
        if let Some(id) = chosen {
            self.actions.push(Action::SendQuickCommand(id));
        }
        if edit {
            self.settings_page = Some(crate::ui::dialogs::SettingsTab::QuickCommands);
            self.actions.push(Action::Settings);
        }
    }

    /// The ☰ menu (Windows and Linux): the macOS menu bar's commands, with their keys.
    pub(super) fn app_menu(&mut self, ui: &mut Ui) {
        let menu_id = egui::Id::new("app-menu");
        let open = egui::Popup::is_id_open(ui.ctx(), menu_id);
        let button = small_icon_button(ui, icons::LIST, t("Menu"), open);
        #[cfg(debug_assertions)]
        crate::devtest::record("app-menu", button.rect);
        let has_tab = !self.tabs.is_empty();
        let mut chosen = None;
        let mut theme = None;
        egui::Popup::menu(&button).id(menu_id).show(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(300.0);
            let mut item = |ui: &mut Ui,
                            icon: &str,
                            label: &str,
                            shortcut: Option<S>,
                            action: Action,
                            enabled: bool| {
                let sc = shortcut.map(|s| s.label());
                if ui
                    .add_enabled_ui(enabled, |ui| {
                        kit::menu_item(ui, Some(icon), label, sc.as_deref())
                    })
                    .inner
                    .clicked()
                {
                    chosen = Some(action);
                    ui.close();
                }
            };
            item(
                ui,
                icons::PLUS,
                t("New Connection…"),
                Some(S::NewConnection),
                Action::NewBookmark { group: None },
                true,
            );
            item(
                ui,
                icons::MAGNIFYING_GLASS,
                t("Quick Connect…"),
                Some(S::QuickConnect),
                Action::QuickConnect,
                true,
            );
            kit::menu_sep(ui);
            item(
                ui,
                icons::TRAY_ARROW_DOWN,
                t("Import Connections…"),
                None,
                Action::Import,
                true,
            );
            item(
                ui,
                icons::FILE_CODE,
                t("Import from ~/.ssh/config"),
                None,
                Action::ImportSshConfig,
                true,
            );
            item(
                ui,
                icons::EXPORT,
                t("Export Connections…"),
                Some(S::Export),
                Action::Export(ExportScope::All),
                true,
            );
            kit::menu_sep(ui);
            kit::submenu(ui, Some(icons::EYE), t("View"), |ui| {
                ui.set_min_width(260.0);
                item(
                    ui,
                    icons::SIDEBAR_SIMPLE,
                    t("Show/Hide Sidebar"),
                    Some(S::Sidebar),
                    Action::ToggleSidebar,
                    true,
                );
                item(
                    ui,
                    icons::FOLDER_SIMPLE,
                    t("Files Panel"),
                    Some(S::Sftp),
                    Action::ToggleSftp,
                    has_tab,
                );
                kit::menu_sep(ui);
                item(
                    ui,
                    icons::TEXT_AA,
                    t("Bigger Text"),
                    Some(S::Bigger),
                    Action::FontBigger,
                    true,
                );
                item(
                    ui,
                    icons::TEXT_AA,
                    t("Smaller Text"),
                    Some(S::Smaller),
                    Action::FontSmaller,
                    true,
                );
                item(
                    ui,
                    icons::ARROW_COUNTER_CLOCKWISE,
                    t("Actual Size"),
                    Some(S::ActualSize),
                    Action::FontReset,
                    true,
                );
                kit::menu_sep(ui);
                kit::submenu(ui, Some(icons::CIRCLE_HALF), t("Theme"), |ui| {
                    ui.set_min_width(200.0);
                    for (mode, label, icon) in [
                        (ThemeMode::System, t("Match System"), icons::CIRCLE_HALF),
                        (ThemeMode::Light, "Paper", icons::SUN),
                        (ThemeMode::Dark, "Graphite", icons::MOON),
                    ] {
                        if kit::menu_item(ui, Some(icon), label, None).clicked() {
                            theme = Some(mode);
                            ui.close();
                        }
                    }
                });
                item(
                    ui,
                    icons::ARROWS_OUT,
                    t("Full Screen"),
                    Some(S::FullScreen),
                    Action::ToggleFullScreen,
                    true,
                );
            });
            kit::submenu(ui, Some(icons::QUESTION), t("Help"), |ui| {
                ui.set_min_width(240.0);
                item(
                    ui,
                    icons::KEYBOARD,
                    t("Keyboard Shortcuts"),
                    None,
                    Action::Shortcuts,
                    true,
                );
                item(
                    ui,
                    icons::INFO,
                    t("About TernaryShell"),
                    None,
                    Action::About,
                    true,
                );
            });
            kit::menu_sep(ui);
            item(
                ui,
                icons::GEAR_SIX,
                t("Settings…"),
                Some(S::Settings),
                Action::Settings,
                true,
            );
            item(
                ui,
                icons::ARROW_CIRCLE_UP,
                t("Check for Updates…"),
                None,
                Action::CheckForUpdates,
                true,
            );
            kit::menu_sep(ui);
            item(
                ui,
                icons::SIGN_OUT,
                t("Quit TernaryShell"),
                Some(S::Quit),
                Action::Quit,
                true,
            );
        });
        if let Some(action) = chosen {
            self.actions.push(action);
        }
        if let Some(mode) = theme {
            self.settings.theme = mode;
        }
    }

    // ----- status bar ---------------------------------------------------------------------

    /// The bottom of the canvas card (28 px): the session's environment tag, state, address,
    /// jump host and the shell's folder on the left; root, transfers, port forwards, the round
    /// trip and the terminal size on the right. Every segment is a quiet button.
    pub(super) fn status_strip(&mut self, ui: &mut Ui, rect: egui::Rect, fill: Color32) {
        let p = style::pal(ui);
        let dark = ui.visuals().dark_mode;
        ui.painter().rect_filled(rect, 0.0, fill);
        ui.painter()
            .hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, p.border));
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let saved = self
            .store
            .bookmark(tab.bookmark.id)
            .unwrap_or(&tab.bookmark);
        let label = self.store.label_of(saved).cloned();
        let via = self
            .store
            .jump_chain(saved)
            .ok()
            .and_then(|c| c.last().map(|b| b.display_name()));
        let state = tab.state();
        let (dot, status, _) = tab_status(tab, &p);
        let connecting = matches!(state, ConnState::Connecting | ConnState::Authenticating);
        let lost = matches!(state, ConnState::Closed(Some(_)));
        let address = tab.bookmark.address();
        let pane = tab.focused_pane();
        let grid = pane
            .and_then(|p| p.view.grid)
            .map(|g| format!("{}×{}", g.cols, g.rows));
        let cwd = pane
            .filter(|_| state.is_connected())
            .and_then(|p| p.title())
            .and_then(|t| crate::ui::tabs::cwd_from_title(&t));
        let root = tab.sftp.as_ref().is_some_and(|s| s.is_root());
        let progress = tab.transfer_progress();
        let (forwards, latency) = {
            let s = tab.conn.shared.lock();
            (s.forwards.clone(), s.latency)
        };
        let connected = state.is_connected();
        let host = saved.display_name();
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(rect.left() + 8.0, rect.top() + 1.0),
                    pos2(rect.right() - 8.0, rect.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 2.0;
        if let Some(l) = &label {
            let tag = l.tag();
            if !tag.is_empty() {
                ui.add_space(2.0);
                kit::env_tag(ui, &tag, style::label_color(l.color, dark), false);
                ui.add_space(4.0);
            }
        }
        let state_color = if lost { p.err } else { p.text_muted };
        let lead = if connecting {
            SegLead::Spinner
        } else {
            SegLead::Dot(dot)
        };
        let status_text = if lost { t("Connection lost") } else { status };
        status_seg(ui, lead, status_text, state_color, false, Sense::hover());
        let r = status_seg(
            ui,
            SegLead::None,
            &address,
            p.text_muted,
            true,
            Sense::click(),
        )
        .on_hover_text(t("Copy Address"));
        if r.clicked() {
            ui.ctx().copy_text(address.clone());
        }
        if let Some(via) = &via {
            status_seg(
                ui,
                SegLead::Icon(icons::ARROW_BEND_DOWN_RIGHT),
                &tf("via {}", &[via]),
                p.text_muted,
                false,
                Sense::hover(),
            );
        }
        if let Some(cwd) = &cwd {
            let r = status_seg(
                ui,
                SegLead::Icon(icons::FOLDER_SIMPLE),
                cwd,
                p.text_muted,
                true,
                Sense::click(),
            )
            .on_hover_text(t("Copy Path"));
            if r.clicked() {
                ui.ctx().copy_text(cwd.clone());
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            if let Some(g) = &grid {
                status_seg(ui, SegLead::None, g, p.text_muted, true, Sense::hover());
            }
            if connected && let Some(l) = latency {
                let ms = l.as_millis();
                let slow = ms > 150;
                status_seg(
                    ui,
                    SegLead::Icon(icons::PULSE),
                    &format!("{ms} ms"),
                    if slow { p.warn } else { p.text_muted },
                    false,
                    Sense::hover(),
                )
                .on_hover_text(t("Round trip to the server"));
            }
            if connected && !forwards.is_empty() {
                let failing = forwards
                    .iter()
                    .filter(|f| f.error.is_some() || !f.listening)
                    .count();
                let count = if forwards.len() == 1 {
                    t("1 forward").to_string()
                } else {
                    tf("{} forwards", &[&forwards.len().to_string()])
                };
                let label = if failing > 0 {
                    tf("{} · {} failing", &[&count, &failing.to_string()])
                } else {
                    count
                };
                let color = if failing > 0 { p.warn } else { p.text_muted };
                let r = status_seg(
                    ui,
                    SegLead::Icon(icons::ARROWS_LEFT_RIGHT),
                    &label,
                    color,
                    false,
                    Sense::click(),
                );
                #[cfg(debug_assertions)]
                crate::devtest::record("forwards-segment", r.rect);
                self.forwards_popover(&r, &forwards, failing, &host);
            }
            if let Some(pr) = &progress {
                let files = pr.files_total.max(1);
                let text = tf(
                    "{}% · {} files",
                    &[
                        &((pr.fraction() * 100.0).floor() as u32).to_string(),
                        &files.to_string(),
                    ],
                );
                let r = status_seg(
                    ui,
                    SegLead::Ring(pr.fraction()),
                    &text,
                    p.accent_text,
                    false,
                    Sense::click(),
                )
                .on_hover_text(format!(
                    "{}\n{}",
                    crate::ui::sftp_panel::summary_title(pr),
                    crate::ui::sftp_panel::summary_detail(pr)
                ));
                if r.clicked() {
                    self.actions.push(Action::ToggleSftp);
                }
            }
            if root {
                status_seg(
                    ui,
                    SegLead::Icon(icons::SHIELD_WARNING),
                    t("root (sudo) in Files"),
                    p.root,
                    false,
                    Sense::hover(),
                );
            }
        });
    }

    /// The port forwarding popover (360 px): a header with the host, then each forward with its
    /// kind, rule and state (copy its address, or why it fails), and Edit forwards….
    fn forwards_popover(
        &mut self,
        anchor: &egui::Response,
        forwards: &[crate::ssh::types::ForwardStatus],
        failing: usize,
        host: &str,
    ) {
        let mut edit = false;
        let mut retry = None;
        egui::Popup::menu(anchor)
            .width(360.0)
            .align(egui::RectAlign::TOP_END)
            .gap(6.0)
            .frame(kit::popover_frame_ctx(&anchor.ctx).inner_margin(egui::Margin::same(6)))
            .show(|ui| {
                kit::menu_body(ui);
                ui.set_width(348.0);
                let p = style::pal(ui);
                // Header.
                let (head, _) = ui.allocate_exact_size(vec2(348.0, 34.0), Sense::hover());
                let painter = ui.painter();
                let g = painter.layout_no_wrap(
                    t("Port forwarding").to_string(),
                    style::semibold(style::BODY),
                    p.text,
                );
                let gw = g.size().x;
                painter.galley(
                    pos2(head.left() + 8.0, head.center().y - g.size().y / 2.0),
                    g,
                    p.text,
                );
                if failing > 0 {
                    paint_badge(
                        painter,
                        pos2(head.left() + 8.0 + gw + 8.0, head.center().y),
                        &tf("{} failing", &[&failing.to_string()]),
                        p.err,
                        p.tone_soft(Tone::Err),
                    );
                }
                painter.text(
                    pos2(head.right() - 8.0, head.center().y),
                    Align2::RIGHT_CENTER,
                    host,
                    style::regular(style::SMALL),
                    p.text_faint,
                );
                for (i, f) in forwards.iter().enumerate() {
                    let (color, detail, err) = match (&f.error, f.listening) {
                        (None, true) => (p.ok, listening_on(&f.forward), false),
                        (Some(e), _) => (p.err, e.clone(), true),
                        (None, false) => (p.err, t("Not listening").to_string(), true),
                    };
                    if forward_row(ui, &f.forward, color, &detail, err) {
                        if err {
                            retry = Some(i);
                        } else {
                            ui.ctx().copy_text(forward_target(&f.forward));
                        }
                    }
                }
                kit::menu_sep(ui);
                if kit::menu_item(ui, Some(icons::PENCIL_SIMPLE), t("Edit forwards…"), None)
                    .clicked()
                {
                    edit = true;
                    ui.close();
                }
            });
        if let Some(i) = retry
            && let Some(tab) = self.tabs.get(self.active)
        {
            tab.conn.retry_forward(i);
        }
        if edit && let Some(tab) = self.tabs.get(self.active) {
            self.editor_section = Some(crate::ui::editor::Section::Network);
            self.actions.push(Action::EditBookmark(tab.bookmark.id));
        }
    }

    /// Light ⇄ dark (the palette and the View menu).
    pub(super) fn toggle_theme(&mut self, ctx: &egui::Context) {
        let dark = ctx.theme() == egui::Theme::Dark;
        self.settings.theme = if dark {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
    }
}

/// The narrowest a tab gets before the strip scrolls (and the ⌄ +N button lists the rest).
const TAB_MIN_W: f32 = 96.0;

/// A toolbar icon button (26 px): quiet until hovered, the selection fill while its popover
/// is open.
fn small_icon_button(ui: &mut Ui, icon: &str, tooltip: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    let resp = if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, tooltip));
    if ui.is_rect_visible(rect) {
        let p = style::pal(ui);
        let hovered = resp.hovered();
        let painter = ui.painter();
        let radius = CornerRadius::same(style::RADIUS);
        if on {
            painter.rect_filled(rect, radius, p.accent_soft);
        } else if resp.is_pointer_button_down_on() {
            painter.rect_filled(rect, radius, p.subtle);
        } else if hovered {
            painter.rect_filled(rect, radius, p.hover);
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon,
            style::icon_font(16.0),
            if on || hovered { p.text } else { p.text_muted },
        );
    }
    resp
}

/// The + after the tabs (26 px, a 14 px icon in the faint colour).
fn tab_add_button(ui: &mut Ui, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    let resp = resp.on_hover_text(tooltip);
    if ui.is_rect_visible(rect) {
        let p = style::pal(ui);
        let hovered = resp.hovered();
        if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(style::RADIUS), p.hover);
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icons::PLUS,
            style::icon_font(14.0),
            if hovered { p.text } else { p.text_faint },
        );
    }
    resp
}

/// The hairline between toolbar groups (16 px high, 4 px either side).
fn toolbar_sep(ui: &mut Ui) {
    let p = style::pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 16.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0, p.border_strong),
    );
}

/// The Files toggle of the toolbar (26 px): a folder and "Files", the selection fill while the
/// panel is open; while files move, an accent tint, a progress ring and the percentage.
fn files_toggle(ui: &mut Ui, on: bool, fraction: Option<f32>) -> egui::Response {
    let p = style::pal(ui);
    let label = match fraction {
        Some(f) => format!("{} {}%", t("Files"), (f * 100.0).floor() as u32),
        None => t("Files").to_string(),
    };
    let g = ui
        .painter()
        .layout_no_wrap(label, style::medium(style::SMALL), Color32::PLACEHOLDER);
    let lead = if fraction.is_some() { 14.0 } else { 15.0 };
    let w = 9.0 + lead + 6.0 + g.size().x + 9.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, style::TOOLBAR_H), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter();
    let (fill, color) = if fraction.is_some() {
        (p.accent_tint, p.accent_text)
    } else if on {
        (p.accent_soft, p.text)
    } else if hovered {
        (p.hover, p.text)
    } else {
        (Color32::TRANSPARENT, p.text_muted)
    };
    painter.rect_filled(rect, CornerRadius::same(style::RADIUS), fill);
    let c = pos2(rect.left() + 9.0 + lead / 2.0, rect.center().y);
    match fraction {
        Some(f) => style::progress_ring(painter, c, 7.0, f, ui.visuals().dark_mode),
        None => {
            painter.text(
                c,
                Align2::CENTER_CENTER,
                icons::FOLDER_SIMPLE,
                style::icon_font(15.0),
                color,
            );
        }
    }
    painter.galley(
        pos2(
            rect.left() + 9.0 + lead + 6.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        color,
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, "Files"));
    resp
}

/// What leads a status bar segment.
enum SegLead {
    None,
    Dot(Color32),
    Spinner,
    Icon(&'static str),
    Ring(f32),
}

/// A status bar segment (20 px): a lead and 11 px text (mono for addresses and paths), a quiet
/// fill when it can be clicked and is hovered.
fn status_seg(
    ui: &mut Ui,
    lead: SegLead,
    text: &str,
    color: Color32,
    mono: bool,
    sense: Sense,
) -> egui::Response {
    let p = style::pal(ui);
    let font = if mono {
        style::mono(11.0)
    } else {
        style::regular(style::XS)
    };
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let lead_w = match lead {
        SegLead::None => 0.0,
        SegLead::Dot(_) => 7.0,
        SegLead::Spinner => 10.0,
        SegLead::Icon(_) => 13.0,
        SegLead::Ring(_) => 14.0,
    };
    let gap = if lead_w > 0.0 { 6.0 } else { 0.0 };
    let w = 6.0 + lead_w + gap + g.size().x + 6.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 20.0), sense);
    if !ui.is_rect_visible(rect) {
        return resp;
    }
    let hovered = sense.senses_click() && resp.hovered();
    let neutral = color == p.text_muted;
    let text_color = if hovered && neutral { p.text } else { color };
    let lead_color = if neutral { p.text_faint } else { color };
    if hovered {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), p.hover);
    }
    let lc = pos2(rect.left() + 6.0 + lead_w / 2.0, rect.center().y);
    match lead {
        SegLead::None => {}
        SegLead::Dot(c) => {
            ui.painter().circle_filled(lc, 3.5, c);
        }
        SegLead::Spinner => kit::paint_spinner(ui, lc, 5.0),
        SegLead::Icon(i) => {
            ui.painter().text(
                lc,
                Align2::CENTER_CENTER,
                i,
                style::icon_font(13.0),
                lead_color,
            );
        }
        SegLead::Ring(f) => style::progress_ring(ui.painter(), lc, 7.0, f, ui.visuals().dark_mode),
    }
    ui.painter().galley(
        pos2(
            rect.left() + 6.0 + lead_w + gap,
            rect.center().y - g.size().y / 2.0 - 0.5,
        ),
        g,
        text_color,
    );
    resp
}

/// A small badge (18 px) with its left edge at `left`, centred on `left.y`.
fn paint_badge(painter: &egui::Painter, left: egui::Pos2, text: &str, fg: Color32, bg: Color32) {
    let g = painter.layout_no_wrap(text.to_string(), style::semibold(style::MICRO), fg);
    let rect = egui::Rect::from_min_size(pos2(left.x, left.y - 9.0), vec2(g.size().x + 12.0, 18.0));
    painter.rect_filled(rect, CornerRadius::same(5), bg);
    painter.galley(rect.center() - g.size() / 2.0, g, fg);
}

/// `-L`, `-R` or `-D`.
fn forward_kind(f: &crate::bookmarks::Forward) -> &'static str {
    use crate::bookmarks::ForwardKind;
    match f.kind {
        ForwardKind::Local => "-L",
        ForwardKind::Remote => "-R",
        ForwardKind::Dynamic => "-D",
    }
}

/// What a forward connects: `5432 → localhost:5432`, or the SOCKS port.
fn forward_target_rule(f: &crate::bookmarks::Forward) -> String {
    use crate::bookmarks::ForwardKind;
    match f.kind {
        ForwardKind::Dynamic => f.listen_port.to_string(),
        _ => format!("{} → {}:{}", f.listen_port, f.host, f.port),
    }
}

/// The address a forward is used through (copied from the popover).
fn forward_target(f: &crate::bookmarks::Forward) -> String {
    use crate::bookmarks::ForwardKind;
    match f.kind {
        ForwardKind::Local | ForwardKind::Dynamic => format!("127.0.0.1:{}", f.listen_port),
        ForwardKind::Remote => format!("{}:{}", f.host, f.port),
    }
}

/// One forward in the popover: its state dot, kind and rule, what it does (or why it fails),
/// and a copy button. Returns whether the copy button was clicked.
fn forward_row(
    ui: &mut Ui,
    f: &crate::bookmarks::Forward,
    color: Color32,
    detail: &str,
    err: bool,
) -> bool {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(348.0, 46.0), Sense::hover());
    let hovered = resp.hovered();
    // A failing forward gets a Retry button; the others copy their address.
    let retry_w = if err {
        ui.painter()
            .layout_no_wrap(t("Retry").to_string(), style::medium(style::SMALL), p.text)
            .size()
            .x
            + 18.0
    } else {
        20.0
    };
    let copy_rect = egui::Rect::from_center_size(
        pos2(rect.right() - 8.0 - retry_w / 2.0, rect.center().y),
        vec2(retry_w, if err { 24.0 } else { 20.0 }),
    );
    let copy = ui.interact(copy_rect, resp.id.with("copy"), Sense::click());
    let copy = if err {
        copy
    } else {
        copy.on_hover_text(t("Copy Address"))
    };
    let painter = ui.painter();
    if hovered || copy.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    painter.circle_filled(pos2(rect.left() + 8.0 + 4.0, rect.center().y), 3.5, color);
    let x = rect.left() + 8.0 + 10.0 + 10.0;
    let kind = painter.layout_no_wrap(
        forward_kind(f).to_string(),
        style::semibold(style::MICRO),
        p.text_faint,
    );
    let kw = kind.size().x;
    let top = rect.top() + 7.0;
    painter.galley(pos2(x, top + 2.0), kind, p.text_faint);
    painter.text(
        pos2(x + kw + 7.0, top),
        Align2::LEFT_TOP,
        forward_target_rule(f),
        style::mono(12.0),
        p.text,
    );
    let clip = egui::Rect::from_min_max(rect.min, pos2(copy_rect.left() - 6.0, rect.bottom()));
    painter.with_clip_rect(clip).text(
        pos2(x, top + 18.0),
        Align2::LEFT_TOP,
        detail,
        style::regular(style::SMALL),
        if err { p.err } else { p.text_faint },
    );
    if err {
        painter.rect(
            copy_rect,
            CornerRadius::same(style::RADIUS),
            if copy.hovered() { p.subtle } else { p.button },
            Stroke::new(1.0, p.button_border),
            egui::StrokeKind::Inside,
        );
        painter.text(
            copy_rect.center(),
            Align2::CENTER_CENTER,
            t("Retry"),
            style::medium(style::SMALL),
            p.text,
        );
        return copy.clicked();
    }
    if copy.hovered() {
        painter.rect_filled(copy_rect, CornerRadius::same(5), p.subtle);
    }
    painter.text(
        copy_rect.center(),
        Align2::CENTER_CENTER,
        icons::COPY,
        style::icon_font(13.0),
        if copy.hovered() { p.text } else { p.text_muted },
    );
    copy.clicked()
}

/// The name of a label colour.
pub(crate) fn label_color_name(c: crate::bookmarks::LabelColor) -> &'static str {
    use crate::bookmarks::LabelColor as L;
    match c {
        L::Red => t("Red"),
        L::Orange => t("Orange"),
        L::Yellow => t("Yellow"),
        L::Green => t("Green"),
        L::Teal => t("Teal"),
        L::Blue => t("Blue"),
        L::Violet => t("Violet"),
        L::Pink => t("Pink"),
        L::Grey => t("Grey"),
    }
}

/// The items of a Label submenu: inherit the group's label (when it has one), each colour, or
/// no label. Returns the chosen label (`Some(None)` = inherit / none).
pub(crate) fn label_menu(
    ui: &mut Ui,
    own: Option<&crate::bookmarks::Label>,
    inherited: Option<(&crate::bookmarks::Label, &str)>,
) -> Option<Option<crate::bookmarks::Label>> {
    use crate::bookmarks::{Label, LabelColor};
    let p = style::pal(ui);
    let dark = ui.visuals().dark_mode;
    ui.set_min_width(200.0);
    let mut chosen = None;
    let check = |ui: &Ui, r: &egui::Response| {
        ui.painter().text(
            pos2(r.rect.right() - 16.0, r.rect.center().y),
            Align2::CENTER_CENTER,
            icons::CHECK,
            style::icon_font(14.0),
            p.accent_text,
        );
    };
    if let Some((gl, group)) = inherited {
        let r = kit::menu_item(ui, None, &tf("Inherit from {}", &[group]), None);
        ui.painter().circle_filled(
            pos2(r.rect.left() + 16.0, r.rect.center().y),
            4.0,
            style::label_color(gl.color, dark),
        );
        if own.is_none() {
            check(ui, &r);
        }
        if r.clicked() {
            chosen = Some(None);
            ui.close();
        }
        kit::menu_sep(ui);
    }
    for c in LabelColor::ALL {
        let r = kit::menu_item(ui, None, label_color_name(c), None);
        ui.painter().circle_filled(
            pos2(r.rect.left() + 16.0, r.rect.center().y),
            4.0,
            style::label_color(c, dark),
        );
        if own.is_some_and(|l| l.color == c) {
            check(ui, &r);
        }
        if r.clicked() {
            let text = own
                .map(|l| l.text.clone())
                .or_else(|| inherited.map(|(l, _)| l.text.clone()))
                .unwrap_or_default();
            chosen = Some(Some(Label { color: c, text }));
            ui.close();
        }
    }
    if inherited.is_none() {
        kit::menu_sep(ui);
        let r = kit::menu_item(ui, Some(icons::PROHIBIT), t("No Label"), None);
        if own.is_none() {
            check(ui, &r);
        }
        if r.clicked() {
            chosen = Some(None);
            ui.close();
        }
    }
    chosen
}

/// `user@host:port`, always with the port (the status strip).
pub fn address_with_port(b: &crate::bookmarks::Bookmark) -> String {
    let a = b.address();
    if b.port == crate::bookmarks::DEFAULT_PORT {
        format!("{a}:{}", b.port)
    } else {
        a
    }
}

/// `-L 8080 → localhost:80`.
pub fn forward_rule(f: &crate::bookmarks::Forward) -> String {
    use crate::bookmarks::ForwardKind;
    match f.kind {
        ForwardKind::Local => format!("-L {} → {}:{}", f.listen_port, f.host, f.port),
        ForwardKind::Remote => format!("-R {} → {}:{}", f.listen_port, f.host, f.port),
        ForwardKind::Dynamic => format!("-D {}", f.listen_port),
    }
}

/// What a working forward does: "Listening on 127.0.0.1:8080".
fn listening_on(f: &crate::bookmarks::Forward) -> String {
    use crate::bookmarks::ForwardKind;
    match f.kind {
        ForwardKind::Local => tf("Listening on 127.0.0.1:{}", &[&f.listen_port.to_string()]),
        ForwardKind::Remote => tf(
            "The server forwards its port {} here",
            &[&f.listen_port.to_string()],
        ),
        ForwardKind::Dynamic => tf("SOCKS proxy on 127.0.0.1:{}", &[&f.listen_port.to_string()]),
    }
}

/// One quick command in the ⚡ popover (44 px): ▶ when it runs at once, ✎ when it is pasted
/// first; its name over a mono preview of what it sends; its ⌃n key on the right.
fn quick_row(ui: &mut Ui, index: usize, qc: &crate::settings::QuickCommand) -> egui::Response {
    let p = style::pal(ui);
    let width = if ui.is_sizing_pass() {
        328.0
    } else {
        ui.available_width()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
    let hl = resp.hovered() || resp.has_focus();
    let painter = ui.painter();
    if hl {
        painter.rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    let icon = if qc.run {
        icons::PLAY
    } else {
        icons::PENCIL_SIMPLE_LINE
    };
    painter.text(
        pos2(rect.left() + 8.0 + 8.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(15.0),
        p.text_muted,
    );
    let name = if qc.name.trim().is_empty() {
        qc.command.clone()
    } else {
        qc.name.clone()
    };
    let x = rect.left() + 8.0 + 16.0 + 9.0;
    let sc = (cfg!(target_os = "macos") && index < 9).then(|| format!("⌃{}", index + 1));
    let right = match &sc {
        Some(sc) => {
            let g = painter.layout_no_wrap(sc.clone(), style::regular(style::SMALL), p.text_faint);
            let w = g.size().x;
            painter.galley(
                pos2(rect.right() - 8.0 - w, rect.center().y - g.size().y / 2.0),
                g,
                p.text_faint,
            );
            rect.right() - 8.0 - w - 18.0
        }
        None => rect.right() - 8.0,
    };
    let clip = egui::Rect::from_min_max(rect.min, pos2(right, rect.bottom()));
    painter.with_clip_rect(clip).text(
        pos2(x, rect.top() + 6.0),
        Align2::LEFT_TOP,
        name,
        style::medium(style::BODY),
        p.text,
    );
    painter.with_clip_rect(clip).text(
        pos2(x, rect.top() + 24.0),
        Align2::LEFT_TOP,
        qc.command.replace('\n', " ⏎ "),
        style::mono(11.0),
        p.text_faint,
    );
    resp.on_hover_text(RichText::new(&qc.command).monospace())
}
