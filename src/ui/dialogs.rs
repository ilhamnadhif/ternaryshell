//! Dialogs: the command palette, confirmations, group names, settings, export, the import
//! preview, and toasts. All share the dialog frame, header and footer of `kit`.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Key, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use uuid::Uuid;

use super::actions::Action;
use super::confirm::{self, ConfirmExtra, ConfirmRow};
use super::keys::{self, Shortcut as S};
use super::style::{self, Tone};
pub use super::toasts::Toasts;
use super::{fonts, icons, kit};
use crate::bookmarks::transfer::{
    ExportScope, ImportPlan, ItemStatus, MatchedBy, Resolution, SourceKind,
};
use crate::bookmarks::{BookmarkStore, search};
use crate::i18n::{Language, t, tf};
use crate::settings::{CursorStyle, FontChoice, QuickCommand, Settings, ThemeMode};
use crate::term::scheme::{SCHEMES, Scheme};

// ----- command palette ------------------------------------------------------------------------

/// A command the palette can run.
#[derive(Clone)]
pub struct Command {
    pub icon: &'static str,
    pub label: String,
    pub shortcut: Option<String>,
    pub action: Action,
    /// A faint word after the label (the tab it acts on, the command a quick command sends).
    pub detail: Option<String>,
    /// A quick command (listed apart from the app's actions).
    pub quick: bool,
}

/// An open tab, for the palette's Tabs.
pub struct PaletteTab {
    pub index: usize,
    pub name: String,
    pub address: String,
    pub bookmark: Uuid,
    pub connected: bool,
}

/// What the palette lists and previews.
pub struct PaletteData<'a> {
    pub store: &'a BookmarkStore,
    /// Recent connections, newest first, with when ("12 min ago").
    pub recent: &'a [(Uuid, String)],
    pub live: &'a HashSet<Uuid>,
    /// How many tabs are open to each connection.
    pub open: &'a std::collections::HashMap<Uuid, usize>,
    pub tabs: &'a [PaletteTab],
    pub commands: &'a [Command],
}

/// What the palette searches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Scope {
    #[default]
    All,
    Hosts,
    Commands,
    Tabs,
    Actions,
}

impl Scope {
    const ALL: [Scope; 5] = [
        Scope::All,
        Scope::Hosts,
        Scope::Commands,
        Scope::Tabs,
        Scope::Actions,
    ];

    fn label(self) -> &'static str {
        match self {
            Scope::All => t("All"),
            Scope::Hosts => t("Hosts"),
            Scope::Commands => t("Commands"),
            Scope::Tabs => t("Tabs"),
            Scope::Actions => t("Actions"),
        }
    }

    fn icon(self) -> Option<&'static str> {
        match self {
            Scope::All => None,
            Scope::Hosts => Some(icons::HARD_DRIVES),
            Scope::Commands => Some(icons::LIGHTNING),
            Scope::Tabs => Some(icons::CARDS),
            Scope::Actions => Some(icons::COMMAND),
        }
    }
}

#[derive(Default)]
pub struct Palette {
    query: String,
    index: usize,
    focused: bool,
    scope: Scope,
    /// The rows for the last query and scope, kept until either changes.
    cache: Option<(CacheKey, Vec<Section>)>,
}

#[derive(Clone, PartialEq, Eq)]
struct CacheKey {
    query: String,
    scope: Scope,
    sizes: [usize; 4],
}

struct Section {
    title: String,
    /// Recent connections: ⌘1–⌘9 open them.
    numbered: bool,
    rows: Vec<Row>,
}

pub enum PaletteResult {
    Open,
    Close,
    Connect(Uuid),
    /// Connect and open the Files panel.
    ConnectFiles(Uuid),
    Edit(Uuid),
    /// An address typed in (`user@host[:port]`).
    ConnectTo(String),
    /// Open the connection editor for the typed address.
    SaveAs(String),
    Run(Action),
}

/// Most connections listed in the palette.
const PALETTE_ROWS: usize = 50;
const PALETTE_W: f32 = 720.0;
const PREVIEW_W: f32 = 260.0;
const BODY_H: f32 = 404.0;

/// One row of the palette, in the order shown.
#[derive(Clone)]
enum Row {
    Host(Uuid),
    ConnectTo(String),
    SaveAs(String),
    Command(usize),
    Tab(usize),
}

/// The modifier key labels of this system: `⌘` / `Ctrl+`.
fn mod_label(mac: &str, other: &str) -> String {
    if cfg!(target_os = "macos") {
        mac.to_string()
    } else {
        other.to_string()
    }
}

/// `⌘3` on macOS, `Alt+3` elsewhere: opens the n-th recent connection (or tab).
fn number_label(n: usize) -> String {
    format!("{}{n}", mod_label("⌘", "Alt+"))
}

/// Whether the query looks like an address to connect to (`user@host`, `host:port`, a name
/// with a dot) and parses as one.
fn address_query(query: &str) -> Option<crate::bookmarks::model::Target> {
    let looks = query.contains('@')
        || query.contains(':')
        || (query.contains('.') && !query.contains(char::is_whitespace));
    looks
        .then(|| crate::bookmarks::model::parse_target(query).ok())
        .flatten()
}

fn build_sections(query: &str, scope: Scope, data: &PaletteData) -> Vec<Section> {
    let store = data.store;
    let section = |title: &str, numbered: bool, rows: Vec<Row>| Section {
        title: title.to_string(),
        numbered,
        rows,
    };
    let actions: Vec<usize> = (0..data.commands.len())
        .filter(|i| !data.commands[*i].quick)
        .collect();
    let quick: Vec<usize> = (0..data.commands.len())
        .filter(|i| data.commands[*i].quick)
        .collect();
    let fuzzy_cmds = |list: &[usize]| -> Vec<Row> {
        let mut hits: Vec<(i32, usize)> = list
            .iter()
            .filter_map(|i| search::fuzzy_score(query, &data.commands[*i].label).map(|s| (s, *i)))
            .collect();
        hits.sort_by_key(|h| std::cmp::Reverse(h.0));
        hits.into_iter()
            .take(12)
            .map(|(_, i)| Row::Command(i))
            .collect()
    };
    let recent_rows = |n: usize| -> Vec<Row> {
        data.recent
            .iter()
            .filter(|(id, _)| store.bookmark(*id).is_some())
            .take(n)
            .map(|(id, _)| Row::Host(*id))
            .collect()
    };
    let mut out = Vec::new();
    if query.is_empty() {
        match scope {
            Scope::All => {
                let rec = recent_rows(5);
                if !rec.is_empty() {
                    out.push(section(t("Recent"), true, rec));
                } else if !store.bookmarks.is_empty() {
                    out.push(section(
                        t("Connections"),
                        false,
                        store
                            .bookmarks
                            .iter()
                            .take(5)
                            .map(|b| Row::Host(b.id))
                            .collect(),
                    ));
                }
                out.push(section(
                    t("Actions"),
                    false,
                    actions.iter().take(3).map(|i| Row::Command(*i)).collect(),
                ));
                out.push(section(
                    t("Quick commands"),
                    false,
                    quick.iter().take(3).map(|i| Row::Command(*i)).collect(),
                ));
            }
            Scope::Hosts => {
                let rec = recent_rows(5);
                let mut seen: HashSet<Uuid> = HashSet::new();
                let mut rows = Vec::new();
                for r in rec {
                    if let Row::Host(id) = r {
                        seen.insert(id);
                        rows.push(Row::Host(id));
                    }
                }
                rows.extend(
                    store
                        .bookmarks
                        .iter()
                        .filter(|b| !seen.contains(&b.id))
                        .map(|b| Row::Host(b.id)),
                );
                out.push(section(t("Connections"), false, rows));
            }
            Scope::Commands => out.push(section(
                t("Quick commands"),
                false,
                quick.iter().map(|i| Row::Command(*i)).collect(),
            )),
            Scope::Tabs => out.push(section(
                t("Tabs"),
                false,
                (0..data.tabs.len()).map(Row::Tab).collect(),
            )),
            Scope::Actions => out.push(section(
                t("Actions"),
                false,
                actions.iter().map(|i| Row::Command(*i)).collect(),
            )),
        }
    } else {
        let address = matches!(scope, Scope::All | Scope::Hosts)
            .then(|| address_query(query))
            .flatten();
        if matches!(scope, Scope::All | Scope::Hosts) {
            if address.is_some() {
                out.push(section(
                    t("Quick connect"),
                    false,
                    vec![
                        Row::ConnectTo(query.to_string()),
                        Row::SaveAs(query.to_string()),
                    ],
                ));
            }
            let mut hits = search::search(store, query);
            if let Some(a) = &address {
                // Saved hosts on the same machine, or with the same user.
                for b in &store.bookmarks {
                    let same_host = b.host.eq_ignore_ascii_case(&a.host);
                    let same_user = a.user.as_ref().is_some_and(|u| *u == b.username);
                    if (same_host || same_user) && !hits.contains(&b.id) {
                        hits.push(b.id);
                    }
                }
            }
            hits.truncate(PALETTE_ROWS);
            if !hits.is_empty() {
                out.push(section(
                    if address.is_some() {
                        t("Matching hosts")
                    } else {
                        t("Connections")
                    },
                    false,
                    hits.into_iter().map(Row::Host).collect(),
                ));
            }
        }
        if matches!(scope, Scope::All | Scope::Actions) && address.is_none() {
            let rows = fuzzy_cmds(&actions);
            if !rows.is_empty() {
                out.push(section(t("Actions"), false, rows));
            }
        }
        if matches!(scope, Scope::All | Scope::Commands) && address.is_none() {
            let rows = fuzzy_cmds(&quick);
            if !rows.is_empty() {
                out.push(section(t("Quick commands"), false, rows));
            }
        }
        if matches!(scope, Scope::All | Scope::Tabs) && address.is_none() {
            let mut hits: Vec<(i32, usize)> = data
                .tabs
                .iter()
                .enumerate()
                .filter_map(|(i, tab)| search::fuzzy_score(query, &tab.name).map(|s| (s, i)))
                .collect();
            hits.sort_by_key(|h| std::cmp::Reverse(h.0));
            if !hits.is_empty() {
                out.push(section(
                    t("Tabs"),
                    false,
                    hits.into_iter().map(|(_, i)| Row::Tab(i)).collect(),
                ));
            }
        }
    }
    out.retain(|s| !s.rows.is_empty());
    out
}

