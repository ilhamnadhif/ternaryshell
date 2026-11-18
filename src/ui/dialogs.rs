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
use super::{fonts, forms, icons, kit};
use crate::bookmarks::transfer::{
    ExportScope, ImportPlan, ItemStatus, MatchedBy, Resolution, SourceKind,
};
use crate::bookmarks::{BookmarkStore, search};
use crate::i18n::{Language, t, tf, tn};
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
const BODY_H: f32 = 330.0;

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

                // The query: 54 px, a magnifier, the text (mono when it is an address).
                let (row, _) = ui.allocate_exact_size(vec2(PALETTE_W, 54.0), Sense::hover());
                ui.painter().text(
                    pos2(row.left() + 16.0 + 9.5, row.center().y),
                    Align2::CENTER_CENTER,
                    icons::MAGNIFYING_GLASS,
                    style::icon_font(19.0),
                    p.text_faint,
                );
                let right_pad = 16.0;
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

                // The scopes: plain text, the selected one underlined (not while a typed address
                // is offered: Tab still moves through them).
                let typing_address =
                    address.is_some() && matches!(self.scope, Scope::All | Scope::Hosts);
                let (tabs, _) = ui.allocate_exact_size(
                    vec2(PALETTE_W, if typing_address { 0.0 } else { 34.0 }),
                    Sense::hover(),
                );
                let mut x = tabs.left() + 16.0;
                for scope in Scope::ALL.into_iter().filter(|_| !typing_address) {
                    let w = scope_tab(
                        ui,
                        pos2(x, tabs.top() + 10.0),
                        scope,
                        scope == self.scope,
                        &p,
                    );
                    if w.1 {
                        self.scope = scope;
                        self.index = 0;
                        r.request_focus();
                    }
                    x += w.0 + 14.0;
                }

                // The list and the preview beside it.
                // With nothing to show, the message takes the whole width (no empty preview).
                let (body, _) = ui.allocate_exact_size(vec2(PALETTE_W, BODY_H), Sense::hover());
                let preview_w = if total == 0 { 0.0 } else { PREVIEW_W };
                let list_rect = egui::Rect::from_min_max(
                    body.min,
                    pos2(body.right() - preview_w, body.bottom()),
                );
                let preview_rect =
                    egui::Rect::from_min_max(pos2(list_rect.right(), body.top()), body.max);
                if total > 0 {
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
                }
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
                                        ui.add_space(4.0);
                                        ui.label(
                                            RichText::new(t(
                                                "Type user@host to connect without saving.",
                                            ))
                                            .font(style::regular(style::SMALL))
                                            .color(p.text_faint),
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
                if total > 0 {
                    let mut preview_ui =
                        ui.new_child(egui::UiBuilder::new().max_rect(preview_rect.shrink(16.0)));
                    preview(&mut preview_ui, selected.as_ref(), data, address.as_ref());
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
            });
        if modal.should_close() && matches!(result, PaletteResult::Open) {
            result = PaletteResult::Close;
        }
        result
    }
}

/// A scope at `pos` (22 px): its name, underlined when selected; returns its width and whether
/// it was clicked.
fn scope_tab(
    ui: &mut Ui,
    pos: egui::Pos2,
    scope: Scope,
    on: bool,
    p: &style::Palette,
) -> (f32, bool) {
    let g = ui.painter().layout_no_wrap(
        scope.label().to_string(),
        style::medium(style::SMALL),
        p.text,
    );
    let w = g.size().x;
    let rect = egui::Rect::from_min_size(pos, vec2(w, 22.0));
    let resp = ui.interact(
        rect,
        egui::Id::new(("palette-scope", scope)),
        Sense::click(),
    );
    let color = if on || resp.hovered() {
        p.text
    } else {
        p.text_faint
    };
    let painter = ui.painter();
    painter.galley(
        pos2(rect.left(), rect.center().y - g.size().y / 2.0),
        g,
        color,
    );
    if on {
        painter.rect_filled(
            egui::Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.max),
            0.0,
            p.text,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
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

/// One palette row (38 px): the host's glyph or the command's icon (plain), the title, a faint
/// detail, and on the right its shortcut as plain text.
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
        pos2(rect.left() + 10.0, rect.center().y - 10.0),
        vec2(20.0, 20.0),
    );
    let neutral_tile = |icon: &str| {
        painter.text(
            tile.center(),
            Align2::CENTER_CENTER,
            icon,
            style::icon_font(15.0),
            p.text_faint,
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
            neutral_tile(icons::ARROW_RIGHT);
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
            (job, String::new(), false, Vec::new())
        }
        Row::SaveAs(_) => {
            neutral_tile(icons::FLOPPY_DISK);
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
            (job, String::new(), false, Vec::new())
        }
        Row::Command(i) => {
            let c = &data.commands[*i];
            neutral_tile(c.icon);
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
    // The shortcut on the right first, as plain text.
    let mut right = rect.right() - 10.0;
    for k in keys.iter().rev() {
        let g = painter.layout_no_wrap(k.clone(), style::regular(style::SMALL), p.text_faint);
        let w = g.size().x;
        painter.galley(
            pos2(right - w, rect.center().y - g.size().y / 2.0),
            g,
            p.text_faint,
        );
        right -= w + 8.0;
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

/// The preview of the selected row, in the palette's right column: text only (the row itself
/// opens with a click, Return or ⇧Return).
fn preview(
    ui: &mut Ui,
    selected: Option<&Row>,
    data: &PaletteData,
    address: Option<&crate::bookmarks::model::Target>,
) {
    let p = style::pal(ui);
    let store = data.store;
    ui.spacing_mut().item_spacing.y = 12.0;
    let host_id = match selected {
        Some(Row::Host(id)) => Some(*id),
        Some(Row::Tab(i)) => data.tabs.get(*i).map(|t| t.bookmark),
        _ => None,
    };
    if let Some(b) = host_id.and_then(|id| store.bookmark(id)) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
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
        let login = match &b.auth {
            crate::bookmarks::Auth::Password => t("Password").to_string(),
            crate::bookmarks::Auth::Key { path } => {
                let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
                tf("Key file · {}", &[file])
            }
            crate::bookmarks::Auth::KeychainKey => t("Pasted key").to_string(),
            crate::bookmarks::Auth::Agent { .. } => t("SSH agent").to_string(),
        };
        let mut rows: Vec<(&str, String, Option<Color32>)> = Vec::new();
        let group = store.group_path(b.group);
        if !group.is_empty() {
            rows.push((t("Group"), group, None));
        }
        rows.push((t("Login"), login, None));
        if let Some(j) = b.jump.and_then(|j| store.bookmark(j)) {
            rows.push((t("Via"), j.display_name(), None));
        }
        if !b.forwards.is_empty() {
            let ports: Vec<String> = b
                .forwards
                .iter()
                .map(|f| f.listen_port.to_string())
                .collect();
            rows.push((t("Forwards"), ports.join(", "), None));
        }
        // Only the connection's own tags: its label shows as its group.
        if !b.tags.is_empty() {
            let tags: Vec<&str> = b.tags.iter().take(4).map(String::as_str).collect();
            rows.push((t("Tags"), tags.join(", "), None));
        }
        let open = data.open.get(&b.id).copied().filter(|n| *n > 0).map(|n| {
            if n == 1 {
                t("1 tab open").to_string()
            } else {
                tf("{} tabs open", &[&n.to_string()])
            }
        });
        let when = data
            .recent
            .iter()
            .find(|(id, _)| *id == b.id)
            .map(|(_, w)| w.clone());
        match (when, open) {
            (Some(w), Some(o)) => rows.push((t("Last"), format!("{w}, {o}"), None)),
            (Some(w), None) => rows.push((t("Last"), w, None)),
            (None, Some(o)) => rows.push((t("Open"), o, None)),
            (None, None) => {}
        }
        kv(ui, &rows);
        return;
    }
    match selected {
        Some(Row::ConnectTo(_) | Row::SaveAs(_)) => {
            ui.label(
                RichText::new(t("Not saved"))
                    .font(style::semibold(style::BODY))
                    .color(p.text),
            );
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
                    style::hint(
                        ui,
                        tf(
                            "Same host as {}, with another user and port.",
                            &[&other.display_name()],
                        ),
                    );
                }
            }
        }
        Some(Row::Command(i)) => {
            let c = &data.commands[*i];
            ui.add(
                egui::Label::new(
                    RichText::new(&c.label)
                        .font(style::semibold(14.0))
                        .color(p.text),
                )
                .wrap(),
            );
            if let Some(d) = &c.detail {
                ui.add(
                    egui::Label::new(
                        RichText::new(d)
                            .font(if c.quick {
                                style::mono(11.5)
                            } else {
                                style::regular(style::SMALL)
                            })
                            .color(p.text_muted),
                    )
                    .wrap(),
                );
            }
            if let Some(k) = &c.shortcut {
                kv(ui, &[(t("Shortcut"), k.clone(), None)]);
            }
        }
        _ => {}
    }
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
        /// What is at stake, as a short list (Quit: sessions, transfers, unsaved files).
        details: Vec<ConfirmRow>,
        button: String,
        action: ConfirmAction,
        /// When it first appeared (an Enter typed just before must not confirm it).
        shown_at: Option<Instant>,
        /// "Don't ask again" (ticked or not), for closing a connected tab.
        dont_ask: Option<bool>,
        /// The width.
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

/// A confirmation with everything: the list, the width, and "Don't ask again".
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
                let width = extra.width.unwrap_or(420.0);
                let modal = style::modal(ctx, id).show(ctx, |ui| {
                    ui.set_width(width);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let p = style::pal(ui);
                    confirm::header(ui, title);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 20,
                            right: 20,
                            top: 8,
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
                            if !details.is_empty() {
                                ui.add_space(8.0);
                                if let Some(a) = confirm::list(ui, details) {
                                    result = DialogResult::RunAndClose(a);
                                }
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
                    let (ok, cancel) = confirm::footer(ui, button.as_str(), destructive);
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
                    // Redesign 3.1: a title only; a subgroup names its parent in it.
                    let (title, button) = match target {
                        GroupTarget::New { .. } if !context.is_empty() => {
                            (format!("{} {}", t("New Group"), context), t("Create"))
                        }
                        GroupTarget::New { .. } => (t("New Group").to_string(), t("Create")),
                        GroupTarget::Rename(_) => (t("Rename Group").to_string(), t("Rename")),
                    };
                    kit::dialog_header(ui, None, &title, None, false);
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
            Dialog::Import(d) => d.show(ctx, store),
            Dialog::Settings(d) => d.show(ctx, settings),
        }
    }
}

// ----- export ---------------------------------------------------------------------------------

/// Which saved secrets the connections have: counted once in the background when the export
/// dialog opens (reading the Keychain can take a moment).
type SecretCounts = std::collections::HashMap<Uuid, (bool, bool)>;

pub struct ExportDialog {
    pub scope: ExportScope,
    error: Option<String>,
    picked: HashSet<Uuid>,
    show_list: bool,
    secrets: Option<SecretCounts>,
    secrets_rx: Option<std::sync::mpsc::Receiver<SecretCounts>>,
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
            secrets: None,
            secrets_rx: None,
        }
    }

    /// The connections the chosen scope exports.
    fn in_scope<'a>(&self, store: &'a BookmarkStore) -> Vec<&'a crate::bookmarks::Bookmark> {
        match &self.scope {
            ExportScope::All => store.bookmarks.iter().collect(),
            ExportScope::Group(g) => {
                let groups = store.group_and_descendants(*g);
                store
                    .bookmarks
                    .iter()
                    .filter(|b| b.group.is_some_and(|x| groups.contains(&x)))
                    .collect()
            }
            ExportScope::Selected(ids) => store
                .bookmarks
                .iter()
                .filter(|b| ids.contains(&b.id))
                .collect(),
        }
    }

    fn show(&mut self, ctx: &egui::Context, store: &BookmarkStore) -> DialogResult {
        use crate::secrets::SecretKind;
        // Count the saved secrets once, off the UI thread.
        if self.secrets.is_none() && self.secrets_rx.is_none() {
            let ids: Vec<Uuid> = store.bookmarks.iter().map(|b| b.id).collect();
            let (tx, rx) = std::sync::mpsc::channel();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let map: SecretCounts = ids
                    .into_iter()
                    .map(|id| {
                        let pw = crate::secrets::has(id, SecretKind::Password);
                        let key = crate::secrets::has(id, SecretKind::PrivateKey);
                        (id, (pw, key))
                    })
                    .collect();
                let _ = tx.send(map);
                c.request_repaint();
            });
            self.secrets_rx = Some(rx);
        }
        if let Some(rx) = &self.secrets_rx
            && let Ok(map) = rx.try_recv()
        {
            self.secrets = Some(map);
            self.secrets_rx = None;
        }
        let mut result = DialogResult::Open;
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let modal = style::modal(ctx, egui::Id::new("export")).show(ctx, |ui| {
            ui.set_width(600.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            if dialog_head(ui, t("Export connections"), None, true) {
                result = DialogResult::Close;
            }
            egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 14, bottom: 18 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(forms::label(t("What to export"), &p));
                ui.add_space(6.0);
                // All.
                let groups_used = store.groups.len();
                if scope_option(ui, self.scope == ExportScope::All, t("All connections"), &tf("{} connections in {} groups", &[&store.bookmarks.len().to_string(), &groups_used.to_string()]), |_| {}).clicked() {
                    self.scope = ExportScope::All;
                }
                ui.add_space(2.0);
                // One group.
                let group = match &self.scope {
                    ExportScope::Group(g) => Some(*g),
                    _ => None,
                };
                let mut pick_group = None;
                let one = scope_option(ui, group.is_some(), t("One group"), t("With its subgroups"), |ui| {
                    ui.add_enabled_ui(!store.groups.is_empty(), |ui| {
                        ui.scope(|ui| {
                            if group.is_none() {
                                ui.multiply_opacity(0.6);
                            }
                            ui.spacing_mut().interact_size.y = 28.0;
                            let label = group.map(|g| store.group_path(Some(g))).unwrap_or_else(|| t("(choose)").into());
                            egui::ComboBox::from_id_salt("export-group")
                                .icon(forms::updown_caret)
                                .selected_text(label)
                                .width(180.0)
                                .show_ui(ui, |ui| {
                                    for g in &store.groups {
                                        if ui.selectable_label(group == Some(g.id), store.group_path(Some(g.id))).clicked() {
                                            pick_group = Some(g.id);
                                        }
                                    }
                                });
                        });
                    });
                });
                if let Some(g) = pick_group {
                    self.scope = ExportScope::Group(g);
                }
                if one.clicked() && group.is_none() && let Some(g) = store.groups.first() {
                    self.scope = ExportScope::Group(g.id);
                }
                ui.add_space(2.0);
                // Selected.
                let selected_mode = matches!(self.scope, ExportScope::Selected(_));
                let n = self.picked.len();
                let caret = if self.show_list && selected_mode { icons::CARET_DOWN } else { icons::CARET_RIGHT };
                let sel = scope_option(ui, selected_mode, t("Selected connections"), &tf("Select them in the sidebar first ({}-click), or pick them here", &[if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" }]), |ui| {
                    kit::icon(ui, caret, 14.0, p.text_faint);
                    ui.label(RichText::new(if n == 0 { t("none selected").to_string() } else { tf("{} selected", &[&n.to_string()]) }).font(style::regular(style::SMALL)).color(p.text_faint));
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
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
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
                ui.add_space(14.0);
                // What the file holds.
                let list = self.in_scope(store);
                let mut groups: HashSet<Uuid> = HashSet::new();
                for b in &list {
                    let mut g = b.group;
                    while let Some(id) = g {
                        if !groups.insert(id) {
                            break;
                        }
                        g = store.group(id).and_then(|x| x.parent);
                    }
                }
                let forwards: usize = list.iter().map(|b| b.forwards.len()).sum();
                // What the file holds, as one sentence.
                let count = |n: usize, one: &str, many: &'static str| if n == 1 { one.to_string() } else { tf(many, &[&n.to_string()]) };
                let conns = count(list.len(), t("1 connection"), "{} connections");
                let grps = count(groups.len(), t("1 group with its label"), "{} groups with their labels");
                let fwds = count(forwards, t("1 forward"), "{} forwards");
                // Zero counts are left out ("2 forwards", never "0 passwords").
                let mut parts = vec![conns];
                if !groups.is_empty() {
                    parts.push(grps);
                }
                if forwards > 0 {
                    parts.push(fwds);
                }
                if let Some(map) = &self.secrets {
                    let pw = list.iter().filter(|b| map.get(&b.id).is_some_and(|s| s.0)).count();
                    let keys = list
                        .iter()
                        .filter(|b| map.get(&b.id).is_some_and(|s| s.1) || matches!(b.auth, crate::bookmarks::Auth::Key { .. }))
                        .count();
                    if pw > 0 {
                        parts.push(count(pw, t("1 password"), "{} passwords"));
                    }
                    if keys > 0 {
                        parts.push(count(keys, t("1 key"), "{} keys"));
                    }
                }
                let what = match parts.split_last() {
                    Some((last, rest)) if !rest.is_empty() => tf("{} and {}", &[&rest.join(", "), last]),
                    _ => parts.concat(),
                };
                let sentence = tf("The file will have {}.", &[&what]);
                let sentence = RichText::new(sentence).font(style::regular(style::BODY)).color(p.text_muted);
                if self.secrets.is_some() {
                    ui.add(egui::Label::new(sentence).wrap());
                } else {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        ui.label(sentence);
                        kit::spinner(ui, 12.0);
                    });
                }
                ui.add_space(14.0);
                // Redesign 3.1: the warning keeps its faint red box, without an icon.
                egui::Frame::NONE.fill(style::soft(p.err, 0.07)).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let mut job = egui::text::LayoutJob::default();
                    job.append(t("Passwords and keys are included as plain text."), 0.0, egui::TextFormat::simple(style::semibold(style::SMALL), p.text));
                    job.append("\n", 0.0, egui::TextFormat::simple(style::regular(style::SMALL), p.text));
                    job.append(t("Anyone with the file can log in to these servers. Keep it safe and delete it once it's imported. The file is readable only by your user."), 0.0, egui::TextFormat::simple(style::regular(style::SMALL), p.text_muted));
                    ui.add(egui::Label::new(job).wrap());
                });
                if let Some(e) = &self.error {
                    ui.add_space(8.0);
                    style::hint_tone(ui, e.as_str(), Tone::Err);
                }
            });
            dialog_foot(ui, |_| {}, |ui| {
                let go = kit::primary(ui, t("Export…")).clicked() || enter;
                if go {
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

/// A dialog's header (Redesign 3.1): the title (16 px), what is under it and a close button; no
/// icon tile. Returns whether close was clicked.
fn dialog_head(ui: &mut Ui, title: &str, sub: Option<egui::WidgetText>, close: bool) -> bool {
    let p = style::pal(ui);
    let mut closed = false;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 12,
            top: 18,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                let w = ui.available_width() - if close { 36.0 } else { 0.0 };
                ui.allocate_ui_with_layout(
                    vec2(w, 10.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(w);
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.label(
                            RichText::new(title)
                                .font(style::semibold(16.0))
                                .color(p.text),
                        );
                        if let Some(s) = sub {
                            ui.add(egui::Label::new(s).wrap());
                        }
                    },
                );
                if close {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if kit::icon_button_ex(ui, icons::X, t("Close"), false, 24.0).clicked() {
                            closed = true;
                        }
                    });
                }
            });
        });
    closed
}

