//! Sidebar: the filter, the connection tree and the footer. The top row (traffic lights, hide
//! button, ☰) is drawn by the window frame.
//!
//! Top-level groups are sections (their label colour, name and count; open unless closed);
//! groups nested in them are folder rows. Each connection shows its host glyph (the kind of
//! machine, tinted with its environment label, its session state as a dot), its name, and how
//! many tabs are open on it.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Key, LayerId, Order, Rect, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};
use uuid::Uuid;

use super::actions::Action;
use super::keys::TreeKey;
use super::style::{self, Palette};
use super::{icons, kit};
use crate::bookmarks::transfer::ExportScope;
use crate::bookmarks::{Auth, Bookmark, BookmarkStore, Label, LabelColor, search};
use crate::i18n::{t, tf};
use crate::settings::RecentConnection;

/// Indent per level of nesting below a section.
const INDENT: f32 = 16.0;
/// Height of a section header.
const SEC_H: f32 = 28.0;
/// How long the pointer rests on a connection before its card opens.
const HOVER_CARD_AFTER: Duration = Duration::from_millis(500);
/// How long a drag rests on a closed group before it opens.
const EXPAND_AFTER: Duration = Duration::from_millis(600);
/// The Ungrouped section's key in `collapsed`.
const UNGROUPED: Uuid = Uuid::nil();
/// Width of the hover card.
const CARD_W: f32 = 290.0;

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

/// One connection found by the filter: the matched letters of its name, and why it matched
/// (its address, `#tag` or group) with the matched letters there.
struct Hit {
    id: Uuid,
    name: Vec<usize>,
    why: String,
    why_hits: Vec<usize>,
}

/// The filter's results, grouped by section, kept until the query or the connections change.
struct Hits {
    key: u64,
    sections: Vec<(Option<Uuid>, Vec<Hit>)>,
    count: usize,
}

#[derive(Default)]
pub struct Sidebar {
    pub search: String,
    pub selected: Option<Uuid>,
    pub focus_search: bool,
    /// Nested groups opened in this run (they start closed).
    expanded: HashSet<Uuid>,
    /// Sections (top-level groups, and Ungrouped) closed in this run (they start open).
    collapsed: HashSet<Uuid>,
    /// "Collapse All Groups" was chosen: close the sections on the next frame.
    collapse_sections: bool,
    search_index: usize,
    /// The connection under the pointer and since when (for its hover card).
    hover: Option<(Uuid, Instant)>,
    /// The open hover card: whose, and where (it stays while the pointer is on it).
    card: Option<(Uuid, Rect)>,
    /// The closed group a drag rests on, and since when (it opens after a moment).
    drag_over: Option<(Uuid, Instant)>,
    hits: Option<Hits>,
    /// The label text being typed in a group's menu.
    label_edit: Option<(Uuid, String)>,
    /// When saved connections were last opened (the hover card's "Last").
    pub recent: Vec<RecentConnection>,
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
    /// The first section has no room above it.
    first: bool,
}

// ---------------------------------------------------------------------------------------------
// Small painters shared with Home
// ---------------------------------------------------------------------------------------------

/// Width of a key cap (18 px high, 5 px padding, at least 18 wide).
pub fn key_width(painter: &egui::Painter, keys: &str) -> f32 {
    let g = painter.layout_no_wrap(
        keys.to_string(),
        style::medium(style::MICRO),
        Color32::PLACEHOLDER,
    );
    (g.size().x + 10.0).max(18.0)
}

/// A key cap (`⌘F`): 18 px, 4 px corners, on the hover fill with a hairline ring and a firmer
/// bottom edge; `ghost` leaves only the ring. Drawn with its left edge at `left`, centred on
/// `left.y`. Returns its width.
pub fn paint_key(
    painter: &egui::Painter,
    left: egui::Pos2,
    keys: &str,
    p: &Palette,
    ghost: bool,
) -> f32 {
    let color = if ghost { p.text_faint } else { p.text_muted };
    let g = painter.layout_no_wrap(keys.to_string(), style::medium(style::MICRO), color);
    let w = (g.size().x + 10.0).max(18.0);
    let rect = Rect::from_min_size(pos2(left.x, left.y - 9.0), vec2(w, 18.0));
    let radius = CornerRadius::same(4);
    if !ghost {
        painter.rect_filled(rect, radius, p.hover);
        painter.hline(
            rect.x_range().shrink(2.0),
            rect.bottom() - 0.5,
            Stroke::new(1.0, p.border_strong),
        );
    }
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, p.border),
        StrokeKind::Outside,
    );
    painter.galley(rect.center() - g.size() / 2.0, g, color);
    w
}

/// A label's colour in this theme.
fn label_rgb(label: Option<&Label>, dark: bool) -> Option<Color32> {
    label.map(|l| style::label_color(l.color, dark))
}

/// The top-level group a group sits in (itself at the top).
fn top_group(store: &BookmarkStore, mut id: Option<Uuid>) -> Option<Uuid> {
    let mut hops = 0;
    while let Some(g) = id {
        let parent = store.group(g)?.parent;
        if parent.is_none() || hops > 64 {
            return Some(g);
        }
        id = parent;
        hops += 1;
    }
    None
}

/// A default label text for a group: PROD, STG, DEV… from its name.
fn default_label_text(name: &str) -> String {
    let n = name.trim().to_lowercase();
    let known = [
        ("prod", "PROD"),
        ("live", "LIVE"),
        ("stag", "STG"),
        ("stg", "STG"),
        ("dev", "DEV"),
        ("test", "TEST"),
        ("qa", "QA"),
        ("home", "HOME"),
        ("lab", "LAB"),
    ];
    for (k, v) in known {
        if n.starts_with(k) {
            return v.to_string();
        }
    }
    name.trim()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .take(4)
        .collect::<String>()
        .to_uppercase()
}

fn blend(base: Color32, top: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    let [tr, tg, tb, _] = top.to_srgba_unmultiplied();
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    Color32::from_rgb(mix(base.r(), tr), mix(base.g(), tg), mix(base.b(), tb))
}

/// A small square icon button (20 px, 5 px corners) painted at `rect`; `strong` keeps a fill
/// (the row's Connect button).
fn small_button(ui: &Ui, resp: &egui::Response, rect: Rect, icon: &str, strong: bool) {
    let p = style::pal(ui);
    let fill = if strong {
        if resp.hovered() {
            blend(p.subtle, p.hover)
        } else {
            p.subtle
        }
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(5), fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(13.0),
        if strong || resp.hovered() {
            p.text
        } else {
            p.text_muted
        },
    );
}

/// Text with some characters (by index) in the accent colour and semibold.
fn marked(
    text: &str,
    hits: &[usize],
    font: egui::FontId,
    color: Color32,
    p: &Palette,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let bold = egui::FontId::new(
        font.size,
        egui::FontFamily::Name(crate::ui::fonts::SEMIBOLD.into()),
    );
    let mut run = String::new();
    let mut run_hit = false;
    let flush = |job: &mut egui::text::LayoutJob, run: &mut String, hit: bool| {
        if run.is_empty() {
            return;
        }
        let mono = matches!(font.family, egui::FontFamily::Monospace);
        job.append(
            run,
            0.0,
            egui::TextFormat {
                font_id: if hit && !mono {
                    bold.clone()
                } else {
                    font.clone()
                },
                color: if hit { p.accent_text } else { color },
                ..Default::default()
            },
        );
        run.clear();
    };
    for (i, c) in text.chars().enumerate() {
        let hit = hits.contains(&i);
        if hit != run_hit {
            flush(&mut job, &mut run, run_hit);
            run_hit = hit;
        }
        run.push(c);
    }
    flush(&mut job, &mut run, run_hit);
    job
}