impl Palette {
    pub fn show(&mut self, ctx: &egui::Context, data: &PaletteData) -> PaletteResult {
        let mut result = PaletteResult::Open;
        // `>` first switches to the actions, as in other editors.
        if let Some(rest) = self.query.strip_prefix('>') {
            self.query = rest.trim_start().to_string();
            self.scope = Scope::Actions;
            self.index = 0;
        }
        let query = self.query.trim().to_string();
        let key = CacheKey {
            query: query.clone(),
            scope: self.scope,
            sizes: [
                data.store.bookmarks.len(),
                data.commands.len(),
                data.tabs.len(),
                data.recent.len(),
            ],
        };
        if self.cache.as_ref().is_none_or(|(k, _)| *k != key) {
            let sections = build_sections(&query, self.scope, data);
            self.cache = Some((key, sections));
        }
        let sections = &self.cache.as_ref().expect("built").1;
        let flat: Vec<(&Row, Option<usize>)> = sections
            .iter()
            .flat_map(|s| {
                s.rows
                    .iter()
                    .enumerate()
                    .map(move |(i, r)| (r, (s.numbered && i < 9).then_some(i + 1)))
            })
            .collect();
        let total = flat.len();
        self.index = self.index.min(total.saturating_sub(1));
        let address = address_query(&query);

        let area = egui::Area::new(egui::Id::new("palette-area"))
            .anchor(Align2::CENTER_TOP, vec2(0.0, 86.0));
        let p = style::palette(ctx.global_style().visuals.dark_mode);
        let modal = egui::Modal::new(egui::Id::new("palette"))
            .area(area)
            .backdrop_color(p.scrim)
            .frame(style::dialog_frame(ctx).fill(p.sheet))
            .show(ctx, |ui| {
                ui.set_width(PALETTE_W);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // Keys the palette handles before the field sees them.
                let (tab, back_tab, down, up, enter, shift_enter, cmd_enter, edit, save, number) =
                    ui.input_mut(|i| {
                        let number = [
                            Key::Num1,
                            Key::Num2,
                            Key::Num3,
                            Key::Num4,
                            Key::Num5,
                            Key::Num6,
                            Key::Num7,
                            Key::Num8,
                            Key::Num9,
                        ]
                        .iter()
                        .position(|k| i.consume_key(keys::tab_number_modifiers(), *k));
                        (
                            i.consume_key(egui::Modifiers::NONE, Key::Tab),
                            i.consume_key(egui::Modifiers::SHIFT, Key::Tab),
                            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                            i.key_pressed(Key::Enter) && i.modifiers.is_none(),
                            i.consume_key(egui::Modifiers::SHIFT, Key::Enter),
                            i.consume_key(egui::Modifiers::COMMAND, Key::Enter),
                            i.consume_key(egui::Modifiers::COMMAND, Key::E),
                            i.consume_key(egui::Modifiers::COMMAND, Key::S),
                            number,
                        )
                    });
                if tab || back_tab {
                    let i = Scope::ALL
                        .iter()
                        .position(|s| *s == self.scope)
                        .unwrap_or(0);
                    let n = Scope::ALL.len();
                    self.scope = Scope::ALL[if tab { (i + 1) % n } else { (i + n - 1) % n }];
                    self.index = 0;
                }
                if down {
                    self.index = (self.index + 1).min(total.saturating_sub(1));
                }
                if up {
                    self.index = self.index.saturating_sub(1);
                }

                // The query: 54 px, a magnifier, the text, ADDRESS when it is one.
                let (row, _) = ui.allocate_exact_size(vec2(PALETTE_W, 54.0), Sense::hover());
                ui.painter().text(
                    pos2(row.left() + 16.0 + 9.5, row.center().y),
                    Align2::CENTER_CENTER,
                    icons::MAGNIFYING_GLASS,
                    style::icon_font(19.0),
                    p.text_faint,
                );
                let right_pad = if address.is_some() {
                    let w = badge_at(
                        ui.painter(),
                        pos2(row.right() - 16.0, row.center().y),
                        t("ADDRESS"),
                        &p,
                    );
                    w + 26.0
                } else {
                    16.0
                };
                let mut edit_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
                        pos2(row.left() + 16.0 + 19.0 + 14.0, row.top()),
                        pos2(row.right() - right_pad, row.bottom()),
                    )));
                let font = if address.is_some() {
                    style::mono(15.0)
                } else {
                    style::regular(16.0)
                };
                let r = edit_ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(
                            RichText::new(t("Search connections and commands, or type user@host…"))
                                .font(style::regular(16.0))
                                .color(p.text_faint),
                        )
                        .font(font)
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 16)))
                        .desired_width(f32::INFINITY),
                );
                ui.painter().hline(
                    row.x_range(),
                    row.bottom() - 0.5,
                    Stroke::new(1.0, p.border),
                );
                // The field keeps the keyboard: Tab switches the scope instead of moving focus.
                let nothing_focused = ui.memory(|m| m.focused().is_none());
                if !self.focused || tab || back_tab || (!r.has_focus() && nothing_focused) {
                    r.request_focus();
                    self.focused = true;
                }
                if r.changed() {
                    self.index = 0;
                }
                if r.has_focus() {
                    ui.memory_mut(|m| {
                        m.set_focus_lock_filter(
                            r.id,
                            egui::EventFilter {
                                vertical_arrows: true,
                                tab: true,
                                ..Default::default()
                            },
                        )
                    });
                }

                // The scopes.
                let (chips, _) = ui.allocate_exact_size(vec2(PALETTE_W, 32.0), Sense::hover());
                let mut x = chips.left() + 12.0;
                for scope in Scope::ALL {
                    let w = scope_chip(
                        ui,
                        pos2(x, chips.top() + 8.0),
                        scope,
                        scope == self.scope,
                        &p,
                    );
                    if w.1 {
                        self.scope = scope;
                        self.index = 0;
                        r.request_focus();
                    }
                    x += w.0 + 4.0;
                }

                // The list and the preview beside it.
                let (body, _) = ui.allocate_exact_size(vec2(PALETTE_W, BODY_H), Sense::hover());
                let list_rect = egui::Rect::from_min_max(
                    body.min,
                    pos2(body.right() - PREVIEW_W, body.bottom()),
                );
                let preview_rect =
                    egui::Rect::from_min_max(pos2(list_rect.right(), body.top()), body.max);
                ui.painter().rect_filled(
                    preview_rect,
                    0.0,
                    if p.dark {
                        Color32::from_white_alpha(4)
                    } else {
                        p.nav
                    },
                );
                ui.painter().vline(
                    list_rect.right() - 0.5,
                    body.y_range(),
                    Stroke::new(1.0, p.border),
                );
                let mut picked: Option<(Row, bool)> = None;
                let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
                egui::ScrollArea::vertical()
                    .id_salt("palette-list")
                    .max_height(BODY_H)
                    .auto_shrink([false, false])
                    .show(&mut list_ui, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                if total == 0 {
                                    ui.add_space(30.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            RichText::new(if data.store.bookmarks.is_empty() {
                                                t("No connections yet")
                                            } else {
                                                t("No matches")
                                            })
                                            .color(p.text_muted),
                                        );
                                    });
                                }
                                let mut n = 0;
                                for s in sections {
                                    let (head, _) = ui.allocate_exact_size(
                                        vec2(ui.available_width(), 30.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().text(
                                        pos2(head.left() + 10.0, head.bottom() - 4.0),
                                        Align2::LEFT_BOTTOM,
                                        &s.title,
                                        style::semibold(style::XS),
                                        p.text_faint,
                                    );
                                    for (i, row) in s.rows.iter().enumerate() {
                                        let selected = n == self.index;
                                        let number = (s.numbered && i < 9).then_some(i + 1);
                                        let resp = palette_row(
                                            ui,
                                            row,
                                            selected,
                                            data,
                                            &query,
                                            number,
                                            address.as_ref(),
                                        );
                                        if selected && (down || up) {
                                            resp.scroll_to_me(None);
                                        }
                                        if resp.clicked() {
                                            picked = Some((row.clone(), false));
                                        }
                                        if resp.hovered()
                                            && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
                                        {
                                            self.index = n;
                                        }
                                        n += 1;
                                    }
                                }
                            });
                    });
                let selected = flat.get(self.index).map(|(r, _)| (*r).clone());
                let mut preview_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(preview_rect.shrink(16.0)));
                if let Some(action) =
                    preview(&mut preview_ui, selected.as_ref(), data, address.as_ref())
                {
                    picked = Some(action);
                }

                // Keys.
                if let Some(n) = number {
                    if self.scope == Scope::Tabs {
                        if let Some(tab) = data.tabs.get(n) {
                            result = PaletteResult::Run(Action::SelectTab(tab.index));
                        }
                    } else if let Some((row, _)) = flat.iter().find(|(_, num)| *num == Some(n + 1))
                    {
                        picked = Some(((*row).clone(), false));
                    }
                }
                if enter || shift_enter {
                    picked = picked.or_else(|| selected.clone().map(|r| (r, shift_enter)));
                }
                if (cmd_enter || save)
                    && let Some(Row::ConnectTo(a) | Row::SaveAs(a)) = &selected
                {
                    result = PaletteResult::SaveAs(a.clone());
                    picked = None;
                }
                if edit {
                    let id = match &selected {
                        Some(Row::Host(id)) => Some(*id),
                        Some(Row::Tab(i)) => data.tabs.get(*i).map(|t| t.bookmark),
                        _ => None,
                    };
                    if let Some(id) = id.filter(|id| data.store.bookmark(*id).is_some()) {
                        result = PaletteResult::Edit(id);
                    }
                }
                if let Some((row, files)) = picked {
                    result = match row {
                        Row::Host(id) if files => PaletteResult::ConnectFiles(id),
                        Row::Host(id) => PaletteResult::Connect(id),
                        Row::ConnectTo(a) => PaletteResult::ConnectTo(a),
                        Row::SaveAs(a) => PaletteResult::SaveAs(a),
                        Row::Command(i) => PaletteResult::Run(data.commands[i].action.clone()),
                        Row::Tab(i) => match data.tabs.get(i) {
                            Some(tab) => PaletteResult::Run(Action::SelectTab(tab.index)),
                            None => PaletteResult::Close,
                        },
                    };
                } else if enter && total == 0 {
                    result = PaletteResult::Close;
                }

                // Key hints along the bottom.
                let (foot, _) = ui.allocate_exact_size(vec2(PALETTE_W, 36.0), Sense::hover());
                let painter = ui.painter();
                painter.hline(foot.x_range(), foot.top() + 0.5, Stroke::new(1.0, p.border));
                let cy = foot.center().y;
                let hint = |x: f32, keys: &[&str], what: &str| -> f32 {
                    let mut x = x;
                    for k in keys {
                        x += kit::paint_kbd_at(painter, pos2(x, cy), k, &p) + 3.0;
                    }
                    x += 3.0;
                    let g = painter.layout_no_wrap(
                        what.to_string(),
                        style::regular(style::SMALL),
                        p.text_faint,
                    );
                    let w = g.size().x;
                    painter.galley(pos2(x, cy - g.size().y / 2.0), g, p.text_faint);
                    x + w + 14.0
                };
                let mut x = foot.left() + 14.0;
                x = hint(x, &["↑", "↓"], t("navigate"));
                x = hint(x, &["↩"], t("open"));
                x = hint(x, &[&mod_label("⇧↩", "Shift+Enter")], t("files"));
                let _ = hint(x, &[&mod_label("⌘E", "Ctrl+E")], t("edit"));
                // Right-aligned: tab scope, esc close.
                let width = |keys: &str, what: &str| {
                    let k = painter
                        .layout_no_wrap(keys.into(), style::regular(style::XS), p.text_muted)
                        .size()
                        .x
                        + 10.0;
                    let w = painter
                        .layout_no_wrap(what.into(), style::regular(style::SMALL), p.text_faint)
                        .size()
                        .x;
                    k.max(18.0) + 6.0 + w
                };
                let esc_x = foot.right() - 14.0 - width("esc", t("close"));
                let tab_x = esc_x - 14.0 - width("tab", t("scope"));
                hint(tab_x, &["tab"], t("scope"));
                hint(esc_x, &["esc"], t("close"));
            });
        if modal.should_close() && matches!(result, PaletteResult::Open) {
            result = PaletteResult::Close;
        }
        result
    }
}

/// A small accent badge (`ADDRESS`) ending at `right`; returns its width.
fn badge_at(painter: &egui::Painter, right: egui::Pos2, text: &str, p: &style::Palette) -> f32 {
    let g = painter.layout_no_wrap(text.to_string(), style::semibold(10.5), p.accent_text);
    let w = g.size().x + 12.0;
    let rect = egui::Rect::from_min_max(
        pos2(right.x - w, right.y - 9.0),
        pos2(right.x, right.y + 9.0),
    );
    painter.rect_filled(rect, CornerRadius::same(5), p.accent_tint);
    painter.galley(
        pos2(rect.left() + 6.0, rect.center().y - g.size().y / 2.0),
        g,
        p.accent_text,
    );
    w
}

/// A scope chip at `pos` (24 px); returns its width and whether it was clicked.
fn scope_chip(
    ui: &mut Ui,
    pos: egui::Pos2,
    scope: Scope,
    on: bool,
    p: &style::Palette,
) -> (f32, bool) {
    let painter = ui.painter();
    let g = painter.layout_no_wrap(
        scope.label().to_string(),
        style::medium(style::SMALL),
        p.text,
    );
    let icon_w = if scope.icon().is_some() {
        15.0 + 6.0
    } else {
        0.0
    };
    let kbd_w = if scope == Scope::Actions {
        6.0 + 15.0
    } else {
        0.0
    };
    let w = 10.0 + icon_w + g.size().x + kbd_w + 10.0;
    let rect = egui::Rect::from_min_size(pos, vec2(w, 24.0));
    let resp = ui.interact(
        rect,
        egui::Id::new(("palette-scope", scope)),
        Sense::click(),
    );
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(12), p.accent_soft);
    } else {
        painter.rect_stroke(
            rect,
            CornerRadius::same(12),
            Stroke::new(
                1.0,
                if resp.hovered() {
                    p.border_strong
                } else {
                    p.border
                },
            ),
            StrokeKind::Inside,
        );
    }
    let color = if on || resp.hovered() {
        p.text
    } else {
        p.text_muted
    };
    let mut x = rect.left() + 10.0;
    if let Some(icon) = scope.icon() {
        painter.text(
            pos2(x + 7.5, rect.center().y),
            Align2::CENTER_CENTER,
            icon,
            style::icon_font(15.0),
            color,
        );
        x += icon_w;
    }
    let gw = g.size().x;
    painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    x += gw;
    if scope == Scope::Actions {
        let cap = egui::Rect::from_min_size(pos2(x + 6.0, rect.center().y - 7.5), vec2(15.0, 15.0));
        kit::paint_kbd(painter, cap, p);
        painter.text(
            cap.center(),
            Align2::CENTER_CENTER,
            ">",
            style::regular(9.5),
            p.text_muted,
        );
    }
    (w, resp.clicked())
}

