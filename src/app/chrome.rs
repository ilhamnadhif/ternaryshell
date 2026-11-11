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
            row.add_space(8.0);
            self.app_menu(&mut row);
            row.add_space(6.0);
            row.label(
                RichText::new("TernaryShell")
                    .font(style::semibold(13.0))
                    .color(p.text),
            );
        }
        row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            let hide = kit::icon_button(
                ui,
                icons::SIDEBAR_SIMPLE,
                &format!("{} ({})", t("Hide Sidebar"), S::Sidebar.label()),
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
        if !self.settings.sidebar_visible {
            if cfg!(target_os = "macos") {
                lead.add_space(if self.fullscreen { 10.0 } else { TRAFFIC_W });
            } else {
                lead.add_space(8.0);
                self.app_menu(&mut lead);
                lead.add_space(2.0);
            }
            if kit::icon_button(
                &mut lead,
                icons::SIDEBAR_SIMPLE,
                &format!("{} ({})", t("Show Sidebar"), S::Sidebar.label()),
            )
            .clicked()
            {
                self.actions.push(Action::ToggleSidebar);
            }
            lead.add_space(6.0);
        }
        let tabs_left = lead.min_rect().right().max(bar.left());

        // The toolbar on the right first, so the tabs know how much room is left.
        let mut tools = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(tabs_left, bar.top()),
                    pos2(bar.right() - GUTTER, bar.bottom()),
                ))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        self.session_tools(&mut tools);
        let tools_left = tools.min_rect().left().min(bar.right() - GUTTER);
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
        let out = egui::ScrollArea::horizontal()
            .id_salt("tab-strip")
            .max_width(area.width())
            .max_height(TITLE_H)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(&mut child, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for i in 0..self.tabs.len() {
                        if let Some(t) = self.tab_pill(ui, i, p, dark) {
                            drag_target = Some(t);
                        }
                    }
                    if kit::icon_button(
                        ui,
                        icons::PLUS,
                        &format!("{} ({})", t("New Tab"), S::NewTab.label()),
                    )
                    .clicked()
                    {
                        self.actions.push(Action::QuickConnect);
                    }
                });
            });
        // Tabs that don't fit scroll: the edge that hides more fades out, and a button lists
        // every tab.
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
        if overflow || offset > 0.5 {
            if overflow {
                kit::fade(
                    ui.painter(),
                    egui::Rect::from_min_size(
                        pos2(view.right() - 60.0, view.top()),
                        vec2(60.0, view.height()),
                    ),
                    p.chrome,
                    true,
                );
            }
            let list = egui::Rect::from_center_size(
                pos2(view.right() - 13.0, view.center().y),
                vec2(26.0, 26.0),
            );
            let resp = ui.interact(list, egui::Id::new("tab-list"), Sense::click());
            ui.painter()
                .rect_filled(list, CornerRadius::same(6), p.chrome);
            kit::paint_icon_button(
                ui,
                &resp,
                list,
                icons::CARET_DOWN,
                t("All Tabs"),
                false,
                false,
            );
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

    /// One tab: a 30 px pill (the active one raised on the canvas color), its state on the
    /// left, the title, and × (or the key of a waiting prompt) on the right. Returns a drop.
    fn tab_pill(
        &mut self,
        ui: &mut Ui,
        i: usize,
        p: &style::Palette,
        dark: bool,
    ) -> Option<(u64, u64)> {
        let active = i == self.active;
        let tab = &self.tabs[i];
        let tab_id = tab.id;
        let (dot, status, ring) = tab_status(tab, p);
        let waiting = tab.conn.shared.lock().prompt.is_some();
        let activity = !active && tab.has_activity();
        let connecting = matches!(
            tab.state(),
            ConnState::Connecting | ConnState::Authenticating
        ) && !waiting;
        let progress = tab.transfer_progress();
        let title = tab.title();
        let font = if active {
            style::medium(style::BODY)
        } else {
            style::regular(style::BODY)
        };
        let galley = ui
            .painter()
            .layout_no_wrap(title.clone(), font, Color32::PLACEHOLDER);
        let lead_w = if progress.is_some() || connecting {
            14.0
        } else {
            7.0
        };
        let width = (11.0 + lead_w + 8.0 + galley.size().x + 8.0 + 18.0 + 6.0).clamp(120.0, 210.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 30.0), Sense::click_and_drag());
        let hovered = ui.rect_contains_pointer(rect);
        let painter = ui.painter();
        let radius = CornerRadius::same(style::RADIUS_MD);
        if active {
            if dark {
                painter.rect_filled(
                    rect.translate(vec2(0.0, 1.0)),
                    radius,
                    Color32::from_black_alpha(100),
                );
                painter.rect(
                    rect,
                    radius,
                    p.tab_on,
                    Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x3a)),
                    StrokeKind::Inside,
                );
            } else {
                painter.rect_filled(
                    rect.translate(vec2(0.0, 1.0)),
                    radius,
                    Color32::from_black_alpha(8),
                );
                painter.rect(
                    rect,
                    radius,
                    p.tab_on,
                    Stroke::new(1.0, p.border),
                    StrokeKind::Inside,
                );
            }
        } else if hovered {
            painter.rect_filled(rect, radius, p.hover);
        }
        let cy = rect.center().y;
        let lead_c = pos2(rect.left() + 11.0 + lead_w / 2.0, cy);
        if let Some(pr) = &progress {
            style::progress_ring(painter, lead_c, 7.0, pr.fraction(), dark);
            resp.clone().on_hover_text(format!(
                "{}\n{}",
                crate::ui::sftp_panel::summary_title(pr),
                crate::ui::sftp_panel::summary_detail(pr)
            ));
        } else if connecting {
            kit::paint_spinner(ui, lead_c, 7.0);
        } else {
            kit::status_dot(painter, lead_c, dot, ring && dot != p.text_faint);
        }
        let text_color = if active || hovered || activity {
            p.text
        } else {
            p.text_muted
        };
        let text_x = rect.left() + 11.0 + lead_w + 8.0;
        let clip = egui::Rect::from_min_max(
            pos2(text_x, rect.top()),
            pos2(rect.right() - 6.0 - 18.0 - 6.0, rect.bottom()),
        );
        painter.with_clip_rect(clip).galley(
            pos2(text_x, cy - galley.size().y / 2.0),
            galley,
            text_color,
        );
        // × on the active or hovered tab; the waiting key instead while a prompt waits.
        let close_rect =
            egui::Rect::from_center_size(pos2(rect.right() - 6.0 - 9.0, cy), vec2(18.0, 18.0));
        let close = ui.interact(
            close_rect,
            egui::Id::new(("tab-close", tab_id)),
            Sense::click(),
        );
        if waiting && !close.hovered() {
            ui.painter().text(
                close_rect.center(),
                Align2::CENTER_CENTER,
                icons::KEY,
                style::icon_font(13.0),
                p.accent_text,
            );
        } else if hovered || active {
            kit::paint_icon_button(
                ui,
                &close,
                close_rect,
                icons::X,
                t("Close Tab"),
                false,
                false,
            );
        } else if activity {
            ui.painter()
                .circle_filled(pos2(rect.right() - 12.0 - 3.0, cy), 3.0, p.accent);
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
        let hover = if waiting {
            format!("{}\n{}", tab.bookmark.address(), t("Waiting for you"))
        } else {
            format!("{}\n{status}", tab.bookmark.address())
        };
        let resp = resp.on_hover_text(hover);
        resp.dnd_set_drag_payload(tab_id);
        let mut target = None;
        if let Some(from) = resp.dnd_release_payload::<u64>() {
            target = Some((*from, tab_id));
        }
        let bookmark_id = tab.bookmark.id;
        let several = self.tabs.len() > 1;
        resp.context_menu(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(230.0);
            if kit::menu_item(
                ui,
                Some(icons::COPY),
                t("Duplicate Tab"),
                Some(&S::NewTab.label()),
            )
            .clicked()
            {
                self.actions.push(Action::DuplicateTab(tab_id));
                ui.close();
            }
            if kit::menu_item(
                ui,
                Some(icons::ARROWS_CLOCKWISE),
                t("Reconnect"),
                Some(&S::Reconnect.label()),
            )
            .clicked()
            {
                self.actions.push(Action::SelectTab(i));
                self.actions.push(Action::Reconnect);
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::PENCIL_SIMPLE), t("Edit Connection…"), None).clicked()
            {
                self.actions.push(Action::EditBookmark(bookmark_id));
                ui.close();
            }
            kit::menu_sep(ui);
            if kit::menu_item(ui, Some(icons::X), t("Close Tab"), Some(&S::Close.label())).clicked()
            {
                self.actions.push(Action::CloseTab(tab_id));
                ui.close();
            }
            ui.add_enabled_ui(several, |ui| {
                if kit::menu_item(ui, None, t("Close Other Tabs"), None).clicked() {
                    self.actions.push(Action::CloseOtherTabs(tab_id));
                    ui.close();
                }
            });
        });
        target
    }

    /// The session toolbar, right to left: SFTP, split, quick commands, and the update pill.
    fn session_tools(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.x = 2.0;
        let has_tab = !self.tabs.is_empty();
        if has_tab {
            // The SFTP toggle; while files move it shows how far along they are.
            let sftp_on = self.tabs.get(self.active).is_some_and(|t| t.show_sftp);
            let progress = self.tabs.get(self.active).and_then(Tab::transfer_progress);
            let (label, hover) = match &progress {
                Some(pr) => (
                    format!("{}%", (pr.fraction() * 100.0).floor() as u32),
                    format!(
                        "{}\n{}",
                        crate::ui::sftp_panel::summary_title(pr),
                        crate::ui::sftp_panel::summary_detail(pr)
                    ),
                ),
                None => (
                    String::new(),
                    format!("{} ({})", t("SFTP Panel"), S::Sftp.label()),
                ),
            };
            let fraction = progress.as_ref().map(|pr| pr.fraction());
            let sftp = sftp_toggle(ui, sftp_on, fraction, &label).on_hover_text(hover);
            #[cfg(debug_assertions)]
            crate::devtest::record("sftp-button", sftp.rect);
            if sftp.clicked() {
                self.actions.push(Action::ToggleSftp);
            }
            kit::vdivider(ui, 16.0);
            let split = kit::icon_button(
                ui,
                icons::SQUARE_SPLIT_HORIZONTAL,
                &format!("{} ({})", t("Split Right"), S::SplitRight.label()),
            );
            egui::Popup::menu(&split).show(|ui| {
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
            let quick = kit::icon_button(ui, icons::LIGHTNING, t("Quick Commands"));
            #[cfg(debug_assertions)]
            crate::devtest::record("quick-button", quick.rect);
            self.quick_commands_popover(&quick);
        }
        if self.updater.toolbar_button(ui) {
            kit::vdivider(ui, 16.0);
        }
    }

    /// The ⚡ popover: a filter, then each command with a preview of what it sends.
    fn quick_commands_popover(&mut self, button: &egui::Response) {
        let mut chosen = None;
        let mut edit = false;
        let filter_id = egui::Id::new("quick-filter");
        let popup = egui::Popup::menu(button).width(320.0).show(|ui| {
            kit::menu_body(ui);
            ui.set_width(320.0 - 10.0);
            let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
            ui.add_space(2.0);
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
            ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
            ui.add_space(6.0);
            kit::menu_label(ui, t("Quick commands"));
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
                    ui.label(
                        RichText::new(t("No quick commands")).color(style::pal(ui).text_faint),
                    );
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
            if kit::menu_item(
                ui,
                Some(icons::PENCIL_SIMPLE),
                t("Edit quick commands…"),
                None,
            )
            .clicked()
            {
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

    /// The ☰ menu (Windows and Linux): every command of the macOS menu bar.
    pub(super) fn app_menu(&mut self, ui: &mut Ui) {
        let button = kit::icon_button(ui, icons::LIST, t("Menu"));
        #[cfg(debug_assertions)]
        crate::devtest::record("app-menu", button.rect);
        let has_tab = !self.tabs.is_empty();
        let mut chosen = None;
        egui::Popup::menu(&button).show(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(290.0);
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
            item(
                ui,
                icons::TERMINAL_WINDOW,
                t("New Tab"),
                Some(S::NewTab),
                Action::NewTab,
                has_tab,
            );
            item(
                ui,
                icons::ARROWS_CLOCKWISE,
                t("Reconnect"),
                Some(S::Reconnect),
                Action::Reconnect,
                has_tab,
            );
            kit::menu_sep(ui);
            item(
                ui,
                icons::COPY,
                t("Copy"),
                Some(S::Copy),
                Action::Copy,
                true,
            );
            item(
                ui,
                icons::CLIPBOARD,
                t("Paste"),
                Some(S::Paste),
                Action::Paste,
                true,
            );
            item(
                ui,
                icons::BROOM,
                t("Clear Scrollback"),
                Some(S::ClearScrollback),
                Action::ClearScrollback,
                has_tab,
            );
            kit::menu_sep(ui);
            kit::submenu(ui, Some(icons::SIDEBAR_SIMPLE), t("View"), |ui| {
                ui.set_min_width(270.0);
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
                    icons::ARROWS_DOWN_UP,
                    t("SFTP Panel"),
                    Some(S::Sftp),
                    Action::ToggleSftp,
                    has_tab,
                );
                kit::menu_sep(ui);
                item(
                    ui,
                    icons::SQUARE_SPLIT_HORIZONTAL,
                    t("Split Right"),
                    Some(S::SplitRight),
                    Action::SplitRight,
                    has_tab,
                );
                item(
                    ui,
                    icons::SQUARE_SPLIT_VERTICAL,
                    t("Split Down"),
                    Some(S::SplitDown),
                    Action::SplitDown,
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
                    icons::CIRCLE_HALF,
                    t("Theme"),
                    None,
                    Action::ToggleTheme,
                    true,
                );
                item(
                    ui,
                    icons::DESKTOP,
                    t("Full Screen"),
                    Some(S::FullScreen),
                    Action::ToggleFullScreen,
                    true,
                );
            });
            kit::submenu(ui, Some(icons::HARD_DRIVES), t("Connections"), |ui| {
                ui.set_min_width(260.0);
                item(
                    ui,
                    icons::DOWNLOAD_SIMPLE,
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
                icons::KEYBOARD,
                t("Keyboard Shortcuts"),
                None,
                Action::Shortcuts,
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
    }

    // ----- status strip -------------------------------------------------------------------

    /// The bottom of the canvas card: the active session's state, address and jump host on the
    /// left; root, port forwards, transfers and the terminal size on the right.
    pub(super) fn status_strip(&mut self, ui: &mut Ui, rect: egui::Rect, fill: Color32) {
        let p = style::pal(ui);
        let dark = ui.visuals().dark_mode;
        ui.painter().rect_filled(rect, 0.0, fill);
        ui.painter().hline(
            rect.x_range(),
            rect.top() + 0.5,
            Stroke::new(
                1.0,
                if dark {
                    Color32::from_rgb(0x2a, 0x2a, 0x2a)
                } else {
                    p.border
                },
            ),
        );
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let (dot, status, _) = tab_status(tab, &p);
        let via = self
            .store
            .jump_chain(&tab.bookmark)
            .ok()
            .and_then(|c| c.last().map(|b| b.display_name()));
        let address = address_with_port(&tab.bookmark);
        let grid = tab
            .focused_pane()
            .and_then(|p| p.view.grid)
            .map(|g| format!("{}×{}", g.cols, g.rows));
        let root = tab.sftp.as_ref().is_some_and(|s| s.is_root());
        let progress = tab.transfer_progress();
        let forwards = tab.conn.shared.lock().forwards.clone();
        let connected = tab.state().is_connected();
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(rect.left() + 12.0, rect.top() + 1.0),
                    pos2(rect.right() - 10.0, rect.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 3.0, dot);
            ui.label(
                RichText::new(status)
                    .font(style::regular(style::XS))
                    .color(p.text),
            );
        });
        ui.add(
            egui::Label::new(
                RichText::new(address)
                    .font(style::mono(11.0))
                    .color(p.text_muted),
            )
            .truncate(),
        );
        if let Some(via) = via {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                kit::icon(ui, icons::ARROW_ELBOW_DOWN_LEFT, 13.0, p.text_faint);
                ui.label(
                    RichText::new(tf("via {}", &[&via]))
                        .font(style::regular(style::XS))
                        .color(p.text_faint),
                );
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if let Some(g) = grid {
                ui.label(RichText::new(g).font(style::mono(11.0)).color(p.text_muted));
            }
            if let Some(pr) = &progress {
                let text = format!(
                    "{}% · {}/s",
                    (pr.fraction() * 100.0).floor() as u32,
                    crate::sftp::entry::human_size(pr.speed as u64)
                );
                let r = status_link(ui, None, &text, p.accent_text, false);
                style::progress_ring(
                    ui.painter(),
                    pos2(r.rect.left() + 6.0 + 6.0, r.rect.center().y),
                    6.0,
                    pr.fraction(),
                    dark,
                );
                let r = r.on_hover_text(format!(
                    "{}\n{}",
                    crate::ui::sftp_panel::summary_title(pr),
                    crate::ui::sftp_panel::summary_detail(pr)
                ));
                if r.clicked() {
                    self.actions.push(Action::ToggleSftp);
                }
            }
            if connected && !forwards.is_empty() {
                let failing = forwards
                    .iter()
                    .filter(|f| f.error.is_some() || !f.listening)
                    .count();
                let label = if failing > 0 {
                    tf(
                        "{} of {} forwards failing",
                        &[&failing.to_string(), &forwards.len().to_string()],
                    )
                } else if forwards.len() == 1 {
                    t("1 forward").to_string()
                } else {
                    tf("{} forwards", &[&forwards.len().to_string()])
                };
                let color = if failing > 0 { p.err } else { p.text_muted };
                let r = status_link(ui, Some(icons::ARROWS_LEFT_RIGHT), &label, color, false);
                self.forwards_popover(&r, &forwards, failing);
            }
            if root {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(
                        RichText::new(t("root (sudo)"))
                            .font(style::regular(style::XS))
                            .color(p.root),
                    );
                    kit::icon(ui, icons::SHIELD_CHECK, 13.0, p.root);
                });
            }
        });
    }

    /// The port forwarding popover: each forward with its state and what it says.
    fn forwards_popover(
        &mut self,
        anchor: &egui::Response,
        forwards: &[crate::ssh::types::ForwardStatus],
        failing: usize,
    ) {
        let mut edit = false;
        egui::Popup::menu(anchor)
            .width(340.0)
            .align(egui::RectAlign::TOP_END)
            .frame(kit::popover_frame_ctx(&anchor.ctx).inner_margin(egui::Margin::same(8)))
            .show(|ui| {
                kit::menu_body(ui);
                ui.set_width(324.0);
                let p = style::pal(ui);
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(t("Port forwarding"))
                            .font(style::semibold(style::BODY))
                            .color(p.text),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if failing > 0 {
                            kit::badge(
                                ui,
                                &tf("{} failing", &[&failing.to_string()]),
                                Tone::Err,
                                None,
                            );
                        }
                    });
                });
                ui.add_space(6.0);
                for f in forwards {
                    let (tone_color, detail, err) = match (&f.error, f.listening) {
                        (None, true) => (p.ok, listening_on(&f.forward), false),
                        (Some(e), _) => (p.err, e.clone(), true),
                        (None, false) => (p.err, t("Not listening").to_string(), true),
                    };
                    ui.horizontal_top(|ui| {
                        ui.add_space(8.0);
                        ui.spacing_mut().item_spacing.x = 10.0;
                        let (r, _) = ui.allocate_exact_size(vec2(8.0, 20.0), Sense::hover());
                        kit::status_dot(
                            ui.painter(),
                            pos2(r.center().x, r.top() + 9.0),
                            tone_color,
                            true,
                        );
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            ui.label(
                                RichText::new(forward_rule(&f.forward))
                                    .font(style::mono(style::MONO))
                                    .color(p.text),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(detail)
                                        .font(style::regular(style::SMALL))
                                        .color(if err { p.err } else { p.text_faint }),
                                )
                                .wrap(),
                            );
                        });
                    });
                    ui.add_space(8.0);
                }
                kit::menu_sep(ui);
                if kit::menu_item(ui, Some(icons::PENCIL_SIMPLE), t("Edit forwards…"), None)
                    .clicked()
                {
                    edit = true;
                    ui.close();
                }
            });
        if edit && let Some(tab) = self.tabs.get(self.active) {
            self.editor_section = Some(crate::ui::editor::Section::Network);
            self.actions.push(Action::EditBookmark(tab.bookmark.id));
        }
    }

    // ----- no tab open --------------------------------------------------------------------

    /// The Start page: a search that opens the palette, recent connections and quick actions.
    pub(super) fn start_page(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let area = ui.max_rect();
        if self.store.bookmarks.is_empty() && self.store.groups.is_empty() {
            self.welcome_page(ui);
            return;
        }
        let col_w = 620.0f32.min(area.width() - 48.0);
        let x0 = area.center().x - col_w / 2.0;
        let mut y = area.top() + (84.0f32).min(area.height() * 0.11);
        // Logo, title, subtitle.
        let logo = egui::Rect::from_center_size(pos2(area.center().x, y + 26.0), vec2(52.0, 52.0));
        icons::paint_logo(ui.painter(), logo);
        y += 52.0 + 14.0;
        ui.painter().text(
            pos2(area.center().x, y),
            Align2::CENTER_TOP,
            t("Where to?"),
            style::semibold(22.0),
            p.text,
        );
        y += 30.0;
        let sub = {
            let mut job = egui::text::LayoutJob::default();
            let f = |font: egui::FontId, color| egui::TextFormat {
                font_id: font,
                color,
                ..Default::default()
            };
            job.append(
                t("Search saved connections, or type"),
                0.0,
                f(style::regular(style::BODY), p.text_muted),
            );
            job.append(
                " user@host:port",
                0.0,
                f(style::mono(style::MONO), p.text_muted),
            );
            ui.painter().layout_job(job)
        };
        ui.painter().galley(
            pos2(area.center().x - sub.size().x / 2.0, y),
            sub,
            p.text_muted,
        );
        y += 20.0 + 20.0;
        // The search field: opens the palette.
        let field = egui::Rect::from_min_size(
            pos2(area.center().x - 260.0f32.min(col_w / 2.0), y),
            vec2(520.0f32.min(col_w), 36.0),
        );
        let resp = ui.interact(field, egui::Id::new("start-search"), Sense::click());
        let painter = ui.painter();
        painter.rect(
            field,
            CornerRadius::same(style::RADIUS),
            p.field,
            Stroke::new(
                1.0,
                if resp.hovered() {
                    p.border_strong
                } else {
                    p.field_border
                },
            ),
            StrokeKind::Inside,
        );
        painter.text(
            pos2(field.left() + 18.0, field.center().y),
            Align2::CENTER_CENTER,
            icons::MAGNIFYING_GLASS,
            style::icon_font(15.0),
            p.text_faint,
        );
        painter.text(
            pos2(field.left() + 34.0, field.center().y),
            Align2::LEFT_CENTER,
            t("Search or connect…"),
            style::regular(style::BODY),
            p.text_faint,
        );
        let kw = {
            let g = painter.layout_no_wrap(
                S::QuickConnect.label(),
                style::regular(style::XS),
                p.text_muted,
            );
            (g.size().x + 10.0).max(20.0)
        };
        kit::paint_kbd_at(
            painter,
            pos2(field.right() - 8.0 - kw, field.center().y),
            &S::QuickConnect.label(),
            &p,
        );
        if resp.clicked() {
            self.actions.push(Action::QuickConnect);
        }
        resp.on_hover_cursor(egui::CursorIcon::Text);
        y += 36.0 + 30.0;

        // Recent connections.
        let recent: Vec<(Uuid, i64)> = self
            .settings
            .recent
            .iter()
            .filter(|r| self.store.bookmark(r.id).is_some())
            .take(6)
            .map(|r| (r.id, r.at))
            .collect();
        if !recent.is_empty() {
            ui.painter().text(
                pos2(x0, y + 12.0),
                Align2::LEFT_CENTER,
                t("Recent").to_uppercase(),
                style::semibold(style::XS),
                p.text_faint,
            );
            let mut head = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(pos2(x0, y), vec2(col_w, 24.0)))
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            if kit::ghost(&mut head, None, t("Show all")).clicked() {
                self.actions.push(Action::QuickConnect);
            }
            y += 24.0 + 10.0;
            let card_w = (col_w - 20.0) / 3.0;
            for (i, (id, at)) in recent.iter().enumerate() {
                let Some(b) = self.store.bookmark(*id) else {
                    continue;
                };
                let (col, row) = (i % 3, i / 3);
                let rect = egui::Rect::from_min_size(
                    pos2(
                        x0 + col as f32 * (card_w + 10.0),
                        y + row as f32 * (74.0 + 10.0),
                    ),
                    vec2(card_w, 74.0),
                );
                let resp = ui.interact(rect, egui::Id::new(("recent", id)), Sense::click());
                let painter = ui.painter();
                let radius = CornerRadius::same(style::RADIUS_LG);
                if i == 0 {
                    painter.rect_stroke(
                        rect,
                        radius,
                        Stroke::new(3.0, p.accent_softer),
                        StrokeKind::Outside,
                    );
                }
                painter.rect(
                    rect,
                    radius,
                    if resp.hovered() {
                        blend_hover(p.card, &p)
                    } else {
                        p.card
                    },
                    Stroke::new(1.0, if i == 0 { p.accent_line } else { p.border }),
                    StrokeKind::Inside,
                );
                let x = rect.left() + 12.0;
                painter.text(
                    pos2(x + 8.0, rect.top() + 21.0),
                    Align2::CENTER_CENTER,
                    icons::HARD_DRIVES,
                    style::icon_font(16.0),
                    p.text_muted,
                );
                let when = ago(*at);
                let when_g = painter.layout_no_wrap(when, style::regular(style::XS), p.text_faint);
                let name_clip = egui::Rect::from_min_max(
                    pos2(x + 24.0, rect.top()),
                    pos2(rect.right() - 16.0 - when_g.size().x, rect.bottom()),
                );
                painter.with_clip_rect(name_clip).text(
                    pos2(x + 24.0, rect.top() + 21.0),
                    Align2::LEFT_CENTER,
                    b.display_name(),
                    style::medium(style::BODY),
                    p.text,
                );
                painter.galley(
                    pos2(
                        rect.right() - 12.0 - when_g.size().x,
                        rect.top() + 21.0 - when_g.size().y / 2.0,
                    ),
                    when_g,
                    p.text_faint,
                );
                let inner =
                    egui::Rect::from_min_max(rect.min, pos2(rect.right() - 12.0, rect.bottom()));
                painter.with_clip_rect(inner).text(
                    pos2(x, rect.top() + 42.0),
                    Align2::LEFT_CENTER,
                    b.address(),
                    style::mono(11.0),
                    p.text_faint,
                );
                let group = if b.group.is_some() {
                    self.store.group_path(b.group)
                } else {
                    "—".into()
                };
                painter.with_clip_rect(inner).text(
                    pos2(x, rect.top() + 58.0),
                    Align2::LEFT_CENTER,
                    group,
                    style::regular(style::XS),
                    p.text_faint,
                );
                if resp.clicked() {
                    self.actions.push(Action::Connect(*id));
                }
                resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            let rows = recent.len().div_ceil(3) as f32;
            y += rows * 74.0 + (rows - 1.0) * 10.0 + 22.0;
        }
        // Quick actions.
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    pos2(area.left(), y),
                    vec2(area.width(), 26.0),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let widths = [
            text_button_w(&row, t("New connection"), true),
            text_button_w(&row, t("Import from ~/.ssh/config"), true),
            text_button_w(&row, t("Import file…"), true),
        ];
        let total: f32 = widths.iter().sum::<f32>() + 16.0;
        row.add_space(((area.width() - total) / 2.0).max(0.0));
        row.spacing_mut().item_spacing.x = 8.0;
        if kit::secondary_sm(&mut row, Some(icons::PLUS), t("New connection")).clicked() {
            self.actions.push(Action::NewBookmark { group: None });
        }
        if kit::ghost(
            &mut row,
            Some(icons::FILE_CODE),
            t("Import from ~/.ssh/config"),
        )
        .clicked()
        {
            self.actions.push(Action::ImportSshConfig);
        }
        if kit::ghost(&mut row, Some(icons::DOWNLOAD_SIMPLE), t("Import file…")).clicked() {
            self.actions.push(Action::Import);
        }
        // Key hints along the bottom.
        let hints = [
            (S::QuickConnect.label(), t("search")),
            (S::NewConnection.label(), t("new connection")),
            (S::Settings.label(), t("settings")),
        ];
        let painter = ui.painter();
        let mut parts = Vec::new();
        let mut total = 0.0;
        for (k, label) in &hints {
            let g = painter.layout_no_wrap(
                label.to_string(),
                style::regular(style::SMALL),
                p.text_faint,
            );
            let kw = (painter
                .layout_no_wrap(k.clone(), style::regular(style::XS), p.text_muted)
                .size()
                .x
                + 10.0)
                .max(20.0);
            total += kw + 6.0 + g.size().x;
            parts.push((k.clone(), kw, g));
        }
        total += 16.0 * (hints.len() - 1) as f32;
        let mut x = area.center().x - total / 2.0;
        let yb = area.bottom() - 22.0 - 10.0;
        if yb > y + 40.0 {
            for (k, kw, g) in parts {
                kit::paint_kbd_at(painter, pos2(x, yb), &k, &p);
                x += kw + 6.0;
                let w = g.size().x;
                painter.galley(pos2(x, yb - g.size().y / 2.0), g, p.text_faint);
                x += w + 16.0;
            }
        }
    }

    /// No connections at all: three ways to begin.
    fn welcome_page(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let area = ui.max_rect();
        let ssh_hosts = *self.ssh_config_hosts.get_or_insert_with(|| {
            crate::bookmarks::ssh_config::parse_file(&crate::paths::home_dir().join(".ssh/config"))
                .map(|h| h.len())
                .unwrap_or(0)
        });
        let block_h = 56.0 + 12.0 + 30.0 + 22.0 + 26.0 + 168.0 + 22.0 + 16.0;
        let mut y = area.center().y - block_h / 2.0 - 20.0;
        let logo = egui::Rect::from_center_size(pos2(area.center().x, y + 28.0), vec2(56.0, 56.0));
        icons::paint_logo(ui.painter(), logo);
        y += 56.0 + 18.0;
        let painter = ui.painter();
        painter.text(
            pos2(area.center().x, y),
            Align2::CENTER_TOP,
            t("Welcome to TernaryShell"),
            style::semibold(22.0),
            p.text,
        );
        y += 32.0;
        painter.text(
            pos2(area.center().x, y),
            Align2::CENTER_TOP,
            t("A small, fast SSH and SFTP client. Let's add your first server."),
            style::regular(style::BODY),
            p.text_muted,
        );
        y += 22.0 + 26.0;
        let mut cards: Vec<(&str, &str, String, String, kit::Kind, Action)> = vec![(
            icons::PLUS,
            t("Add a server"),
            t("Host, user and how to log in.").to_string(),
            t("New connection").to_string(),
            kit::Kind::Primary,
            Action::NewBookmark { group: None },
        )];
        if ssh_hosts > 0 {
            cards.push((
                icons::FILE_CODE,
                t("From ~/.ssh/config"),
                tf(
                    "{} hosts found on {}.",
                    &[&ssh_hosts.to_string(), crate::platform::this_computer()],
                ),
                tf("Import {} hosts", &[&ssh_hosts.to_string()]),
                kit::Kind::Secondary,
                Action::ImportSshConfig,
            ));
        }
        cards.push((
            icons::DOWNLOAD_SIMPLE,
            t("From a file"),
            t("Electerm or a TernaryShell export.").to_string(),
            t("Choose file…").to_string(),
            kit::Kind::Secondary,
            Action::Import,
        ));
        let w = 200.0;
        let total = cards.len() as f32 * w + (cards.len() - 1) as f32 * 12.0;
        let mut x = area.center().x - total / 2.0;
        for (icon, title, desc, button, kind, action) in cards {
            let rect = egui::Rect::from_min_size(pos2(x, y), vec2(w, 168.0));
            x += w + 12.0;
            let painter = ui.painter();
            painter.rect(
                rect,
                CornerRadius::same(style::RADIUS_LG),
                p.card,
                Stroke::new(1.0, p.border),
                StrokeKind::Inside,
            );
            let tile = egui::Rect::from_min_size(rect.min + vec2(16.0, 16.0), vec2(36.0, 36.0));
            kit::paint_icon_tile(painter, tile, icon, Tone::Neutral, &p);
            painter.text(
                pos2(rect.left() + 16.0, rect.top() + 16.0 + 36.0 + 12.0),
                Align2::LEFT_TOP,
                title,
                style::semibold(style::BODY),
                p.text,
            );
            let g = painter.layout(desc, style::regular(style::SMALL), p.text_muted, w - 32.0);
            painter.galley(
                pos2(rect.left() + 16.0, rect.top() + 16.0 + 36.0 + 12.0 + 22.0),
                g,
                p.text_muted,
            );
            let mut b = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        pos2(rect.left() + 16.0, rect.bottom() - 16.0 - 26.0),
                        pos2(rect.right() - 16.0, rect.bottom() - 16.0),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if kit::button_ex(&mut b, kind, None, &button, true).clicked() {
                self.actions.push(action);
            }
        }
        y += 168.0 + 22.0;
        ui.painter().text(
            pos2(area.center().x, y),
            Align2::CENTER_TOP,
            tf(
                "Passwords and keys are kept in {}, never in TernaryShell's files.",
                &[crate::platform::keychain()],
            ),
            style::regular(style::SMALL),
            p.text_faint,
        );
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

/// The SFTP toggle of the toolbar (26 px): its icon, or a progress ring while files move, then
/// "SFTP" and the percentage. Accent-tinted while the panel is open.
fn sftp_toggle(ui: &mut Ui, on: bool, fraction: Option<f32>, percent: &str) -> egui::Response {
    let p = style::pal(ui);
    let label = ui.painter().layout_no_wrap(
        "SFTP".into(),
        style::medium(style::SMALL),
        Color32::PLACEHOLDER,
    );
    let pct = (!percent.is_empty()).then(|| {
        ui.painter().layout_no_wrap(
            percent.to_string(),
            style::medium(style::SMALL),
            Color32::PLACEHOLDER,
        )
    });
    let w =
        9.0 + 14.0 + 5.0 + label.size().x + pct.as_ref().map_or(0.0, |g| 4.0 + g.size().x) + 9.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, style::TOOLBAR_H), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter();
    let fill = if on {
        p.accent_tint
    } else if hovered {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(style::RADIUS), fill);
    let color = if on {
        p.accent_text
    } else if hovered {
        p.text
    } else {
        p.text_muted
    };
    let c = pos2(rect.left() + 9.0 + 7.0, rect.center().y);
    match fraction {
        Some(f) => style::progress_ring(painter, c, 7.0, f, ui.visuals().dark_mode),
        None => {
            painter.text(
                c,
                Align2::CENTER_CENTER,
                icons::ARROWS_DOWN_UP,
                style::icon_font(14.0),
                color,
            );
        }
    }
    let mut x = rect.left() + 9.0 + 14.0 + 5.0;
    let lw = label.size().x;
    painter.galley(
        pos2(x, rect.center().y - label.size().y / 2.0),
        label,
        color,
    );
    x += lw + 4.0;
    if let Some(g) = pct {
        painter.galley(
            pos2(x, rect.center().y - g.size().y / 2.0),
            g,
            color.gamma_multiply(0.8),
        );
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, "SFTP"));
    resp
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
            "Listening on port {} of the server",
            &[&f.listen_port.to_string()],
        ),
        ForwardKind::Dynamic => tf("SOCKS proxy on 127.0.0.1:{}", &[&f.listen_port.to_string()]),
    }
}