/// A dialog's footer band: content on the left and buttons on the right (right to left).
fn dialog_foot(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    let p = style::pal(ui);
    let width = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(width, 55.0), Sense::hover());
    // Redesign 3.1: no band behind the footer, only the line above it.
    ui.painter()
        .hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, p.border));
    let inner = r.shrink2(vec2(16.0, 12.0));
    let mut l = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    l.spacing_mut().item_spacing.x = 8.0;
    left(&mut l);
    let mut rr = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    rr.spacing_mut().item_spacing.x = 8.0;
    right(&mut rr);
}

/// An export scope as a card: radio, title, a line under it, and room on the right.
fn scope_option(
    ui: &mut Ui,
    on: bool,
    title: &str,
    desc: &str,
    right: impl FnOnce(&mut Ui),
) -> egui::Response {
    let p = style::pal(ui);
    let width = ui.available_width();
    // Redesign 3.1: a plain radio row, no card around it.
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 12.0 + 18.2 + 16.8), Sense::click());
    let painter = ui.painter();
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    kit::paint_radio(
        painter,
        pos2(rect.left() + 2.0 + 8.0, rect.center().y),
        on,
        &p,
    );
    let x = rect.left() + 2.0 + 16.0 + 12.0;
    painter.text(
        pos2(x, rect.center().y - 9.0),
        Align2::LEFT_CENTER,
        title,
        style::medium(style::BODY),
        p.text,
    );
    painter.text(
        pos2(x, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        desc,
        style::regular(style::SMALL),
        p.text_faint,
    );
    let mut r = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                pos2(rect.center().x, rect.top()),
                pos2(rect.right() - 12.0, rect.bottom()),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    r.spacing_mut().item_spacing.x = 8.0;
    right(&mut r);
    resp
}

// ----- import ---------------------------------------------------------------------------------

pub struct ImportDialog {
    pub plan: ImportPlan,
    pub source: String,
    passphrase: String,
    show_passphrase: bool,
    unlock_message: Option<(String, bool)>,
    /// Key derivation runs on a worker thread (it takes a moment by design).
    unlocking: Option<std::sync::mpsc::Receiver<anyhow::Result<Vec<u8>>>>,
    /// An encrypted export asks for its passphrase first (or to go on without the secrets).
    asking: bool,
    /// What differs, per conflicting item (worked out once).
    diffs: Option<Vec<Vec<(String, String, String)>>>,
}