/// A name with the matched letters in the accent colour (semibold).
fn highlighted(
    job: &mut egui::text::LayoutJob,
    text: &str,
    hits: &[usize],
    font: egui::FontId,
    bold: egui::FontId,
    color: Color32,
    accent: Color32,
) {
    for (i, c) in text.chars().enumerate() {
        let hit = hits.contains(&i);
        job.append(
            &c.to_string(),
            0.0,
            egui::TextFormat {
                font_id: if hit { bold.clone() } else { font.clone() },
                color: if hit { accent } else { color },
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
    }
}

/// Where `needle` occurs in `text` (char indices), for highlighting a typed host or user.
fn substring_hits(text: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    match lower.find(&needle.to_lowercase()) {
        Some(byte) => {
            let start = lower[..byte].chars().count();
            (start..start + needle.chars().count()).collect()
        }
        None => Vec::new(),
    }
}

/// One palette row (38 px): a 26 px tile (the host's glyph, or the command's icon), the title,
/// a faint detail, and on the right its key.
fn palette_row(
    ui: &mut Ui,
    row: &Row,
    selected: bool,
    data: &PaletteData,
    query: &str,
    number: Option<usize>,
    address: Option<&crate::bookmarks::model::Target>,
) -> egui::Response {
    let p = style::pal(ui);
    let dark = p.dark;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let painter = ui.painter();
    let row_fill = if selected {
        p.accent_soft
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, CornerRadius::same(8), row_fill);
    // The status dot is cut out of the row's own colour.
    let ring = over(p.sheet, row_fill);
    let tile = egui::Rect::from_min_size(
        pos2(rect.left() + 10.0, rect.center().y - 13.0),
        vec2(26.0, 26.0),
    );
    let neutral_tile = |icon: &str, tone: Tone| {
        let (fill, color) = match tone {
            Tone::Accent => (p.accent_tint, p.accent_text),
            _ => (p.hover, p.text_muted),
        };
        painter.rect_filled(tile, CornerRadius::same(7), fill);
        painter.text(
            tile.center(),
            Align2::CENTER_CENTER,
            icon,
            style::icon_font(14.0),
            color,
        );
    };
    let store = data.store;
    let (title, sub, sub_mono, keys): (egui::text::LayoutJob, String, bool, Vec<String>) = match row
    {
        Row::Host(id) => {
            let b = store.bookmark(*id);
            let name = b.map(|b| b.display_name()).unwrap_or_default();
            let label = b.and_then(|b| store.label_of(b));
            kit::paint_host_glyph(
                painter,
                tile,
                kit::host_icon(b.map_or(Default::default(), |b| b.host_icon())),
                label.map(|l| style::label_color(l.color, dark)),
                data.live.contains(id).then_some(p.ok),
                ring,
                &p,
            );
            let mut job = egui::text::LayoutJob::default();
            let hits = if query.is_empty() || address.is_some() {
                Vec::new()
            } else {
                search::match_positions(query, &name)
            };
            highlighted(
                &mut job,
                &name,
                &hits,
                style::medium(style::BODY),
                style::semibold(style::BODY),
                p.text,
                p.accent_text,
            );
            let keys = number.map(number_label).into_iter().collect();
            (job, b.map(|b| b.address()).unwrap_or_default(), true, keys)
        }
        Row::ConnectTo(a) => {
            neutral_tile(icons::ARROW_RIGHT, Tone::Accent);
            let mut job = egui::text::LayoutJob::default();
            let base = t("Connect to {}");
            let (before, after) = base.split_once("{}").unwrap_or((base, ""));
            let f = |font: egui::FontId| egui::TextFormat {
                font_id: font,
                color: p.text,
                valign: egui::Align::Center,
                ..Default::default()
            };
            job.append(before, 0.0, f(style::medium(style::BODY)));
            job.append(a, 0.0, f(style::medium(style::BODY)));
            job.append(after, 0.0, f(style::medium(style::BODY)));
            (job, String::new(), false, vec!["↩".to_string()])
        }
        Row::SaveAs(_) => {
            neutral_tile(icons::FLOPPY_DISK, Tone::Neutral);
            let mut job = egui::text::LayoutJob::default();
            highlighted(
                &mut job,
                t("Save as New Connection…"),
                &[],
                style::medium(style::BODY),
                style::medium(style::BODY),
                p.text,
                p.text,
            );
            (job, String::new(), false, vec![mod_label("⌘S", "Ctrl+S")])
        }
        Row::Command(i) => {
            let c = &data.commands[*i];
            neutral_tile(c.icon, Tone::Neutral);
            let mut job = egui::text::LayoutJob::default();
            let hits = if query.is_empty() {
                Vec::new()
            } else {
                search::match_positions(query, &c.label)
            };
            highlighted(
                &mut job,
                &c.label,
                &hits,
                style::medium(style::BODY),
                style::semibold(style::BODY),
                p.text,
                p.accent_text,
            );
            (
                job,
                c.detail.clone().unwrap_or_default(),
                c.quick,
                c.shortcut.clone().into_iter().collect(),
            )
        }
        Row::Tab(i) => {
            let tab = &data.tabs[*i];
            let b = store.bookmark(tab.bookmark);
            let label = b.and_then(|b| store.label_of(b));
            kit::paint_host_glyph(
                painter,
                tile,
                kit::host_icon(b.map_or(Default::default(), |b| b.host_icon())),
                label.map(|l| style::label_color(l.color, dark)),
                tab.connected.then_some(p.ok),
                ring,
                &p,
            );
            let mut job = egui::text::LayoutJob::default();
            let hits = if query.is_empty() {
                Vec::new()
            } else {
                search::match_positions(query, &tab.name)
            };
            highlighted(
                &mut job,
                &tab.name,
                &hits,
                style::medium(style::BODY),
                style::semibold(style::BODY),
                p.text,
                p.accent_text,
            );
            let keys = (tab.index < 9)
                .then(|| number_label(tab.index + 1))
                .into_iter()
                .collect();
            (job, tab.address.clone(), true, keys)
        }
    };
    // Keys on the right first.
    let mut right = rect.right() - 10.0;
    for k in keys.iter().rev() {
        let w = (painter
            .layout_no_wrap(k.clone(), style::regular(style::XS), p.text_muted)
            .size()
            .x
            + 10.0)
            .max(18.0);
        kit::paint_kbd_at(painter, pos2(right - w, rect.center().y), k, &p);
        right -= w + 6.0;
    }
    let clip = egui::Rect::from_min_max(rect.min, pos2(right - 6.0, rect.bottom()));
    let g = painter.layout_job(title);
    let gw = g.size().x;
    let x = tile.right() + 10.0;
    painter
        .with_clip_rect(clip)
        .galley(pos2(x, rect.center().y - g.size().y / 2.0), g, p.text);
    if !sub.is_empty() {
        let mut job = egui::text::LayoutJob::default();
        // A typed address highlights where it matches this host.
        let hits: Vec<usize> = match (row, address) {
            (Row::Host(_), Some(a)) => {
                let mut h = substring_hits(&sub, &a.host);
                if let Some(u) = &a.user
                    && sub.starts_with(&format!("{u}@"))
                {
                    h.extend(0..u.chars().count());
                }
                h
            }
            _ => Vec::new(),
        };
        let font = if sub_mono {
            style::mono(11.0)
        } else {
            style::regular(style::SMALL)
        };
        highlighted(
            &mut job,
            &sub,
            &hits,
            font.clone(),
            font,
            p.text_faint,
            p.accent_text,
        );
        let sg = painter.layout_job(job);
        painter.with_clip_rect(clip).galley(
            pos2(x + gw + 10.0, rect.center().y - sg.size().y / 2.0),
            sg,
            p.text_faint,
        );
    }
    resp
}

/// `top` (maybe translucent) painted over the opaque `base`.
fn over(base: Color32, top: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    let [tr, tg, tb, _] = top.to_srgba_unmultiplied();
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    Color32::from_rgb(mix(base.r(), tr), mix(base.g(), tg), mix(base.b(), tb))
}

/// The preview of the selected row, in the palette's right column. Returns a row to open when
/// one of its buttons was clicked (and whether to open its files).
fn preview(
    ui: &mut Ui,
    selected: Option<&Row>,
    data: &PaletteData,
    address: Option<&crate::bookmarks::model::Target>,
) -> Option<(Row, bool)> {
    let p = style::pal(ui);
    let store = data.store;
    let mut picked = None;
    ui.spacing_mut().item_spacing.y = 12.0;
    let bottom_buttons = |ui: &mut Ui, n: usize| {
        // Room for the buttons at the bottom.
        let h = n as f32 * 30.0 + (n.saturating_sub(1)) as f32 * 6.0;
        let space = ui.available_height() - h;
        if space > 0.0 {
            ui.add_space(space);
        }
    };
    let full_button = wide_button;
    let host_id = match selected {
        Some(Row::Host(id)) => Some(*id),
        Some(Row::Tab(i)) => data.tabs.get(*i).map(|t| t.bookmark),
        _ => None,
    };
    if let Some(b) = host_id.and_then(|id| store.bookmark(id)) {
        let label = store.label_of(b).cloned();
        let label_color = label.as_ref().map(|l| style::label_color(l.color, p.dark));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            kit::host_glyph(
                ui,
                40.0,
                kit::host_icon(b.host_icon()),
                label_color,
                data.live.contains(&b.id).then_some(p.ok),
                if p.dark {
                    Color32::from_rgb(0x1e, 0x1e, 0x1e)
                } else {
                    p.nav
                },
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add(
                    egui::Label::new(
                        RichText::new(b.display_name())
                            .font(style::semibold(14.0))
                            .color(p.text),
                    )
                    .truncate(),
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(b.address())
                            .font(style::mono(11.0))
                            .color(p.text_faint),
                    )
                    .truncate(),
                );
            });
        });
        let has_chips = label.as_ref().is_some_and(|l| !l.tag().is_empty()) || !b.tags.is_empty();
        if has_chips {
            let w = ui.available_width();
            ui.allocate_ui_with_layout(
                vec2(w, 22.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_clip_rect(ui.max_rect());
                    ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                    kit::label_tag(ui, label.as_ref(), false);
                    for tag in b.tags.iter().take(4) {
                        tag_chip(ui, tag, &p);
                    }
                },
            );
        }
        let login = match &b.auth {
            crate::bookmarks::Auth::Password => t("Password").to_string(),
            crate::bookmarks::Auth::Key { path } => {
                let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
                tf("Key file · {}", &[file])
            }
            crate::bookmarks::Auth::KeychainKey => t("Pasted key").to_string(),
            crate::bookmarks::Auth::Agent { .. } => t("SSH agent").to_string(),
        };
        let mut rows: Vec<(&str, String, Option<Color32>)> = vec![(t("Login"), login, None)];
        let group = store.group_path(b.group);
        if !group.is_empty() {
            rows.push((t("Group"), group, None));
        }
        if let Some(j) = b.jump.and_then(|j| store.bookmark(j)) {
            rows.push((t("Via"), j.display_name(), None));
        }
        if !b.forwards.is_empty() {
            let ports: Vec<String> = b
                .forwards
                .iter()
                .map(|f| f.listen_port.to_string())
                .collect();
            rows.push((
                t("Forwards"),
                format!("{} ({})", b.forwards.len(), ports.join(", ")),
                None,
            ));
        }
        if let Some((_, when)) = data.recent.iter().find(|(id, _)| *id == b.id) {
            rows.push((t("Last"), when.clone(), None));
        }
        if let Some(n) = data.open.get(&b.id).filter(|n| **n > 0) {
            let text = if *n == 1 {
                t("1 tab open").to_string()
            } else {
                tf("{} tabs open", &[&n.to_string()])
            };
            rows.push((t("Open"), text, None));
        }
        kv(ui, &rows);
        bottom_buttons(ui, 2);
        ui.spacing_mut().item_spacing.y = 6.0;
        let row = match selected {
            Some(Row::Tab(i)) => Row::Tab(*i),
            _ => Row::Host(b.id),
        };
        if full_button(ui, kit::Kind::Primary, t("Connect"), "↩") {
            picked = Some((row, false));
        }
        if full_button(
            ui,
            kit::Kind::Secondary,
            t("Open Files"),
            &mod_label("⇧↩", "Shift+Enter"),
        ) {
            picked = Some((Row::Host(b.id), true));
        }
        return picked;
    }
    match selected {
        Some(Row::ConnectTo(a) | Row::SaveAs(a)) => {
            ui.label(style::section_label(t("Not saved"), p.text_faint));
            if let Some(target) = address {
                let rows: Vec<(&str, String, Option<Color32>)> = vec![
                    (
                        t("User"),
                        target.user.clone().unwrap_or_else(|| "—".to_string()),
                        None,
                    ),
                    (t("Host"), target.host.clone(), None),
                    (
                        t("Port"),
                        target
                            .port
                            .unwrap_or(crate::bookmarks::DEFAULT_PORT)
                            .to_string(),
                        None,
                    ),
                    (t("Login"), t("asks when connecting").to_string(), None),
                ];
                kv_mixed(ui, &rows, 3);
                // A saved connection to the same machine with another user or port.
                if let Some(other) = store.bookmarks.iter().find(|b| {
                    b.host.eq_ignore_ascii_case(&target.host)
                        && (target.user.as_ref().is_some_and(|u| *u != b.username)
                            || target.port.unwrap_or(crate::bookmarks::DEFAULT_PORT) != b.port)
                }) {
                    kit::callout(
                        ui,
                        icons::INFO,
                        Tone::Neutral,
                        &tf(
                            "Same host as {} on another user and port.",
                            &[&other.display_name()],
                        ),
                    );
                }
            }
            bottom_buttons(ui, 1);
            if full_button(ui, kit::Kind::Primary, t("Connect"), "↩") {
                picked = Some((Row::ConnectTo(a.clone()), false));
            }
        }
        Some(Row::Command(i)) => {
            let c = &data.commands[*i];
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (r, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(11), p.hover);
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    c.icon,
                    style::icon_font(20.0),
                    p.text_muted,
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(&c.label)
                            .font(style::semibold(14.0))
                            .color(p.text),
                    )
                    .wrap(),
                );
            });
            if let Some(d) = &c.detail {
                egui::Frame::NONE
                    .fill(p.well)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            egui::Label::new(
                                RichText::new(d)
                                    .font(if c.quick {
                                        style::mono(11.5)
                                    } else {
                                        style::regular(style::SMALL)
                                    })
                                    .color(p.text),
                            )
                            .wrap(),
                        );
                    });
            }
            if let Some(k) = &c.shortcut {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(
                        RichText::new(t("Shortcut"))
                            .font(style::regular(style::SMALL))
                            .color(p.text_faint),
                    );
                    kit::kbd(ui, k);
                });
            }
            bottom_buttons(ui, 1);
            if full_button(ui, kit::Kind::Primary, t("Run"), "↩") {
                picked = Some((Row::Command(*i), false));
            }
        }
        _ => {}
    }
    picked
}