/// A link in the status strip: an optional icon and a label, a quiet fill when hovered.
fn status_link(
    ui: &mut Ui,
    icon: Option<&str>,
    text: &str,
    color: Color32,
    ring: bool,
) -> egui::Response {
    let p = style::pal(ui);
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), style::regular(style::XS), color);
    let lead = if icon.is_some() || !ring {
        if icon.is_some() { 19.0 } else { 18.0 }
    } else {
        0.0
    };
    let w = g.size().x + lead + 12.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 20.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.hover);
    }
    let mut x = rect.left() + 6.0;
    if let Some(i) = icon {
        ui.painter().text(
            pos2(x + 6.5, rect.center().y),
            Align2::CENTER_CENTER,
            i,
            style::icon_font(13.0),
            color,
        );
        x += 19.0;
    } else {
        x += 18.0;
    }
    ui.painter()
        .galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    resp
}

/// One quick command in the ⚡ popover: name over a mono preview, "paste only" when it isn't
/// run, and its ⌃n shortcut.
fn quick_row(ui: &mut Ui, index: usize, qc: &crate::settings::QuickCommand) -> egui::Response {
    let p = style::pal(ui);
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 40.0), Sense::click());
    let hl = resp.hovered();
    let painter = ui.painter();
    if hl {
        painter.rect_filled(rect, CornerRadius::same(5), p.accent);
    }
    let fg = if hl { p.on_accent } else { p.text };
    let sub = if hl {
        p.on_accent.gamma_multiply(0.85)
    } else {
        p.text_faint
    };
    let icon = if qc.run {
        icons::TERMINAL
    } else {
        icons::CLIPBOARD
    };
    painter.text(
        pos2(rect.left() + 16.0, rect.top() + 13.0),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(15.0),
        if hl { p.on_accent } else { p.text_muted },
    );
    let name = if qc.name.trim().is_empty() {
        qc.command.clone()
    } else {
        qc.name.clone()
    };
    let x = rect.left() + 33.0;
    let name_g = painter.layout_no_wrap(name, style::regular(style::BODY), fg);
    let name_w = name_g.size().x;
    painter.galley(pos2(x, rect.top() + 6.0), name_g, fg);
    if !qc.run {
        let g = painter.layout_no_wrap(
            t("paste only").to_string(),
            style::medium(10.0),
            if hl { p.on_accent } else { p.text_muted },
        );
        let b = egui::Rect::from_min_size(
            pos2(x + name_w + 6.0, rect.top() + 6.0),
            vec2(g.size().x + 12.0, 16.0),
        );
        painter.rect_filled(
            b,
            CornerRadius::same(8),
            if hl {
                Color32::from_white_alpha(40)
            } else {
                p.hover
            },
        );
        painter.galley(
            b.center() - g.size() / 2.0,
            g,
            if hl { p.on_accent } else { p.text_muted },
        );
    }
    let sc = if cfg!(target_os = "macos") && index < 9 {
        Some(format!("⌃{}", index + 1))
    } else {
        None
    };
    let right = if let Some(sc) = &sc {
        let g = painter.layout_no_wrap(sc.clone(), style::regular(style::SMALL), sub);
        let w = g.size().x;
        painter.galley(pos2(rect.right() - 8.0 - w, rect.top() + 6.0), g, sub);
        rect.right() - 16.0 - w
    } else {
        rect.right() - 8.0
    };
    let clip = egui::Rect::from_min_max(rect.min, pos2(right, rect.bottom()));
    painter.with_clip_rect(clip).text(
        pos2(x, rect.top() + 24.0),
        Align2::LEFT_TOP,
        qc.command.replace('\n', " ⏎ "),
        style::mono(11.0),
        sub,
    );
    resp.on_hover_text(RichText::new(&qc.command).monospace())
}

fn text_button_w(ui: &Ui, text: &str, small: bool) -> f32 {
    let size = if small { style::SMALL } else { style::BODY };
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), style::medium(size), Color32::WHITE);
    g.size().x + 2.0 * if small { 9.0 } else { 12.0 } + 14.0 + 5.0
}

fn blend_hover(base: Color32, p: &style::Palette) -> Color32 {
    let a = p.hover.a() as f32 / 255.0;
    let [tr, tg, tb, _] = p.hover.to_srgba_unmultiplied();
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    Color32::from_rgb(mix(base.r(), tr), mix(base.g(), tg), mix(base.b(), tb))
}