impl ImportDialog {
    pub fn new(plan: ImportPlan, source: String) -> Self {
        let asking = plan.encrypted_credentials.is_some();
        Self {
            plan,
            source,
            passphrase: String::new(),
            show_passphrase: false,
            unlock_message: None,
            unlocking: None,
            asking,
            diffs: None,
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
        match result.and_then(|plain| self.plan.attach_credentials(&plain)) {
            Ok(n) => {
                self.unlock_message = Some((
                    tn(
                        "Unlocked credentials for {} connection.",
                        "Unlocked credentials for {} connections.",
                        n,
                        &[],
                    ),
                    false,
                ));
                self.asking = false;
            }
            Err(e) => {
                let mut m = format!("{e:#}");
                if m == t("Decryption failed.") {
                    m = t("Decryption failed. Check the passphrase and try again.").to_string();
                }
                self.unlock_message = Some((m, true));
            }
        }
    }

    /// The fields of an incoming connection that differ from the one already here.
    fn differences(&mut self, store: &BookmarkStore) {
        if self.diffs.is_some() {
            return;
        }
        let login = |b: &crate::bookmarks::Bookmark| -> String {
            match &b.auth {
                crate::bookmarks::Auth::Password => t("password").into(),
                crate::bookmarks::Auth::Key { .. } => t("key file").into(),
                crate::bookmarks::Auth::KeychainKey => t("pasted key").into(),
                crate::bookmarks::Auth::Agent { .. } => t("SSH agent").into(),
            }
        };
        let jump = |b: &crate::bookmarks::Bookmark, s: &BookmarkStore| -> String {
            b.jump
                .and_then(|j| s.bookmark(j))
                .map_or_else(|| t("none").to_string(), |j| j.display_name())
        };
        let mut all = Vec::new();
        for item in &self.plan.items {
            let mut d = Vec::new();
            if let ItemStatus::Conflict { existing, .. } = item.status
                && let Some(old) = store.bookmark(existing)
            {
                let new = &item.incoming;
                if old.address() != new.address() {
                    d.push((t("address").to_string(), old.address(), new.address()));
                }
                if login(old) != login(new) {
                    d.push((t("login").to_string(), login(old), login(new)));
                }
                let (oj, nj) = (
                    jump(old, store),
                    new.jump
                        .map_or_else(|| t("none").to_string(), |_| jump(new, store)),
                );
                if old.jump != new.jump && oj != nj {
                    d.push((t("jump").to_string(), oj, nj));
                }
                if old.forwards.len() != new.forwards.len() {
                    d.push((
                        t("forwards").to_string(),
                        old.forwards.len().to_string(),
                        new.forwards.len().to_string(),
                    ));
                }
                if old.name != new.name {
                    d.push((t("name").to_string(), old.name.clone(), new.name.clone()));
                }
            }
            all.push(d);
        }
        self.diffs = Some(all);
    }

    fn show(&mut self, ctx: &egui::Context, store: &BookmarkStore) -> DialogResult {
        self.poll_unlock();
        if self.asking {
            return self.show_unlock(ctx);
        }
        self.differences(store);
        let mut result = DialogResult::Open;
        let screen = ctx.content_rect();
        let dark = ctx.global_style().visuals.dark_mode;
        let modal = style::modal(ctx, egui::Id::new("import")).show(ctx, |ui| {
            ui.set_width(760.0f32.min(screen.width() - 48.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            let kind = match self.plan.kind {
                SourceKind::TernaryShell => t("a TernaryShell export"),
                SourceKind::Electerm => t("an Electerm export"),
                SourceKind::SshConfig => t("an SSH config"),
            };
            let groups: HashSet<&str> = self.plan.items.iter().map(|i| i.group_path.as_str()).filter(|g| !g.is_empty()).collect();
            let sub = tf(
                "{}, {} with {} connections in {} groups",
                &[&self.source, kind, &self.plan.items.len().to_string(), &groups.len().to_string()],
            );
            let sub = RichText::new(sub).font(style::regular(style::BODY)).color(p.text_muted);
            if dialog_head(ui, t("Import connections"), Some(sub.into()), true) {
                result = DialogResult::Close;
            }
            egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 14, bottom: 18 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                // The numbers.
                let new = self.plan.items.iter().filter(|i| i.status == ItemStatus::New).count();
                let changed = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Conflict { matched_by: MatchedBy::Id, .. })).count();
                let here = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Identical { .. })).count();
                let conflicts = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Conflict { matched_by: MatchedBy::Endpoint, .. })).count();
                let mut parts: Vec<String> = Vec::new();
                if new > 0 {
                    parts.push(tf("{} new", &[&new.to_string()]));
                }
                if changed > 0 {
                    parts.push(tf("{} changed", &[&changed.to_string()]));
                }
                if here > 0 {
                    parts.push(tf("{} already here", &[&here.to_string()]));
                }
                if conflicts > 0 {
                    parts.push(if conflicts == 1 { t("1 conflict").to_string() } else { tf("{} conflicts", &[&conflicts.to_string()]) });
                }
                let summary = if parts.is_empty() { t("Nothing to import.").to_string() } else { format!("{}.", parts.join(", ")) };
                ui.label(RichText::new(summary).font(style::regular(style::BODY)).color(p.text_muted));
                for w in &self.plan.warnings {
                    ui.add_space(12.0);
                    kit::callout(ui, icons::WARNING, Tone::Warn, w);
                }
                if let Some((msg, false)) = &self.unlock_message {
                    ui.add_space(12.0);
                    kit::callout(ui, icons::CHECK_CIRCLE, Tone::Ok, msg);
                }
                let missing = self.plan.keys_missing();
                if missing > 0 {
                    ui.add_space(12.0);
                    kit::callout(ui, icons::WARNING, Tone::Warn, &tn("{} connection logs in with a pasted key that this file doesn't include. Paste the key again in Edit Connection before connecting (or choose a key file).", "{} connections log in with a pasted key that this file doesn't include. Paste the key again in Edit Connection before connecting (or choose a key file).", missing, &[]));
                }
                ui.add_space(14.0);
                // The list, grouped.
                let list_h = (screen.height() - 470.0).clamp(140.0, 330.0);
                let width = ui.available_width();
                let label_of = |path: &str| -> Option<Color32> {
                    let name = path.rsplit(" / ").next().unwrap_or(path);
                    self.plan
                        .groups
                        .iter()
                        .find(|g| g.name == name)
                        .and_then(|g| g.label.as_ref())
                        .or_else(|| store.groups.iter().find(|g| g.name == name).and_then(|g| store.group_label(Some(g.id))))
                        .map(|l| style::label_color(l.color, dark))
                };
                let mut order: Vec<String> = Vec::new();
                for i in &self.plan.items {
                    if !order.contains(&i.group_path) {
                        order.push(i.group_path.clone());
                    }
                }
                let diffs = self.diffs.clone().unwrap_or_default();
                egui::Frame::NONE
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(8))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            let mut first = true;
                            for path in &order {
                                // Group header.
                                let count = self.plan.items.iter().filter(|i| &i.group_path == path).count();
                                let (r, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
                                let painter = ui.painter();
                                if !first {
                                    painter.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, p.line_soft));
                                }
                                first = false;
                                let mut x = r.left() + 12.0;
                                if let Some(c) = label_of(path) {
                                    painter.circle_filled(pos2(x + 4.0, r.center().y), 4.0, c);
                                    x += 8.0 + 8.0;
                                }
                                painter.text(pos2(x, r.center().y), Align2::LEFT_CENTER, if path.is_empty() { t("Ungrouped") } else { path.as_str() }, style::semibold(style::SMALL), p.text_muted);
                                painter.text(pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, count.to_string(), style::regular(style::SMALL), p.text_faint);
                                for (i, item) in self.plan.items.iter_mut().enumerate() {
                                    if &item.group_path != path {
                                        continue;
                                    }
                                    let diff = diffs.get(i).cloned().unwrap_or_default();
                                    import_row(ui, i, item, &diff, width, label_of(path), dark);
                                }
                            }
                        });
                    });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    kit::icon(ui, icons::INFO, 13.0, p.text_faint);
                    ui.label(RichText::new(t("Importing the same file twice changes nothing.")).font(style::regular(style::SMALL)).color(p.text_faint));
                });
                let commands = self.plan.startup_command_count();
                if commands > 0 {
                    ui.add_space(14.0);
                    let r = egui::Frame::NONE
                        .fill(style::soft(p.warn, 0.07))
                        .corner_radius(CornerRadius::same(style::RADIUS))
                        .inner_margin(egui::Margin::symmetric(12, 9))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                let on = self.plan.import_startup_commands;
                                let (b, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                                let bx = egui::Rect::from_center_size(b.center(), vec2(16.0, 16.0));
                                if on {
                                    ui.painter().rect_filled(bx, 4.0, p.accent);
                                    ui.painter().text(bx.center(), Align2::CENTER_CENTER, icons::CHECK, style::icon_font(11.0), Color32::WHITE);
                                } else {
                                    ui.painter().rect(bx, 4.0, p.field, Stroke::new(1.0, p.border_strong), StrokeKind::Inside);
                                }
                                let mut job = egui::text::LayoutJob::default();
                                job.append(t("Import startup commands"), 0.0, egui::TextFormat::simple(style::semibold(style::SMALL), p.text));
                                job.append(&format!(" {}", tf("({} connections have them)", &[&commands.to_string()])), 0.0, egui::TextFormat::simple(style::regular(style::SMALL), p.text_muted));
                                job.append("\n", 0.0, egui::TextFormat::simple(style::regular(style::SMALL), p.text));
                                job.append(t("They run on the server every time the connection opens. Only import them from a file you trust."), 0.0, egui::TextFormat::simple(style::regular(style::SMALL), p.text_muted));
                                ui.add(egui::Label::new(job).wrap());
                            });
                        });
                    if ui.interact(r.response.rect, egui::Id::new("import-commands"), Sense::click()).clicked() {
                        self.plan.import_startup_commands = !self.plan.import_startup_commands;
                    }
                }
            });
            let (add, change, _) = self.plan.summary();
            let has_conflicts = self.plan.conflicts() > 0;
            let res: Vec<Resolution> = self.plan.items.iter().filter(|i| matches!(i.status, ItemStatus::Conflict { .. })).map(|i| i.resolution).collect();
            let mut current = if !res.is_empty() && res.iter().all(|r| *r == res[0]) { Some(res[0]) } else { None };
            let before = current;
            let busy = self.unlocking.is_some();
            let mut do_import = false;
            let mut cancel = false;
            dialog_foot(ui, |ui| {
                if has_conflicts {
                    ui.label(RichText::new(t("For all conflicts")).font(style::regular(style::SMALL)).color(p.text_faint));
                    kit::segmented(ui, &mut current, &[(Some(Resolution::Skip), t("Skip")), (Some(Resolution::Overwrite), t("Overwrite")), (Some(Resolution::KeepBoth), t("Keep both"))], true);
                }
            }, |ui| {
                let import = ui.add_enabled(add + change > 0 && !busy, style::primary(tf("Import {}", &[&(add + change).to_string()])));
                #[cfg(debug_assertions)]
                crate::devtest::record("import-ok", import.rect);
                do_import = import.clicked();
                cancel = kit::secondary(ui, None, t("Cancel")).clicked();
            });
            if current != before && let Some(r) = current {
                self.plan.resolve_all_conflicts(r);
            }
            if do_import {
                result = DialogResult::Import(Box::new(self.plan.clone()));
            }
            if cancel {
                result = DialogResult::Close;
            }
        });
        if escape_only(ctx, &modal) && matches!(result, DialogResult::Open) {
            result = DialogResult::Close;
        }
        result
    }

    /// An encrypted export from TernaryShell 1.2.3 or earlier: its passphrase, or go on
    /// without the secrets.
    fn show_unlock(&mut self, ctx: &egui::Context) -> DialogResult {
        let mut result = DialogResult::Open;
        let enter = ctx.input(|i| i.key_pressed(Key::Enter));
        let modal = style::modal(ctx, egui::Id::new("import-unlock")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let p = style::pal(ui);
            dialog_head(
                ui,
                t("Unlock passwords and keys"),
                Some(RichText::new(t("This file contains encrypted passwords and keys. Enter the export passphrase to import them, or continue without them.")).font(style::regular(style::BODY)).color(p.text_muted).into()),
                false,
            );
            let busy = self.unlocking.is_some();
            egui::Frame::NONE.inner_margin(egui::Margin { left: 20, right: 20, top: 14, bottom: 18 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 10.0;
                // The file.
                let mut job = egui::text::LayoutJob::default();
                job.append(&format!("{}  ", icons::FILE_CODE), 0.0, egui::TextFormat::simple(style::icon_font(13.0), p.text_muted));
                job.append(&self.source, 0.0, egui::TextFormat::simple(style::mono(11.5), p.text));
                job.append(&format!("  · {}", tf("{} connections", &[&self.plan.items.len().to_string()])), 0.0, egui::TextFormat::simple(style::regular(style::SMALL), p.text_faint));
                let g = ui.painter().layout_job(job);
                let (r, _) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 24.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(6), p.hover);
                ui.painter().galley(pos2(r.left() + 9.0, r.center().y - g.size().y / 2.0), g, p.text);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.label(forms::label(t("Passphrase"), &p));
                    let error = matches!(self.unlock_message, Some((_, true)));
                    let field = ui.add_enabled(
                        !busy,
                        style::input(&mut self.passphrase)
                            .password(!self.show_passphrase)
                            .margin(egui::Margin { left: 10, right: 30, top: 7, bottom: 7 })
                            .error(error),
                    );
                    #[cfg(debug_assertions)]
                    crate::devtest::record("import-passphrase", field.rect);
                    if field.changed() {
                        self.unlock_message = None;
                    }
                    let eye = egui::Rect::from_center_size(pos2(field.rect.right() - 15.0, field.rect.center().y), vec2(20.0, 20.0));
                    let er = ui.interact(eye, egui::Id::new("import-eye"), Sense::click());
                    kit::paint_icon_button(ui, &er, eye, if self.show_passphrase { icons::EYE_SLASH } else { icons::EYE }, t("Show"), false, false);
                    if er.clicked() {
                        self.show_passphrase = !self.show_passphrase;
                    }
                    if let Some((m, true)) = &self.unlock_message {
                        style::hint_tone(ui, m.as_str(), Tone::Err);
                    }
                });
            });
            let (mut skip, mut go, mut cancel) = (false, false, false);
            let typed = !self.passphrase.is_empty();
            dialog_foot(ui, |ui| {
                skip = kit::ghost(ui, None, t("Continue Without")).clicked();
            }, |ui| {
                go = ui.add_enabled_ui(!busy && typed, |ui| kit::primary(ui, t("Unlock"))).inner.clicked()
                    || (enter && !busy && typed);
                if busy {
                    kit::spinner(ui, 14.0);
                }
                cancel = kit::secondary(ui, None, t("Cancel")).clicked();
            });
            if skip {
                self.asking = false;
                self.plan.encrypted_credentials = None;
            }
            if go && !busy {
                self.start_unlock(ui.ctx());
            }
            if cancel {
                result = DialogResult::Close;
            }
        });
        if escape_only(ctx, &modal) && matches!(result, DialogResult::Open) {
            result = DialogResult::Close;
        }
        result
    }
}