impl Sidebar {
    /// Draws everything under the top row: filter, tree, footer.
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
            first: true,
        };
        if std::mem::take(&mut self.collapse_sections) {
            self.collapsed = store.child_groups(None).map(|g| g.id).collect();
            self.collapsed.insert(UNGROUPED);
        }
        // Everything under the top row (whatever spacing the panel leaves after it).
        let below = ui.available_rect_before_wrap();
        let full = Rect::from_min_max(
            pos2(below.left(), below.top() - ui.spacing().item_spacing.y),
            below.max,
        );
        ui.allocate_rect(below, Sense::hover());

        // Footer first (bottom), so the tree knows how much room it has.
        let foot_h = 8.0 + style::TOOLBAR_H + 10.0;
        let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - foot_h), full.max);
        self.footer(ui, &mut cx, foot);

        let mut main = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(full.min, pos2(full.right(), foot.top()))),
        );
        let ui = &mut main;
        ui.spacing_mut().item_spacing.y = 0.0;
        let empty = store.bookmarks.is_empty() && store.groups.is_empty();
        let searching = !self.search.trim().is_empty();

        // The filter (28 px, 4 px under the top row, 10 px from the sides) and + beside it.
        ui.add_space(4.0);
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
        let plus_w = if searching { 0.0 } else { 24.0 + 6.0 };
        let field = Rect::from_min_max(
            pos2(row.left() + 10.0, row.top()),
            pos2(row.right() - 10.0 - plus_w, row.bottom()),
        );
        if !searching {
            let plus = Rect::from_center_size(
                pos2(row.right() - 10.0 - 12.0, row.center().y),
                vec2(24.0, 24.0),
            );
            let mut pu = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(plus)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            let resp = kit::icon_button_ex(
                &mut pu,
                icons::PLUS,
                &format!(
                    "{} ({})",
                    t("New connection"),
                    super::keys::Shortcut::NewConnection.label()
                ),
                false,
                24.0,
            );
            #[cfg(debug_assertions)]
            crate::devtest::record("sidebar-plus", resp.rect);
            if resp.clicked() {
                cx.actions.push(Action::NewBookmark {
                    group: self.selected_group(store),
                });
            }
            egui::Popup::context_menu(&resp).show(|ui| {
                kit::menu_body(ui);
                ui.set_min_width(220.0);
                if kit::menu_item(ui, Some(icons::HARD_DRIVES), t("New Connection…"), None)
                    .clicked()
                {
                    cx.actions.push(Action::NewBookmark { group: None });
                    ui.close();
                }
                if kit::menu_item(ui, Some(icons::FOLDER_PLUS), t("New Group"), None).clicked() {
                    cx.actions.push(Action::NewGroup { parent: None });
                    ui.close();
                }
            });
        }
        let count = self.hits.as_ref().filter(|_| searching).map(|h| h.count);
        let resp = self.filter_field(ui, field, count);
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
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && searching {
            let shift = ui.input(|i| i.modifiers.shift);
            let ids = self.hit_ids(store);
            if let Some(id) = ids.get(self.search_index.min(ids.len().saturating_sub(1))) {
                cx.actions.push(if shift {
                    Action::OpenFiles(*id)
                } else {
                    Action::Connect(*id)
                });
            }
        }
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.search.clear();
        }
        ui.add_space(8.0);

        if empty && !searching {
            self.empty_state(ui);
            return (cx.actions, cx.moved);
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
        if searching {
            self.refresh_hits(store);
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
                        if searching {
                            self.filtered(ui, &mut cx);
                        } else {
                            self.tree_root(ui, &mut cx);
                            self.bottom_space(ui, &mut cx);
                        }
                    });
            });

        self.hover_card_ui(ui, &mut cx);
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

    // ----- filter -------------------------------------------------------------------------

    /// The filter field: the hover fill, a magnifier, ⌘F at rest; while filtering the number of
    /// matches and a clear button.
    fn filter_field(&mut self, ui: &mut Ui, rect: Rect, count: Option<usize>) -> egui::Response {
        let p = style::pal(ui);
        let id = egui::Id::new("sidebar-search");
        // Redesign 3.1: no keycap in the field; ⌘F is in the menu.
        let keys: Option<&str> = None;
        let right_w = if self.search.is_empty() {
            keys.map_or(0.0, |k| key_width(ui.painter(), k) + 6.0)
        } else {
            20.0 + 22.0
        };
        let fill = if p.dark {
            p.hover
        } else {
            style::soft(Color32::BLACK, 0.05)
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(style::RADIUS), fill);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let resp = child
            .scope(|ui| {
                let v = ui.visuals_mut();
                v.text_edit_bg_color = Some(Color32::TRANSPARENT);
                v.extreme_bg_color = Color32::TRANSPARENT;
                for w in [
                    &mut v.widgets.inactive,
                    &mut v.widgets.hovered,
                    &mut v.widgets.active,
                ] {
                    w.bg_stroke = Stroke::NONE;
                    w.bg_fill = Color32::TRANSPARENT;
                    w.weak_bg_fill = Color32::TRANSPARENT;
                }
                v.selection.stroke = Stroke::NONE;
                ui.add_sized(
                    rect.size(),
                    egui::TextEdit::singleline(&mut self.search)
                        .id(id)
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                            left: 33,
                            right: (right_w + 10.0) as i8,
                            top: 5,
                            bottom: 5,
                        }))
                        .font(style::regular(style::BODY))
                        .hint_text(
                            egui::RichText::new(t("Filter"))
                                .font(style::regular(style::BODY))
                                .color(p.text_faint),
                        )
                        .vertical_align(egui::Align::Center),
                )
            })
            .inner;
        if resp.has_focus() {
            style::focus_halo(ui, rect, style::RADIUS);
        }
        let painter = ui.painter();
        painter.text(
            pos2(rect.left() + 10.0 + 7.5, rect.center().y),
            Align2::CENTER_CENTER,
            icons::MAGNIFYING_GLASS,
            style::icon_font(15.0),
            p.text_faint,
        );
        if self.search.is_empty() {
            if let Some(k) = keys {
                let w = key_width(painter, k);
                paint_key(
                    painter,
                    pos2(rect.right() - 6.0 - w, rect.center().y),
                    k,
                    &p,
                    false,
                );
            }
        } else {
            let x_rect = Rect::from_center_size(
                pos2(rect.right() - 4.0 - 10.0, rect.center().y),
                vec2(20.0, 20.0),
            );
            let x = ui.interact(x_rect, id.with("clear"), Sense::click());
            small_button(ui, &x, x_rect, icons::X_CIRCLE, false);
            if x.clicked() {
                self.search.clear();
            }
            if let Some(n) = count {
                ui.painter().text(
                    pos2(x_rect.left() - 4.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    n.to_string(),
                    style::regular(style::XS),
                    p.text_faint,
                );
            }
        }
        resp
    }

    /// Recomputes the filter's results when the query or the connections changed.
    fn refresh_hits(&mut self, store: &BookmarkStore) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.search.trim().hash(&mut h);
        for b in &store.bookmarks {
            (
                b.id,
                &b.name,
                &b.host,
                &b.username,
                b.port,
                &b.tags,
                b.group,
            )
                .hash(&mut h);
            b.label.as_ref().map(|l| (l.color, &l.text)).hash(&mut h);
        }
        for g in &store.groups {
            (g.id, &g.name, g.parent).hash(&mut h);
            g.label.as_ref().map(|l| (l.color, &l.text)).hash(&mut h);
        }
        let key = h.finish();
        if self.hits.as_ref().is_some_and(|x| x.key == key) {
            return;
        }
        let query = self.search.trim().to_string();
        let mut sections: Vec<(Option<Uuid>, Vec<Hit>)> = Vec::new();
        let mut count = 0;
        for id in search::search(store, &query) {
            let Some(b) = store.bookmark(id) else {
                continue;
            };
            let path = store.group_path(b.group);
            let (why, why_hits) = search::match_reason(&query, b, &path);
            let hit = Hit {
                id,
                name: search::match_positions(&query, &b.display_name()),
                why,
                why_hits,
            };
            let top = top_group(store, b.group);
            match sections.iter_mut().find(|s| s.0 == top) {
                Some(s) => s.1.push(hit),
                None => sections.push((top, vec![hit])),
            }
            count += 1;
        }
        self.hits = Some(Hits {
            key,
            sections,
            count,
        });
    }

    /// The filter's results in the order they are shown (↑ ↓ and ↩ go by it).
    fn hit_ids(&mut self, store: &BookmarkStore) -> Vec<Uuid> {
        self.refresh_hits(store);
        self.hits
            .as_ref()
            .map(|h| {
                h.sections
                    .iter()
                    .flat_map(|s| s.1.iter().map(|x| x.id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The filter's results under their sections, then the keys that act on them.
    fn filtered(&mut self, ui: &mut Ui, cx: &mut Ctx) {
        let p = style::pal(ui);
        let Some(hits) = self.hits.take() else {
            return;
        };
        if hits.count == 0 {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(t("No matches"))
                        .font(style::regular(style::BODY))
                        .color(p.text_muted),
                )
            });
        }
        self.search_index = self.search_index.min(hits.count.saturating_sub(1));
        let mut i = 0;
        for (group, list) in &hits.sections {
            let (name, color) = match group.and_then(|g| cx.store.group(g)) {
                Some(g) => (
                    g.name.clone(),
                    Some(
                        label_rgb(cx.store.group_label(Some(g.id)), p.dark)
                            .unwrap_or(p.accent_text),
                    ),
                ),
                None => (t("Ungrouped").to_string(), None),
            };
            let top = if cx.first { 2.0 } else { 10.0 };
            cx.first = false;
            self.section_head(ui, cx, *group, &name, color, list.len(), top, false);
            for hit in list {
                self.host_row(ui, cx, hit.id, 0, Some(hit), i == self.search_index);
                i += 1;
            }
        }
        self.hits = Some(hits);
    }

    // ----- tree ---------------------------------------------------------------------------

    /// The group the selected item belongs to (for "new connection here").
    fn selected_group(&self, store: &BookmarkStore) -> Option<Uuid> {
        let id = self.selected?;
        if store.group(id).is_some() {
            return Some(id);
        }
        store.bookmark(id).and_then(|b| b.group)
    }

    /// Closes every group and section.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
        self.collapse_sections = true;
    }

    /// Sections first, then the connections in no group (under "Ungrouped" when there are
    /// sections).
    fn tree_root(&mut self, ui: &mut Ui, cx: &mut Ctx) {
        let p = style::pal(ui);
        let sections: Vec<Uuid> = cx.store.child_groups(None).map(|g| g.id).collect();
        for gid in &sections {
            let Some(g) = cx.store.group(*gid) else {
                continue;
            };
            let name = g.name.clone();
            let color =
                label_rgb(cx.store.group_label(Some(*gid)), p.dark).unwrap_or(p.accent_text);
            let count = cx
                .store
                .bookmarks
                .iter()
                .filter(|b| {
                    b.group
                        .is_some_and(|bg| top_group(cx.store, Some(bg)) == Some(*gid))
                })
                .count();
            let top = if cx.first { 2.0 } else { 10.0 };
            cx.first = false;
            self.section_head(ui, cx, Some(*gid), &name, Some(color), count, top, true);
            if !self.collapsed.contains(gid) {
                self.tree(ui, cx, Some(*gid), 0);
            }
        }
        let loose: Vec<Uuid> = cx.store.bookmarks_in(None).map(|b| b.id).collect();
        if loose.is_empty() {
            return;
        }
        if sections.is_empty() {
            for id in loose {
                self.host_row(ui, cx, id, 0, None, false);
            }
            return;
        }
        let top = if cx.first { 2.0 } else { 12.0 };
        self.section_head(ui, cx, None, t("Ungrouped"), None, loose.len(), top, true);
        if !self.collapsed.contains(&UNGROUPED) {
            for id in loose {
                self.host_row(ui, cx, id, 0, None, false);
            }
        }
    }

    /// What is inside a group: its subgroups (folder rows), then its connections.
    fn tree(&mut self, ui: &mut Ui, cx: &mut Ctx, parent: Option<Uuid>, depth: usize) {
        let groups: Vec<Uuid> = cx.store.child_groups(parent).map(|g| g.id).collect();
        for gid in groups {
            self.folder_row(ui, cx, gid, depth);
            if self.expanded.contains(&gid) {
                self.tree(ui, cx, Some(gid), depth + 1);
            }
        }
        let bookmarks: Vec<Uuid> = cx.store.bookmarks_in(parent).map(|b| b.id).collect();
        for id in bookmarks {
            self.host_row(ui, cx, id, depth, None, false);
        }
    }

    /// Empty space under the tree: drop here to move to the top level; right-click to add.
    fn bottom_space(&mut self, ui: &mut Ui, cx: &mut Ctx) {
        let p = style::pal(ui);
        let (rect, resp) = ui.allocate_exact_size(
            vec2(
                ui.available_width(),
                ui.available_height().max(style::row_h() * 2.0),
            ),
            Sense::click(),
        );
        if resp.dnd_hover_payload::<DragItem>().is_some() {
            ui.painter().rect(
                rect.shrink(2.0),
                CornerRadius::same(style::RADIUS),
                p.accent_softer,
                Stroke::new(1.5, p.accent_line),
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
            if kit::menu_item(ui, Some(icons::HARD_DRIVES), t("New Connection…"), None).clicked()
            {
                cx.actions.push(Action::NewBookmark { group: None });
                ui.close();
            }
            if kit::menu_item(ui, Some(icons::FOLDER_PLUS), t("New Group"), None).clicked() {
                cx.actions.push(Action::NewGroup { parent: None });
                ui.close();
            }
        });
    }

    /// The pale copy of a dragged row that follows the pointer.
    fn drag_ghost(ui: &Ui, resp: &egui::Response, icon: &str, color: Option<Color32>, label: &str) {
        if !resp.dragged() {
            return;
        }
        let Some(pos) = ui.ctx().pointer_interact_pos() else {
            return;
        };
        let painter = ui
            .ctx()
            .layer_painter(LayerId::new(Order::Tooltip, egui::Id::new("sidebar-drag")));
        let p = style::pal(ui);
        let r = Rect::from_min_size(pos + vec2(14.0, -14.0), vec2(180.0, 28.0));
        painter.add(
            ui.visuals()
                .popup_shadow
                .as_shape(r, CornerRadius::same(style::RADIUS_LG)),
        );
        painter.rect(
            r,
            CornerRadius::same(style::RADIUS_LG),
            p.raised,
            ui.visuals().window_stroke,
            StrokeKind::Outside,
        );
        let glyph =
            Rect::from_center_size(pos2(r.left() + 8.0 + 10.0, r.center().y), vec2(20.0, 20.0));
        kit::paint_host_glyph(&painter, glyph, icon, color, None, p.raised, &p);
        painter.with_clip_rect(r.shrink(2.0)).text(
            pos2(glyph.right() + 9.0, r.center().y),
            Align2::LEFT_CENTER,
            label,
            style::regular(style::BODY),
            p.text,
        );
    }

    /// Opens a closed group a drag has rested on for a moment.
    fn auto_expand(&mut self, ui: &Ui, id: Uuid, hovered: bool, closed: bool, section: bool) {
        if !hovered || !closed {
            if self.drag_over.is_some_and(|(g, _)| g == id) {
                self.drag_over = None;
            }
            return;
        }
        match self.drag_over {
            Some((g, since)) if g == id => {
                if since.elapsed() >= EXPAND_AFTER {
                    if section {
                        self.collapsed.remove(&id);
                    } else {
                        self.expanded.insert(id);
                    }
                    self.drag_over = None;
                } else {
                    ui.ctx()
                        .request_repaint_after(EXPAND_AFTER.saturating_sub(since.elapsed()));
                }
            }
            _ => {
                self.drag_over = Some((id, Instant::now()));
                ui.ctx().request_repaint_after(EXPAND_AFTER);
            }
        }
    }

    /// A section header: caret, label dot, name, count; + and ⋯ while hovered. `group` is
    /// `None` for Ungrouped; `tree` is false in the filter's results (no caret, no actions).
    #[allow(clippy::too_many_arguments)]
    fn section_head(
        &mut self,
        ui: &mut Ui,
        cx: &mut Ctx,
        group: Option<Uuid>,
        name: &str,
        color: Option<Color32>,
        count: usize,
        top: f32,
        tree: bool,
    ) {
        let p = style::pal(ui);
        ui.add_space(top);
        let key = group.unwrap_or(UNGROUPED);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), SEC_H), Sense::hover());
        let sense = if tree {
            if group.is_some() {
                Sense::click_and_drag()
            } else {
                Sense::click()
            }
        } else {
            Sense::hover()
        };
        let resp = ui.interact(rect, egui::Id::new(("section", key)), sense);
        #[cfg(debug_assertions)]
        crate::devtest::record(format!("group:{name}"), rect);
        let dragging = egui::DragAndDrop::has_any_payload(ui.ctx());
        let hovered = tree && !dragging && ui.rect_contains_pointer(rect);
        let closed = self.collapsed.contains(&key);
        let selected = tree && group.is_some() && self.selected == group;
        let mut painter = ui.painter().clone();
        if group.is_some_and(|g| {
            egui::DragAndDrop::payload::<DragItem>(ui.ctx())
                .is_some_and(|d| *d == DragItem::Group(g))
        }) {
            painter.set_opacity(0.35);
        }
        if selected {
            painter.rect_filled(rect, CornerRadius::same(style::RADIUS), p.accent_soft);
        } else if hovered {
            painter.rect_filled(rect, CornerRadius::same(style::RADIUS), p.hover);
        }
        let cy = rect.center().y;
        let mut x = rect.left() + 6.0;
        if tree {
            painter.text(
                pos2(x + 6.0, cy),
                Align2::CENTER_CENTER,
                if closed {
                    icons::CARET_RIGHT
                } else {
                    icons::CARET_DOWN
                },
                style::icon_font(10.0),
                p.text_faint,
            );
            x += 12.0 + 8.0;
        }
        if let Some(c) = color {
            painter.circle_filled(pos2(x + 4.0, cy), 4.0, c);
        }
        x += 8.0 + 8.0;
        // Right side: + and ⋯ while hovered, the count otherwise.
        let mut right = rect.right() - 8.0;
        let mut add = None;
        let mut more = None;
        if hovered && group.is_some() {
            let more_r = Rect::from_min_size(pos2(right - 20.0, cy - 10.0), vec2(20.0, 20.0));
            let add_r = Rect::from_min_size(
                pos2(more_r.left() - 2.0 - 20.0, cy - 10.0),
                vec2(20.0, 20.0),
            );
            let a = ui
                .interact(add_r, egui::Id::new(("section-add", key)), Sense::click())
                .on_hover_text(t("New Connection Here"));
            small_button(ui, &a, add_r, icons::PLUS, false);
            let m = ui
                .interact(more_r, egui::Id::new(("section-more", key)), Sense::click())
                .on_hover_text(t("More"));
            small_button(ui, &m, more_r, icons::DOTS_THREE, false);
            right = add_r.left() - 4.0;
            add = Some(a);
            more = Some(m);
        } else {
            let g = painter.layout_no_wrap(
                count.to_string(),
                style::medium(style::MICRO),
                p.text_faint,
            );
            let w = (g.size().x + 10.0).max(18.0);
            painter.galley(
                pos2(right - w / 2.0 - g.size().x / 2.0, cy - g.size().y / 2.0),
                g,
                p.text_faint,
            );
            right -= w + 4.0;
        }
        painter
            .with_clip_rect(Rect::from_min_max(
                pos2(x, rect.top()),
                pos2(right, rect.bottom()),
            ))
            .text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                name,
                style::semibold(style::SMALL),
                p.text_muted,
            );
        if !tree {
            return;
        }
        if resp.clicked() || resp.secondary_clicked() || resp.drag_started() {
            ui.memory_mut(|m| m.request_focus(tree_keys_id()));
        }
        if resp.clicked() {
            if let Some(g) = group {
                self.selected = Some(g);
            }
            if !self.collapsed.remove(&key) {
                self.collapsed.insert(key);
            }
        }
        if add.as_ref().is_some_and(|a| a.clicked()) {
            cx.actions.push(Action::NewBookmark { group });
        }
        let Some(gid) = group else {
            return;
        };
        // Drag and drop: a section moves among sections; connections and groups drop into it.
        resp.dnd_set_drag_payload(DragItem::Group(gid));
        Self::drag_ghost(ui, &resp, icons::FOLDER_SIMPLE, color, name);
        let hover_item = resp.dnd_hover_payload::<DragItem>();
        self.auto_expand(ui, gid, hover_item.is_some(), closed, true);
        if let (Some(item), Some(ptr)) = (hover_item, ui.ctx().pointer_interact_pos()) {
            let into = matches!(*item, DragItem::Bookmark(_)) || ptr.y > rect.center().y;
            if *item == DragItem::Group(gid) {
                // Over itself: nothing.
            } else if into {
                drop_target(ui, rect, &p);
            } else {
                drop_line(ui, rect, rect.top(), &p);
            }
            if let Some(item) = resp.dnd_release_payload::<DragItem>() {
                cx.moved = match *item {
                    DragItem::Group(id) if id == gid => None,
                    DragItem::Bookmark(id) => Some(TreeMove::Bookmark {
                        id,
                        group: Some(gid),
                        before: None,
                    }),
                    DragItem::Group(id) if into => Some(TreeMove::Group {
                        id,
                        parent: Some(gid),
                        before: None,
                    }),
                    DragItem::Group(id) => Some(TreeMove::Group {
                        id,
                        parent: None,
                        before: Some(gid),
                    }),
                };
                if into {
                    self.collapsed.remove(&gid);
                }
            }
        }
        if let Some(m) = &more {
            let popup =
                egui::Popup::menu(m).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
            popup.show(|ui| self.group_menu(ui, cx, gid));
        }
        let popup = egui::Popup::context_menu(&resp)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        popup.show(|ui| {
            self.selected = Some(gid);
            self.group_menu(ui, cx, gid)
        });
    }

    /// A group nested in a section: caret, folder, name, count.
    fn folder_row(&mut self, ui: &mut Ui, cx: &mut Ctx, gid: Uuid, depth: usize) {
        let Some(group) = cx.store.group(gid) else {
            return;
        };
        let p = style::pal(ui);
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), style::row_h()), Sense::hover());
        let resp = ui.interact(
            rect,
            egui::Id::new(("group-row", gid)),
            Sense::click_and_drag(),
        );
        #[cfg(debug_assertions)]
        crate::devtest::record(format!("group:{}", group.name), rect);
        let expanded = self.expanded.contains(&gid);
        let selected = self.selected == Some(gid);
        let dragging = egui::DragAndDrop::has_any_payload(ui.ctx());
        let hovered = !dragging && ui.rect_contains_pointer(rect);
        let mut painter = ui.painter().clone();
        if egui::DragAndDrop::payload::<DragItem>(ui.ctx())
            .is_some_and(|d| *d == DragItem::Group(gid))
        {
            painter.set_opacity(0.35);
        }
        if selected {
            kit::paint_selected_row(&painter, rect, &p);
        } else if hovered {
            painter.rect_filled(rect, CornerRadius::same(style::RADIUS), p.hover);
        }
        let cy = rect.center().y;
        let x = rect.left() + 8.0 + depth as f32 * INDENT;
        painter.text(
            pos2(x + 6.0, cy),
            Align2::CENTER_CENTER,
            if expanded {
                icons::CARET_DOWN
            } else {
                icons::CARET_RIGHT
            },
            style::icon_font(10.0),
            p.text_faint,
        );
        painter.text(
            pos2(x + 12.0 + 6.0 + 7.5, cy),
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
        let g =
            painter.layout_no_wrap(count.to_string(), style::medium(style::MICRO), p.text_faint);
        let cw = (g.size().x + 10.0).max(18.0);
        let right = rect.right() - 6.0;
        painter.galley(
            pos2(right - cw / 2.0 - g.size().x / 2.0, cy - g.size().y / 2.0),
            g,
            p.text_faint,
        );
        let name_x = x + 12.0 + 6.0 + 15.0 + 8.0;
        painter
            .with_clip_rect(Rect::from_min_max(
                pos2(name_x, rect.top()),
                pos2(right - cw - 6.0, rect.bottom()),
            ))
            .text(
                pos2(name_x, cy),
                Align2::LEFT_CENTER,
                &group.name,
                style::regular(style::BODY),
                p.text,
            );
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
        Self::drag_ghost(ui, &resp, icons::FOLDER_SIMPLE, None, &group.name);
        let hover_item = resp.dnd_hover_payload::<DragItem>();
        self.auto_expand(ui, gid, hover_item.is_some(), !expanded, false);
        if let (Some(item), Some(ptr)) = (hover_item, ui.ctx().pointer_interact_pos()) {
            let into = matches!(*item, DragItem::Bookmark(_)) || ptr.y > rect.center().y;
            if *item != DragItem::Group(gid) {
                if into {
                    drop_target(ui, rect, &p);
                } else {
                    drop_line(ui, rect, rect.top(), &p);
                }
            }
            if let Some(item) = resp.dnd_release_payload::<DragItem>() {
                cx.moved = match *item {
                    DragItem::Group(id) if id == gid => None,
                    DragItem::Bookmark(id) => Some(TreeMove::Bookmark {
                        id,
                        group: Some(gid),
                        before: None,
                    }),
                    DragItem::Group(id) if into => Some(TreeMove::Group {
                        id,
                        parent: Some(gid),
                        before: None,
                    }),
                    DragItem::Group(id) => Some(TreeMove::Group {
                        id,
                        parent: group.parent,
                        before: Some(gid),
                    }),
                };
                if into {
                    self.expanded.insert(gid);
                }
            }
        }
        let popup = egui::Popup::context_menu(&resp)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
        popup.show(|ui| {
            self.selected = Some(gid);
            self.group_menu(ui, cx, gid)
        });
    }

    /// The menu of a group: add here, its label (colour swatches and text), rename, export,
    /// collapse all, delete.
    fn group_menu(&mut self, ui: &mut Ui, cx: &mut Ctx, gid: Uuid) {
        let p = style::pal(ui);
        kit::menu_body(ui);
        ui.set_min_width(236.0);
        ui.set_max_width(236.0);
        if kit::menu_item(ui, Some(icons::PLUS), t("New Connection Here"), None).clicked() {
            cx.actions.push(Action::NewBookmark { group: Some(gid) });
            ui.close();
        }
        if kit::menu_item(ui, Some(icons::FOLDER_PLUS), t("New Subgroup"), None).clicked() {
            cx.actions.push(Action::NewGroup { parent: Some(gid) });
            ui.close();
        }
        kit::menu_sep(ui);
        // Label: nine colours (the chosen one ringed; choosing it again removes the label) and
        // the short text.
        let own = cx.store.group(gid).and_then(|g| g.label.clone());
        let name = cx
            .store
            .group(gid)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let (hr, _) = ui.allocate_exact_size(
            vec2(
                if ui.is_sizing_pass() {
                    120.0
                } else {
                    ui.available_width()
                },
                22.0,
            ),
            Sense::hover(),
        );
        ui.painter().text(
            pos2(hr.left() + 8.0, hr.bottom() - 4.0),
            Align2::LEFT_BOTTOM,
            t("Label"),
            style::semibold(style::XS),
            p.text_faint,
        );
        let (sw, _) = ui.allocate_exact_size(
            vec2(9.0 * 16.0 + 8.0 * 7.0 + 16.0, 4.0 + 16.0 + 6.0),
            Sense::hover(),
        );
        for (i, c) in LabelColor::ALL.iter().enumerate() {
            let center = pos2(
                sw.left() + 8.0 + 8.0 + i as f32 * 23.0,
                sw.top() + 4.0 + 8.0,
            );
            let r = Rect::from_center_size(center, vec2(16.0, 16.0));
            let resp = ui.interact(
                r.expand(2.0),
                egui::Id::new(("label-swatch", gid, i)),
                Sense::click(),
            );
            let color = style::label_color(*c, p.dark);
            let on = own.as_ref().is_some_and(|l| l.color == *c);
            let painter = ui.painter();
            painter.circle_filled(center, if resp.hovered() { 8.5 } else { 8.0 }, color);
            if on {
                painter.circle_stroke(center, 10.75, Stroke::new(1.5, color));
            }
            if resp.clicked() {
                let label = if on {
                    None
                } else {
                    let text = own
                        .as_ref()
                        .map(|l| l.text.clone())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| default_label_text(&name));
                    Some(Label { color: *c, text })
                };
                self.label_edit = label.as_ref().map(|l| (gid, l.text.clone()));
                cx.actions.push(Action::SetGroupLabel { id: gid, label });
            }
            resp.on_hover_text(match c {
                LabelColor::Red => t("Red"),
                LabelColor::Orange => t("Orange"),
                LabelColor::Yellow => t("Yellow"),
                LabelColor::Green => t("Green"),
                LabelColor::Teal => t("Teal"),
                LabelColor::Blue => t("Blue"),
                LabelColor::Violet => t("Violet"),
                LabelColor::Pink => t("Pink"),
                LabelColor::Grey => t("Grey"),
            });
        }
        // The label text: kept while typing, saved as it changes.
        if self.label_edit.as_ref().is_none_or(|(g, _)| *g != gid) {
            self.label_edit = Some((
                gid,
                own.as_ref().map(|l| l.text.clone()).unwrap_or_default(),
            ));
        }
        let (fr, _) = ui.allocate_exact_size(
            vec2(
                if ui.is_sizing_pass() {
                    200.0
                } else {
                    ui.available_width()
                },
                2.0 + 26.0 + 6.0,
            ),
            Sense::hover(),
        );
        let field = Rect::from_min_max(
            pos2(fr.left() + 8.0, fr.top() + 2.0),
            pos2(fr.right() - 8.0, fr.top() + 28.0),
        );
        let enabled = own.is_some();
        let painter = ui.painter();
        painter.rect(
            field,
            CornerRadius::same(style::RADIUS),
            p.field,
            Stroke::new(1.0, p.field_border),
            StrokeKind::Inside,
        );
        let suffix = painter.layout_no_wrap(
            t("label text").to_string(),
            style::regular(style::SMALL),
            p.text_faint,
        );
        let sx = field.right() - 10.0 - suffix.size().x;
        painter.galley(
            pos2(sx, field.center().y - suffix.size().y / 2.0),
            suffix,
            p.text_faint,
        );
        if let Some((_, text)) = &mut self.label_edit {
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(
                field.min,
                pos2(sx - 6.0, field.bottom()),
            )));
            child.add_enabled_ui(enabled, |ui| {
                let v = ui.visuals_mut();
                v.text_edit_bg_color = Some(Color32::TRANSPARENT);
                v.extreme_bg_color = Color32::TRANSPARENT;
                for w in [
                    &mut v.widgets.inactive,
                    &mut v.widgets.hovered,
                    &mut v.widgets.active,
                ] {
                    w.bg_stroke = Stroke::NONE;
                }
                v.selection.stroke = Stroke::NONE;
                let r = ui.add_sized(
                    vec2(sx - 6.0 - field.left(), 26.0),
                    egui::TextEdit::singleline(text)
                        .char_limit(Label::MAX_TEXT)
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 4)))
                        .font(style::regular(style::BODY))
                        .vertical_align(egui::Align::Center),
                );
                if r.has_focus() {
                    style::focus_halo(ui, field, style::RADIUS);
                }
                if r.changed() {
                    *text = text.to_uppercase();
                    if let Some(l) = &own {
                        cx.actions.push(Action::SetGroupLabel {
                            id: gid,
                            label: Some(Label {
                                color: l.color,
                                text: text.clone(),
                            }),
                        });
                    }
                }
            });
        }
        kit::menu_sep(ui);
        if kit::menu_item(
            ui,
            Some(icons::PENCIL_SIMPLE),
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
        if kit::menu_item(
            ui,
            Some(icons::ARROWS_IN_SIMPLE),
            t("Collapse All Groups"),
            None,
        )
        .clicked()
        {
            cx.actions.push(Action::CollapseAllGroups);
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
    }

    /// A connection: its host glyph (state as a dot), the name; on hover → Connect and ⋯; in
    /// the filter's results the matched letters and why it matched on the right.
    fn host_row(
        &mut self,
        ui: &mut Ui,
        cx: &mut Ctx,
        id: Uuid,
        depth: usize,
        hit: Option<&Hit>,
        highlighted: bool,
    ) {
        let Some(b) = cx.store.bookmark(id) else {
            return;
        };
        let p = style::pal(ui);
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), style::row_h()), Sense::hover());
        let resp = ui.interact(
            rect,
            egui::Id::new(("bookmark-row", id)),
            Sense::click_and_drag(),
        );
        let selected = if hit.is_some() {
            highlighted
        } else {
            self.selected == Some(id)
        };
        let dragging = egui::DragAndDrop::has_any_payload(ui.ctx());
        let hovered = !dragging && ui.rect_contains_pointer(rect);
        let me_dragged = egui::DragAndDrop::payload::<DragItem>(ui.ctx())
            .is_some_and(|d| *d == DragItem::Bookmark(id));
        let mut painter = ui.painter().clone();
        if me_dragged {
            painter.set_opacity(0.35);
        }
        let bg = if selected {
            kit::paint_selected_row(&painter, rect, &p);
            blend(p.chrome, p.accent_soft)
        } else if hovered {
            painter.rect_filled(rect, CornerRadius::same(style::RADIUS), p.hover);
            blend(p.chrome, p.hover)
        } else {
            p.chrome
        };
        let cy = rect.center().y;
        let x = rect.left() + 8.0 + depth as f32 * INDENT;
        let glyph = Rect::from_center_size(pos2(x + 10.0, cy), vec2(20.0, 20.0));
        let label_color = label_rgb(cx.store.label_of(b), p.dark);
        let status = cx.live.get(&id).map(|l| match l {
            Live::Connected => p.ok,
            Live::Connecting => p.warn,
            Live::Failed => p.err,
        });
        let icon = kit::host_icon(b.host_icon());
        kit::paint_host_glyph(&painter, glyph, icon, label_color, status, bg, &p);

        // Right side: the hover actions, why it matched, or how many tabs are open on it.
        let mut right = rect.right() - 6.0;
        let mut connect_click = false;
        let mut more_resp = None;
        if hovered && hit.is_none() {
            let more_r = Rect::from_min_size(pos2(right - 20.0, cy - 10.0), vec2(20.0, 20.0));
            let go_r = Rect::from_min_size(
                pos2(more_r.left() - 2.0 - 20.0, cy - 10.0),
                vec2(20.0, 20.0),
            );
            let go = ui
                .interact(go_r, egui::Id::new(("row-go", id)), Sense::click())
                .on_hover_text(t("Connect"));
            small_button(ui, &go, go_r, icons::ARROW_RIGHT, true);
            let more = ui
                .interact(more_r, egui::Id::new(("row-more", id)), Sense::click())
                .on_hover_text(t("More"));
            small_button(ui, &more, more_r, icons::DOTS_THREE, false);
            connect_click = go.clicked();
            more_resp = Some(more);
            right = go_r.left() - 6.0;
        } else if let Some(h) = hit {
            let font = style::mono(11.0);
            let mut job = marked(&h.why, &h.why_hits, font, p.text_faint, &p);
            job.wrap = egui::text::TextWrapping {
                max_width: 96.0,
                max_rows: 1,
                break_anywhere: true,
                overflow_character: Some('…'),
            };
            let g = painter.layout_job(job);
            let w = g.size().x;
            painter.galley(pos2(right - w, cy - g.size().y / 2.0), g, p.text_faint);
            right -= w + 8.0;
        } else if let Some(n) = cx.open.get(&id).copied().filter(|n| *n >= 2) {
            let g =
                painter.layout_no_wrap(n.to_string(), style::medium(style::MICRO), p.text_faint);
            let w = g.size().x;
            painter.galley(pos2(right - w, cy - g.size().y / 2.0), g, p.text_faint);
            right -= w + 8.0;
        }
        let name = b.display_name();
        let name_x = glyph.right() + 9.0;
        let clip = Rect::from_min_max(pos2(name_x, rect.top()), pos2(right, rect.bottom()));
        let font = style::regular(style::BODY);
        let job = match hit {
            Some(h) => marked(&name, &h.name, font, p.text, &p),
            None => egui::text::LayoutJob::simple_singleline(name.clone(), font, p.text),
        };
        let g = painter.layout_job(job);
        painter
            .with_clip_rect(clip)
            .galley(pos2(name_x, cy - g.size().y / 2.0), g, p.text);
        #[cfg(debug_assertions)]
        crate::devtest::record(format!("bookmark:{name}"), rect);
        if hovered && hit.is_none() {
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
        Self::drag_ghost(ui, &resp, icon, label_color, &name);
        if hit.is_none()
            && let (Some(item), Some(ptr)) = (
                resp.dnd_hover_payload::<DragItem>(),
                ui.ctx().pointer_interact_pos(),
            )
        {
            let before = ptr.y < rect.center().y;
            if matches!(*item, DragItem::Bookmark(x) if x != id) {
                let y = if before { rect.top() } else { rect.bottom() };
                drop_line(ui, rect, y, &p);
            }
            if let Some(item) = resp.dnd_release_payload::<DragItem>() {
                // "After this row" = before the next row that isn't the dragged one itself.
                let after = |dragged: Uuid| {
                    let ids: Vec<Uuid> = cx.store.bookmarks_in(b.group).map(|x| x.id).collect();
                    let pos = ids.iter().position(|x| *x == id).unwrap_or(0);
                    ids[pos + 1..].iter().copied().find(|x| *x != dragged)
                };
                cx.moved = match *item {
                    DragItem::Bookmark(dragged) if dragged == id => None,
                    DragItem::Bookmark(dragged) => Some(TreeMove::Bookmark {
                        id: dragged,
                        group: b.group,
                        before: if before { Some(id) } else { after(dragged) },
                    }),
                    DragItem::Group(g) => Some(TreeMove::Group {
                        id: g,
                        parent: b.group,
                        before: None,
                    }),
                };
            }
        }
        if let Some(more) = &more_resp {
            egui::Popup::menu(more).show(|ui| host_menu(ui, cx, b));
        }
        resp.context_menu(|ui| {
            self.selected = Some(id);
            host_menu(ui, cx, b);
        });
    }

    // ----- footer, empty state, hover card --------------------------------------------------

    /// The footer: + New connection, then Import and Settings on the right.
    fn footer(&mut self, ui: &mut Ui, cx: &mut Ctx, foot: Rect) {
        let p = style::pal(ui);
        let inner = Rect::from_min_max(
            pos2(foot.left() + 10.0, foot.top() + 8.0),
            pos2(foot.right() - 10.0, foot.bottom() - 10.0),
        );
        // "+ New connection": a quiet 26 px button, 6 px before the icon and 8 after the text.
        let text = ui.painter().layout_no_wrap(
            t("New connection").to_string(),
            style::medium(style::SMALL),
            Color32::PLACEHOLDER,
        );
        let w = 6.0 + 14.0 + 5.0 + text.size().x + 8.0;
        let new_r = Rect::from_min_size(pos2(inner.left(), inner.center().y - 13.0), vec2(w, 26.0));
        let new = ui.interact(new_r, egui::Id::new("sidebar-new"), Sense::click());
        #[cfg(debug_assertions)]
        crate::devtest::record("sidebar-new", new.rect);
        let color = if new.hovered() { p.text } else { p.text_muted };
        let painter = ui.painter();
        if new.hovered() {
            painter.rect_filled(new_r, CornerRadius::same(style::RADIUS), p.hover);
        }
        painter.text(
            pos2(new_r.left() + 6.0 + 7.0, new_r.center().y),
            Align2::CENTER_CENTER,
            icons::PLUS,
            style::icon_font(14.0),
            color,
        );
        painter.galley(
            pos2(
                new_r.left() + 6.0 + 14.0 + 5.0,
                new_r.center().y - text.size().y / 2.0,
            ),
            text,
            color,
        );
        if new.clicked() {
            cx.actions.push(Action::NewBookmark {
                group: self.selected_group(cx.store),
            });
        }
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 4.0;
        let settings = kit::icon_button_ex(
            &mut row,
            icons::GEAR_SIX,
            &format!(
                "{} ({})",
                t("Settings"),
                super::keys::Shortcut::Settings.label()
            ),
            false,
            24.0,
        );
        if settings.clicked() {
            cx.actions.push(Action::Settings);
        }
        let import = kit::icon_button_ex(
            &mut row,
            icons::TRAY_ARROW_DOWN,
            t("Import & export"),
            false,
            24.0,
        );
        #[cfg(debug_assertions)]
        crate::devtest::record("sidebar-import", import.rect);
        egui::Popup::menu(&import).show(|ui| {
            kit::menu_body(ui);
            ui.set_min_width(250.0);
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
            kit::menu_sep(ui);
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
        });
    }

    /// No connections: a tile, a title and what will appear here.
    /// No connections yet: one quiet line (Redesign 3.1); the canvas explains what to do.
    fn empty_state(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let area = ui.available_rect_before_wrap();
        ui.painter().text(
            pos2(area.left() + 14.0, area.top() + 18.0),
            Align2::LEFT_TOP,
            t("No connections yet."),
            style::regular(12.5),
            p.text_faint,
        );
    }

    /// The hover card of the connection under the pointer (after a moment), kept open while
    /// the pointer is on it.
    fn hover_card_ui(&mut self, ui: &Ui, cx: &mut Ctx) {
        let ptr = ui.ctx().pointer_hover_pos();
        let on_card = self
            .card
            .is_some_and(|(_, r)| ptr.is_some_and(|p| r.expand(2.0).contains(p)));
        // No card while dragging or while a menu is open (it would cover the menu).
        let dragging = egui::DragAndDrop::has_any_payload(ui.ctx())
            || (egui::Popup::is_any_open(ui.ctx()) && !on_card);
        let show = match cx.hovered {
            Some((id, rect)) if !dragging => {
                let since = match self.hover {
                    Some((h, at)) if h == id => at,
                    _ => {
                        let now = Instant::now();
                        self.hover = Some((id, now));
                        now
                    }
                };
                if since.elapsed() >= HOVER_CARD_AFTER || self.card.is_some_and(|(c, _)| c == id) {
                    Some((id, rect.top()))
                } else {
                    ui.ctx()
                        .request_repaint_after(HOVER_CARD_AFTER.saturating_sub(since.elapsed()));
                    None
                }
            }
            _ if on_card && !dragging => {
                let (id, r) = self.card.unwrap();
                Some((id, r.top() + 8.0))
            }
            _ => {
                self.hover = None;
                None
            }
        };
        match show {
            Some((id, top)) => {
                let rect = self.hover_card(ui.ctx(), cx, id, top);
                self.card = rect.map(|r| (id, r));
            }
            None => self.card = None,
        }
    }

    /// A connection's details beside the sidebar: glyph, name, label and address; login, jump
    /// host, forwards, tags, when last used; Connect, Files and Edit.
    fn hover_card(&self, ctx: &egui::Context, cx: &mut Ctx, id: Uuid, top: f32) -> Option<Rect> {
        let b = cx.store.bookmark(id)?;
        let store = cx.store;
        let resp = egui::Area::new(egui::Id::new("sidebar-hover-card"))
            .order(Order::Foreground)
            .fixed_pos(pos2(cx.right - 12.0, top - 8.0))
            .show(ctx, |ui| {
                let p = style::pal(ui);
                kit::popover_frame(ui)
                    .inner_margin(egui::Margin::same(14))
                    .show(ui, |ui| {
                        ui.set_width(CARD_W - 28.0);
                        ui.spacing_mut().item_spacing.y = 12.0;
                        ui.spacing_mut().interact_size.y = 0.0;
                        // Name and address (Redesign 3.1: text only).
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(
                                egui::RichText::new(b.display_name())
                                    .font(style::semibold(style::BODY))
                                    .color(p.text),
                            );
                            ui.label(
                                egui::RichText::new(crate::app::address_with_port(b))
                                    .font(style::mono(11.0))
                                    .color(p.text_faint),
                            );
                        });
                        // The details.
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
                        let last = self.recent.iter().find(|r| r.id == id).map(|r| r.at);
                        let tabs = cx.open.get(&id).copied().unwrap_or(0);
                        egui::Grid::new(("hover-kv", id))
                            .num_columns(2)
                            .spacing([10.0, 7.0])
                            .min_col_width(64.0)
                            .show(ui, |ui| {
                                let key = |ui: &mut Ui, text: &str| {
                                    ui.label(
                                        egui::RichText::new(text)
                                            .font(style::regular(style::SMALL))
                                            .color(p.text_faint),
                                    );
                                };
                                let val = |ui: &mut Ui, text: &str| {
                                    ui.label(
                                        egui::RichText::new(text)
                                            .font(style::regular(style::SMALL))
                                            .color(p.text),
                                    );
                                };
                                key(ui, t("Login"));
                                val(ui, &login);
                                ui.end_row();
                                if let Some(v) = &via {
                                    key(ui, t("Via"));
                                    val(ui, v);
                                    ui.end_row();
                                }
                                if !b.forwards.is_empty() {
                                    key(ui, t("Forwards"));
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        for f in &b.forwards {
                                            ui.label(
                                                egui::RichText::new(crate::app::forward_rule(f))
                                                    .font(style::mono(11.0))
                                                    .color(p.text),
                                            );
                                        }
                                    });
                                    ui.end_row();
                                }
                                if !b.tags.is_empty() {
                                    key(ui, t("Tags"));
                                    val(ui, &b.tags.join(", "));
                                    ui.end_row();
                                }
                                if last.is_some() || tabs > 0 {
                                    key(ui, t("Last"));
                                    let mut parts = Vec::new();
                                    if let Some(at) = last {
                                        parts.push(crate::app::ago(at));
                                    }
                                    let tabs_text = match tabs {
                                        0 => String::new(),
                                        1 => t("1 tab open").to_string(),
                                        n => tf("{} tabs open", &[&n.to_string()]),
                                    };
                                    if tabs > 0 {
                                        parts.push(tabs_text);
                                    }
                                    let text = parts.join(" · ");
                                    if tabs > 0 && cx.live.get(&id) == Some(&Live::Connected) {
                                        ui.label(
                                            egui::RichText::new(text)
                                                .font(style::regular(style::SMALL))
                                                .color(p.text),
                                        );
                                    } else {
                                        val(ui, &text);
                                    }
                                    ui.end_row();
                                }
                            });
                        if !b.notes.trim().is_empty() {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(b.notes.trim())
                                        .font(style::regular(style::SMALL))
                                        .color(p.text_faint),
                                )
                                .wrap(),
                            );
                        }
                    });
            });
        Some(resp.response.rect)
    }

    /// The keys of the tree while it has the keyboard, for the selected item.
    fn tree_keys(&mut self, ui: &mut Ui, cx: &mut Ctx) {
        let (open, files, edit, rename, delete, escape) = ui.input_mut(|i| {
            (
                TreeKey::Open.take(i),
                i.consume_key(egui::Modifiers::SHIFT, Key::Enter),
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
            } else if files {
                cx.actions.push(Action::OpenFiles(id));
            } else if edit {
                cx.actions.push(Action::EditBookmark(id));
            } else if delete {
                cx.actions.push(Action::DeleteBookmark(id));
            }
        } else if let Some(g) = cx.store.group(id) {
            if open {
                if g.parent.is_none() {
                    if !self.collapsed.remove(&id) {
                        self.collapsed.insert(id);
                    }
                } else if !self.expanded.remove(&id) {
                    self.expanded.insert(id);
                }
            } else if edit || rename {
                cx.actions.push(Action::RenameGroup(id));
            } else if delete {
                cx.actions.push(Action::DeleteGroup(id));
            }
        }
    }

    /// Selects and reveals a bookmark or a group (opening the groups it is in).
    pub fn reveal(&mut self, store: &BookmarkStore, id: Uuid) {
        self.selected = Some(id);
        let mut g = match store.bookmark(id) {
            Some(b) => {
                if b.group.is_none() {
                    self.collapsed.remove(&UNGROUPED);
                }
                b.group
            }
            None => store.group(id).and_then(|x| x.parent),
        };
        while let Some(gid) = g {
            self.expanded.insert(gid);
            self.collapsed.remove(&gid);
            g = store.group(gid).and_then(|x| x.parent);
        }
    }

    /// Selects a group and opens it (and the groups it is in).
    pub fn reveal_group(&mut self, store: &BookmarkStore, id: Uuid) {
        self.reveal(store, id);
        self.expanded.insert(id);
        self.collapsed.remove(&id);
        self.search.clear();
    }

    pub fn status_line(store: &BookmarkStore) -> String {
        tf("{} connections", &[&store.bookmarks.len().to_string()])
    }
}