/// A button as wide as its column (30 px): the label and its key cap, centred.
fn wide_button(ui: &mut Ui, kind: kit::Kind, text: &str, key: &str) -> bool {
    let p = style::pal(ui);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), style::CONTROL_H), Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let primary = kind == kit::Kind::Primary;
    let (fill, stroke, color) = if primary {
        (
            if pressed {
                p.accent_press
            } else if hovered {
                p.accent_hover
            } else {
                p.accent
            },
            Stroke::NONE,
            p.on_accent,
        )
    } else {
        (
            if pressed {
                p.subtle
            } else if hovered {
                p.hover
            } else {
                p.button
            },
            Stroke::new(
                1.0,
                if hovered {
                    p.border_strong
                } else {
                    p.button_border
                },
            ),
            p.text,
        )
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(style::RADIUS),
        fill,
        stroke,
        StrokeKind::Inside,
    );
    if primary {
        painter.hline(
            rect.x_range().shrink(3.0),
            rect.top() + 0.5,
            Stroke::new(1.0, Color32::from_white_alpha(36)),
        );
    }
    let g = painter.layout_no_wrap(text.to_string(), style::medium(style::BODY), color);
    let k = painter.layout_no_wrap(key.to_string(), style::regular(style::XS), color);
    let kw = (k.size().x + 10.0).max(20.0);
    let total = g.size().x + 6.0 + kw;
    let x = rect.center().x - total / 2.0;
    let gw = g.size().x;
    painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    let cap = egui::Rect::from_min_size(pos2(x + gw + 6.0, rect.center().y - 9.0), vec2(kw, 18.0));
    painter.rect_filled(
        cap,
        CornerRadius::same(4),
        if primary {
            Color32::from_white_alpha(36)
        } else {
            p.hover
        },
    );
    painter.galley(
        cap.center() - k.size() / 2.0,
        k,
        if primary {
            color.gamma_multiply(0.85)
        } else {
            p.text_faint
        },
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// A tag chip (22 px, square corners).
fn tag_chip(ui: &mut Ui, text: &str, p: &style::Palette) {
    let g =
        ui.painter()
            .layout_no_wrap(text.to_string(), style::regular(style::SMALL), p.text_muted);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 16.0, 22.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(5), p.hover);
    ui.painter().galley(
        pos2(rect.left() + 8.0, rect.center().y - g.size().y / 2.0),
        g,
        p.text_muted,
    );
}

/// Label / value rows of the preview (12 px; labels in a 74 px column).
fn kv(ui: &mut Ui, rows: &[(&str, String, Option<Color32>)]) {
    kv_font(ui, rows, style::regular(style::SMALL));
}

fn kv_font(ui: &mut Ui, rows: &[(&str, String, Option<Color32>)], font: egui::FontId) {
    kv_rows(ui, rows, 0, font);
}

/// [`kv`] with the first `mono` values in the mono font (an address split into its parts).
fn kv_mixed(ui: &mut Ui, rows: &[(&str, String, Option<Color32>)], mono: usize) {
    kv_rows(ui, rows, mono, style::regular(style::SMALL));
}

fn kv_rows(ui: &mut Ui, rows: &[(&str, String, Option<Color32>)], mono: usize, font: egui::FontId) {
    let p = style::pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 7.0;
        for (i, (k, v, color)) in rows.iter().enumerate() {
            let font = if i < mono {
                style::mono(11.5)
            } else {
                font.clone()
            };
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
            ui.painter().text(
                pos2(r.left(), r.center().y),
                Align2::LEFT_CENTER,
                *k,
                style::regular(style::SMALL),
                p.text_faint,
            );
            let value = egui::Rect::from_min_max(pos2(r.left() + 84.0, r.top()), r.max);
            let g = ui
                .painter()
                .layout_no_wrap(v.clone(), font.clone(), color.unwrap_or(p.text));
            ui.painter().with_clip_rect(value).galley(
                pos2(value.left(), r.center().y - g.size().y / 2.0),
                g,
                color.unwrap_or(p.text),
            );
        }
    });
}

// ----- simple dialogs -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmAction {
    DeleteBookmark(Uuid),
    DeleteGroup(Uuid),
    /// Tab id.
    CloseTab(u64),
    /// Tab id.
    Reconnect(u64),
    /// Quit so the downloaded update can install.
    RestartForUpdate,
    Quit,
}

pub enum GroupTarget {
    New { parent: Option<Uuid> },
    Rename(Uuid),
}

pub enum Dialog {
    Confirm {
        title: String,
        message: String,
        /// What is affected, one row each (running programs, transfers, edits, what is kept).
        details: Vec<ConfirmRow>,
        button: String,
        action: ConfirmAction,
        /// When it first appeared (an Enter typed just before must not confirm it).
        shown_at: Option<Instant>,
        /// "Don't ask again" (ticked or not), for closing a connected tab.
        dont_ask: Option<bool>,
        /// The header line, the before → after tree, the keys.
        extra: Box<ConfirmExtra>,
    },
    GroupName {
        target: GroupTarget,
        /// The parent group's path, said under the title.
        context: String,
        name: String,
        focus: bool,
    },
    Export(ExportDialog),
    Import(ImportDialog),
    Settings(SettingsDialog),
}

pub enum DialogResult {
    Open,
    Close,
    Confirmed(ConfirmAction),
    GroupName(GroupTarget, String),
    Export {
        scope: ExportScope,
    },
    Import(Box<ImportPlan>),
    /// A button in a dialog asked for an app action (Settings → Check Now).
    Run(Action),
    /// A link in a confirmation ("Review"): close it and run the action.
    RunAndClose(Action),
}

/// For dialogs holding typed input: only Esc closes them, not a click beside the dialog.
pub fn escape_only<T>(ctx: &egui::Context, modal: &egui::ModalResponse<T>) -> bool {
    ctx.input(|i| i.key_pressed(Key::Escape)) && modal.should_close()
}

pub fn confirm(title: &str, message: String, button: &str, action: ConfirmAction) -> Dialog {
    confirm_details(title, message, Vec::<ConfirmRow>::new(), button, action)
}

/// A confirmation listing what is affected.
pub fn confirm_details(
    title: &str,
    message: String,
    details: Vec<impl Into<ConfirmRow>>,
    button: &str,
    action: ConfirmAction,
) -> Dialog {
    confirm_rows(
        title,
        message,
        details.into_iter().map(Into::into).collect(),
        button,
        action,
        ConfirmExtra::default(),
        false,
    )
}

/// The question before closing a connected tab, with "Don't ask again".
pub fn confirm_close(
    title: &str,
    message: String,
    details: Vec<impl Into<ConfirmRow>>,
    button: &str,
    action: ConfirmAction,
) -> Dialog {
    confirm_rows(
        title,
        message,
        details.into_iter().map(Into::into).collect(),
        button,
        action,
        ConfirmExtra::default(),
        true,
    )
}

/// A confirmation with everything: rows, the header line or tree, and "Don't ask again".
pub fn confirm_rows(
    title: &str,
    message: String,
    details: Vec<ConfirmRow>,
    button: &str,
    action: ConfirmAction,
    extra: ConfirmExtra,
    dont_ask: bool,
) -> Dialog {
    Dialog::Confirm {
        title: title.into(),
        message,
        details,
        button: button.into(),
        action,
        shown_at: None,
        dont_ask: dont_ask.then_some(false),
        extra: Box::new(extra),
    }
}

impl Dialog {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        store: &BookmarkStore,
        settings: &mut Settings,
    ) -> DialogResult {
        match self {
            Dialog::Confirm {
                title,
                message,
                details,
                button,
                action,
                shown_at,
                dont_ask,
                extra,
            } => {
                let mut result = DialogResult::Open;
                // Enter confirms only when pressed (not auto-repeated, no modifiers) once the
                // dialog has been up for a moment, and only while it is on top.
                let settled = shown_at.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_millis(400);
                let id = egui::Id::new("confirm");
                let on_top = ctx.memory(|m| m.top_modal_layer().is_none_or(|l| l.id == id));
                let enter = settled
                    && on_top
                    && ctx.input(|i| {
                        i.events.iter().any(|e| {
                            matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, repeat: false, modifiers, .. } if modifiers.is_none())
                        })
                    });
                let destructive = !matches!(
                    action,
                    ConfirmAction::Reconnect(_) | ConfirmAction::RestartForUpdate
                );
                let (icon, tone) = extra.icon.unwrap_or(match action {
                    ConfirmAction::DeleteBookmark(_) => (icons::TRASH, Tone::Err),
                    ConfirmAction::DeleteGroup(_) => (icons::FOLDER_MINUS, Tone::Warn),
                    ConfirmAction::CloseTab(_) => (icons::TERMINAL_WINDOW, Tone::Neutral),
                    ConfirmAction::Reconnect(_) => (icons::ARROWS_CLOCKWISE, Tone::Accent),
                    ConfirmAction::RestartForUpdate => (icons::ARROW_CIRCLE_UP, Tone::Accent),
                    ConfirmAction::Quit => (icons::POWER, Tone::Neutral),
                });
                let width = extra.width.unwrap_or(420.0);
                let modal = style::modal(ctx, id).show(ctx, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let p = style::pal(ui);
                    confirm::header(ui, icon, tone, title, extra.sub.as_ref());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 20,
                            right: 20,
                            top: 14,
                            bottom: 16,
                        })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 12.0;
                            if !message.is_empty() {
                                let mut job =
                                    confirm::rich_job(message, style::BODY, p.text_muted, p.text);
                                for s in &mut job.sections {
                                    s.format.line_height = Some(20.0);
                                }
                                ui.add(egui::Label::new(job).wrap());
                            }
                            if let Some(tree) = &extra.tree {
                                confirm::tree(ui, tree);
                            }
                            if !details.is_empty()
                                && let Some(a) = confirm::rows_card(ui, details)
                            {
                                result = DialogResult::RunAndClose(a);
                            }
                            if let Some(ticked) = dont_ask {
                                let r = style::checkbox(
                                    ui,
                                    ticked,
                                    RichText::new(t("Don't ask again")).color(p.text_muted),
                                )
                                .on_hover_text(t("You can turn it back on in Settings."));
                                #[cfg(debug_assertions)]
                                crate::devtest::record("confirm-dont-ask", r.rect);
                                if r.changed() {
                                    r.surrender_focus();
                                }
                            }
                        });
                    let escape = extra.escape.as_deref().unwrap_or(t("to cancel"));
                    let keys = extra.keys.as_deref().unwrap_or("↩");
                    let (ok, cancel) =
                        confirm::footer(ui, escape, button.as_str(), keys, destructive);
                    #[cfg(debug_assertions)]
                    crate::devtest::record("confirm-ok", ok.rect);
                    #[cfg(debug_assertions)]
                    crate::devtest::record("confirm-cancel", cancel.rect);
                    if ok.clicked() || enter {
                        result = DialogResult::Confirmed(action.clone());
                    }
                    if cancel.clicked() {
                        result = DialogResult::Close;
                    }
                });
                if modal.should_close() && matches!(result, DialogResult::Open) {
                    result = DialogResult::Close;
                }
                if matches!(result, DialogResult::Confirmed(_)) && *dont_ask == Some(true) {
                    settings.confirm_close_connected = false;
                }
                result
            }
            Dialog::GroupName {
                target,
                context,
                name,
                focus,
            } => {
                let mut result = DialogResult::Open;
                let mut submit = false;
                let modal = style::modal(ctx, egui::Id::new("group-name")).show(ctx, |ui| {
                    ui.set_width(380.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let (title, button) = match target {
                        GroupTarget::New { .. } => (t("New Group"), t("Create")),
                        GroupTarget::Rename(_) => (t("Rename Group"), t("Rename")),
                    };
                    let sub = (!context.is_empty()).then_some(context.as_str());
                    kit::dialog_header(
                        ui,
                        Some((icons::FOLDER_PLUS, Tone::Neutral)),
                        title,
                        sub,
                        false,
                    );
                    kit::dialog_body(ui, |ui| {
                        let r = ui.add(style::input(name).hint_text(t("Group name")));
                        if std::mem::take(focus) {
                            r.request_focus();
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            submit = true;
                        }
                    });
                    kit::dialog_footer(
                        ui,
                        |_| {},
                        |ui| {
                            if ui
                                .add_enabled(!name.trim().is_empty(), style::primary(button))
                                .clicked()
                            {
                                submit = true;
                            }
                            if kit::secondary(ui, None, t("Cancel")).clicked() {
                                result = DialogResult::Close;
                            }
                        },
                    );
                });
                if submit && !name.trim().is_empty() {
                    let target = std::mem::replace(target, GroupTarget::New { parent: None });
                    return DialogResult::GroupName(target, name.trim().to_string());
                }
                if modal.should_close() && matches!(result, DialogResult::Open) {
                    result = DialogResult::Close;
                }
                result
            }
            Dialog::Export(d) => d.show(ctx, store),
            Dialog::Import(d) => d.show(ctx),
            Dialog::Settings(d) => d.show(ctx, settings),
        }
    }
}