/// A connection in the import list: a checkbox (whether it is imported), its glyph, name,
/// address and what will happen in coloured words; a conflict shows what differs.
fn import_row(
    ui: &mut Ui,
    i: usize,
    item: &mut crate::bookmarks::transfer::ImportItem,
    diff: &[(String, String, String)],
    width: f32,
    label: Option<Color32>,
    _dark: bool,
) {
    let p = style::pal(ui);
    let conflict = matches!(
        item.status,
        ItemStatus::Conflict {
            matched_by: MatchedBy::Endpoint,
            ..
        }
    );
    let changed = matches!(
        item.status,
        ItemStatus::Conflict {
            matched_by: MatchedBy::Id,
            ..
        }
    );
    let identical = matches!(item.status, ItemStatus::Identical { .. });
    let show_diff = conflict && !diff.is_empty();
    let h = if show_diff { 40.0 + 20.0 } else { 40.0 };
    let (r, _) = ui.allocate_exact_size(vec2(width, h), Sense::hover());
    let painter = ui.painter();
    painter.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, p.line_soft));
    let row = egui::Rect::from_min_size(r.min, vec2(width, 40.0));
    let cy = row.center().y;
    // Checkbox.
    let on = item.resolution != Resolution::Skip && !identical;
    let bx = egui::Rect::from_center_size(pos2(row.left() + 12.0 + 8.0, cy), vec2(16.0, 16.0));
    let cb = ui.interact(
        bx.expand(3.0),
        egui::Id::new(("import-check", i)),
        if identical {
            Sense::hover()
        } else {
            Sense::click()
        },
    );
    let painter = ui.painter();
    if on {
        painter.rect_filled(bx, 4.0, p.accent);
        painter.text(
            bx.center(),
            Align2::CENTER_CENTER,
            icons::CHECK,
            style::icon_font(11.0),
            Color32::WHITE,
        );
    } else {
        painter.rect(
            bx,
            4.0,
            p.field,
            Stroke::new(1.0, p.border_strong),
            StrokeKind::Inside,
        );
    }
    if cb.clicked() {
        item.resolution = if on {
            Resolution::Skip
        } else {
            match item.status {
                ItemStatus::New => Resolution::Add,
                ItemStatus::Conflict {
                    matched_by: MatchedBy::Id,
                    ..
                } => Resolution::Overwrite,
                _ => Resolution::KeepBoth,
            }
        };
    }
    // Glyph, name, address.
    let glyph = egui::Rect::from_min_size(pos2(bx.right() + 10.0, cy - 10.0), vec2(20.0, 20.0));
    kit::paint_host_glyph(
        painter,
        glyph,
        kit::host_icon(item.incoming.host_icon()),
        label,
        None,
        p.dialog,
        &p,
    );
    let name = item.incoming.display_name();
    let ng = painter.layout_no_wrap(
        name,
        style::medium(style::BODY),
        if identical { p.text_muted } else { p.text },
    );
    let nw = ng.size().x;
    painter.galley(
        pos2(glyph.right() + 10.0, cy - ng.size().y / 2.0),
        ng,
        p.text,
    );
    let mut addr = item.incoming.address();
    if !item.secrets.is_empty() {
        addr.push_str(&format!("  {}", icons::KEY));
    }
    painter.text(
        pos2(glyph.right() + 10.0 + nw + 10.0, cy),
        Align2::LEFT_CENTER,
        addr,
        style::mono(11.0),
        p.text_faint,
    );
    // What happens, on the right.
    let mut right = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                pos2(row.center().x - 40.0, row.top()),
                pos2(row.right() - 12.0, row.bottom()),
            ))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right.spacing_mut().item_spacing.x = 8.0;
    match item.status {
        ItemStatus::New => {
            kit::badge(&mut right, t("new"), Tone::Ok, None);
        }
        ItemStatus::Identical { .. } => {
            right.label(
                RichText::new(t("already here"))
                    .font(style::regular(style::SMALL))
                    .color(p.text_faint),
            );
        }
        ItemStatus::Conflict {
            matched_by: MatchedBy::Id,
            ..
        } => {
            let summary = match diff.first() {
                Some((field, _, new)) if field == t("forwards") => tf("{} forwards now", &[new]),
                Some((field, _, _)) => tf("{} changed", &[field]),
                None => String::new(),
            };
            if !summary.is_empty() {
                right.label(
                    RichText::new(summary)
                        .font(style::regular(style::SMALL))
                        .color(p.text_faint),
                );
            }
            kit::badge(&mut right, t("changed"), Tone::Accent, None);
        }
        ItemStatus::Conflict { .. } => {
            let mut res = item.resolution;
            kit::segmented(
                &mut right,
                &mut res,
                &[
                    (Resolution::KeepBoth, t("Keep both")),
                    (Resolution::Overwrite, t("Overwrite")),
                    (Resolution::Skip, t("Skip")),
                ]
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>(),
                true,
            );
            item.resolution = res;
            kit::badge(&mut right, t("Conflict"), Tone::Warn, None)
                .on_hover_text(t("same host, port and user"));
        }
    }
    let _ = changed;
    // What differs.
    if show_diff {
        let mut job = egui::text::LayoutJob::default();
        let fmt = |c: Color32| egui::TextFormat::simple(style::regular(11.5), c);
        job.append(t("same host, port and user"), 0.0, fmt(p.text_muted));
        for (field, old, new) in diff {
            job.append(&format!(" · {field} "), 0.0, fmt(p.text_muted));
            job.append(
                old,
                0.0,
                egui::TextFormat {
                    strikethrough: Stroke::new(1.0, style::soft(p.err, 0.6)),
                    ..fmt(p.err)
                },
            );
            job.append(" → ", 0.0, fmt(p.text_muted));
            job.append(new, 0.0, fmt(p.ok));
        }
        job.wrap = egui::text::TextWrapping::truncate_at_width(width - 30.0 - 24.0);
        let g = ui.painter().layout_job(job);
        ui.painter()
            .galley(pos2(glyph.left(), row.bottom() - 2.0), g, p.text);
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

impl SettingsTab {
    const ALL: [SettingsTab; 5] = [
        SettingsTab::General,
        SettingsTab::Appearance,
        SettingsTab::Terminal,
        SettingsTab::QuickCommands,
        SettingsTab::Shortcuts,
    ];
}

#[derive(Default)]
pub struct SettingsDialog {
    tab: SettingsTab,
    shortcut_filter: String,
    /// The settings search, and the hits per page for it.
    query: String,
    hits: Option<(String, [usize; 5])>,
    /// Which terminal colour gallery is shown (dark or light schemes); `None` = the theme's.
    schemes_dark: Option<bool>,
    /// Shortcuts shown for macOS (or for Windows and Linux).
    keys_mac: Option<bool>,
}

/// The settings as the search sees them: page, group, row label and description.
fn settings_index() -> Vec<(SettingsTab, &'static str, &'static str, &'static str)> {
    use SettingsTab::*;
    let meta = if cfg!(target_os = "macos") {
        t("Use Option as Meta key")
    } else {
        t("Use Alt as Meta key")
    };
    vec![
        (
            General,
            t("Language"),
            t("Language"),
            t("The menu bar follows the system language."),
        ),
        (
            General,
            t("Connections"),
            t("Keepalive"),
            t("Checks the server this often, so a dead connection is noticed within seconds."),
        ),
        (
            General,
            t("Connections"),
            t("Ask before closing a connected tab"),
            t("Only when something is still running there."),
        ),
        (
            General,
            t("Updates"),
            t("Check for updates automatically"),
            t(
                "At start and every 6 hours. The only connection TernaryShell makes besides your servers.",
            ),
        ),
        (General, t("Updates"), t("Version"), ""),
        (
            General,
            t("Data"),
            t("Data folder"),
            t("connections, settings, known hosts"),
        ),
        (General, t("Data"), t("Passwords and keys"), ""),
        (Appearance, t("Theme"), t("Theme"), t("Match System")),
        (
            Appearance,
            t("Window"),
            t("Accent"),
            t("Focus rings, the main button and links."),
        ),
        (
            Appearance,
            t("Window"),
            t("Density"),
            t("Compact uses 24 px rows in the sidebar and in Files."),
        ),
        (Appearance, t("Terminal colors"), t("Terminal colors"), ""),
        (
            Terminal,
            t("Text"),
            t("Font"),
            t("Menlo, SF Mono, the built-in Hack, or your own file."),
        ),
        (Terminal, t("Text"), t("Text size"), ""),
        (
            Terminal,
            t("Text"),
            t("Chinese, Japanese and Korean fallback fonts"),
            t("Loaded from the system on first use; adds about 20 MB of memory."),
        ),
        (Terminal, t("Cursor"), t("Shape"), ""),
        (Terminal, t("Cursor"), t("Blink"), ""),
        (
            Terminal,
            t("Behavior"),
            t("Scrollback"),
            t("Lower it if you keep many busy tabs open."),
        ),
        (Terminal, t("Behavior"), meta, ""),
        (Terminal, t("Behavior"), t("Copy text when selected"), ""),
        (
            QuickCommands,
            t("Quick Commands"),
            t("Quick Commands"),
            t("Commands you send often"),
        ),
        (
            Shortcuts,
            t("Shortcuts"),
            t("Shortcuts"),
            t("Keyboard Shortcuts"),
        ),
    ]
}

/// Whether a row is a hit for the search `q`: its label, description or group matches.
fn hit(q: &str, group: &str, label: &str, desc: &str) -> bool {
    forms::matches(group, q) || forms::matches(label, q) || forms::matches(desc, q)
}

/// The settings dialog's search: the query and what a page needs to draw hits.
#[derive(Clone, Copy)]
struct Search<'a> {
    q: &'a str,
}

impl Search<'_> {
    fn on(&self) -> bool {
        !self.q.trim().is_empty()
    }

    /// A group: dimmed when nothing in it matches.
    fn group(
        &self,
        ui: &mut Ui,
        title: &str,
        labels: &[(&str, &str)],
        right: Option<&mut dyn FnMut(&mut Ui)>,
        add: impl FnOnce(&mut Ui, &mut forms::Rows),
    ) {
        let any = !self.on() || labels.iter().any(|(l, d)| hit(self.q, title, l, d));
        ui.scope(|ui| {
            if !any {
                ui.multiply_opacity(0.42);
            }
            forms::group_q(ui, &self.title(title), self.q, right, add);
        });
        ui.add_space(18.0);
    }

    fn title(&self, title: &str) -> String {
        title.to_string()
    }

    /// A row of a group: the label with matches in the accent, tinted when it is a hit.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &self,
        ui: &mut Ui,
        rows: &mut forms::Rows,
        group: &str,
        label: &str,
        desc: Option<egui::WidgetText>,
        desc_text: &str,
        control: impl FnOnce(&mut Ui),
    ) {
        let p = style::pal(ui);
        let is_hit = self.on() && hit(self.q, group, label, desc_text);
        let job = forms::highlight(
            label,
            self.q,
            style::medium(style::BODY),
            p.text,
            p.accent_text,
        );
        forms::row(ui, rows, job.into(), desc, is_hit, control);
    }
}

impl SettingsDialog {
    pub fn new(tab: Option<SettingsTab>) -> Self {
        Self {
            tab: tab.unwrap_or_default(),
            ..Default::default()
        }
    }

    /// Debug builds: search the settings for `query` (the end-to-end driver).
    #[cfg(debug_assertions)]
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
    }

    fn show(&mut self, ctx: &egui::Context, s: &mut Settings) -> DialogResult {
        let mut result = DialogResult::Open;
        let screen = ctx.content_rect();
        let size = vec2(
            980.0f32.min(screen.width() - 40.0),
            680.0f32.min(screen.height() - 40.0),
        );
        // Hits per page, recounted only when the search changes.
        if self.hits.as_ref().is_none_or(|(q, _)| *q != self.query) {
            let mut counts = [0usize; 5];
            if !self.query.trim().is_empty() {
                for (tab, group, label, desc) in settings_index() {
                    if hit(&self.query, group, label, desc) {
                        counts[tab as usize] += 1;
                    }
                }
            }
            // The page shown has nothing for this search: go to the first one that has.
            if counts[self.tab as usize] == 0
                && let Some(tab) = SettingsTab::ALL
                    .into_iter()
                    .find(|t| counts[*t as usize] > 0)
            {
                self.tab = tab;
            }
            self.hits = Some((self.query.clone(), counts));
        }
        let counts = self.hits.as_ref().map(|(_, c)| *c).unwrap_or_default();
        let modal = style::modal(ctx, egui::Id::new("settings")).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let p = style::pal(ui);
            let full = egui::Rect::from_min_size(ui.max_rect().min, size);
            let nav = egui::Rect::from_min_size(full.min, vec2(220.0, size.y));
            forms::paint_nav(ui.painter(), nav, &p);
            // Title and search.
            ui.painter().text(
                pos2(nav.left() + 10.0 + 8.0, nav.top() + 16.0 + 2.0 + 10.5),
                Align2::LEFT_CENTER,
                t("Settings"),
                style::semibold(15.0),
                p.text,
            );
            let search_r = egui::Rect::from_min_size(
                pos2(
                    nav.left() + 10.0,
                    nav.top() + 16.0 + 2.0 + 21.0 + 12.0 + 2.0,
                ),
                vec2(nav.width() - 20.0, 28.0),
            );
            let mut search_ui = ui.new_child(egui::UiBuilder::new().max_rect(search_r));
            search_ui.spacing_mut().interact_size.y = 28.0;
            let sr = kit::quiet_search(
                &mut search_ui,
                egui::Id::new("settings-search"),
                &mut self.query,
                t("Search settings"),
                None,
            );
            #[cfg(debug_assertions)]
            crate::devtest::record("settings-search", sr.rect);
            let _ = sr;
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
            let searching = !self.query.trim().is_empty();
            let items = egui::Rect::from_min_max(
                pos2(nav.left() + 10.0, search_r.bottom() + 12.0),
                pos2(nav.right() - 10.0, nav.bottom() - 70.0),
            );
            let mut nav_ui = ui.new_child(egui::UiBuilder::new().max_rect(items));
            nav_ui.spacing_mut().item_spacing.y = 2.0;
            for (tab, icon, label) in pages {
                let n = counts[tab as usize];
                let r = forms::nav_item(
                    &mut nav_ui,
                    icon,
                    label,
                    self.tab == tab,
                    forms::NavExtra {
                        count: (searching && n > 0).then_some(n),
                        dim: searching && n == 0 && self.tab != tab,
                        ..Default::default()
                    },
                );
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
            // A search that has nothing on this page moves to the first page that has.
            if searching
                && counts[self.tab as usize] == 0
                && let Some(first) = SettingsTab::ALL.iter().find(|t| counts[**t as usize] > 0)
                && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(_))))
            {
                self.tab = *first;
            }
            // Version and the update check, at the bottom of the navigation.
            let foot_x = nav.left() + 10.0 + 10.0;
            let foot_top = nav.bottom() - 14.0 - 2.0 * 19.2;
            ui.painter().text(
                pos2(foot_x, foot_top + 9.6),
                Align2::LEFT_CENTER,
                format!("TernaryShell {}", crate::update::CURRENT),
                style::regular(style::SMALL),
                p.text_faint,
            );
            let g = ui.painter().layout_no_wrap(
                t("Check for updates…").to_string(),
                style::regular(style::SMALL),
                p.accent_text,
            );
            let link_r =
                egui::Rect::from_min_size(pos2(foot_x, foot_top + 19.2), vec2(g.size().x, 19.2));
            let link = ui.interact(link_r, egui::Id::new("settings-check"), Sense::click());
            ui.painter().galley(
                pos2(foot_x, link_r.center().y - g.size().y / 2.0),
                g,
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
            let title = pages.iter().find(|x| x.0 == self.tab).map_or("", |x| x.2);
            if forms::sheet_head(
                ui,
                main.left() + 24.0,
                main.top() + 20.0,
                main.right() - 24.0,
                title,
            ) {
                result = DialogResult::Close;
            }
            let body = egui::Rect::from_min_max(
                pos2(main.left(), main.top() + 20.0 + 22.4 + 4.0),
                main.max,
            );
            let mut body_ui = ui.new_child(egui::UiBuilder::new().max_rect(body));
            let search = Search { q: &self.query };
            let out = egui::ScrollArea::vertical()
                .id_salt(("settings-page", self.tab as u8))
                .auto_shrink([false, false])
                .show(&mut body_ui, |ui| {
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 24,
                            right: 24,
                            top: 12,
                            bottom: 2,
                        })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.spacing_mut().item_spacing.y = 0.0;
                            match self.tab {
                                SettingsTab::General => {
                                    if general(ui, s, search) {
                                        result = DialogResult::Run(Action::CheckForUpdates);
                                    }
                                }
                                SettingsTab::Appearance => {
                                    appearance(ui, s, search, &mut self.schemes_dark)
                                }
                                SettingsTab::Terminal => terminal(ui, s, search),
                                SettingsTab::QuickCommands => quick_commands(ui, s),
                                SettingsTab::Shortcuts => {
                                    shortcuts(ui, &mut self.shortcut_filter, &mut self.keys_mac)
                                }
                            }
                        });
                });
            // More below: a fade at the bottom edge.
            let more = out.content_size.y - out.state.offset.y - out.inner_rect.height();
            if more > 2.0 {
                let fade = egui::Rect::from_min_max(
                    pos2(body.left(), body.bottom() - 40.0),
                    pos2(body.right() - 10.0, body.bottom() - 1.0),
                );
                fade_down(ui.painter(), fade, p.dialog);
            }
        });
        if modal.should_close() && matches!(result, DialogResult::Open) {
            result = DialogResult::Close;
        }
        result
    }
}