/// The menu of a connection: connect, open files, edit, duplicate, copy the address, its label,
/// export, delete.
fn host_menu(ui: &mut Ui, cx: &mut Ctx, b: &Bookmark) {
    let id = b.id;
    let p = style::pal(ui);
    kit::menu_body(ui);
    ui.set_min_width(240.0);
    if kit::menu_item(
        ui,
        Some(icons::ARROW_RIGHT),
        t("Connect"),
        Some(TreeKey::Open.label()),
    )
    .clicked()
    {
        cx.actions.push(Action::Connect(id));
        ui.close();
    }
    if kit::menu_item(ui, Some(icons::FOLDER_SIMPLE), t("Open Files"), Some("⇧↩")).clicked() {
        cx.actions.push(Action::OpenFiles(id));
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
        cx.actions.push(Action::EditBookmark(id));
        ui.close();
    }
    if kit::menu_item(ui, Some(icons::COPY), t("Duplicate"), None).clicked() {
        cx.actions.push(Action::DuplicateBookmark(id));
        ui.close();
    }
    if kit::menu_item(ui, Some(icons::CLIPBOARD_TEXT), t("Copy Address"), None).clicked() {
        ui.ctx().copy_text(b.address());
        ui.close();
    }
    // Label ▸: inherit the group's, one of the colours, or none.
    let inherited = cx.store.group_label(b.group).cloned();
    let current = cx.store.label_of(b).map(|l| l.color);
    let resp = kit::menu_item_ex(ui, Some(icons::TAG), t("Label"), None, false, true);
    if let Some(c) = current {
        ui.painter().circle_filled(
            pos2(resp.rect.right() - 8.0 - 20.0 - 4.0, resp.rect.center().y),
            4.0,
            style::label_color(c, p.dark),
        );
    }
    egui::containers::menu::SubMenu::new().show(ui, &resp, |ui| {
        kit::menu_body(ui);
        ui.set_min_width(200.0);
        let check = |ui: &Ui, r: &egui::Response| {
            ui.painter().text(
                pos2(r.rect.right() - 8.0 - 7.0, r.rect.center().y),
                Align2::CENTER_CENTER,
                icons::CHECK,
                style::icon_font(13.0),
                if r.hovered() {
                    p.on_accent
                } else {
                    p.accent_text
                },
            );
        };
        let dot = |ui: &Ui, r: &egui::Response, c: Color32| {
            ui.painter()
                .circle_filled(pos2(r.rect.left() + 8.0 + 8.0, r.rect.center().y), 4.0, c);
        };
        if let Some(g) = &inherited {
            let group = cx
                .store
                .group(b.group.unwrap_or_default())
                .map(|g| g.name.clone())
                .unwrap_or_default();
            let r = kit::menu_item(ui, None, &tf("Inherit from {}", &[&group]), None);
            dot(ui, &r, style::label_color(g.color, p.dark));
            if b.label.is_none() {
                check(ui, &r);
            }
            if r.clicked() {
                cx.actions
                    .push(Action::SetBookmarkLabel { id, label: None });
                ui.close();
            }
            kit::menu_sep(ui);
        }
        for c in LabelColor::ALL {
            let name = match c {
                LabelColor::Red => t("Red"),
                LabelColor::Orange => t("Orange"),
                LabelColor::Yellow => t("Yellow"),
                LabelColor::Green => t("Green"),
                LabelColor::Teal => t("Teal"),
                LabelColor::Blue => t("Blue"),
                LabelColor::Violet => t("Violet"),
                LabelColor::Pink => t("Pink"),
                LabelColor::Grey => t("Grey"),
            };
            let r = kit::menu_item(ui, None, name, None);
            dot(ui, &r, style::label_color(c, p.dark));
            if b.label.as_ref().is_some_and(|l| l.color == c) {
                check(ui, &r);
            }
            if r.clicked() {
                let text = b
                    .label
                    .as_ref()
                    .or(inherited.as_ref())
                    .map(|l| l.text.clone())
                    .unwrap_or_default();
                cx.actions.push(Action::SetBookmarkLabel {
                    id,
                    label: Some(Label { color: c, text }),
                });
                ui.close();
            }
        }
        if inherited.is_none() {
            kit::menu_sep(ui);
            let r = kit::menu_item(ui, Some(icons::PROHIBIT), t("No Label"), None);
            if b.label.is_none() {
                check(ui, &r);
            }
            if r.clicked() {
                cx.actions
                    .push(Action::SetBookmarkLabel { id, label: None });
                ui.close();
            }
        }
    });
    kit::menu_sep(ui);
    if kit::menu_item(ui, Some(icons::EXPORT), t("Export…"), None).clicked() {
        cx.actions
            .push(Action::Export(ExportScope::Selected(vec![id])));
        ui.close();
    }
    kit::menu_sep(ui);
    let delete = if cfg!(target_os = "macos") {
        "⌘⌫"
    } else {
        TreeKey::Delete.label()
    };
    if kit::menu_item_ex(
        ui,
        Some(icons::TRASH),
        t("Delete…"),
        Some(delete),
        true,
        false,
    )
    .clicked()
    {
        cx.actions.push(Action::DeleteBookmark(id));
        ui.close();
    }
}

/// The insertion line of a drag: 2 px of accent from 30 px into the row, with a ring at its
/// start.
fn drop_line(ui: &Ui, row: Rect, y: f32, p: &Palette) {
    let painter = ui.painter();
    let left = row.left() + 30.0;
    painter.hline(left..=row.right() - 8.0, y, Stroke::new(2.0, p.accent));
    painter.circle(pos2(left, y), 3.0, p.chrome, Stroke::new(2.0, p.accent));
}

/// A group a drag would drop into: the accent wash and a 1.5 px accent ring.
fn drop_target(ui: &Ui, rect: Rect, p: &Palette) {
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(style::RADIUS), p.accent_softer);
    painter.rect_stroke(
        rect,
        CornerRadius::same(style::RADIUS),
        Stroke::new(1.5, p.accent_line),
        StrokeKind::Inside,
    );
}