// ----- export ---------------------------------------------------------------------------------

pub struct ExportDialog {
    pub scope: ExportScope,
    error: Option<String>,
    picked: HashSet<Uuid>,
    show_list: bool,
}

impl ExportDialog {
    pub fn new(scope: ExportScope) -> Self {
        let picked: HashSet<Uuid> = match &scope {
            ExportScope::Selected(ids) => ids.iter().copied().collect(),
            _ => HashSet::new(),
        };
        let show_list = !picked.is_empty();
        Self {
            scope,
            error: None,
            picked,
            show_list,
        }
    }

    fn show(&mut self, ctx: &egui::Context, store: &BookmarkStore) -> DialogResult {
        let mut result = DialogResult::Open;
        let modal = style::modal(ctx, egui::Id::new("export")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            if kit::dialog_header(ui, Some((icons::EXPORT, Tone::Neutral)), t("Export connections"), Some(t("A JSON file you can import on another computer.")), true) {
                result = DialogResult::Close;
            }
            kit::dialog_body(ui, |ui| {
                ui.add_space(2.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let groups_used = store.groups.len();
                let all = kit::radio_card(
                    ui,
                    self.scope == ExportScope::All,
                    t("All connections"),
                    Some(&tf("{} connections in {} groups", &[&store.bookmarks.len().to_string(), &groups_used.to_string()])),
                    |_| {},
                );
                if all.clicked() {
                    self.scope = ExportScope::All;
                }
                let group = match &self.scope {
                    ExportScope::Group(g) => Some(*g),
                    _ => None,
                };
                let one = kit::radio_card(ui, group.is_some(), t("One group"), None, |ui| {
                    ui.add_enabled_ui(!store.groups.is_empty(), |ui| {
                        let label = group.map(|g| store.group_path(Some(g))).unwrap_or_else(|| t("(choose)").into());
                        egui::ComboBox::from_id_salt("export-group")
                            .icon(kit::combo_caret)
                            .selected_text(label)
                            .width(180.0)
                            .show_ui(ui, |ui| {
                                for g in &store.groups {
                                    if ui.selectable_label(group == Some(g.id), store.group_path(Some(g.id))).clicked() {
                                        self.scope = ExportScope::Group(g.id);
                                    }
                                }
                            });
                    });
                });
                if one.clicked() && group.is_none() && let Some(g) = store.groups.first() {
                    self.scope = ExportScope::Group(g.id);
                }
                let selected_mode = matches!(self.scope, ExportScope::Selected(_));
                let sel = kit::radio_card(ui, selected_mode, t("Selected connections"), Some(t("Pick from a list")), |ui| {
                    kit::icon(ui, if self.show_list && selected_mode { icons::CARET_DOWN } else { icons::CARET_RIGHT }, 14.0, p.text_faint);
                });
                if sel.clicked() {
                    if !selected_mode {
                        self.scope = ExportScope::Selected(self.picked.iter().copied().collect());
                        self.show_list = true;
                    } else {
                        self.show_list = !self.show_list;
                    }
                }
                if selected_mode && self.show_list {
                    egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for b in &store.bookmarks {
                            let mut on = self.picked.contains(&b.id);
                            let label = RichText::new(format!("{}  ", b.display_name())).color(p.text);
                            if style::checkbox(ui, &mut on, label).changed() {
                                if on {
                                    self.picked.insert(b.id);
                                } else {
                                    self.picked.remove(&b.id);
                                }
                            }
                        }
                    });
                }
                if selected_mode {
                    self.scope = ExportScope::Selected(store.bookmarks.iter().filter(|b| self.picked.contains(&b.id)).map(|b| b.id).collect());
                }
                ui.add_space(6.0);
                kit::callout_rich(ui, icons::WARNING, Tone::Warn, |ui| {
                    ui.label(RichText::new(t("Passwords and keys are included as plain text.")).font(style::semibold(style::SMALL)).color(p.text));
                    ui.add(egui::Label::new(RichText::new(t("Anyone with the file can log in to these servers. Keep it safe and delete it once it's imported. The file is readable only by your user.")).font(style::regular(style::SMALL)).color(p.text_muted)).wrap());
                });
                if let Some(e) = &self.error {
                    style::hint_tone(ui, e.as_str(), Tone::Err);
                }
            });
            kit::dialog_footer(ui, |_| {}, |ui| {
                if kit::primary(ui, t("Export…")).clicked() {
                    if matches!(&self.scope, ExportScope::Selected(ids) if ids.is_empty()) {
                        self.error = Some(t("Select at least one connection.").into());
                    } else {
                        result = DialogResult::Export { scope: self.scope.clone() };
                    }
                }
                if kit::secondary(ui, None, t("Cancel")).clicked() {
                    result = DialogResult::Close;
                }
            });
        });
        if escape_only(ctx, &modal) && matches!(result, DialogResult::Open) {
            result = DialogResult::Close;
        }
        result
    }
}

// ----- import ---------------------------------------------------------------------------------

pub struct ImportDialog {
    pub plan: ImportPlan,
    pub source: String,
    passphrase: String,
    unlock_message: Option<(String, bool)>,
    /// Key derivation runs on a worker thread (it takes a moment by design).
    unlocking: Option<std::sync::mpsc::Receiver<anyhow::Result<Vec<u8>>>>,
}

impl ImportDialog {
    pub fn new(plan: ImportPlan, source: String) -> Self {
        Self {
            plan,
            source,
            passphrase: String::new(),
            unlock_message: None,
            unlocking: None,
        }
    }

    fn start_unlock(&mut self, ctx: &egui::Context) {
        let Some(blob) = self.plan.encrypted_credentials.clone() else {
            return;
        };
        let passphrase = self.passphrase.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(crate::crypto::decrypt(&passphrase, &blob));
            ctx.request_repaint();
        });
        self.unlocking = Some(rx);
        self.unlock_message = None;
    }

    fn poll_unlock(&mut self) {
        let Some(rx) = &self.unlocking else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("{}", t("Decryption failed.")))
            }
        };
        self.unlocking = None;
        self.unlock_message = Some(
            match result.and_then(|plain| self.plan.attach_credentials(&plain)) {
                Ok(n) => (
                    tf(
                        "Unlocked credentials for {} connection(s).",
                        &[&n.to_string()],
                    ),
                    false,
                ),
                Err(e) => (format!("{e:#}"), true),
            },
        );
    }

    fn show(&mut self, ctx: &egui::Context) -> DialogResult {
        self.poll_unlock();
        let mut result = DialogResult::Open;
        let screen = ctx.content_rect();
        let modal = style::modal(ctx, egui::Id::new("import")).show(ctx, |ui| {
            ui.set_width(760.0f32.min(screen.width() - 48.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            let kind = match self.plan.kind {
                SourceKind::TernaryShell => "TernaryShell",
                SourceKind::Electerm => "Electerm",
                SourceKind::SshConfig => "SSH config",
            };
            let sub = tf("from {} · {}", &[&self.source, kind]);
            if kit::dialog_header(ui, Some((icons::DOWNLOAD_SIMPLE, Tone::Neutral)), t("Import connections"), Some(&sub), true) {
                result = DialogResult::Close;
            }
            kit::dialog_body(ui, |ui| {
                ui.add_space(2.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                // The numbers.
                let new = self.plan.items.iter().filter(|i| i.status == ItemStatus::New).count();
                let changed = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Conflict { matched_by: MatchedBy::Id, .. })).count();
                let here = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Identical { .. })).count();
                let conflicts = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Conflict { matched_by: MatchedBy::Endpoint, .. })).count();
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let w = (ui.available_width() - 30.0) / 4.0;
                    for (n, label, color) in [
                        (new, t("new"), p.ok),
                        (changed, t("changed"), p.info),
                        (here, t("already here"), p.text),
                        (conflicts, t("conflicts"), p.warn),
                    ] {
                        let (r, _) = ui.allocate_exact_size(vec2(w, 62.0), Sense::hover());
                        let painter = ui.painter();
                        painter.rect(r, CornerRadius::same(style::RADIUS_LG), p.card, Stroke::new(1.0, p.border), StrokeKind::Inside);
                        painter.text(pos2(r.left() + 12.0, r.top() + 10.0), Align2::LEFT_TOP, n.to_string(), style::semibold(20.0), color);
                        painter.text(pos2(r.left() + 12.0, r.top() + 38.0), Align2::LEFT_TOP, label, style::regular(style::SMALL), p.text_faint);
                    }
                });
                for w in &self.plan.warnings {
                    ui.add_space(12.0);
                    kit::callout(ui, icons::WARNING, Tone::Warn, w);
                }
                if self.plan.encrypted_credentials.is_some() {
                    ui.add_space(12.0);
                    kit::callout_rich(ui, icons::LOCK_KEY, Tone::Info, |ui| {
                        ui.add(egui::Label::new(RichText::new(t("This file contains encrypted passwords and keys. Enter the export passphrase to import them, or continue without them.")).font(style::regular(style::SMALL)).color(p.text)).wrap());
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let busy = self.unlocking.is_some();
                            let r = ui.add_enabled(!busy, style::input(&mut self.passphrase).password(true).hint_text(t("Passphrase")).desired_width(240.0));
                            #[cfg(debug_assertions)]
                            crate::devtest::record("import-passphrase", r.rect);
                            let go = ui.add_enabled_ui(!busy, |ui| kit::secondary_sm(ui, None, t("Unlock"))).inner.clicked()
                                || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)));
                            if busy {
                                kit::spinner(ui, 14.0);
                            } else if go && !self.passphrase.is_empty() {
                                self.start_unlock(ui.ctx());
                            }
                        });
                        if let Some((msg, err)) = &self.unlock_message {
                            style::hint_tone(ui, msg.as_str(), if *err { Tone::Err } else { Tone::Ok });
                        }
                    });
                }
                let missing = self.plan.keys_missing();
                if missing > 0 {
                    ui.add_space(12.0);
                    kit::callout(ui, icons::WARNING, Tone::Warn, &tf("{} connection(s) log in with a pasted key that this file doesn't include. Paste the key again in Edit Connection before connecting (or choose a key file).", &[&missing.to_string()]));
                }
                // The list.
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.set_height(26.0);
                    ui.label(RichText::new(tf("{} connections", &[&self.plan.items.len().to_string()])).font(style::semibold(style::BODY)).color(p.text));
                    if self.plan.conflicts() > 0 {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let res: Vec<Resolution> = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Conflict { .. })).map(|i| i.resolution).collect();
                            let mut current = if res.iter().all(|r| *r == res[0]) { Some(res[0]) } else { None };
                            let before = current;
                            kit::segmented(ui, &mut current, &[(Some(Resolution::KeepBoth), t("Keep both")), (Some(Resolution::Overwrite), t("Overwrite")), (Some(Resolution::Skip), t("Skip"))].iter().rev().copied().collect::<Vec<_>>(), true);
                            if current != before && let Some(r) = current {
                                self.plan.resolve_all_conflicts(r);
                            }
                            ui.label(RichText::new(t("For all conflicts")).font(style::regular(style::SMALL)).color(p.text_faint));
                        });
                    }
                });
                ui.add_space(6.0);
                let list_h = (screen.height() - 470.0).clamp(140.0, 320.0);
                egui::Frame::NONE
                    .fill(p.card)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(style::RADIUS_LG))
                    .inner_margin(egui::Margin::symmetric(4, 0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let width = ui.available_width();
                        let cols = [0.26, 0.34, 0.18];
                        let col_x = |i: usize| -> f32 { cols[..i].iter().sum::<f32>() * (width - 150.0) / cols.iter().sum::<f32>() };
                        let head = |ui: &mut Ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
                            let painter = ui.painter();
                            for (i, h) in [t("Connection"), t("Address"), t("Group")].iter().enumerate() {
                                painter.text(pos2(r.left() + 10.0 + col_x(i), r.center().y), Align2::LEFT_CENTER, h, style::medium(style::SMALL), p.text_faint);
                            }
                            painter.text(pos2(r.right() - 150.0 + 10.0, r.center().y), Align2::LEFT_CENTER, t("Action"), style::medium(style::SMALL), p.text_faint);
                            painter.hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.border));
                        };
                        head(ui);
                        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let n = self.plan.items.len();
                            for (i, item) in self.plan.items.iter_mut().enumerate() {
                                let (r, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
                                let identical = matches!(item.status, ItemStatus::Identical { .. });
                                let name_color = if identical { p.text_faint } else { p.text };
                                let painter = ui.painter();
                                let clip = |i: usize| egui::Rect::from_min_max(pos2(r.left() + col_x(i), r.top()), pos2(r.left() + col_x(i + 1).min(width - 150.0), r.bottom()));
                                let mut name = item.incoming.display_name();
                                if !item.secrets.is_empty() {
                                    name.push_str(&format!("  {}", icons::KEY));
                                }
                                painter.with_clip_rect(clip(0)).text(pos2(r.left() + 10.0 + col_x(0), r.center().y), Align2::LEFT_CENTER, name, style::regular(style::BODY), name_color);
                                let (address, color) = match (&item.status, &item.existing_address) {
                                    (ItemStatus::Conflict { .. }, Some(old)) => (format!("{old} → {}", item.incoming.address()), p.warn),
                                    _ => (item.incoming.address(), p.text_faint),
                                };
                                painter.with_clip_rect(clip(1)).text(pos2(r.left() + 10.0 + col_x(1), r.center().y), Align2::LEFT_CENTER, address, style::mono(12.0), color);
                                painter.with_clip_rect(clip(2)).text(pos2(r.left() + 10.0 + col_x(2), r.center().y), Align2::LEFT_CENTER, &item.group_path, style::regular(style::BODY), if identical { p.text_faint } else { p.text });
                                let mut cell = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(egui::Rect::from_min_max(pos2(r.right() - 150.0 + 10.0, r.top()), pos2(r.right() - 4.0, r.bottom())))
                                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                );
                                cell.spacing_mut().item_spacing.x = 6.0;
                                match item.status {
                                    ItemStatus::New => {
                                        kit::badge(&mut cell, t("Add"), Tone::Ok, None);
                                    }
                                    ItemStatus::Identical { .. } => {
                                        cell.label(RichText::new(t("Already here")).font(style::regular(style::SMALL)).color(p.text_faint));
                                    }
                                    ItemStatus::Conflict { matched_by, .. } => {
                                        let (label, tone, why) = match matched_by {
                                            MatchedBy::Id => (t("Update"), Tone::Info, t("same connection, changed")),
                                            MatchedBy::Endpoint => (t("Conflict"), Tone::Warn, t("same host, port and user")),
                                        };
                                        kit::badge(&mut cell, label, tone, None).on_hover_text(why);
                                        cell.scope(|ui| {
                                            ui.spacing_mut().interact_size.y = 24.0;
                                            egui::ComboBox::from_id_salt(("res", i))
                                                .icon(kit::combo_caret)
                                                .selected_text(RichText::new(match item.resolution {
                                                    Resolution::Skip => t("Skip"),
                                                    Resolution::Overwrite => t("Overwrite"),
                                                    Resolution::KeepBoth => t("Keep both"),
                                                    Resolution::Add => t("Add"),
                                                }).font(style::regular(style::SMALL)))
                                                .width(78.0)
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut item.resolution, Resolution::Skip, t("Skip"));
                                                    ui.selectable_value(&mut item.resolution, Resolution::Overwrite, t("Overwrite"));
                                                    ui.selectable_value(&mut item.resolution, Resolution::KeepBoth, t("Keep both"));
                                                });
                                        });
                                    }
                                }
                                if i + 1 < n {
                                    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, p.line_soft));
                                }
                            }
                        });
                    });
                let commands = self.plan.startup_command_count();
                if commands > 0 {
                    ui.add_space(14.0);
                    style::checkbox(ui, &mut self.plan.import_startup_commands, {
                        let mut job = egui::text::LayoutJob::default();
                        job.append(t("Import startup commands"), 0.0, egui::TextFormat { font_id: style::regular(style::BODY), color: p.text, ..Default::default() });
                        job.append(&format!("  {}", tf("({} connections)", &[&commands.to_string()])), 0.0, egui::TextFormat { font_id: style::regular(style::BODY), color: p.text_faint, ..Default::default() });
                        job
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(24.0);
                        style::hint(ui, t("They run on the server every time the connection opens. Only import them from a file you trust."));
                    });
                }
            });
            let (add, change, _) = self.plan.summary();
            kit::dialog_footer(ui, |ui| kit::footer_hint(ui, t("Importing the same file twice changes nothing.")), |ui| {
                let import = ui.add_enabled(add + change > 0 && self.unlocking.is_none(), style::primary(tf("Import {}", &[&(add + change).to_string()])));
                #[cfg(debug_assertions)]
                crate::devtest::record("import-ok", import.rect);
                if import.clicked() {
                    result = DialogResult::Import(Box::new(self.plan.clone()));
                }
                if kit::secondary(ui, None, t("Cancel")).clicked() {
                    result = DialogResult::Close;
                }
            });
        });
        if escape_only(ctx, &modal) && matches!(result, DialogResult::Open) {
            result = DialogResult::Close;
        }
        result
    }
}