/// A vertical fade to `color` at the bottom of `rect` (more content below).
fn fade_down(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let mut mesh = egui::Mesh::default();
    let clear = Color32::from_rgba_premultiplied(0, 0, 0, 0);
    mesh.colored_vertex(rect.left_top(), clear);
    mesh.colored_vertex(rect.right_top(), clear);
    mesh.colored_vertex(rect.left_bottom(), color);
    mesh.colored_vertex(rect.right_bottom(), color);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
}

/// An item of a dialog's side navigation (30 px): icon, label, accent when on, a red dot when
/// the page has a problem.
pub fn nav_item(ui: &mut Ui, icon: &str, label: &str, on: bool, problem: bool) -> egui::Response {
    forms::nav_item(
        ui,
        icon,
        label,
        on,
        forms::NavExtra {
            problem,
            ..Default::default()
        },
    )
}

/// A number typed into a field (with a unit inside it on the right): the text is kept while it
/// is edited and the value follows when it parses.
fn number_field<T>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    unit: &str,
    width: f32,
    thousands: bool,
) where
    T: Copy + PartialOrd + std::str::FromStr + std::fmt::Display + Into<u64>,
{
    let p = style::pal(ui);
    let id = egui::Id::new(id);
    let shown = |v: T| {
        if thousands {
            format_thousands(Into::<u64>::into(v) as usize)
        } else {
            v.to_string()
        }
    };
    let mut text: String = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| shown(*value));
    let unit_w = ui
        .painter()
        .layout_no_wrap(unit.to_string(), style::regular(style::SMALL), p.text_faint)
        .size()
        .x;
    let r = ui.add(
        style::input(&mut text)
            .id(id.with("edit"))
            .desired_width(width)
            .mono()
            .margin(egui::Margin {
                left: 10,
                right: (unit_w + 18.0) as i8,
                top: 7,
                bottom: 7,
            }),
    );
    ui.painter().text(
        pos2(r.rect.right() - 10.0, r.rect.center().y),
        Align2::RIGHT_CENTER,
        unit,
        style::regular(style::SMALL),
        p.text_faint,
    );
    if r.changed()
        && let Ok(v) = text.replace(',', "").trim().parse::<T>()
        && range.contains(&v)
    {
        *value = v;
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
}

/// The relative time of a Unix timestamp ("2 h ago").
fn ago(at: i64) -> String {
    let secs = (chrono::Utc::now().timestamp() - at).max(0);
    if secs < 60 {
        t("Just now").to_string()
    } else if secs < 3600 {
        tf("{} min ago", &[&(secs / 60).to_string()])
    } else if secs < 86_400 {
        tf("{} h ago", &[&(secs / 3600).to_string()])
    } else if secs < 2 * 86_400 {
        t("Yesterday").to_string()
    } else {
        tf("{} days ago", &[&(secs / 86_400).to_string()])
    }
}

fn small(text: impl Into<String>, color: Color32) -> egui::WidgetText {
    RichText::new(text.into())
        .font(style::regular(style::SMALL))
        .color(color)
        .into()
}

/// General. Returns true when "Check Now" was clicked.
fn general(ui: &mut Ui, s: &mut Settings, q: Search) -> bool {
    let p = style::pal(ui);
    let mut check_now = false;
    let g = t("Language");
    q.group(
        ui,
        g,
        &[(
            t("Language"),
            t("The menu bar follows the system language."),
        )],
        None,
        |ui, rows| {
            q.row(
                ui,
                rows,
                g,
                t("Language"),
                Some(small(
                    t("The menu bar follows the system language."),
                    p.text_faint,
                )),
                t("The menu bar follows the system language."),
                |ui| {
                    egui::ComboBox::from_id_salt("lang")
                        .icon(forms::updown_caret)
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
        },
    );
    let g = t("Connections");
    let keep = t("Checks the server this often, so a dead connection is noticed within seconds.");
    let ask = t("Only when something is still running there.");
    q.group(
        ui,
        g,
        &[
            (t("Keepalive"), keep),
            (t("Ask before closing a connected tab"), ask),
        ],
        None,
        |ui, rows| {
            q.row(
                ui,
                rows,
                g,
                t("Keepalive"),
                Some(small(keep, p.text_faint)),
                keep,
                |ui| {
                    number_field(
                        ui,
                        "settings-keepalive",
                        &mut s.keepalive_secs,
                        0..=600,
                        t("sec"),
                        92.0,
                        false,
                    );
                },
            );
            q.row(
                ui,
                rows,
                g,
                t("Ask before closing a connected tab"),
                Some(small(ask, p.text_faint)),
                ask,
                |ui| {
                    kit::switch(ui, &mut s.confirm_close_connected);
                },
            );
        },
    );
    let g = t("Updates");
    let auto = t(
        "At start and every 6 hours. The only connection TernaryShell makes besides your servers.",
    );
    let checked = match s.last_update_check {
        Some(at) => tf("Checked {}", &[&ago(at).to_lowercase()]),
        None => t("Not checked yet").to_string(),
    };
    q.group(
        ui,
        g,
        &[
            (t("Check for updates automatically"), auto),
            (t("Version"), ""),
        ],
        None,
        |ui, rows| {
            q.row(
                ui,
                rows,
                g,
                t("Check for updates automatically"),
                Some(small(auto, p.text_faint)),
                auto,
                |ui| {
                    kit::switch(ui, &mut s.check_updates);
                },
            );
            q.row(
                ui,
                rows,
                g,
                &format!("TernaryShell {}", crate::update::CURRENT),
                Some(small(checked.clone(), p.text_faint)),
                "",
                |ui| {
                    if kit::secondary_sm(ui, None, t("Check Now")).clicked() {
                        check_now = true;
                    }
                },
            );
        },
    );
    let g = t("Data");
    let folder = crate::paths::contract_tilde(crate::paths::data_dir());
    let mut desc = egui::text::LayoutJob::default();
    desc.append(
        &folder,
        0.0,
        egui::TextFormat::simple(style::mono(11.5), p.text_faint),
    );
    desc.append(
        &format!(" · {}", t("connections, settings, known hosts")),
        0.0,
        egui::TextFormat::simple(style::regular(style::SMALL), p.text_faint),
    );
    let mut secrets_desc = egui::text::LayoutJob::default();
    secrets_desc.append(
        &tf("Kept in {} (service ", &[crate::platform::keychain()]),
        0.0,
        egui::TextFormat::simple(style::regular(style::SMALL), p.text_faint),
    );
    secrets_desc.append(
        "ternaryshell",
        0.0,
        egui::TextFormat::simple(style::mono(11.5), p.text_faint),
    );
    secrets_desc.append(
        t("), never in TernaryShell's files."),
        0.0,
        egui::TextFormat::simple(style::regular(style::SMALL), p.text_faint),
    );
    q.group(
        ui,
        g,
        &[
            (t("Data folder"), t("connections, settings, known hosts")),
            (t("Passwords and keys"), ""),
        ],
        None,
        |ui, rows| {
            q.row(
                ui,
                rows,
                g,
                t("Data folder"),
                Some(desc.into()),
                t("connections, settings, known hosts"),
                |ui| {
                    if kit::button_ex(
                        ui,
                        kit::Kind::Secondary,
                        Some(icons::ARROW_SQUARE_OUT),
                        crate::platform::reveal_label(),
                        true,
                    )
                    .clicked()
                    {
                        crate::platform::open(crate::paths::data_dir());
                    }
                },
            );
            q.row(
                ui,
                rows,
                g,
                t("Passwords and keys"),
                Some(secrets_desc.into()),
                "",
                |ui| {
                    kit::chip(
                        ui,
                        Some(icons::LOCK_SIMPLE),
                        crate::platform::SECRET_STORE,
                        false,
                    );
                },
            );
        },
    );
    check_now
}

/// Appearance: the theme as preview cards, the accent and density, the terminal colours.
fn appearance(ui: &mut Ui, s: &mut Settings, q: Search, schemes_dark: &mut Option<bool>) {
    let p = style::pal(ui);
    let g = t("Theme");
    q.group(ui, g, &[(t("Theme"), t("Match System"))], None, |ui, _| {
        egui::Frame::NONE
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
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
            });
    });
    let g = t("Window");
    let acc = t("Focus rings, the main button and links.");
    let den = t("Compact uses 24 px rows in the sidebar and in Files.");
    q.group(
        ui,
        g,
        &[(t("Accent"), acc), (t("Density"), den)],
        None,
        |ui, rows| {
            q.row(
                ui,
                rows,
                g,
                t("Accent"),
                Some(small(acc, p.text_faint)),
                acc,
                |ui| {
                    accent_picker(ui, &mut s.accent);
                },
            );
            q.row(
                ui,
                rows,
                g,
                t("Density"),
                Some(small(den, p.text_faint)),
                den,
                |ui| {
                    kit::segmented(
                        ui,
                        &mut s.density,
                        &[
                            (crate::settings::Density::Compact, t("Compact")),
                            (crate::settings::Density::Comfortable, t("Comfortable")),
                        ]
                        .iter()
                        .rev()
                        .copied()
                        .collect::<Vec<_>>(),
                        false,
                    );
                },
            );
        },
    );
    let dark = schemes_dark.unwrap_or(p.dark);
    let mut pick = dark;
    let g = t("Terminal colors");
    q.group(
        ui,
        g,
        &[(t("Terminal colors"), "")],
        Some(&mut |ui: &mut Ui| {
            kit::segmented_icons(
                ui,
                &mut pick,
                &[
                    (true, Some(icons::MOON), t("Dark theme")),
                    (false, Some(icons::SUN), t("Light theme")),
                ],
                false,
            );
        }),
        |ui, _| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let value = if dark {
                        &mut s.scheme_dark
                    } else {
                        &mut s.scheme_light
                    };
                    let schemes: Vec<&Scheme> =
                        SCHEMES.iter().filter(|sc| sc.dark == dark).collect();
                    let w = (ui.available_width() - 20.0) / 3.0;
                    ui.spacing_mut().item_spacing.y = 10.0;
                    for chunk in schemes.chunks(3) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            for sc in chunk {
                                if scheme_card(ui, sc, *value == sc.name, w).clicked() {
                                    *value = sc.name.to_string();
                                }
                            }
                        });
                    }
                });
        },
    );
    if pick != dark {
        *schemes_dark = Some(pick);
    }
}

