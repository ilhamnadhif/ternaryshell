//! Sidebar: the filter, the connection tree (nested groups, drag and drop) and the footer.
//! The top row (traffic lights, hide button, ☰) is drawn by the window frame.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Key, LayerId, Order, RichText, Sense, Stroke, StrokeKind,
    Ui, pos2, vec2,
};
use uuid::Uuid;

use super::actions::Action;
use super::keys::TreeKey;
use super::style::{self, Tone};
use super::{icons, kit};
use crate::bookmarks::transfer::ExportScope;
use crate::bookmarks::{Auth, BookmarkStore, search};
use crate::i18n::{t, tf};

const ROW_H: f32 = 28.0;
const ROW_H2: f32 = 42.0;
const INDENT: f32 = 16.0;
/// How long the pointer rests on a connection before its card opens.
const HOVER_CARD_AFTER: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DragItem {
    Bookmark(Uuid),
    Group(Uuid),
}

/// A reorder/move requested by drag and drop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeMove {
    Bookmark {
        id: Uuid,
        group: Option<Uuid>,
        before: Option<Uuid>,
    },
    Group {
        id: Uuid,
        parent: Option<Uuid>,
        before: Option<Uuid>,
    },
}

/// The tree's keyboard focus (see `Sidebar::tree_keys`).
fn tree_keys_id() -> egui::Id {
    egui::Id::new("sidebar-tree-keys")
}

#[derive(Default)]
pub struct Sidebar {
    pub search: String,
    pub selected: Option<Uuid>,
    pub focus_search: bool,
    /// Groups opened in this run: every group starts closed when TernaryShell opens.
    expanded: HashSet<Uuid>,
    search_index: usize,
    /// The connection under the pointer and since when (for its hover card).
    hover: Option<(Uuid, Instant)>,
}

/// Connection state per bookmark, for the dot on its icon.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Live {
    Connected,
    Connecting,
    Failed,
}

struct Ctx<'a> {
    store: &'a BookmarkStore,
    live: &'a HashMap<Uuid, Live>,
    open: &'a HashMap<Uuid, usize>,
    actions: Vec<Action>,
    moved: Option<TreeMove>,
    /// The row hovered this frame and its rect (for the hover card).
    hovered: Option<(Uuid, egui::Rect)>,
    /// The sidebar's right edge (the hover card opens beside it).
    right: f32,
}