// ----- settings -------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum SettingsTab {
    #[default]
    General,
    Appearance,
    Terminal,
    QuickCommands,
    Shortcuts,
}

#[derive(Default)]
pub struct SettingsDialog {
    tab: SettingsTab,
    shortcut_filter: String,
}

impl SettingsDialog {
    pub fn new(tab: Option<SettingsTab>) -> Self {
        Self {
            tab: tab.unwrap_or_default(),
            shortcut_filter: String::new(),
        }
    }

    fn show(&mut self, ctx: &egui::Context, s: &mut Settings) -> DialogResult {
        let mut result = DialogResult::Open;
        let screen = ctx.content_rect();
        let size = vec2(
            880.0f32.min(screen.width() - 40.0),
            610.0f32.min(screen.height() - 40.0),
        );
        let modal = style::modal(ctx, egui::Id::new("settings")).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let p = style::pal(ui);
            let full = ui.max_rect();
            let nav = egui::Rect::from_min_size(full.min, vec2(200.0, size.y));
            ui.painter().rect_filled(
                nav,
                CornerRadius {
                    nw: style::RADIUS_XL,
                    sw: style::RADIUS_XL,
                    ne: 0,
                    se: 0,
                },
                p.nav,
            );
            ui.painter()
                .vline(nav.right() - 0.5, nav.y_range(), Stroke::new(1.0, p.border));
            let mut nav_ui =
                ui.new_child(egui::UiBuilder::new().max_rect(nav.shrink2(vec2(8.0, 14.0))));
            nav_ui.spacing_mut().item_spacing.y = 1.0;
            nav_ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(t("Settings"))
                        .font(style::semibold(style::TITLE))
                        .color(p.text),
                );
            });
            nav_ui.add_space(12.0);
            let pages = [
                (
                    SettingsTab::General,
                    icons::SLIDERS_HORIZONTAL,
                    t("General"),
                ),
                (SettingsTab::Appearance, icons::PALETTE, t("Appearance")),
                (SettingsTab::Terminal, icons::TERMINAL_WINDOW, t("Terminal")),
                (
                    SettingsTab::QuickCommands,
                    icons::LIGHTNING,
                    t("Quick Commands"),
                ),
                (SettingsTab::Shortcuts, icons::KEYBOARD, t("Shortcuts")),
            ];
            for (tab, icon, label) in pages {
                let r = nav_item(&mut nav_ui, icon, label, self.tab == tab, false);
                #[cfg(debug_assertions)]
                {
                    if tab == SettingsTab::Terminal {
                        crate::devtest::record("settings-terminal", r.rect);
                    }
                    if tab == SettingsTab::QuickCommands {
                        crate::devtest::record("settings-commands", r.rect);
                    }
                    crate::devtest::record(format!("settings-page:{}", tab as u8), r.rect);
                }
                if r.clicked() {
                    self.tab = tab;
                }
            }
            // Version and the update check, at the bottom of the navigation.
            let foot = egui::Rect::from_min_max(
                pos2(nav.left() + 18.0, nav.bottom() - 52.0),
                pos2(nav.right() - 8.0, nav.bottom() - 14.0),
            );
            let painter = ui.painter();
            painter.text(
                foot.left_top(),
                Align2::LEFT_TOP,
                format!("TernaryShell {}", crate::update::CURRENT),
                style::regular(style::XS),
                p.text_faint,
            );
            let link = ui.interact(
                egui::Rect::from_min_size(foot.left_top() + vec2(0.0, 17.0), vec2(120.0, 16.0)),
                egui::Id::new("settings-check"),
                Sense::click(),
            );
            ui.painter().text(
                foot.left_top() + vec2(0.0, 17.0),
                Align2::LEFT_TOP,
                t("Check for updates…"),
                style::regular(style::XS),
                if link.hovered() {
                    p.accent
                } else {
                    p.accent_text
                },
            );
            if link.clicked() {
                result = DialogResult::Run(Action::CheckForUpdates);
            }
            // The page.
            let main = egui::Rect::from_min_max(pos2(nav.right(), full.top()), full.max);
            let mut main_ui = ui.new_child(egui::UiBuilder::new().max_rect(main));
            let subtitle = match self.tab {
                SettingsTab::General => t("Changes apply right away."),
                SettingsTab::Appearance => t("How TernaryShell and its terminals look."),
                SettingsTab::Terminal => t("Text, cursor and keyboard in every terminal."),
                SettingsTab::QuickCommands => t(
                    "Snippets you can send to the active terminal from ⚡ or the command palette.",
                ),
                SettingsTab::Shortcuts => {
                    t("Every command is also in the menu and in the command palette.")
                }
            };
            let title = pages.iter().find(|x| x.0 == self.tab).map_or("", |x| x.2);
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 24,
                    right: 16,
                    top: 18,
                    bottom: 4,
                })
                .show(&mut main_ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(
                                RichText::new(title)
                                    .font(style::semibold(style::TITLE))
                                    .color(p.text),
                            );
                            ui.label(
                                RichText::new(subtitle)
                                    .font(style::regular(style::SMALL))
                                    .color(p.text_muted),
                            );
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            if kit::icon_button_ex(ui, icons::X, t("Close"), false, 22.0).clicked()
                            {
                                result = DialogResult::Close;
                            }
                        });
                    });
                });
            let body_h = main_ui.available_height();
            egui::ScrollArea::vertical()
                .id_salt(("settings-page", self.tab as u8))
                .max_height(body_h)
                .auto_shrink([false, false])
                .show(&mut main_ui, |ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 24,
                            right: 24,
                            top: 10,
                            bottom: 18,
                        })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 0.0;
                            match self.tab {
                                SettingsTab::General => {
                                    if general(ui, s) {
                                        result = DialogResult::Run(Action::CheckForUpdates);
                                    }
                                }
                                SettingsTab::Appearance => appearance(ui, s),
                                SettingsTab::Terminal => terminal(ui, s),
                                SettingsTab::QuickCommands => quick_commands(ui, s),
                                SettingsTab::Shortcuts => shortcuts(ui, &mut self.shortcut_filter),
                            }
                        });
                });
        });
        if modal.should_close() && matches!(result, DialogResult::Open) {
            result = DialogResult::Close;
        }
        result
    }
}

/// An item of a dialog's side navigation (30 px): icon, label, accent when on, a red dot when
/// the page has a problem.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, on: bool, problem: bool) -> egui::Response {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let painter = ui.painter();
    if on {
        painter.rect_filled(rect, CornerRadius::same(style::RADIUS), p.accent_soft);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(style::RADIUS), p.hover);
    }
    let color = if on {
        p.accent_text
    } else if resp.hovered() {
        p.text
    } else {
        p.text_muted
    };
    painter.text(
        pos2(rect.left() + 10.0 + 8.0, rect.center().y),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(16.0),
        color,
    );
    painter.text(
        pos2(rect.left() + 10.0 + 16.0 + 9.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        style::medium(style::BODY),
        color,
    );
    if problem {
        painter.circle_filled(pos2(rect.right() - 13.0, rect.center().y), 3.0, p.err);
    }
    resp
}