/// The six accents as round swatches with their names; the chosen one ringed.
fn accent_picker(ui: &mut Ui, accent: &mut crate::settings::Accent) {
    use crate::settings::Accent;
    let p = style::pal(ui);
    let dark = p.dark;
    // Laid out right to left: draw the last first.
    for a in Accent::ALL.iter().rev() {
        let color = accent_swatch(*a, dark);
        let name = match a {
            Accent::Blue => t("Blue"),
            Accent::Violet => t("Violet"),
            Accent::Green => t("Green"),
            Accent::Orange => t("Orange"),
            Accent::Pink => t("Pink"),
            Accent::Graphite => t("Graphite"),
        };
        let on = *accent == *a;
        let g = ui.painter().layout_no_wrap(
            name.to_string(),
            if on {
                style::medium(11.5)
            } else {
                style::regular(11.5)
            },
            p.text,
        );
        let w = g.size().x.max(24.0);
        let (r, resp) = ui.allocate_exact_size(vec2(w, 24.0 + 6.0 + 16.0), Sense::click());
        let c = pos2(r.center().x, r.top() + 12.0);
        let painter = ui.painter();
        if on {
            painter.circle_stroke(c, 12.0 + 2.0 + 0.75, Stroke::new(1.5, p.text_muted));
        }
        painter.circle_filled(c, if resp.hovered() && !on { 12.5 } else { 12.0 }, color);
        painter.circle_stroke(c, 11.5, Stroke::new(1.0, Color32::from_white_alpha(38)));
        painter.galley(
            pos2(r.center().x - g.size().x / 2.0, r.bottom() - 16.0),
            g,
            if on { p.text } else { p.text_muted },
        );
        if resp.clicked() {
            *accent = *a;
        }
        ui.add_space(6.0);
    }
}