impl Sidebar {
    /// Draws everything under the top row: filter, section header, tree, footer.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        store: &BookmarkStore,
        live: &HashMap<Uuid, Live>,
        open: &HashMap<Uuid, usize>,
        input_enabled: bool,
    ) -> (Vec<Action>, Option<TreeMove>) {
        let mut cx = Ctx {
            store,
            live,
            open,
            actions: Vec::new(),
            moved: None,
            hovered: None,
            right: ui.max_rect().right(),
        };
        let p = style::pal(ui);
        let full = ui.available_rect_before_wrap();

        // Footer first (bottom), so the tree knows how much room it has.
        let foot_h = 8.0 + style::TOOLBAR_H + 10.0;
        let foot = egui::Rect::from_min_max(pos2(full.left(), full.bottom() - foot_h), full.max);
        let mut foot_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(foot.left() + 10.0, foot.top() + 8.0),
                    pos2(foot.right() - 10.0, foot.bottom() - 10.0),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        foot_ui.spacing_mut().item_spacing.x = 4.0;
        let new = kit::ghost(&mut foot_ui, Some(icons::PLUS), t("New connection"));
        #[cfg(debug_assertions)]
        crate::devtest::record("sidebar-new", new.rect);
        if new.clicked() {
            cx.actions.push(Action::NewBookmark {
                group: self.selected_group(store),
            });
        }
        foot_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let settings = kit::icon_button(
                ui,
                icons::GEAR_SIX,
                &format!(
                    "{} ({})",
                    t("Settings"),
                    super::keys::Shortcut::Settings.label()
                ),
            );
            if settings.clicked() {
                cx.actions.push(Action::Settings);
            }
        });

        let mut main = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
            full.min,
            pos2(full.right(), foot.top()),
        )));
        let ui = &mut main;
        // The filter: 2 px under the top row, 10 px from the sides, 8 px above the tree.
        ui.add_space(2.0);
        let search_id = egui::Id::new("sidebar-search");
        let mut field = None;
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: 10,
                right: 10,
                top: 0,
                bottom: 8,
            })
            .show(ui, |ui| {
                let keys = cfg!(target_os = "macos").then_some("⌘F");
                field = Some(kit::quiet_search(
                    ui,
                    search_id,
                    &mut self.search,
                    t("Filter"),
                    keys,
                ));
            });
        let resp = field.unwrap();
        if std::mem::take(&mut self.focus_search) {
            resp.request_focus();
        }
        #[cfg(debug_assertions)]
        crate::devtest::record("sidebar-search", resp.rect);
        if resp.changed() {
            self.search_index = 0;
        }
        if resp.has_focus() {
            let (down, up) =
                ui.input(|i| (i.key_pressed(Key::ArrowDown), i.key_pressed(Key::ArrowUp)));
            if down {
                self.search_index += 1;
            }
            if up {
                self.search_index = self.search_index.saturating_sub(1);
            }
        }
        if resp.lost_focus()
            && ui.input(|i| i.key_pressed(Key::Enter))
            && !self.search.trim().is_empty()
        {
            let hits = search::search(store, &self.search);
            if let Some(id) = hits.get(self.search_index.min(hits.len().saturating_sub(1))) {
                cx.actions.push(Action::Connect(*id));
            }
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.search.clear();
        }

        let searching = !self.search.trim().is_empty();
        let empty = store.bookmarks.is_empty() && store.groups.is_empty();
        if empty && !searching {
            ui.allocate_ui(ui.available_size(), |ui| {
                ui.add_space((ui.available_height() / 2.0 - 90.0).max(12.0));
                kit::empty_state(
                    ui,
                    icons::HARD_DRIVES,
                    t("No connections yet"),
                    t("Servers you add show up here, grouped the way you like."),
                    |_| {},
                );
            });
            return (cx.actions, cx.moved);
        }

        // Section header: CONNECTIONS 9 ⋯, or the number of matches while filtering.
        ui.add_space(4.0);
        let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        let mut head_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    pos2(head.left() + 18.0, head.top()),
                    pos2(head.right() - 8.0, head.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        head_ui.spacing_mut().item_spacing.x = 6.0;
        let hits = searching.then(|| search::search(store, &self.search));
        match &hits {
            Some(h) => {
                head_ui.label(style::section_label(
                    &tf("{} matches", &[&h.len().to_string()]),
                    p.text_faint,
                ));
                head_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(t("↑↓ · ↩ connect"))
                            .font(style::regular(style::XS))
                            .color(p.text_faint),
                    );
                });
            }
            None => {
                head_ui.label(style::section_label(t("Connections"), p.text_faint));
                kit::count(&mut head_ui, store.bookmarks.len());
                head_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let more = kit::icon_button_ex(ui, icons::DOTS_THREE, t("More"), false, 22.0);
                    egui::Popup::menu(&more).show(|ui| {
                        kit::menu_body(ui);
                        ui.set_min_width(250.0);
                        if kit::menu_item(ui, Some(icons::FOLDER_PLUS), t("New Group"), None)
                            .clicked()
                        {
                            cx.actions.push(Action::NewGroup { parent: None });
                            ui.close();
                        }
                        kit::menu_sep(ui);
                        if kit::menu_item(
                            ui,
                            Some(icons::DOWNLOAD_SIMPLE),
                            t("Import Connections…"),
                            None,
                        )
                        .clicked()
                        {
                            cx.actions.push(Action::Import);
                            ui.close();
                        }
                        if kit::menu_item(
                            ui,
                            Some(icons::FILE_CODE),
                            t("Import from ~/.ssh/config"),
                            None,
                        )
                        .clicked()
                        {
                            cx.actions.push(Action::ImportSshConfig);
                            ui.close();
                        }
                        let export = super::keys::Shortcut::Export.label();
                        if kit::menu_item(
                            ui,
                            Some(icons::EXPORT),
                            t("Export Connections…"),
                            Some(&export),
                        )
                        .clicked()
                        {
                            cx.actions.push(Action::Export(ExportScope::All));
                            ui.close();
                        }
                        kit::menu_sep(ui);
                        if kit::menu_item(ui, Some(icons::CARET_UP), t("Collapse All Groups"), None)
                            .clicked()
                        {
                            cx.actions.push(Action::CollapseAllGroups);
                            ui.close();
                        }
                    });
                });
            }
        }

        // A click in the tree gives it the keyboard: ↩ ⌘E F2 ⌫ act on the selected item, Esc
        // gives the keyboard back to the terminal.
        let tree_bg = ui.interact(
            ui.available_rect_before_wrap(),
            tree_keys_id(),
            Sense::click(),
        );
        if tree_bg.clicked() || tree_bg.secondary_clicked() {
            ui.memory_mut(|m| m.request_focus(tree_keys_id()));
        }
        egui::ScrollArea::vertical()
            .id_salt("sidebar-tree")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(8, 0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        match &hits {
                            None => {
                                self.tree(ui, &mut cx, None, 0);
                                // Empty space at the bottom: drop here to move to the top level.
                                let (rect, resp) = ui.allocate_exact_size(
                                    vec2(
                                        ui.available_width(),
                                        ui.available_height().max(ROW_H * 2.0),
                                    ),
                                    Sense::click(),
                                );
                                if resp.dnd_hover_payload::<DragItem>().is_some() {
                                    ui.painter().rect_stroke(
                                        rect.shrink(2.0),
                                        CornerRadius::same(style::RADIUS),
                                        Stroke::new(1.5, p.accent),
                                        StrokeKind::Inside,
                                    );
                                }
                                if let Some(item) = resp.dnd_release_payload::<DragItem>() {
                                    cx.moved = Some(match *item {
                                        DragItem::Bookmark(id) => TreeMove::Bookmark {
                                            id,
                                            group: None,
                                            before: None,
                                        },
                                        DragItem::Group(id) => TreeMove::Group {
                                            id,
                                            parent: None,
                                            before: None,
                                        },
                                    });
                                }
                                if resp.clicked() {
                                    self.selected = None;
                                }
                                resp.context_menu(|ui| {
                                    kit::menu_body(ui);
                                    ui.set_min_width(220.0);
                                    if kit::menu_item(
                                        ui,
                                        Some(icons::HARD_DRIVES),
                                        t("New Connection…"),
                                        None,
                                    )
                                    .clicked()
                                    {
                                        cx.actions.push(Action::NewBookmark { group: None });
                                        ui.close();
                                    }
                                    if kit::menu_item(
                                        ui,
                                        Some(icons::FOLDER_PLUS),
                                        t("New Group"),
                                        None,
                                    )
                                    .clicked()
                                    {
                                        cx.actions.push(Action::NewGroup { parent: None });
                                        ui.close();
                                    }
                                });
                            }
                            Some(hits) => {
                                if hits.is_empty() {
                                    ui.add_space(16.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(RichText::new(t("No matches")).color(p.text_muted))
                                    });
                                }
                                self.search_index =
                                    self.search_index.min(hits.len().saturating_sub(1));
                                let query = self.search.clone();
                                for (i, id) in hits.iter().enumerate() {
                                    let Some(b) = store.bookmark(*id) else {
                                        continue;
                                    };
                                    let path = store.group_path(b.group);
                                    let sub = if path.is_empty() {
                                        b.address()
                                    } else {
                                        format!("{} · {}", b.address(), path)
                                    };
                                    let highlighted = i == self.search_index;
                                    self.bookmark_row(
                                        ui,
                                        &mut cx,
                                        *id,
                                        0,
                                        Some((sub, query.as_str())),
                                        highlighted,
                                    );
                                }
                            }
                        }
                    });
            });

        // The hover card of the connection under the pointer.
        match cx.hovered {
            Some((id, rect)) => {
                let since = match self.hover {
                    Some((h, at)) if h == id => at,
                    _ => {
                        self.hover = Some((id, Instant::now()));
                        Instant::now()
                    }
                };
                let dragging = egui::DragAndDrop::has_any_payload(ui.ctx());
                if since.elapsed() >= HOVER_CARD_AFTER && !dragging {
                    hover_card(ui.ctx(), store, id, rect, cx.right);
                } else {
                    ui.ctx()
                        .request_repaint_after(HOVER_CARD_AFTER.saturating_sub(since.elapsed()));
                }
            }
            None => self.hover = None,
        }
        if ui.memory(|m| m.has_focus(tree_keys_id())) {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    tree_keys_id(),
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
            if input_enabled {
                self.tree_keys(ui, &mut cx);
            }
        }
        (cx.actions, cx.moved)
    }

    /// The group the selected item belongs to (for "new connection here").
    fn selected_group(&self, store: &BookmarkStore) -> Option<Uuid> {
        let id = self.selected?;
        if store.group(id).is_some() {
            return Some(id);
        }
        store.bookmark(id).and_then(|b| b.group)
    }

    /// Closes every group.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    fn tree(&mut self, ui: &mut Ui, cx: &mut Ctx, parent: Option<Uuid>, depth: usize) {
        let groups: Vec<Uuid> = cx.store.child_groups(parent).map(|g| g.id).collect();
        for gid in groups {
            self.group_row(ui, cx, gid, depth);
            if self.expanded.contains(&gid) {
                self.tree(ui, cx, Some(gid), depth + 1);
            }
        }
        let bookmarks: Vec<Uuid> = cx.store.bookmarks_in(parent).map(|b| b.id).collect();
        for id in bookmarks {
            self.bookmark_row(ui, cx, id, depth, None, false);
        }
    }

    fn row_frame(
        &self,
        ui: &mut Ui,
        id: egui::Id,
        selected: bool,
        height: f32,
    ) -> (egui::Rect, egui::Response) {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let p = style::pal(ui);
        let fill = if selected {
            Some(p.accent_soft)
        } else if resp.hovered() {
            Some(p.hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(style::RADIUS), fill);
        }
        (rect, resp)
    }

    fn drag_preview(ui: &Ui, resp: &egui::Response, icon: &str, label: &str) {
        if resp.dragged()
            && let Some(pos) = ui.ctx().pointer_interact_pos()
        {
            let painter = ui
                .ctx()
                .layer_painter(LayerId::new(Order::Tooltip, egui::Id::new("sidebar-drag")));
            let pal = style::pal(ui);
            let text = format!("{icon}  {label}");
            let galley = painter.layout_no_wrap(text, style::regular(style::SMALL), pal.text);
            let r =
                egui::Rect::from_min_size(pos + vec2(12.0, 4.0), galley.size() + vec2(16.0, 10.0));
            painter.add(
                ui.visuals()
                    .popup_shadow
                    .as_shape(r, CornerRadius::same(style::RADIUS)),
            );
            painter.rect(
                r,
                CornerRadius::same(style::RADIUS),
                pal.raised,
                ui.visuals().window_stroke,
                StrokeKind::Inside,
            );
            painter.galley(r.min + vec2(8.0, 5.0), galley, pal.text);
        }
    }

    fn group_row(&mut self, ui: &mut Ui, cx: &mut Ctx, gid: Uuid, depth: usize) {
        let Some(group) = cx.store.group(gid) else {
            return;
        };
        let selected = self.selected == Some(gid);
        let (rect, resp) = self.row_frame(ui, egui::Id::new(("group-row", gid)), selected, ROW_H);
        #[cfg(debug_assertions)]
        crate::devtest::record(format!("group:{}", group.name), rect);
        let expanded = self.expanded.contains(&gid);
        let p = style::pal(ui);
        let x = rect.left() + 8.0 + depth as f32 * INDENT;
        let cy = rect.center().y;
        let painter = ui.painter();
        painter.text(
            pos2(x + 6.0, cy),
            Align2::CENTER_CENTER,
            if expanded {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            },
            style::icon_font(11.0),
            p.text_faint,
        );
        painter.text(
            pos2(x + 12.0 + 7.0 + 7.5, cy),
            Align2::CENTER_CENTER,
            if expanded {
                icons::FOLDER_OPEN
            } else {
                icons::FOLDER_SIMPLE
            },
            style::icon_font(15.0),
            p.folder,
        );
        let count = cx.store.bookmarks_in(Some(gid)).count();
        let name_x = x + 12.0 + 7.0 + 15.0 + 7.0;
        let count_g =
            painter.layout_no_wrap(count.to_string(), style::medium(style::MICRO), p.text_faint);
        let pill = egui::Rect::from_min_size(
            pos2(rect.right() - 8.0 - (count_g.size().x + 12.0), cy - 8.0),
            vec2(count_g.size().x + 12.0, 16.0),
        );
        let name_font = if selected {
            style::medium(style::BODY)
        } else {
            style::regular(style::BODY)
        };
        let name_g = painter.layout_no_wrap(group.name.clone(), name_font, p.text);
        painter
            .with_clip_rect(egui::Rect::from_min_max(
                pos2(name_x, rect.top()),
                pos2(pill.left() - 6.0, rect.bottom()),
            ))
            .galley(pos2(name_x, cy - name_g.size().y / 2.0), name_g, p.text);
        kit::paint_count(painter, pill, count_g, &p);
        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
            ui.memory_mut(|m| m.request_focus(tree_keys_id()));
        }
        if resp.clicked() {
            self.selected = Some(gid);
            if !self.expanded.remove(&gid) {
                self.expanded.insert(gid);
            }
        }
        resp.dnd_set_drag_payload(DragItem::Group(gid));
        Self::drag_preview(ui, &resp, icons::FOLDER_SIMPLE, &group.name);
        if let (Some(item), Some(ptr)) = (
            resp.dnd_hover_payload::<DragItem>(),
            ui.ctx().pointer_interact_pos(),
        ) {
            let into = matches!(*item, DragItem::Bookmark(_)) || ptr.y > rect.center().y;
            if into {
                ui.painter()
                    .rect_filled(rect, CornerRadius::same(style::RADIUS), p.accent_softer);
                ui.painter().rect_stroke(
                    rect,
                    CornerRadius::same(style::RADIUS),
                    Stroke::new(1.5, p.accent),
                    StrokeKind::Inside,
                );
            } else {
                drop_line(ui, rect.left(), rect.right(), rect.top(), &p);
            }
            if let Some(item) = resp.dnd_release_payload::<DragItem>() {
                cx.moved = Some(match *item {
                    DragItem::Bookmark(id) => TreeMove::Bookmark {
                        id,
                        group: Some(gid),
                        before: None,
                    },
                    DragItem::Group(id) if into => TreeMove::Group {
                        id,
                        parent: Some(gid),
                        before: None,
                    },
                    DragItem::Group(id) => TreeMove::Group {
                        id,
                        parent: group.parent,
                        before: Some(gid),
                    },
                });
                if into {
                    self.expanded.insert(gid);
                }
            }
        }
        resp.context_menu(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(230.0);
            if kit::menu_item(ui, Some(icons::PLUS), t("New Connection Here"), None).clicked() {
                cx.actions.push(Action::NewBookmark { group: Some(gid) });
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::FOLDER_PLUS), t("New Subgroup"), None).clicked() {
                cx.actions.push(Action::NewGroup { parent: Some(gid) });
                ui.close();
            }
            kit::menu_sep(ui);
            if kit::menu_item(
                ui,
                Some(icons::CURSOR_TEXT),
                t("Rename…"),
                Some(TreeKey::Rename.label()),
            )
            .clicked()
            {
                cx.actions.push(Action::RenameGroup(gid));
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::EXPORT), t("Export Group…"), None).clicked() {
                cx.actions.push(Action::Export(ExportScope::Group(gid)));
                ui.close();
            }
            kit::menu_sep(ui);
            if kit::menu_item_ex(
                ui,
                Some(icons::TRASH),
                t("Delete Group…"),
                Some(TreeKey::Delete.label()),
                true,
                false,
            )
            .clicked()
            {
                cx.actions.push(Action::DeleteGroup(gid));
                ui.close();
            }
        });
    }

    /// A connection: its icon (with the session's state as a dot on it), the name, and on hover
    /// → Connect and ⋯. With `search`, a second line (address · group) and the matched letters.
    fn bookmark_row(
        &mut self,
        ui: &mut Ui,
        cx: &mut Ctx,
        id: Uuid,
        depth: usize,
        search: Option<(String, &str)>,
        highlighted: bool,
    ) {
        let Some(b) = cx.store.bookmark(id) else {
            return;
        };
        let selected = self.selected == Some(id) || highlighted;
        let height = if search.is_some() { ROW_H2 } else { ROW_H };
        let (rect, resp) =
            self.row_frame(ui, egui::Id::new(("bookmark-row", id)), selected, height);
        let p = style::pal(ui);
        let depth = if search.is_some() { 0 } else { depth };
        let x = rect.left() + 8.0 + depth as f32 * INDENT;
        let name_y = if search.is_some() {
            rect.top() + 13.0
        } else {
            rect.center().y
        };
        let live = cx.live.get(&id).copied();
        let hovered =
            ui.rect_contains_pointer(rect) && !egui::DragAndDrop::has_any_payload(ui.ctx());
        let painter = ui.painter();
        let icon_c = pos2(
            x + 12.0 + 7.0 + 7.5,
            if search.is_some() {
                rect.center().y
            } else {
                name_y
            },
        );
        painter.text(
            icon_c,
            Align2::CENTER_CENTER,
            icons::HARD_DRIVES,
            style::icon_font(15.0),
            p.text_faint,
        );
        if let Some(l) = live {
            let color = match l {
                Live::Connected => p.ok,
                Live::Connecting => p.warn,
                Live::Failed => p.err,
            };
            let c = icon_c + vec2(7.0, 4.5);
            let ring = if selected {
                blend(p.chrome, p.accent_soft)
            } else if hovered {
                blend(p.chrome, p.hover)
            } else {
                p.chrome
            };
            painter.circle_filled(c, 5.0, ring);
            painter.circle_filled(c, 3.5, color);
        }
        // Right side: the hover actions, or how many tabs are open on it.
        let mut right = rect.right() - 8.0;
        let mut connect_click = false;
        let mut more_resp = None;
        if hovered {
            let more_r =
                egui::Rect::from_min_size(pos2(right - 18.0, name_y - 9.0), vec2(18.0, 18.0));
            let go_r = egui::Rect::from_min_size(
                pos2(more_r.left() - 2.0 - 18.0, name_y - 9.0),
                vec2(18.0, 18.0),
            );
            let go = ui
                .interact(go_r, egui::Id::new(("row-go", id)), Sense::click())
                .on_hover_text(t("Connect"));
            kit::paint_icon_button(ui, &go, go_r, icons::ARROW_RIGHT, t("Connect"), false, true);
            let more = ui
                .interact(more_r, egui::Id::new(("row-more", id)), Sense::click())
                .on_hover_text(t("More"));
            kit::paint_icon_button(
                ui,
                &more,
                more_r,
                icons::DOTS_THREE,
                t("More"),
                false,
                false,
            );
            connect_click = go.clicked();
            more_resp = Some(more);
            right = go_r.left() - 4.0;
        } else if let Some(n) = cx.open.get(&id).copied().filter(|n| *n >= 2) {
            let painter = ui.painter();
            let g =
                painter.layout_no_wrap(n.to_string(), style::regular(style::MICRO), p.accent_text);
            let pill = egui::Rect::from_min_size(
                pos2(right - g.size().x - 12.0, name_y - 8.0),
                vec2(g.size().x + 12.0, 16.0),
            );
            painter.rect_filled(pill, CornerRadius::same(8), p.accent_tint);
            painter.galley(pill.center() - g.size() / 2.0, g, p.accent_text);
            right = pill.left() - 6.0;
        }
        let name = b.display_name();
        let name_x = x + 12.0 + 7.0 + 15.0 + 7.0;
        let painter = ui.painter();
        let clip = egui::Rect::from_min_max(pos2(name_x, rect.top()), pos2(right, rect.bottom()));
        let font = if selected {
            style::medium(style::BODY)
        } else {
            style::regular(style::BODY)
        };
        match &search {
            Some((sub, query)) => {
                let hits = search::match_positions(query, &name);
                let mut job = egui::text::LayoutJob::default();
                for (i, c) in name.chars().enumerate() {
                    let hit = hits.contains(&i);
                    job.append(
                        &c.to_string(),
                        0.0,
                        egui::TextFormat {
                            font_id: if hit {
                                style::semibold(style::BODY)
                            } else {
                                font.clone()
                            },
                            color: if hit { p.accent_text } else { p.text },
                            ..Default::default()
                        },
                    );
                }
                let g = painter.layout_job(job);
                painter.with_clip_rect(clip).galley(
                    pos2(name_x, name_y - g.size().y / 2.0),
                    g,
                    p.text,
                );
                painter.with_clip_rect(clip).text(
                    pos2(name_x, rect.top() + 23.0),
                    Align2::LEFT_TOP,
                    sub,
                    style::mono(style::XS),
                    p.text_faint,
                );
            }
            None => {
                painter.with_clip_rect(clip).text(
                    pos2(name_x, name_y),
                    Align2::LEFT_CENTER,
                    &name,
                    font,
                    p.text,
                );
            }
        }
        #[cfg(debug_assertions)]
        crate::devtest::record(format!("bookmark:{name}"), rect);
        if hovered && search.is_none() {
            cx.hovered = Some((id, rect));
        }
        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
            ui.memory_mut(|m| m.request_focus(tree_keys_id()));
        }
        if connect_click || resp.double_clicked() {
            cx.actions.push(Action::Connect(id));
        } else if resp.clicked() {
            self.selected = Some(id);
        }
        resp.dnd_set_drag_payload(DragItem::Bookmark(id));
        Self::drag_preview(ui, &resp, icons::HARD_DRIVES, &name);
        if search.is_none()
            && let (Some(item), Some(ptr)) = (
                resp.dnd_hover_payload::<DragItem>(),
                ui.ctx().pointer_interact_pos(),
            )
        {
            let before = ptr.y < rect.center().y;
            if matches!(*item, DragItem::Bookmark(_)) {
                let y = if before { rect.top() } else { rect.bottom() };
                drop_line(ui, rect.left(), rect.right(), y, &p);
            }
            if let Some(item) = resp.dnd_release_payload::<DragItem>() {
                // "After this row" = before the next row that isn't the dragged one itself.
                let after = |dragged: Uuid| {
                    let ids: Vec<Uuid> = cx.store.bookmarks_in(b.group).map(|x| x.id).collect();
                    let pos = ids.iter().position(|x| *x == id).unwrap_or(0);
                    ids[pos + 1..].iter().copied().find(|x| *x != dragged)
                };
                cx.moved = Some(match *item {
                    DragItem::Bookmark(dragged) => TreeMove::Bookmark {
                        id: dragged,
                        group: b.group,
                        before: if before { Some(id) } else { after(dragged) },
                    },
                    DragItem::Group(g) => TreeMove::Group {
                        id: g,
                        parent: b.group,
                        before: None,
                    },
                });
            }
        }
        let address = b.address();
        let menu = |ui: &mut Ui, actions: &mut Vec<Action>| {
            kit::menu_body(ui);
            ui.set_min_width(230.0);
            if kit::menu_item(
                ui,
                Some(icons::PLUGS_CONNECTED),
                t("Connect"),
                Some(TreeKey::Open.label()),
            )
            .clicked()
            {
                actions.push(Action::Connect(id));
                ui.close();
            }
            kit::menu_sep(ui);
            if kit::menu_item(
                ui,
                Some(icons::PENCIL_SIMPLE),
                t("Edit…"),
                Some(TreeKey::Edit.label()),
            )
            .clicked()
            {
                actions.push(Action::EditBookmark(id));
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::COPY), t("Duplicate"), None).clicked() {
                actions.push(Action::DuplicateBookmark(id));
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::CLIPBOARD), t("Copy Address"), None).clicked() {
                ui.ctx().copy_text(address.clone());
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::EXPORT), t("Export…"), None).clicked() {
                actions.push(Action::Export(ExportScope::Selected(vec![id])));
                ui.close();
            }
            kit::menu_sep(ui);
            if kit::menu_item_ex(
                ui,
                Some(icons::TRASH),
                t("Delete…"),
                Some(TreeKey::Delete.label()),
                true,
                false,
            )
            .clicked()
            {
                actions.push(Action::DeleteBookmark(id));
                ui.close();
            }
        };
        if let Some(more) = &more_resp {
            egui::Popup::menu(more).show(|ui| menu(ui, &mut cx.actions));
        }
        resp.context_menu(|ui| {
            self.selected = Some(id);
            menu(ui, &mut cx.actions);
        });
    }

    /// The keys of the tree while it has the keyboard, for the selected item.
    fn tree_keys(&mut self, ui: &mut Ui, cx: &mut Ctx) {
        let (open, edit, rename, delete, escape) = ui.input_mut(|i| {
            (
                TreeKey::Open.take(i),
                TreeKey::Edit.take(i),
                TreeKey::Rename.take(i),
                TreeKey::Delete.take(i),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            ui.memory_mut(|m| m.surrender_focus(tree_keys_id()));
        }
        let Some(id) = self.selected else {
            return;
        };
        if cx.store.bookmark(id).is_some() {
            if open {
                cx.actions.push(Action::Connect(id));
            } else if edit {
                cx.actions.push(Action::EditBookmark(id));
            } else if delete {
                cx.actions.push(Action::DeleteBookmark(id));
            }
        } else if cx.store.group(id).is_some() {
            if open {
                if !self.expanded.remove(&id) {
                    self.expanded.insert(id);
                }
            } else if edit || rename {
                cx.actions.push(Action::RenameGroup(id));
            } else if delete {
                cx.actions.push(Action::DeleteGroup(id));
            }
        }
    }

    /// Selects and reveals a bookmark or a group (expanding the groups it is in).
    pub fn reveal(&mut self, store: &BookmarkStore, id: Uuid) {
        self.selected = Some(id);
        let mut g = match store.bookmark(id) {
            Some(b) => b.group,
            None => store.group(id).and_then(|x| x.parent),
        };
        while let Some(gid) = g {
            self.expanded.insert(gid);
            g = store.group(gid).and_then(|x| x.parent);
        }
    }

    pub fn status_line(store: &BookmarkStore) -> String {
        tf("{} connections", &[&store.bookmarks.len().to_string()])
    }
}

/// The insertion line of a drag: 2 px of accent with a ring at its start.
fn drop_line(ui: &Ui, left: f32, right: f32, y: f32, p: &style::Palette) {
    let painter = ui.painter();
    painter.hline(left..=right, y, Stroke::new(2.0, p.accent));
    painter.circle(
        pos2(left + 3.0, y),
        3.0,
        p.chrome,
        Stroke::new(1.5, p.accent),
    );
}

fn blend(base: Color32, top: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    let [tr, tg, tb, _] = top.to_srgba_unmultiplied();
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    Color32::from_rgb(mix(base.r(), tr), mix(base.g(), tg), mix(base.b(), tb))
}

/// A connection's details beside the sidebar: address, login, jump host, forwards, tags and
/// notes.
fn hover_card(ctx: &egui::Context, store: &BookmarkStore, id: Uuid, row: egui::Rect, right: f32) {
    let Some(b) = store.bookmark(id) else { return };
    egui::Area::new(egui::Id::new("sidebar-hover-card"))
        .order(Order::Tooltip)
        .fixed_pos(pos2(right - 12.0, row.top() - 8.0))
        .interactable(false)
        .show(ctx, |ui| {
            let p = style::pal(ui);
            kit::popover_frame(ui)
                .inner_margin(egui::Margin::same(14))
                .show(ui, |ui| {
                    ui.set_width(270.0 - 28.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        kit::icon_tile(ui, icons::HARD_DRIVES, Tone::Neutral, 32.0);
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(
                                RichText::new(b.display_name())
                                    .font(style::semibold(style::BODY))
                                    .color(p.text),
                            );
                            ui.label(
                                RichText::new(crate::app::address_with_port(b))
                                    .font(style::mono(style::XS))
                                    .color(p.text_faint),
                            );
                        });
                    });
                    ui.add_space(12.0);
                    let login = match &b.auth {
                        Auth::Password => t("Password").to_string(),
                        Auth::Key { path } => {
                            let file = path.rsplit('/').next().unwrap_or(path);
                            tf("Key file · {}", &[file])
                        }
                        Auth::KeychainKey => t("Pasted key").to_string(),
                        Auth::Agent { .. } => t("SSH agent").to_string(),
                    };
                    let via = store
                        .jump_chain(b)
                        .ok()
                        .and_then(|c| c.last().map(|j| j.display_name()));
                    egui::Grid::new(("hover-kv", id))
                        .num_columns(2)
                        .spacing([8.0, 8.0])
                        .min_col_width(64.0)
                        .show(ui, |ui| {
                            let key = |ui: &mut Ui, text: &str| {
                                ui.label(
                                    RichText::new(text)
                                        .font(style::regular(style::SMALL))
                                        .color(p.text_muted),
                                );
                            };
                            let val = |ui: &mut Ui, text: &str| {
                                ui.label(
                                    RichText::new(text)
                                        .font(style::regular(style::SMALL))
                                        .color(p.text),
                                );
                            };
                            key(ui, t("Login"));
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                if !matches!(b.auth, Auth::Password) {
                                    kit::icon(ui, icons::KEY, 12.0, p.text_muted);
                                }
                                val(ui, &login);
                            });
                            ui.end_row();
                            if let Some(v) = &via {
                                key(ui, t("Via"));
                                val(ui, v);
                                ui.end_row();
                            }
                            if !b.forwards.is_empty() {
                                key(ui, t("Forwards"));
                                ui.vertical(|ui| {
                                    for f in &b.forwards {
                                        ui.label(
                                            RichText::new(crate::app::forward_rule(f))
                                                .font(style::mono(style::XS))
                                                .color(p.text),
                                        );
                                    }
                                });
                                ui.end_row();
                            }
                            if !b.tags.is_empty() {
                                key(ui, t("Tags"));
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                                    for tag in &b.tags {
                                        let g = ui.painter().layout_no_wrap(
                                            tag.clone(),
                                            style::regular(style::XS),
                                            p.text_muted,
                                        );
                                        let (r, _) = ui.allocate_exact_size(
                                            vec2(g.size().x + 12.0, 18.0),
                                            Sense::hover(),
                                        );
                                        ui.painter().rect(
                                            r,
                                            CornerRadius::same(style::RADIUS),
                                            p.hover,
                                            Stroke::new(1.0, p.line_soft),
                                            StrokeKind::Inside,
                                        );
                                        ui.painter().galley(
                                            r.center() - g.size() / 2.0,
                                            g,
                                            p.text_muted,
                                        );
                                    }
                                });
                                ui.end_row();
                            }
                        });
                    if !b.notes.trim().is_empty() {
                        ui.add_space(10.0);
                        let y = ui.cursor().top();
                        ui.painter()
                            .hline(ui.max_rect().x_range(), y, Stroke::new(1.0, p.border));
                        ui.add_space(10.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(b.notes.trim())
                                    .font(style::regular(style::SMALL))
                                    .color(p.text_faint),
                            )
                            .wrap(),
                        );
                    }
                });
        });
}