/// General. Returns true when "Check Now" was clicked.
fn general(ui: &mut Ui, s: &mut Settings) -> bool {
    let mut check_now = false;
    kit::setting_group(ui, t("Language"), |ui| {
        kit::setting_row(
            ui,
            t("Language"),
            t("The menu bar follows the system language."),
            true,
            |ui| {
                egui::ComboBox::from_id_salt("lang")
                    .icon(kit::combo_caret)
                    .width(200.0)
                    .selected_text(match s.language {
                        Language::System => t("System (English)"),
                        Language::English => "English",
                        Language::Indonesian => "Bahasa Indonesia",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.language, Language::System, t("System"));
                        ui.selectable_value(&mut s.language, Language::English, "English");
                        ui.selectable_value(
                            &mut s.language,
                            Language::Indonesian,
                            "Bahasa Indonesia",
                        );
                    });
            },
        );
    });
    kit::setting_group(ui, t("Connections"), |ui| {
        kit::setting_row(
            ui,
            t("Keepalive"),
            t(
                "Ping the server every N seconds so idle sessions stay open. 0 = off. Applies to new connections.",
            ),
            false,
            |ui| {
                ui.add_sized(
                    vec2(110.0, style::CONTROL_H),
                    egui::DragValue::new(&mut s.keepalive_secs)
                        .range(0..=600)
                        .update_while_editing(false)
                        .suffix(format!(" {}", t("sec"))),
                );
            },
        );
        kit::setting_row(
            ui,
            t("Ask before closing a connected tab"),
            t("Only when something is still running there."),
            true,
            |ui| {
                kit::switch(ui, &mut s.confirm_close_connected);
            },
        );
    });
    kit::setting_group(ui, t("Updates"), |ui| {
        kit::setting_row(
            ui,
            t("Check for updates automatically"),
            t(
                "At launch and every 6 hours. The only connection TernaryShell makes besides your servers.",
            ),
            true,
            |ui| {
                kit::switch(ui, &mut s.check_updates);
                if kit::secondary_sm(ui, None, t("Check Now")).clicked() {
                    check_now = true;
                }
            },
        );
    });
    kit::setting_group(ui, t("Data"), |ui| {
        let folder = crate::paths::contract_tilde(crate::paths::data_dir());
        let desc = RichText::new(folder)
            .font(style::mono(style::SMALL))
            .color(style::pal(ui).text_faint);
        kit::setting_row_rich(ui, t("Data folder"), Some(desc), true, |ui| {
            if kit::button_ex(
                ui,
                kit::Kind::Secondary,
                Some(icons::FOLDER_OPEN),
                crate::platform::reveal_label(),
                true,
            )
            .clicked()
            {
                crate::platform::open(crate::paths::data_dir());
            }
        });
    });
    check_now
}

/// Appearance: the app theme as preview cards, then the terminal colors for each theme.
fn appearance(ui: &mut Ui, s: &mut Settings) {
    let p = style::pal(ui);
    ui.label(style::section_label(t("Theme"), p.text_faint));
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let w = (ui.available_width() - 20.0) / 3.0;
        for (mode, icon, label) in [
            (ThemeMode::System, icons::CIRCLE_HALF, t("Match System")),
            (ThemeMode::Light, icons::SUN, "Paper"),
            (ThemeMode::Dark, icons::MOON, "Graphite"),
        ] {
            if theme_card(ui, mode, icon, label, s.theme == mode, w).clicked() {
                s.theme = mode;
            }
        }
    });
    ui.add_space(18.0);
    for (title, dark) in [
        (t("Terminal colors · dark theme"), true),
        (t("Terminal colors · light theme"), false),
    ] {
        ui.label(style::section_label(title, p.text_faint));
        ui.add_space(10.0);
        let value = if dark {
            &mut s.scheme_dark
        } else {
            &mut s.scheme_light
        };
        let schemes: Vec<&Scheme> = SCHEMES.iter().filter(|sc| sc.dark == dark).collect();
        let w = (ui.available_width() - 20.0) / 3.0;
        for chunk in schemes.chunks(3) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                for sc in chunk {
                    if scheme_card(ui, sc, *value == sc.name, w).clicked() {
                        *value = sc.name.to_string();
                    }
                }
            });
            ui.add_space(10.0);
        }
        ui.add_space(8.0);
    }
}

/// A theme as a card: a miniature window (sidebar, canvas, lines) and its name.
fn theme_card(
    ui: &mut Ui,
    mode: ThemeMode,
    icon: &str,
    label: &str,
    on: bool,
    w: f32,
) -> egui::Response {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 74.0 + 34.0), Sense::click());
    let painter = ui.painter();
    let radius = CornerRadius::same(style::RADIUS_MD);
    if on {
        painter.rect_stroke(
            rect,
            radius,
            Stroke::new(3.0, p.accent_halo),
            StrokeKind::Outside,
        );
    }
    let prev = egui::Rect::from_min_size(rect.min, vec2(w, 74.0));
    let mini = |painter: &egui::Painter, r: egui::Rect, dark: bool| {
        let q = style::palette(dark);
        painter.rect_filled(r, 0.0, q.chrome);
        let side_w = r.width() * 0.34;
        for (i, (len, acc)) in [(0.8, true), (0.6, false), (0.6, false)].iter().enumerate() {
            let y = r.top() + 8.0 + i as f32 * 9.0;
            painter.rect_filled(
                egui::Rect::from_min_size(
                    pos2(r.left() + 6.0, y),
                    vec2((side_w - 12.0) * len, 5.0),
                ),
                CornerRadius::same(3),
                if *acc { q.accent } else { q.border },
            );
        }
        let card = egui::Rect::from_min_max(
            pos2(r.left() + side_w, r.top() + 6.0),
            pos2(r.right() - 6.0, r.bottom() - 6.0),
        );
        painter.rect(
            card,
            CornerRadius::same(6),
            q.canvas,
            Stroke::new(1.0, q.border),
            StrokeKind::Inside,
        );
        for (i, (len, acc)) in [(0.7, false), (0.45, false), (0.58, true)]
            .iter()
            .enumerate()
        {
            let y = card.top() + 8.0 + i as f32 * 8.0;
            let c = if *acc {
                style::soft(q.accent, 0.6)
            } else {
                q.border
            };
            painter.rect_filled(
                egui::Rect::from_min_size(
                    pos2(card.left() + 8.0, y),
                    vec2((card.width() - 16.0) * len, 4.0),
                ),
                CornerRadius::same(2),
                c,
            );
        }
    };
    let clip = painter.with_clip_rect(prev);
    match mode {
        ThemeMode::System => {
            mini(
                &clip.with_clip_rect(egui::Rect::from_min_max(
                    prev.min,
                    pos2(prev.center().x, prev.bottom()),
                )),
                prev,
                false,
            );
            mini(
                &clip.with_clip_rect(egui::Rect::from_min_max(
                    pos2(prev.center().x, prev.top()),
                    prev.max,
                )),
                prev,
                true,
            );
        }
        ThemeMode::Light => mini(&clip, prev, false),
        ThemeMode::Dark => mini(&clip, prev, true),
    }
    let label_r = egui::Rect::from_min_max(pos2(rect.left(), prev.bottom()), rect.max);
    painter.rect_filled(
        label_r,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: style::RADIUS_MD,
            se: style::RADIUS_MD,
        },
        p.card,
    );
    painter.hline(
        label_r.x_range(),
        label_r.top() + 0.5,
        Stroke::new(1.0, p.border),
    );
    painter.text(
        pos2(label_r.left() + 10.0 + 7.0, label_r.center().y),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(14.0),
        p.text_muted,
    );
    painter.text(
        pos2(label_r.left() + 10.0 + 14.0 + 8.0, label_r.center().y),
        Align2::LEFT_CENTER,
        label,
        style::medium(style::SMALL),
        p.text,
    );
    kit::round_corners(painter, rect, style::RADIUS_MD as f32, p.dialog);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, if on { p.accent } else { p.border_strong }),
        StrokeKind::Inside,
    );
    resp
}

/// A terminal color scheme as a card: a prompt, a listing and a log line in its colors, a strip
/// of its six colors, and its name.
fn scheme_card(ui: &mut Ui, sc: &Scheme, on: bool, w: f32) -> egui::Response {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 64.0 + 6.0 + 30.0), Sense::click());
    let painter = ui.painter();
    let radius = CornerRadius::same(style::RADIUS_MD);
    let sample = egui::Rect::from_min_size(rect.min, vec2(w, 64.0));
    painter.rect_filled(
        sample,
        CornerRadius {
            nw: style::RADIUS_MD,
            ne: style::RADIUS_MD,
            sw: 0,
            se: 0,
        },
        sc.background,
    );
    let font = style::mono(10.5);
    let [_, r, g, y, b, m, c, ..] = sc.ansi;
    let lines: [Vec<(&str, Color32)>; 3] = [
        vec![
            ("deploy@web-01", g),
            (":", sc.foreground),
            ("~/app", b),
            ("$ ls", sc.foreground),
        ],
        vec![
            ("config", b),
            ("  ", sc.foreground),
            ("deploy.sh", g),
            ("  ", sc.foreground),
            ("backup.tgz", r),
        ],
        vec![
            ("WARN", y),
            (" ", sc.foreground),
            ("cache", m),
            (" ", sc.foreground),
            ("hit 98%", c),
        ],
    ];
    let clip = painter.with_clip_rect(sample.shrink(1.0));
    for (i, line) in lines.iter().enumerate() {
        let mut job = egui::text::LayoutJob::default();
        for (text, color) in line {
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: *color,
                    ..Default::default()
                },
            );
        }
        let gal = clip.layout_job(job);
        clip.galley(
            pos2(sample.left() + 10.0, sample.top() + 8.0 + i as f32 * 15.0),
            gal,
            sc.foreground,
        );
    }
    let strip = egui::Rect::from_min_size(pos2(rect.left(), sample.bottom()), vec2(w, 6.0));
    let sw = w / 6.0;
    for (i, col) in [r, g, y, b, m, c].iter().enumerate() {
        painter.rect_filled(
            egui::Rect::from_min_size(
                pos2(strip.left() + i as f32 * sw, strip.top()),
                vec2(sw + 0.5, 6.0),
            ),
            0.0,
            *col,
        );
    }
    let label = egui::Rect::from_min_max(pos2(rect.left(), strip.bottom()), rect.max);
    painter.rect_filled(
        label,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: style::RADIUS_MD,
            se: style::RADIUS_MD,
        },
        if p.dark {
            Color32::from_rgb(0x26, 0x26, 0x26)
        } else {
            p.surface
        },
    );
    painter.hline(
        label.x_range(),
        label.top() + 0.5,
        Stroke::new(1.0, p.border),
    );
    painter.with_clip_rect(label.shrink2(vec2(10.0, 0.0))).text(
        pos2(label.left() + 10.0, label.center().y),
        Align2::LEFT_CENTER,
        sc.name,
        style::regular(style::SMALL),
        p.text,
    );
    if on {
        painter.text(
            pos2(label.right() - 10.0 - 7.0, label.center().y),
            Align2::CENTER_CENTER,
            icons::CHECK_CIRCLE,
            style::icon_font(14.0),
            p.accent_text,
        );
    }
    kit::round_corners(painter, rect, style::RADIUS_MD as f32, p.dialog);
    if on {
        painter.rect_stroke(rect, radius, Stroke::new(2.0, p.accent), StrokeKind::Inside);
    } else {
        painter.rect_stroke(rect, radius, Stroke::new(1.0, p.border), StrokeKind::Inside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Terminal: the font with a live preview, the size, fallbacks; cursor, scrollback, keys.
fn terminal(ui: &mut Ui, s: &mut Settings) {
    let p = style::pal(ui);
    ui.label(style::section_label(t("Text"), p.text_faint));
    ui.add_space(4.0);
    egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let current = fonts::effective_choice(&s.font);
                let stepper_w = 2.0 + 22.0 + 34.0 + 22.0 + 2.0;
                let button_w = 90.0;
                let combo_w = ui.available_width() - stepper_w - button_w - 16.0;
                egui::ComboBox::from_id_salt("font")
                    .icon(kit::combo_caret)
                    .selected_text(current.name.clone())
                    .width(combo_w)
                    .show_ui(ui, |ui| {
                        for preset in fonts::PRESETS {
                            if fonts::is_available(preset)
                                && ui
                                    .selectable_label(current.name == preset.name, preset.name)
                                    .clicked()
                            {
                                s.font = FontChoice {
                                    name: preset.name.into(),
                                    path: None,
                                    index: 0,
                                };
                            }
                        }
                    });
                if kit::secondary_sm(ui, None, t("Font File…")).clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter(t("Fonts"), &["ttf", "otf", "ttc"])
                        .pick_file()
                {
                    let name = path
                        .file_stem()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    s.font = FontChoice {
                        name,
                        path: Some(crate::paths::contract_tilde(&path)),
                        index: 0,
                    };
                }
                // − 13 +
                let (r, _) =
                    ui.allocate_exact_size(vec2(stepper_w, style::CONTROL_H), Sense::hover());
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::same(style::RADIUS),
                    Stroke::new(1.0, p.field_border),
                    StrokeKind::Inside,
                );
                let minus = egui::Rect::from_min_size(
                    pos2(r.left() + 2.0, r.center().y - 11.0),
                    vec2(22.0, 22.0),
                );
                let plus = egui::Rect::from_min_size(
                    pos2(r.right() - 24.0, r.center().y - 11.0),
                    vec2(22.0, 22.0),
                );
                let m = ui.interact(minus, egui::Id::new("font-smaller"), Sense::click());
                kit::paint_icon_button(
                    ui,
                    &m,
                    minus,
                    icons::MINUS,
                    t("Smaller Text"),
                    false,
                    false,
                );
                let pl = ui.interact(plus, egui::Id::new("font-bigger"), Sense::click());
                kit::paint_icon_button(ui, &pl, plus, icons::PLUS, t("Bigger Text"), false, false);
                ui.painter().text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    format!("{}", s.font_size),
                    style::mono(style::BODY),
                    p.text,
                );
                if m.clicked() {
                    s.font_size = (s.font_size - 1.0).max(Settings::MIN_FONT);
                }
                if pl.clicked() {
                    s.font_size = (s.font_size + 1.0).min(Settings::MAX_FONT);
                }
            });
            ui.add_space(10.0);
            // The preview, in the chosen font and the dark scheme's colors.
            let scheme = crate::term::scheme::find(
                if ui.visuals().dark_mode {
                    &s.scheme_dark
                } else {
                    &s.scheme_light
                },
                ui.visuals().dark_mode,
            );
            egui::Frame::NONE
                .fill(scheme.background)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::same(style::RADIUS_MD))
                .inner_margin(egui::Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let font = egui::FontId::monospace(s.font_size);
                    let [_, _, g, _, b, ..] = scheme.ansi;
                    let fmt = |c: Color32| egui::TextFormat {
                        font_id: font.clone(),
                        color: c,
                        ..Default::default()
                    };
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let lines: [Vec<(&str, Color32)>; 3] = [
                        vec![
                            ("deploy@web-01", g),
                            (":", scheme.foreground),
                            ("/srv/app", b),
                            ("$ echo \"Box ┌──┬──┐ ▓▒░ ✓ → ≠ ≤\"", scheme.foreground),
                        ],
                        vec![
                            ("Box ┌──┬──┐ ▓▒░ ✓ → ≠ ≤   ", scheme.foreground),
                            ("日本語 中文 한국어", scheme.ansi[8]),
                        ],
                        vec![("0O 1lI {} [] () <> != ->", g)],
                    ];
                    for line in lines {
                        let mut job = egui::text::LayoutJob::default();
                        for (text, color) in line {
                            job.append(text, 0.0, fmt(color));
                        }
                        ui.add(egui::Label::new(job).wrap_mode(egui::TextWrapMode::Truncate));
                    }
                });
            ui.add_space(4.0);
            kit::setting_row(
                ui,
                t("Chinese, Japanese and Korean fallback fonts"),
                t("Loaded from the system on first use; adds about 20 MB of memory."),
                true,
                |ui| {
                    kit::switch(ui, &mut s.load_cjk_font);
                },
            );
        });
    ui.add_space(18.0);
    kit::setting_group(ui, t("Behavior"), |ui| {
        kit::setting_row(ui, t("Cursor"), "", false, |ui| {
            kit::switch(ui, &mut s.cursor_blink);
            ui.label(
                RichText::new(t("Blink"))
                    .font(style::regular(style::SMALL))
                    .color(p.text_muted),
            );
            ui.add_space(10.0);
            kit::segmented(
                ui,
                &mut s.cursor_style,
                &[
                    (CursorStyle::Underline, t("Underline")),
                    (CursorStyle::Beam, t("Bar")),
                    (CursorStyle::Block, t("Block")),
                ]
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>(),
                true,
            );
        });
        let mb = s.scrollback as f64 * 120.0 * 24.0 / 1_000_000.0;
        let desc = tf(
            "About 24 bytes per cell: {} lines at 120 columns ≈ {} MB per pane.",
            &[&format_thousands(s.scrollback), &format!("{mb:.0}")],
        );
        kit::setting_row(ui, t("Scrollback"), &desc, false, |ui| {
            ui.add_sized(
                vec2(150.0, style::CONTROL_H),
                egui::DragValue::new(&mut s.scrollback)
                    .range(0..=Settings::MAX_SCROLLBACK)
                    .speed(100)
                    .update_while_editing(false)
                    .suffix(t(" lines")),
            );
        });
        let (meta, meta_desc) = if cfg!(target_os = "macos") {
            (
                t("Use Option as Meta key"),
                t("For readline and Emacs. Turn off to type special characters with Option."),
            )
        } else {
            (
                t("Use Alt as Meta key"),
                t("For readline and Emacs. Turn off to type special characters with Alt."),
            )
        };
        kit::setting_row(ui, meta, meta_desc, false, |ui| {
            kit::switch(ui, &mut s.option_as_meta);
        });
        kit::setting_row(ui, t("Copy text when selected"), "", true, |ui| {
            kit::switch(ui, &mut s.copy_on_select);
        });
    });
}