/// The swatch colour of an accent (its dark-theme fill, or light).
fn accent_swatch(a: crate::settings::Accent, dark: bool) -> Color32 {
    use crate::settings::Accent;
    let hex = match (a, dark) {
        (Accent::Blue, true) => 0x3a7be0,
        (Accent::Blue, false) => 0x1f5fbf,
        (Accent::Violet, true) => 0x7a62e6,
        (Accent::Violet, false) => 0x6246d6,
        (Accent::Green, true) => 0x2f9e66,
        (Accent::Green, false) => 0x1c7f4c,
        (Accent::Orange, true) => 0xd9702b,
        (Accent::Orange, false) => 0xc25a14,
        (Accent::Pink, true) => 0xcf4f91,
        (Accent::Pink, false) => 0xb83a7c,
        (Accent::Graphite, true) => 0x6b6b6b,
        (Accent::Graphite, false) => 0x2b2b2b,
    };
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A theme as a card: a miniature window (sidebar lines, a canvas with lines) and its name.
fn theme_card(
    ui: &mut Ui,
    mode: ThemeMode,
    icon: &str,
    label: &str,
    on: bool,
    w: f32,
) -> egui::Response {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 64.0 + 34.0), Sense::click());
    let painter = ui.painter();
    let prev = egui::Rect::from_min_size(rect.min, vec2(w, 64.0));
    let half = |painter: &egui::Painter, r: egui::Rect, dark: bool| {
        let q = style::palette(dark);
        let (bg, bar, canvas, ring, line) = if dark {
            (
                q.chrome,
                Color32::from_rgb(0x2a, 0x2a, 0x2a),
                q.canvas,
                Color32::from_rgb(0x26, 0x26, 0x26),
                Color32::from_rgb(0x2c, 0x2c, 0x2c),
            )
        } else {
            (
                q.chrome,
                Color32::from_rgb(0xdc, 0xdc, 0xda),
                q.canvas,
                Color32::from_rgb(0xe2, 0xe2, 0xe0),
                Color32::from_rgb(0xe9, 0xe9, 0xe8),
            )
        };
        painter.rect_filled(r, 0.0, bg);
        // Sidebar: three bars.
        for (i, len) in [0.8, 0.6, 0.7].iter().enumerate() {
            let y = r.top() + 10.0 + 6.0 + i as f32 * 9.0;
            painter.rect_filled(
                egui::Rect::from_min_size(pos2(r.left() + 10.0, y), vec2(34.0 * len, 4.0)),
                CornerRadius::same(2),
                bar,
            );
        }
        let card = egui::Rect::from_min_max(
            pos2(r.left() + 10.0 + 34.0 + 6.0, r.top() + 10.0),
            pos2(r.right() + 6.0, r.bottom() + 6.0),
        );
        painter.rect(
            card,
            CornerRadius {
                nw: 6,
                ne: 0,
                sw: 0,
                se: 0,
            },
            canvas,
            Stroke::new(1.0, ring),
            StrokeKind::Outside,
        );
        let inner_w = card.width() - 16.0 - 6.0;
        for (i, (len, acc)) in [(0.7, false), (0.5, false), (0.4, true)].iter().enumerate() {
            let y = card.top() + 8.0 + i as f32 * 9.0;
            painter.rect_filled(
                egui::Rect::from_min_size(pos2(card.left() + 8.0, y), vec2(inner_w * len, 4.0)),
                CornerRadius::same(2),
                if *acc { q.accent } else { line },
            );
        }
    };
    let clip = painter.with_clip_rect(prev);
    match mode {
        ThemeMode::System => {
            let l = egui::Rect::from_min_max(prev.min, pos2(prev.center().x, prev.bottom()));
            let r = egui::Rect::from_min_max(pos2(prev.center().x, prev.top()), prev.max);
            half(&clip.with_clip_rect(l), l, false);
            half(&clip.with_clip_rect(r), r, true);
        }
        ThemeMode::Light => half(&clip, prev, false),
        ThemeMode::Dark => half(&clip, prev, true),
    }
    let label_r = egui::Rect::from_min_max(pos2(rect.left(), prev.bottom()), rect.max);
    painter.rect_filled(
        label_r,
        CornerRadius {
            nw: 0,
            ne: 0,
            sw: 10,
            se: 10,
        },
        p.surface,
    );
    painter.hline(
        label_r.x_range(),
        label_r.top() + 0.5,
        Stroke::new(1.0, p.border),
    );
    painter.text(
        pos2(label_r.left() + 12.0 + 8.0, label_r.center().y),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(16.0),
        p.text_muted,
    );
    painter.text(
        pos2(label_r.left() + 12.0 + 16.0 + 8.0, label_r.center().y),
        Align2::LEFT_CENTER,
        label,
        style::medium(style::BODY),
        p.text,
    );
    if on {
        painter.text(
            pos2(label_r.right() - 12.0 - 8.0, label_r.center().y),
            Align2::CENTER_CENTER,
            icons::CHECK_CIRCLE,
            style::icon_font(16.0),
            p.accent_text,
        );
    }
    kit::round_corners(painter, rect, 10.0, card_bg(&p));
    painter.rect_stroke(
        rect,
        CornerRadius::same(10),
        if on {
            Stroke::new(2.0, p.accent)
        } else {
            Stroke::new(1.0, p.border_strong)
        },
        StrokeKind::Outside,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A terminal color scheme as a card: a prompt, a listing and a log line in its colors, a strip
/// of its six colors, and its name.
fn scheme_card(ui: &mut Ui, sc: &Scheme, on: bool, w: f32) -> egui::Response {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 52.0 + 4.0 + 30.0), Sense::click());
    let painter = ui.painter();
    let sample = egui::Rect::from_min_size(rect.min, vec2(w, 52.0));
    painter.rect_filled(
        sample,
        CornerRadius {
            nw: 9,
            ne: 9,
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
            pos2(sample.left() + 10.0, sample.top() + 6.0 + i as f32 * 14.0),
            gal,
            sc.foreground,
        );
    }
    let strip = egui::Rect::from_min_size(pos2(rect.left(), sample.bottom()), vec2(w, 4.0));
    let sw = w / 6.0;
    for (i, col) in [r, g, y, b, m, c].iter().enumerate() {
        painter.rect_filled(
            egui::Rect::from_min_size(
                pos2(strip.left() + i as f32 * sw, strip.top()),
                vec2(sw + 0.5, 4.0),
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
            sw: 9,
            se: 9,
        },
        p.surface,
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
        style::medium(style::SMALL),
        p.text,
    );
    if on {
        painter.text(
            pos2(label.right() - 10.0 - 7.5, label.center().y),
            Align2::CENTER_CENTER,
            icons::CHECK_CIRCLE,
            style::icon_font(15.0),
            p.accent_text,
        );
    }
    kit::round_corners(painter, rect, 9.0, card_bg(&p));
    painter.rect_stroke(
        rect,
        CornerRadius::same(9),
        if on {
            Stroke::new(2.0, p.accent)
        } else {
            Stroke::new(1.0, p.border_strong)
        },
        StrokeKind::Outside,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Terminal: a live preview, the text (font, size, CJK), the cursor, behaviour.
fn terminal(ui: &mut Ui, s: &mut Settings, q: Search) {
    let p = style::pal(ui);
    let current = fonts::effective_choice(&s.font);
    let cursor_name = match s.cursor_style {
        CursorStyle::Block => t("Block"),
        CursorStyle::Beam => t("Bar"),
        CursorStyle::Underline => t("Underline"),
    };
    // The preview.
    let scheme = crate::term::scheme::find(
        if p.dark {
            &s.scheme_dark
        } else {
            &s.scheme_light
        },
        p.dark,
    );
    let width = ui.available_width();
    egui::Frame::NONE
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            let (head, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(
                head,
                CornerRadius {
                    nw: 9,
                    ne: 9,
                    sw: 0,
                    se: 0,
                },
                p.hover,
            );
            painter.hline(
                head.x_range(),
                head.bottom() - 0.5,
                Stroke::new(1.0, p.border),
            );
            painter.text(
                pos2(head.left() + 12.0 + 7.0, head.center().y),
                Align2::CENTER_CENTER,
                icons::EYE,
                style::icon_font(14.0),
                p.text_faint,
            );
            painter.text(
                pos2(head.left() + 12.0 + 14.0 + 8.0, head.center().y),
                Align2::LEFT_CENTER,
                t("Preview"),
                style::regular(11.5),
                p.text_faint,
            );
            let info = format!(
                "{} · {} pt · {}{}",
                current.name.trim_end_matches(" (built-in)"),
                s.font_size,
                cursor_name,
                if s.cursor_blink {
                    format!(", {}", t("blinking"))
                } else {
                    String::new()
                }
            );
            painter.text(
                pos2(head.right() - 12.0, head.center().y),
                Align2::RIGHT_CENTER,
                info,
                style::regular(11.5),
                p.text_faint,
            );
            let (body, _) = ui.allocate_exact_size(vec2(width, 104.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(
                body.shrink(0.5),
                CornerRadius {
                    nw: 0,
                    ne: 0,
                    sw: 9,
                    se: 9,
                },
                scheme.background,
            );
            let font = egui::FontId::monospace(s.font_size);
            let [_, _, g, _, b, ..] = scheme.ansi;
            let dim = scheme.ansi[8];
            let fg = scheme.foreground;
            let prompt = |job: &mut egui::text::LayoutJob| {
                for (text, c) in [("deploy@web-01", g), (":", fg), ("/srv/app", b), ("$ ", fg)] {
                    job.append(text, 0.0, egui::TextFormat::simple(font.clone(), c));
                }
            };
            let line_h = (s.font_size * 1.45).round();
            let lines: Vec<egui::text::LayoutJob> = {
                let mut out = Vec::new();
                let mut j = egui::text::LayoutJob::default();
                prompt(&mut j);
                j.append(
                    "tail -n 2 logs/web.log",
                    0.0,
                    egui::TextFormat::simple(font.clone(), fg),
                );
                out.push(j);
                for (time, path, ms) in [
                    ("09:15:05", " GET /api/v1/orders ", " 165ms"),
                    ("09:15:06", " GET /api/v1/cart ", " 42ms"),
                ] {
                    let mut j = egui::text::LayoutJob::default();
                    j.append(time, 0.0, egui::TextFormat::simple(font.clone(), dim));
                    j.append(path, 0.0, egui::TextFormat::simple(font.clone(), fg));
                    j.append("200", 0.0, egui::TextFormat::simple(font.clone(), g));
                    j.append(ms, 0.0, egui::TextFormat::simple(font.clone(), fg));
                    out.push(j);
                }
                let mut j = egui::text::LayoutJob::default();
                prompt(&mut j);
                out.push(j);
                out
            };
            let clip = painter.with_clip_rect(body.shrink(1.0));
            let mut y = body.top() + 10.0;
            let mut end = pos2(body.left() + 14.0, y);
            for job in lines {
                let gal = clip.layout_job(job);
                end = pos2(body.left() + 14.0 + gal.size().x, y);
                clip.galley(pos2(body.left() + 14.0, y), gal, fg);
                y += line_h;
            }
            // The cursor, in its shape.
            let cw = s.font_size * 0.6;
            let ch = s.font_size * 1.2;
            let cur = egui::Rect::from_min_size(pos2(end.x, end.y), vec2(cw, ch));
            let shape = match s.cursor_style {
                CursorStyle::Block => cur,
                CursorStyle::Beam => egui::Rect::from_min_size(cur.min, vec2(2.0, ch)),
                CursorStyle::Underline => {
                    egui::Rect::from_min_max(pos2(cur.left(), cur.bottom() - 2.0), cur.max)
                }
            };
            clip.rect_filled(shape, 0.0, scheme.cursor);
        });
    ui.add_space(18.0);
    // Text.
    let g = t("Text");
    let font_desc = t("Menlo, SF Mono, the built-in Hack, or your own file.");
    let size_desc = format!(
        "{} {} · {} {} · {} {}",
        S::Bigger.label(),
        t("bigger"),
        S::Smaller.label(),
        t("smaller"),
        S::ActualSize.label(),
        t("actual size")
    );
    let cjk_desc = t("Loaded from the system on first use; adds about 20 MB of memory.");
    q.group(
        ui,
        g,
        &[
            (t("Font"), font_desc),
            (t("Text size"), ""),
            (t("Chinese, Japanese and Korean fallback fonts"), cjk_desc),
        ],
        None,
        |ui, rows| {
            q.row(
                ui,
                rows,
                g,
                t("Font"),
                Some(small(font_desc, p.text_faint)),
                font_desc,
                |ui| {
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
                    egui::ComboBox::from_id_salt("font")
                        .icon(forms::updown_caret)
                        .selected_text(current.name.clone())
                        .width(170.0)
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
                },
            );
            q.row(
                ui,
                rows,
                g,
                t("Text size"),
                Some(small(size_desc.clone(), p.text_faint)),
                "",
                |ui| {
                    size_slider(ui, &mut s.font_size);
                },
            );
            q.row(
                ui,
                rows,
                g,
                t("Chinese, Japanese and Korean fallback fonts"),
                Some(small(cjk_desc, p.text_faint)),
                cjk_desc,
                |ui| {
                    kit::switch(ui, &mut s.load_cjk_font);
                },
            );
        },
    );
    // Cursor.
    let g = t("Cursor");
    q.group(
        ui,
        g,
        &[(t("Shape"), ""), (t("Blink"), "")],
        None,
        |ui, rows| {
            q.row(ui, rows, g, t("Shape"), None, "", |ui| {
                for (style_, label) in [
                    (CursorStyle::Underline, t("Underline")),
                    (CursorStyle::Beam, t("Bar")),
                    (CursorStyle::Block, t("Block")),
                ] {
                    if cursor_card(ui, style_, label, s.cursor_style == style_).clicked() {
                        s.cursor_style = style_;
                    }
                }
            });
            q.row(ui, rows, g, t("Blink"), None, "", |ui| {
                kit::switch(ui, &mut s.cursor_blink);
            });
        },
    );
    // Behaviour.
    let g = t("Behavior");
    let mb = s.scrollback as f64 * 120.0 * 24.0 / 1_000_000.0;
    let scroll_desc = tf(
        "About {} MB per tab at 120 columns. Lower it if you keep many busy tabs open.",
        &[&format!("{mb:.0}")],
    );
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
    q.group(
        ui,
        g,
        &[
            (
                t("Scrollback"),
                t("Lower it if you keep many busy tabs open."),
            ),
            (meta, meta_desc),
            (t("Copy text when selected"), ""),
        ],
        None,
        |ui, rows| {
            q.row(
                ui,
                rows,
                g,
                t("Scrollback"),
                Some(small(scroll_desc.clone(), p.text_faint)),
                t("Lower it if you keep many busy tabs open."),
                |ui| {
                    let mut v = s.scrollback as u64;
                    number_field(
                        ui,
                        "settings-scrollback",
                        &mut v,
                        0..=Settings::MAX_SCROLLBACK as u64,
                        t("lines"),
                        130.0,
                        true,
                    );
                    s.scrollback = v as usize;
                },
            );
            q.row(
                ui,
                rows,
                g,
                meta,
                Some(small(meta_desc, p.text_faint)),
                meta_desc,
                |ui| {
                    kit::switch(ui, &mut s.option_as_meta);
                },
            );
            q.row(ui, rows, g, t("Copy text when selected"), None, "", |ui| {
                kit::switch(ui, &mut s.copy_on_select);
            });
        },
    );
}

/// A cursor shape as a small card: "ls" and the cursor drawn in that shape.
fn cursor_card(ui: &mut Ui, shape: CursorStyle, label: &str, on: bool) -> egui::Response {
    let p = style::pal(ui);
    let (r, resp) = ui.allocate_exact_size(vec2(76.0, 56.0), Sense::click());
    let painter = ui.painter();
    if !on && resp.hovered() {
        painter.rect_filled(r, CornerRadius::same(8), p.hover);
    }
    forms::choice_outline(painter, r, 8, on, &p);
    let g = painter.layout_no_wrap("ls".into(), style::mono(14.0), p.text);
    let gw = g.size().x;
    let total = gw + 1.0 + 9.0;
    let x = r.center().x - total / 2.0;
    let gy = r.top() + 9.0 + 9.0;
    let label_y = r.top() + 9.0 + 18.0 + 6.0 + 8.0;
    painter.galley(pos2(x, gy - g.size().y / 2.0), g, p.text);
    let cx = x + gw + 1.0;
    let cell = egui::Rect::from_min_size(pos2(cx, gy - 8.0), vec2(9.0, 16.0));
    let cur = match shape {
        CursorStyle::Block => cell,
        CursorStyle::Beam => egui::Rect::from_min_size(cell.min, vec2(2.0, 16.0)),
        CursorStyle::Underline => {
            egui::Rect::from_min_max(pos2(cell.left(), cell.bottom() - 2.0), cell.max)
        }
    };
    painter.rect_filled(cur, 0.0, p.text);
    painter.text(
        pos2(r.center().x, label_y),
        Align2::CENTER_CENTER,
        label,
        if on {
            style::medium(11.5)
        } else {
            style::regular(11.5)
        },
        if on { p.text } else { p.text_muted },
    );
    resp
}

/// The text size as a slider (8–36) between a small and a big "A", with the number.
fn size_slider(ui: &mut Ui, size: &mut f32) {
    let p = style::pal(ui);
    let (r, _) = ui.allocate_exact_size(vec2(300.0, 24.0), Sense::hover());
    let painter = ui.painter();
    painter.text(
        pos2(r.right(), r.center().y),
        Align2::RIGHT_CENTER,
        format!("{}", *size),
        style::mono(style::SMALL),
        p.text,
    );
    let big_x = r.right() - 36.0 - 10.0;
    painter.text(
        pos2(big_x, r.center().y),
        Align2::RIGHT_CENTER,
        "A",
        style::regular(15.0),
        p.text_faint,
    );
    let small_x = r.left();
    painter.text(
        pos2(small_x, r.center().y),
        Align2::LEFT_CENTER,
        "A",
        style::regular(11.0),
        p.text_faint,
    );
    let track = egui::Rect::from_min_max(
        pos2(small_x + 8.0 + 10.0, r.center().y - 2.0),
        pos2(big_x - 10.0 - 10.0, r.center().y + 2.0),
    );
    let resp = ui.interact(
        track.expand2(vec2(8.0, 8.0)),
        egui::Id::new("settings-size"),
        Sense::click_and_drag(),
    );
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let f = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
        *size = (Settings::MIN_FONT + f * (Settings::MAX_FONT - Settings::MIN_FONT)).round();
    }
    let f =
        ((*size - Settings::MIN_FONT) / (Settings::MAX_FONT - Settings::MIN_FONT)).clamp(0.0, 1.0);
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), p.border_strong);
    let fill = egui::Rect::from_min_max(
        track.min,
        pos2(track.left() + track.width() * f, track.bottom()),
    );
    painter.rect_filled(fill, CornerRadius::same(2), p.accent);
    let knob = pos2(track.left() + track.width() * f, track.center().y);
    painter.circle_filled(knob + vec2(0.0, 1.0), 8.0, Color32::from_black_alpha(70));
    painter.circle_filled(knob, 8.0, Color32::WHITE);
    painter.circle_stroke(knob, 8.0, Stroke::new(1.0, Color32::from_black_alpha(30)));
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

/// Quick commands: a table (drag handle, name, command, Run / Paste, key, remove); the row being
/// edited is tinted and shows its fields, the others read as text.
fn quick_commands(ui: &mut Ui, s: &mut Settings) {
    let p = style::pal(ui);
    let mut remove = None;
    let mut moved: Option<(usize, usize)> = None;
    let mut add = false;
    let width = ui.available_width();
    let focused = ui.memory(|m| m.focused());
    let mod_key = if cfg!(target_os = "macos") {
        "⌃"
    } else {
        "Ctrl+"
    };
    egui::Frame::NONE
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(10.0, 0.0);
            let cols = |r: egui::Rect| {
                let x0 = r.left() + 8.0;
                let grip = egui::Rect::from_min_size(pos2(x0, r.top()), vec2(18.0, r.height()));
                let name = egui::Rect::from_min_size(
                    pos2(grip.right() + 10.0, r.top()),
                    vec2(170.0, r.height()),
                );
                let right = r.right() - 10.0;
                let del =
                    egui::Rect::from_min_size(pos2(right - 24.0, r.top()), vec2(24.0, r.height()));
                let key = egui::Rect::from_min_size(
                    pos2(del.left() - 10.0 - 46.0, r.top()),
                    vec2(46.0, r.height()),
                );
                let mode = egui::Rect::from_min_size(
                    pos2(key.left() - 10.0 - 112.0, r.top()),
                    vec2(112.0, r.height()),
                );
                let cmd = egui::Rect::from_min_max(
                    pos2(name.right() + 10.0, r.top()),
                    pos2(mode.left() - 10.0, r.bottom()),
                );
                (grip, name, cmd, mode, key, del)
            };
            let (head, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(
                head,
                CornerRadius {
                    nw: 9,
                    ne: 9,
                    sw: 0,
                    se: 0,
                },
                p.hover,
            );
            let (_, n, c, m, k, _) = cols(head);
            for (r, text) in [
                (n, t("Name")),
                (c, t("Command")),
                (m, t("When chosen")),
                (k, t("Key")),
            ] {
                painter.text(
                    pos2(r.left(), head.center().y),
                    Align2::LEFT_CENTER,
                    text,
                    style::medium(style::XS),
                    p.text_faint,
                );
            }
            for (i, qc) in s.quick_commands.iter_mut().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(width, 44.0), Sense::hover());
                let name_id = egui::Id::new(("qc-name", qc.id));
                let cmd_id = egui::Id::new(("qc-cmd", qc.id));
                let editing = focused == Some(name_id) || focused == Some(cmd_id);
                let painter = ui.painter();
                if editing {
                    painter.rect_filled(r, 0.0, p.accent_softer);
                }
                painter.hline(r.x_range(), r.top() + 0.5, Stroke::new(1.0, p.line_soft));
                let (grip, name_r, cmd_r, mode_r, key_r, del_r) = cols(r);
                let handle = ui.interact(grip, egui::Id::new(("qc-grip", qc.id)), Sense::drag());
                ui.painter().text(
                    grip.center(),
                    Align2::CENTER_CENTER,
                    icons::DOTS_SIX_VERTICAL,
                    style::icon_font(14.0),
                    if handle.hovered() {
                        p.text_muted
                    } else {
                        p.text_ghost
                    },
                );
                if handle.hovered() || handle.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                }
                handle.dnd_set_drag_payload(i);
                if let Some(from) = ui
                    .interact(r, egui::Id::new(("qc-drop", i)), Sense::hover())
                    .dnd_release_payload::<usize>()
                {
                    moved = Some((*from, i));
                }
                quiet_field(ui, name_r, &mut qc.name, name_id, t("Name"), false, editing);
                quiet_field(ui, cmd_r, &mut qc.command, cmd_id, "df -h", true, editing);
                run_paste(ui, mode_r, &mut qc.run, qc.id);
                if i < 9 {
                    let painter = ui.painter();
                    let label = format!("{mod_key}{}", i + 1);
                    let g =
                        painter.layout_no_wrap(label, style::medium(style::MICRO), p.text_faint);
                    let kr = egui::Rect::from_min_size(
                        pos2(key_r.left(), key_r.center().y - 9.0),
                        vec2((g.size().x + 10.0).max(18.0), 18.0),
                    );
                    painter.rect_stroke(
                        kr,
                        CornerRadius::same(4),
                        Stroke::new(1.0, p.border),
                        StrokeKind::Inside,
                    );
                    painter.galley(kr.center() - g.size() / 2.0, g, p.text_faint);
                }
                let del = egui::Rect::from_center_size(del_r.center(), vec2(20.0, 20.0));
                let dr = ui.interact(del, egui::Id::new(("qc-del", qc.id)), Sense::click());
                kit::paint_icon_button(
                    ui,
                    &dr,
                    del,
                    icons::MINUS_CIRCLE,
                    t("Remove"),
                    false,
                    false,
                );
                if dr.clicked() {
                    remove = Some(i);
                }
            }
            let (foot, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
            ui.painter().hline(
                foot.x_range(),
                foot.top() + 0.5,
                Stroke::new(1.0, p.line_soft),
            );
            let mut f = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(foot.shrink2(vec2(8.0, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if kit::button_ex(
                &mut f,
                kit::Kind::Ghost,
                Some(icons::PLUS),
                t("Add Command"),
                true,
            )
            .clicked()
            {
                add = true;
            }
            f.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut job = egui::text::LayoutJob::default();
                let fmt = egui::TextFormat::simple(style::regular(style::SMALL), p.text_faint);
                job.append(t("Drag"), 0.0, fmt.clone());
                job.append(
                    &format!(" {} ", icons::DOTS_SIX_VERTICAL),
                    0.0,
                    egui::TextFormat::simple(style::icon_font(12.0), p.text_faint),
                );
                job.append(
                    &tf(
                        "to reorder · the first nine get {}",
                        &[if cfg!(target_os = "macos") {
                            "⌃1–⌃9"
                        } else {
                            "Ctrl+1–Ctrl+9"
                        }],
                    ),
                    0.0,
                    fmt,
                );
                ui.label(job);
            });
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
    if add {
        s.quick_commands.push(QuickCommand::new("", ""));
    }
    ui.add_space(18.0);
    kit::callout_rich(ui, icons::INFO, Tone::Neutral, |ui| {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            t("Paste"),
            0.0,
            egui::TextFormat::simple(style::semibold(style::SMALL), p.text),
        );
        job.append(
            &format!(" {}", t("= the command is pasted, so you can edit it before pressing Enter. Use it for anything that changes the server.")),
            0.0,
            egui::TextFormat::simple(style::regular(style::SMALL), p.text_muted),
        );
        ui.add(egui::Label::new(job).wrap());
    });
}