fn format_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Quick commands: a table of rows (drag handle, name, command, Run, remove).
fn quick_commands(ui: &mut Ui, s: &mut Settings) {
    let p = style::pal(ui);
    let mut remove = None;
    let mut moved: Option<(usize, usize)> = None;
    egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .inner_margin(egui::Margin::same(4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let width = ui.available_width();
            let cmd_w = width - 20.0 - 18.0 - 170.0 - 70.0 - 28.0 - 4.0 * 10.0;
            // Header.
            let (head, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
            let painter = ui.painter();
            let x0 = head.left() + 10.0 + 18.0 + 10.0;
            for (x, label) in [
                (x0, t("Name")),
                (x0 + 170.0 + 10.0, t("Command")),
                (x0 + 170.0 + 10.0 + cmd_w + 10.0, t("Run")),
            ] {
                painter.text(
                    pos2(x, head.center().y),
                    Align2::LEFT_CENTER,
                    label.to_uppercase(),
                    style::semibold(style::XS),
                    p.text_faint,
                );
            }
            let focused_row = ui.memory(|m| m.focused());
            for (i, qc) in s.quick_commands.iter_mut().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
                let name_id = egui::Id::new(("qc-name", qc.id));
                let cmd_id = egui::Id::new(("qc-cmd", qc.id));
                if focused_row == Some(name_id) || focused_row == Some(cmd_id) {
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(style::RADIUS_MD),
                        p.accent_softer,
                    );
                }
                let mut row = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(r.shrink2(vec2(10.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                row.spacing_mut().item_spacing.x = 10.0;
                let (h, handle) = row.allocate_exact_size(vec2(18.0, 24.0), Sense::drag());
                row.painter().text(
                    h.center(),
                    Align2::CENTER_CENTER,
                    icons::DOTS_SIX_VERTICAL,
                    style::icon_font(15.0),
                    if handle.hovered() {
                        p.text_muted
                    } else {
                        p.text_faint
                    },
                );
                if handle.hovered() || handle.dragged() {
                    row.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                handle.dnd_set_drag_payload(i);
                if let Some(from) = row
                    .interact(r, egui::Id::new(("qc-drop", i)), Sense::hover())
                    .dnd_release_payload::<usize>()
                {
                    moved = Some((*from, i));
                }
                row.add(
                    style::input(&mut qc.name)
                        .id(name_id)
                        .desired_width(170.0)
                        .hint_text(t("Name")),
                );
                row.add(
                    style::input(&mut qc.command)
                        .id(cmd_id)
                        .desired_width(cmd_w)
                        .mono()
                        .hint_text("df -h"),
                );
                row.allocate_ui_with_layout(
                    vec2(70.0, 24.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(70.0);
                        kit::switch(ui, &mut qc.run).on_hover_text(t("Press Enter after sending"));
                    },
                );
                if kit::icon_button_ex(&mut row, icons::TRASH, t("Remove"), false, 22.0).clicked() {
                    remove = Some(i);
                }
            }
        });
    if let Some(i) = remove {
        s.quick_commands.remove(i);
    }
    if let Some((from, to)) = moved
        && from != to
        && from < s.quick_commands.len()
    {
        let item = s.quick_commands.remove(from);
        s.quick_commands
            .insert(to.min(s.quick_commands.len()), item);
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        if kit::secondary_sm(ui, Some(icons::PLUS), t("Add Command")).clicked() {
            s.quick_commands.push(QuickCommand::new("", ""));
        }
        style::hint(
            ui,
            t("Run off = the command is pasted, so you can edit it before pressing Enter."),
        );
    });
}

/// Shortcuts: every command and its keys, grouped, with a filter.
fn shortcuts(ui: &mut Ui, filter: &mut String) {
    let p = style::pal(ui);
    let w = ui.available_width();
    kit::search_field(
        ui,
        egui::Id::new("shortcut-filter"),
        filter,
        t("Search shortcuts"),
        w,
    );
    ui.add_space(14.0);
    use super::keys::{FileKey, TreeKey};
    type Group<'a> = (&'a str, Vec<(&'a str, Vec<String>)>);
    let groups: Vec<Group> = vec![
        (
            t("Connections"),
            vec![
                (t("Quick connect / search"), vec![S::QuickConnect.label()]),
                (t("New connection"), vec![S::NewConnection.label()]),
                (t("Reconnect"), vec![S::Reconnect.label()]),
                (t("Export connections"), vec![S::Export.label()]),
                (
                    t("Edit / delete the selected connection"),
                    vec![TreeKey::Edit.label().into(), TreeKey::Delete.label().into()],
                ),
            ],
        ),
        (
            t("Tabs & panes"),
            vec![
                (t("New tab (same server)"), vec![S::NewTab.label()]),
                (t("Close pane or tab"), vec![S::Close.label()]),
                (
                    t("Switch to tab 1–9"),
                    vec![super::keys::tab_number_label().to_string()],
                ),
                (
                    t("Previous / next tab"),
                    vec![S::PrevTab.label(), S::NextTab.label()],
                ),
                (
                    t("Split right / down"),
                    vec![S::SplitRight.label(), S::SplitDown.label()],
                ),
                (
                    t("Previous / next pane"),
                    vec![S::PrevPane.label(), S::NextPane.label()],
                ),
            ],
        ),
        (
            t("Terminal"),
            vec![
                (t("Copy / paste"), vec![S::Copy.label(), S::Paste.label()]),
                (t("Clear scrollback"), vec![S::ClearScrollback.label()]),
                (t("Scroll history"), vec!["⇧PgUp".into(), "⇧PgDn".into()]),
                (
                    t("Bigger / smaller / actual size"),
                    vec![S::Bigger.label(), S::Smaller.label(), S::ActualSize.label()],
                ),
            ],
        ),
        (
            t("View & app"),
            vec![
                (t("Sidebar"), vec![S::Sidebar.label()]),
                (t("SFTP panel"), vec![S::Sftp.label()]),
                (t("Full screen"), vec![S::FullScreen.label()]),
                (t("Settings"), vec![S::Settings.label()]),
                (t("Save (file editor)"), vec![S::Save.label()]),
                (t("Quit"), vec![S::Quit.label()]),
            ],
        ),
        (
            t("Files (SFTP, after a click in a list)"),
            vec![
                (
                    t("Open folder / edit file"),
                    vec![FileKey::Open.label().into()],
                ),
                (t("Rename"), vec![FileKey::Rename.label().into()]),
                (t("Delete"), vec![FileKey::Delete.label().into()]),
                (
                    t("Download / upload selected"),
                    vec![
                        FileKey::Download.label().into(),
                        FileKey::Upload.label().into(),
                    ],
                ),
                (t("Copy path"), vec![FileKey::CopyPath.label().into()]),
                (t("Back to the terminal"), vec!["esc".into()]),
            ],
        ),
    ];
    let q = filter.trim().to_lowercase();
    let col_w = (w - 16.0) / 2.0;
    for pair in groups.chunks(2) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (title, rows) in pair {
                ui.allocate_ui_with_layout(
                    vec2(col_w, 10.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(col_w);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let rows: Vec<_> = rows
                            .iter()
                            .filter(|(l, _)| q.is_empty() || l.to_lowercase().contains(&q))
                            .collect();
                        if rows.is_empty() {
                            return;
                        }
                        ui.label(style::section_label(title, p.text_faint));
                        ui.add_space(6.0);
                        for (label, keys) in rows {
                            let (r, _) = ui.allocate_exact_size(vec2(col_w, 28.0), Sense::hover());
                            let painter = ui.painter();
                            painter.text(
                                pos2(r.left(), r.center().y),
                                Align2::LEFT_CENTER,
                                label,
                                style::regular(12.5),
                                p.text,
                            );
                            let mut x = r.right();
                            for k in keys.iter().rev() {
                                let kw = (painter
                                    .layout_no_wrap(
                                        k.clone(),
                                        style::regular(style::XS),
                                        p.text_muted,
                                    )
                                    .size()
                                    .x
                                    + 10.0)
                                    .max(20.0);
                                x -= kw;
                                kit::paint_kbd_at(painter, pos2(x, r.center().y), k, &p);
                                x -= 3.0;
                            }
                            painter.hline(
                                r.x_range(),
                                r.bottom() - 0.5,
                                Stroke::new(1.0, p.line_soft),
                            );
                        }
                    },
                );
            }
        });
        ui.add_space(16.0);
    }
}