/// A field of the quick commands table: plain text until the row is edited.
fn quiet_field(
    ui: &mut Ui,
    r: egui::Rect,
    text: &mut String,
    id: egui::Id,
    hint: &str,
    mono: bool,
    editing: bool,
) {
    let p = style::pal(ui);
    let mut cell = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(r)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    cell.spacing_mut().interact_size.y = 28.0;
    let mut field = style::input(text)
        .id(id)
        .desired_width(r.width())
        .margin(egui::Margin::symmetric(10, 6))
        .hint_text(hint);
    if mono {
        field = field.mono();
    }
    if editing {
        cell.add(field);
    } else {
        cell.scope(|ui| {
            let v = ui.visuals_mut();
            v.text_edit_bg_color = Some(Color32::TRANSPARENT);
            for w in [&mut v.widgets.inactive, &mut v.widgets.hovered] {
                w.bg_stroke = Stroke::NONE;
            }
            v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.border);
            ui.add(field);
        });
    }
}

/// "▶ Run | ✎ Paste" for a quick command.
fn run_paste(ui: &mut Ui, r: egui::Rect, run: &mut bool, id: Uuid) {
    let p = style::pal(ui);
    let painter = ui.painter();
    let (font, icon_font) = (style::medium(11.5), style::icon_font(12.0));
    let segs = [
        (true, icons::PLAY, t("Run")),
        (false, icons::PENCIL_SIMPLE_LINE, t("Paste")),
    ];
    let widths: Vec<f32> = segs
        .iter()
        .map(|(_, _, l)| {
            8.0 + 12.0
                + 5.0
                + painter
                    .layout_no_wrap(l.to_string(), font.clone(), p.text)
                    .size()
                    .x
                + 8.0
        })
        .collect();
    let total = widths.iter().sum::<f32>() + 4.0;
    let track = egui::Rect::from_min_size(pos2(r.left(), r.center().y - 13.0), vec2(total, 26.0));
    painter.rect_filled(track, CornerRadius::same(6), p.hover);
    let mut x = track.left() + 2.0;
    for ((value, icon, label), w) in segs.into_iter().zip(widths) {
        let seg = egui::Rect::from_min_size(pos2(x, track.top() + 2.0), vec2(w, 22.0));
        x += w;
        let resp = ui.interact(seg, egui::Id::new(("qc-mode", id, value)), Sense::click());
        let on = *run == value;
        let painter = ui.painter();
        if on {
            painter.rect(
                seg,
                CornerRadius::same(4),
                if p.dark {
                    Color32::from_rgb(0x2e, 0x2e, 0x2e)
                } else {
                    p.raised
                },
                Stroke::new(1.0, p.border),
                StrokeKind::Inside,
            );
        } else if resp.hovered() {
            painter.rect_filled(seg, CornerRadius::same(4), p.hover);
        }
        let color = if on { p.text } else { p.text_faint };
        let icon_color = if on && value { p.ok } else { color };
        painter.text(
            pos2(seg.left() + 8.0 + 6.0, seg.center().y),
            Align2::CENTER_CENTER,
            icon,
            icon_font.clone(),
            icon_color,
        );
        painter.text(
            pos2(seg.left() + 8.0 + 12.0 + 5.0, seg.center().y),
            Align2::LEFT_CENTER,
            label,
            font.clone(),
            color,
        );
        if resp.clicked() {
            *run = value;
        }
    }
}

/// A shortcut as written on the other platform.
fn shortcut_label(sc: S, mac: bool) -> String {
    if mac == cfg!(target_os = "macos") {
        return sc.label();
    }
    let (m, w) = match sc {
        S::QuickConnect => ("⌘K", "Ctrl+Shift+K"),
        S::NewConnection => ("⌘N", "Ctrl+Shift+N"),
        S::NewTab => ("⌘T", "Ctrl+Shift+T"),
        S::Close => ("⌘W", "Ctrl+Shift+W"),
        S::Reconnect => ("⌘R", "Ctrl+Shift+R"),
        S::Export => ("⇧⌘E", "Ctrl+Shift+E"),
        S::Copy => ("⌘C", "Ctrl+Shift+C"),
        S::Paste => ("⌘V", "Ctrl+Shift+V"),
        S::ClearScrollback => ("⌥⌘K", "Ctrl+Shift+L"),
        S::Sidebar => ("⌘B", "Ctrl+Shift+B"),
        S::Sftp => ("⇧⌘F", "Ctrl+Shift+F"),
        S::SplitRight => ("⌘D", "Ctrl+Shift+D"),
        S::SplitDown => ("⇧⌘D", "Ctrl+Alt+Shift+D"),
        S::NextPane => ("⌘]", "Ctrl+Shift+]"),
        S::PrevPane => ("⌘[", "Ctrl+Shift+["),
        S::NextTab => ("⇧⌘]", "Ctrl+Tab"),
        S::PrevTab => ("⇧⌘[", "Ctrl+Shift+Tab"),
        S::Bigger => ("⌘=", "Ctrl+="),
        S::Smaller => ("⌘-", "Ctrl+-"),
        S::ActualSize => ("⌘0", "Ctrl+0"),
        S::Settings => ("⌘,", "Ctrl+,"),
        S::Quit => ("⌘Q", "Ctrl+Shift+Q"),
        S::FullScreen => ("⌃⌘F", "F11"),
        S::Save => ("⌘S", "Ctrl+S"),
        S::SaveAndConnect => ("⌘↩", "Ctrl+Enter"),
    };
    if mac { m } else { w }.to_string()
}

/// A shortcut split into key caps: `⇧⌘F` → ⇧ ⌘ F, `Ctrl+Shift+F` → Ctrl Shift F.
fn key_caps(label: &str) -> Vec<String> {
    if label.contains('+') && label.len() > 1 {
        return label.split('+').map(str::to_string).collect();
    }
    let mut caps = Vec::new();
    let mut rest = String::new();
    for c in label.chars() {
        if "⌘⇧⌥⌃".contains(c) && rest.is_empty() {
            caps.push(c.to_string());
        } else {
            rest.push(c);
        }
    }
    if !rest.is_empty() {
        caps.push(rest);
    }
    caps
}

/// Shortcuts: every command and its keys in two columns of cards, searchable, for macOS or for
/// Windows and Linux.
fn shortcuts(ui: &mut Ui, filter: &mut String, mac: &mut Option<bool>) {
    let p = style::pal(ui);
    let w = ui.available_width();
    let mut on_mac = mac.unwrap_or(cfg!(target_os = "macos"));
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let seg_w = 260.0;
        kit::search_field(
            ui,
            egui::Id::new("shortcut-filter"),
            filter,
            t("Search shortcuts"),
            w - seg_w - 8.0,
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            kit::segmented_icons(
                ui,
                &mut on_mac,
                &[
                    (true, Some(icons::APPLE_LOGO), "macOS"),
                    (false, Some(icons::WINDOWS_LOGO), t("Windows & Linux")),
                ],
                false,
            );
        });
    });
    *mac = Some(on_mac);
    ui.add_space(16.0);
    let k = |sc: S| shortcut_label(sc, on_mac);
    let tab_keys = if on_mac { "⌘ 1…9" } else { "Alt+1…9" };
    let qc_keys = if on_mac { "⌃ 1…9" } else { "Ctrl+1…9" };
    type Rows<'a> = Vec<(&'a str, Vec<String>)>;
    let left: Vec<(&str, Rows)> = vec![
        (
            t("Connections"),
            vec![
                (t("Quick connect / search"), vec![k(S::QuickConnect)]),
                (t("New connection"), vec![k(S::NewConnection)]),
                (t("Reconnect"), vec![k(S::Reconnect)]),
                (t("Export connections"), vec![k(S::Export)]),
            ],
        ),
        (
            t("Tabs & panes"),
            vec![
                (t("New tab (same server)"), vec![k(S::NewTab)]),
                (t("Close pane or tab"), vec![k(S::Close)]),
                (t("Switch to tab 1–9"), vec![tab_keys.to_string()]),
                (t("Previous / next tab"), vec![k(S::PrevTab), k(S::NextTab)]),
                (
                    t("Split right / down"),
                    vec![k(S::SplitRight), k(S::SplitDown)],
                ),
                (
                    t("Previous / next pane"),
                    vec![k(S::PrevPane), k(S::NextPane)],
                ),
            ],
        ),
    ];
    let right: Vec<(&str, Rows)> = vec![
        (
            t("Terminal"),
            vec![
                (t("Copy / paste"), vec![k(S::Copy), k(S::Paste)]),
                (t("Clear scrollback"), vec![k(S::ClearScrollback)]),
                (t("Scroll history"), vec!["⇧ PgUp".into(), "⇧ PgDn".into()]),
                (
                    t("Text size"),
                    vec![k(S::Bigger), k(S::Smaller), k(S::ActualSize)],
                ),
                (t("Quick command 1–9"), vec![qc_keys.to_string()]),
            ],
        ),
        (
            t("Files & view"),
            vec![
                (t("Files panel"), vec![k(S::Sftp)]),
                (t("Save (file editor)"), vec![k(S::Save)]),
                (t("Sidebar"), vec![k(S::Sidebar)]),
                (t("Full screen"), vec![k(S::FullScreen)]),
                (t("Settings"), vec![k(S::Settings)]),
            ],
        ),
    ];
    let q = filter.trim().to_lowercase();
    let col_w = (w - 18.0) / 2.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 18.0;
        for column in [left, right] {
            ui.allocate_ui_with_layout(
                vec2(col_w, 10.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(col_w);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (title, rows) in column {
                        let rows: Vec<_> = rows
                            .into_iter()
                            .filter(|(l, _)| {
                                q.is_empty()
                                    || l.to_lowercase().contains(&q)
                                    || title.to_lowercase().contains(&q)
                            })
                            .collect();
                        if rows.is_empty() {
                            continue;
                        }
                        forms::group(ui, title, None, |ui, _| {
                            let n = rows.len();
                            for (i, (label, keys)) in rows.into_iter().enumerate() {
                                let (r, _) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 34.0),
                                    Sense::hover(),
                                );
                                let painter = ui.painter();
                                if i > 0 {
                                    painter.hline(
                                        r.x_range(),
                                        r.top() + 0.5,
                                        Stroke::new(1.0, p.line_soft),
                                    );
                                }
                                painter.text(
                                    pos2(r.left() + 12.0, r.center().y),
                                    Align2::LEFT_CENTER,
                                    label,
                                    style::regular(12.5),
                                    p.text,
                                );
                                let mut x = r.right() - 12.0;
                                for (j, combo) in keys.iter().enumerate().rev() {
                                    for cap in key_caps(combo).iter().rev() {
                                        let cw = (painter
                                            .layout_no_wrap(
                                                cap.clone(),
                                                style::medium(style::MICRO),
                                                p.text_muted,
                                            )
                                            .size()
                                            .x
                                            + 10.0)
                                            .max(18.0);
                                        x -= cw;
                                        kit::paint_kbd_at(painter, pos2(x, r.center().y), cap, &p);
                                        x -= 3.0;
                                    }
                                    if j > 0 {
                                        x -= 4.0;
                                    }
                                }
                                let _ = n;
                            }
                        });
                        ui.add_space(16.0);
                    }
                },
            );
        }
    });
}

/// What a settings card looks like behind its rounded children (the dialog under the card's
/// faint wash).
fn card_bg(p: &style::Palette) -> Color32 {
    if p.dark {
        p.dialog.lerp_to_gamma(Color32::WHITE, 0.022)
    } else {
        p.surface
    }
}
