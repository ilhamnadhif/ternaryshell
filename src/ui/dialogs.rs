//! Dialogs: the command palette, confirmations, group names, settings, export, the import
//! preview, and toasts. All share the dialog frame, header and footer of `kit`.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Key, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use uuid::Uuid;

use super::actions::Action;
use super::keys::Shortcut as S;
use super::style::{self, Tone};
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
}

#[derive(Default)]
pub struct Palette {
    query: String,
    index: usize,
    focused: bool,
}

pub enum PaletteResult {
    Open,
    Close,
    Connect(Uuid),
    /// An address typed in (`user@host[:port]`) that matches no bookmark.
    ConnectTo(String),
    /// Open the connection editor for the typed address.
    SaveAs(String),
    Run(Action),
}

/// Most connections listed in the palette.
const PALETTE_ROWS: usize = 50;

/// One row of the palette, in the order shown.
enum Row {
    Connection(Uuid),
    ConnectTo(String),
    SaveAs(String),
    Command(usize),
}

impl Palette {
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        store: &BookmarkStore,
        recent: &[Uuid],
        live: &HashSet<Uuid>,
        commands: &[Command],
    ) -> PaletteResult {
        let mut result = PaletteResult::Open;
        let query = self.query.trim().to_string();
        // What to list: recent connections and the first commands with no query; matches of
        // both, and the typed address, with one.
        let mut sections: Vec<(&str, Vec<Row>)> = Vec::new();
        if query.is_empty() {
            let rec: Vec<Row> = recent
                .iter()
                .filter(|id| store.bookmark(**id).is_some())
                .take(5)
                .map(|id| Row::Connection(*id))
                .collect();
            if !rec.is_empty() {
                sections.push((t("Recent"), rec));
            } else if !store.bookmarks.is_empty() {
                sections.push((
                    t("Connections"),
                    store
                        .bookmarks
                        .iter()
                        .take(5)
                        .map(|b| Row::Connection(b.id))
                        .collect(),
                ));
            }
            sections.push((
                t("Commands"),
                (0..commands.len().min(5)).map(Row::Command).collect(),
            ));
        } else {
            let mut hits = search::search(store, &query);
            hits.truncate(PALETTE_ROWS);
            let address = (hits.is_empty() && query.contains(['@', '.', ':']))
                .then(|| crate::bookmarks::model::parse_target(&query).ok())
                .flatten()
                .map(|_| query.clone());
            if let Some(a) = &address {
                sections.push(("", vec![Row::ConnectTo(a.clone()), Row::SaveAs(a.clone())]));
            }
            if !hits.is_empty() {
                sections.push((
                    t("Connections"),
                    hits.into_iter().map(Row::Connection).collect(),
                ));
            }
            let mut cmds: Vec<(i32, usize)> = commands
                .iter()
                .enumerate()
                .filter_map(|(i, c)| search::fuzzy_score(&query, &c.label).map(|s| (s, i)))
                .collect();
            cmds.sort_by_key(|c| std::cmp::Reverse(c.0));
            if !cmds.is_empty() && address.is_none() {
                sections.push((
                    t("Commands"),
                    cmds.into_iter()
                        .take(8)
                        .map(|(_, i)| Row::Command(i))
                        .collect(),
                ));
            }
        }
        let total: usize = sections.iter().map(|s| s.1.len()).sum();
        self.index = self.index.min(total.saturating_sub(1));

        let area = egui::Area::new(egui::Id::new("palette-area"))
            .anchor(Align2::CENTER_TOP, vec2(0.0, 86.0));
        let p = style::palette(ctx.global_style().visuals.dark_mode);
        let modal = egui::Modal::new(egui::Id::new("palette"))
            .area(area)
            .backdrop_color(p.scrim)
            .frame(style::dialog_frame(ctx).fill(p.sheet))
            .show(ctx, |ui| {
                ui.set_width(640.0);
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                // The query: 52 px, a magnifier, the text, esc.
                let (row, _) = ui.allocate_exact_size(vec2(640.0, 52.0), Sense::hover());
                ui.painter().text(
                    pos2(row.left() + 16.0 + 9.5, row.center().y),
                    Align2::CENTER_CENTER,
                    icons::MAGNIFYING_GLASS,
                    style::icon_font(19.0),
                    p.text_faint,
                );
                let esc_w = kit::paint_kbd_at(
                    ui.painter(),
                    pos2(row.right() - 16.0 - 26.0, row.center().y),
                    "esc",
                    &p,
                );
                let _ = esc_w;
                let mut edit_ui =
                    ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
                        pos2(row.left() + 16.0 + 19.0 + 12.0, row.top()),
                        pos2(row.right() - 16.0 - 30.0 - 12.0, row.bottom()),
                    )));
                let r = edit_ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(
                            RichText::new(t("Search connections and commands, or type user@host…"))
                                .color(p.text_faint),
                        )
                        .font(style::regular(15.0))
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 16)))
                        .desired_width(f32::INFINITY),
                );
                ui.painter().hline(
                    row.x_range(),
                    row.bottom() - 0.5,
                    Stroke::new(1.0, p.border),
                );
                if !self.focused {
                    r.request_focus();
                    self.focused = true;
                }
                if r.changed() {
                    self.index = 0;
                }
                let (down, up, enter, cmd_enter) = ui.input_mut(|i| {
                    (
                        i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                        i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                        i.key_pressed(Key::Enter) && !i.modifiers.command,
                        i.consume_key(egui::Modifiers::COMMAND, Key::Enter),
                    )
                });
                if down {
                    self.index = (self.index + 1).min(total.saturating_sub(1));
                }
                if up {
                    self.index = self.index.saturating_sub(1);
                }
                if r.has_focus() {
                    ui.memory_mut(|m| {
                        m.set_focus_lock_filter(
                            r.id,
                            egui::EventFilter {
                                vertical_arrows: true,
                                ..Default::default()
                            },
                        )
                    });
                }
                // The list.
                let mut picked: Option<&Row> = None;
                let mut n = 0;
                egui::ScrollArea::vertical()
                    .max_height(440.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(6))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                if total == 0 {
                                    ui.add_space(14.0);
                                    ui.vertical_centered(|ui| {
                                        ui.label(
                                            RichText::new(if store.bookmarks.is_empty() {
                                                t("No connections yet")
                                            } else {
                                                t("No matches")
                                            })
                                            .color(p.text_muted),
                                        );
                                    });
                                    ui.add_space(14.0);
                                }
                                for (title, rows) in &sections {
                                    if !title.is_empty() {
                                        ui.add_space(4.0);
                                        ui.horizontal(|ui| {
                                            ui.add_space(10.0);
                                            ui.label(style::section_label(title, p.text_faint));
                                        });
                                        ui.add_space(4.0);
                                    }
                                    for row in rows {
                                        let selected = n == self.index;
                                        let resp = palette_row(
                                            ui, row, selected, store, live, commands, &query,
                                        );
                                        if selected && (down || up) {
                                            resp.scroll_to_me(None);
                                        }
                                        if resp.clicked() {
                                            picked = Some(row);
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
                if enter || cmd_enter {
                    let mut flat = sections.iter().flat_map(|s| s.1.iter());
                    picked = picked.or_else(|| flat.nth(self.index));
                    if cmd_enter && let Some(Row::ConnectTo(a)) = picked {
                        result = PaletteResult::SaveAs(a.clone());
                        picked = None;
                    }
                }
                if let Some(row) = picked {
                    result = match row {
                        Row::Connection(id) => PaletteResult::Connect(*id),
                        Row::ConnectTo(a) => PaletteResult::ConnectTo(a.clone()),
                        Row::SaveAs(a) => PaletteResult::SaveAs(a.clone()),
                        Row::Command(i) => PaletteResult::Run(commands[*i].action.clone()),
                    };
                } else if enter && total == 0 {
                    result = PaletteResult::Close;
                }
                // Key hints along the bottom.
                let (foot, _) = ui.allocate_exact_size(vec2(640.0, 38.0), Sense::hover());
                let painter = ui.painter();
                painter.hline(foot.x_range(), foot.top() + 0.5, Stroke::new(1.0, p.border));
                let mut x = foot.left() + 14.0;
                let cy = foot.center().y;
                let hint = |x: &mut f32, keys: &[&str], what: &str| {
                    for k in keys {
                        *x += kit::paint_kbd_at(painter, pos2(*x, cy), k, &p) + 3.0;
                    }
                    *x += 2.0;
                    let g = painter.layout_no_wrap(
                        what.to_string(),
                        style::regular(style::XS),
                        p.text_faint,
                    );
                    let w = g.size().x;
                    painter.galley(pos2(*x, cy - g.size().y / 2.0), g, p.text_faint);
                    *x += w + 14.0;
                };
                hint(&mut x, &["↑", "↓"], t("navigate"));
                hint(
                    &mut x,
                    &["↩"],
                    if query.is_empty() {
                        t("open")
                    } else {
                        t("connect")
                    },
                );
                if matches!(
                    sections.first().and_then(|s| s.1.first()),
                    Some(Row::ConnectTo(_))
                ) {
                    hint(&mut x, &[&S::SaveAndConnect.label()], t("save"));
                }
                let right = if query.is_empty() {
                    String::new()
                } else {
                    let conns: usize = sections
                        .iter()
                        .flat_map(|s| s.1.iter())
                        .filter(|r| matches!(r, Row::Connection(_)))
                        .count();
                    let cmds: usize = sections
                        .iter()
                        .flat_map(|s| s.1.iter())
                        .filter(|r| matches!(r, Row::Command(_)))
                        .count();
                    tf(
                        "{} connections · {} commands",
                        &[&conns.to_string(), &cmds.to_string()],
                    )
                };
                if right.is_empty() {
                    let g = painter.layout_no_wrap(
                        t("close").to_string(),
                        style::regular(style::XS),
                        p.text_faint,
                    );
                    let gw = g.size().x;
                    painter.galley(
                        pos2(foot.right() - 14.0 - gw, cy - g.size().y / 2.0),
                        g,
                        p.text_faint,
                    );
                    let kw = painter
                        .layout_no_wrap("esc".into(), style::regular(style::XS), p.text_muted)
                        .size()
                        .x
                        + 10.0;
                    kit::paint_kbd_at(
                        painter,
                        pos2(foot.right() - 14.0 - gw - 5.0 - kw, cy),
                        "esc",
                        &p,
                    );
                } else {
                    painter.text(
                        pos2(foot.right() - 14.0, cy),
                        Align2::RIGHT_CENTER,
                        right,
                        style::regular(style::XS),
                        p.text_faint,
                    );
                }
            });
        if modal.should_close() && matches!(result, PaletteResult::Open) {
            result = PaletteResult::Close;
        }
        result
    }
}

/// One palette row (40 px): an icon tile (accent when selected), the name, the address in mono,
/// and on the right the group or the shortcut.
fn palette_row(
    ui: &mut Ui,
    row: &Row,
    selected: bool,
    store: &BookmarkStore,
    live: &HashSet<Uuid>,
    commands: &[Command],
    query: &str,
) -> egui::Response {
    let p = style::pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(style::RADIUS_MD), p.accent_soft);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(style::RADIUS_MD), p.hover);
    }
    let (icon, name, address, right, kbd, is_live): (
        &str,
        String,
        String,
        String,
        Option<String>,
        bool,
    ) = match row {
        Row::Connection(id) => {
            let b = store.bookmark(*id);
            (
                icons::HARD_DRIVES,
                b.map(|b| b.display_name()).unwrap_or_default(),
                b.map(|b| b.address()).unwrap_or_default(),
                b.map(|b| store.group_path(b.group)).unwrap_or_default(),
                selected.then(|| "↩".to_string()),
                live.contains(id),
            )
        }
        Row::ConnectTo(a) => (
            icons::ARROW_RIGHT,
            tf("Connect to {}", &[a]),
            String::new(),
            String::new(),
            Some("↩".into()),
            false,
        ),
        Row::SaveAs(_) => (
            icons::PLUS,
            t("Save as New Connection…").to_string(),
            String::new(),
            String::new(),
            Some(S::SaveAndConnect.label()),
            false,
        ),
        Row::Command(i) => {
            let c = &commands[*i];
            (
                c.icon,
                c.label.clone(),
                String::new(),
                String::new(),
                c.shortcut.clone(),
                false,
            )
        }
    };
    let tile = egui::Rect::from_min_size(
        pos2(rect.left() + 10.0, rect.center().y - 13.0),
        vec2(26.0, 26.0),
    );
    painter.rect_filled(
        tile,
        CornerRadius::same(7),
        if selected { p.accent } else { p.hover },
    );
    painter.text(
        tile.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(15.0),
        if selected {
            Color32::WHITE
        } else {
            p.text_muted
        },
    );
    if is_live {
        let c = pos2(tile.right() - 2.0, tile.bottom() - 2.0);
        painter.circle_filled(c, 6.0, p.sheet);
        painter.circle_filled(c, 4.0, p.ok);
    }
    let mut x = tile.right() + 10.0;
    // Right side first.
    let mut right_edge = rect.right() - 10.0;
    if let Some(k) = &kbd {
        let w = (painter
            .layout_no_wrap(k.clone(), style::regular(style::XS), p.text_muted)
            .size()
            .x
            + 10.0)
            .max(20.0);
        kit::paint_kbd_at(painter, pos2(right_edge - w, rect.center().y), k, &p);
        right_edge -= w + 8.0;
    }
    if !right.is_empty() {
        let g = painter.layout_no_wrap(right, style::regular(style::SMALL), p.text_faint);
        let w = g.size().x;
        painter.galley(
            pos2(right_edge - w, rect.center().y - g.size().y / 2.0),
            g,
            p.text_faint,
        );
        right_edge -= w + 12.0;
    }
    let clip = egui::Rect::from_min_max(rect.min, pos2(right_edge, rect.bottom()));
    // The name, matched letters in the accent color (connections and commands).
    let mono_name = matches!(row, Row::ConnectTo(_));
    let hits = if query.is_empty() || mono_name {
        Vec::new()
    } else {
        search::match_positions(query, &name)
    };
    let mut job = egui::text::LayoutJob::default();
    if let Row::ConnectTo(a) = row {
        let base = t("Connect to {}");
        let before = base.split("{}").next().unwrap_or("");
        let after = base.split("{}").nth(1).unwrap_or("");
        let f = |font: egui::FontId| egui::TextFormat {
            font_id: font,
            color: p.text,
            ..Default::default()
        };
        job.append(before, 0.0, f(style::medium(style::BODY)));
        job.append(a, 0.0, f(style::mono(style::BODY)));
        job.append(after, 0.0, f(style::medium(style::BODY)));
    } else {
        for (i, c) in name.chars().enumerate() {
            let hit = hits.contains(&i);
            job.append(
                &c.to_string(),
                0.0,
                egui::TextFormat {
                    font_id: if hit {
                        style::semibold(style::BODY)
                    } else {
                        style::medium(style::BODY)
                    },
                    color: if hit { p.accent_text } else { p.text },
                    ..Default::default()
                },
            );
        }
    }
    let g = painter.layout_job(job);
    let gw = g.size().x;
    painter
        .with_clip_rect(clip)
        .galley(pos2(x, rect.center().y - g.size().y / 2.0), g, p.text);
    x += gw + 10.0;
    if !address.is_empty() {
        painter.with_clip_rect(clip).text(
            pos2(x, rect.center().y),
            Align2::LEFT_CENTER,
            address,
            style::mono(11.5),
            p.text_faint,
        );
    }
    resp
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
        /// What is affected, one line each with its icon (running programs, transfers, edits).
        details: Vec<(&'static str, String)>,
        button: String,
        action: ConfirmAction,
        /// When it first appeared (an Enter typed just before must not confirm it).
        shown_at: Option<Instant>,
        /// "Don't ask again" (ticked or not), for closing a connected tab.
        dont_ask: Option<bool>,
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
}

/// For dialogs holding typed input: only Esc closes them, not a click beside the dialog.
pub fn escape_only<T>(ctx: &egui::Context, modal: &egui::ModalResponse<T>) -> bool {
    ctx.input(|i| i.key_pressed(Key::Escape)) && modal.should_close()
}

pub fn confirm(title: &str, message: String, button: &str, action: ConfirmAction) -> Dialog {
    confirm_details(title, message, Vec::new(), button, action)
}

/// A confirmation listing what is affected.
pub fn confirm_details(
    title: &str,
    message: String,
    details: Vec<(&'static str, String)>,
    button: &str,
    action: ConfirmAction,
) -> Dialog {
    Dialog::Confirm {
        title: title.into(),
        message,
        details,
        button: button.into(),
        action,
        shown_at: None,
        dont_ask: None,
    }
}

/// The question before closing a connected tab, with "Don't ask again".
pub fn confirm_close(
    title: &str,
    message: String,
    details: Vec<(&'static str, String)>,
    button: &str,
    action: ConfirmAction,
) -> Dialog {
    Dialog::Confirm {
        title: title.into(),
        message,
        details,
        button: button.into(),
        action,
        shown_at: None,
        dont_ask: Some(false),
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
            } => {
                let mut result = DialogResult::Open;
                // Enter confirms only when pressed (not auto-repeated, no modifiers) once the
                // dialog has been up for a moment, and only while it is on top.
                let settled = shown_at.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_millis(300);
                let id = egui::Id::new("confirm");
                let on_top = ctx.memory(|m| m.top_modal_layer().is_none_or(|l| l.id == id));
                let enter = settled
                    && on_top
                    && ctx.input(|i| {
                        i.events.iter().any(|e| {
                            matches!(e, egui::Event::Key { key: Key::Enter, pressed: true, repeat: false, modifiers, .. } if modifiers.is_none())
                        })
                    });
                let destructive = matches!(
                    action,
                    ConfirmAction::DeleteBookmark(_) | ConfirmAction::DeleteGroup(_)
                );
                let (icon, tone) = match action {
                    ConfirmAction::DeleteBookmark(_) => (icons::TRASH, Tone::Err),
                    ConfirmAction::DeleteGroup(_) => (icons::FOLDER_SIMPLE, Tone::Err),
                    ConfirmAction::CloseTab(_) => (icons::X, Tone::Neutral),
                    ConfirmAction::Reconnect(_) => (icons::ARROWS_CLOCKWISE, Tone::Neutral),
                    ConfirmAction::RestartForUpdate => (icons::ARROW_CIRCLE_UP, Tone::Neutral),
                    ConfirmAction::Quit => (icons::SIGN_OUT, Tone::Warn),
                };
                let modal = style::modal(ctx, id).show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let p = style::pal(ui);
                    kit::dialog_header(ui, Some((icon, tone)), title, None, false);
                    kit::dialog_body(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if !details.is_empty() {
                            details_card(ui, details);
                            if !message.is_empty() {
                                ui.add_space(10.0);
                            }
                        }
                        if !message.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(message.as_str()).color(p.text_muted),
                                )
                                .wrap(),
                            );
                        }
                        if let Some(ticked) = dont_ask {
                            ui.add_space(12.0);
                            let r = style::checkbox(
                                ui,
                                ticked,
                                RichText::new(t("Don't ask again")).color(p.text),
                            )
                            .on_hover_text(t("You can turn it back on in Settings."));
                            #[cfg(debug_assertions)]
                            crate::devtest::record("confirm-dont-ask", r.rect);
                            if r.changed() {
                                r.surrender_focus();
                            }
                        }
                    });
                    kit::dialog_footer(
                        ui,
                        |_| {},
                        |ui| {
                            let ok = if destructive {
                                kit::danger(ui, button.as_str())
                            } else {
                                kit::primary(ui, button.as_str())
                            };
                            #[cfg(debug_assertions)]
                            crate::devtest::record("confirm-ok", ok.rect);
                            if ok.clicked() || enter {
                                result = DialogResult::Confirmed(action.clone());
                            }
                            let cancel = kit::secondary(ui, None, t("Cancel"));
                            #[cfg(debug_assertions)]
                            crate::devtest::record("confirm-cancel", cancel.rect);
                            if cancel.clicked() {
                                result = DialogResult::Close;
                            }
                        },
                    );
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

/// A card listing what a confirmation affects: one line per item with its icon.
fn details_card(ui: &mut Ui, details: &[(&'static str, String)]) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(style::RADIUS_LG))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            for (icon, text) in details {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    kit::icon(ui, icon, 14.0, p.text_faint);
                    ui.add(
                        egui::Label::new(
                            RichText::new(text).font(style::regular(12.5)).color(p.text),
                        )
                        .wrap(),
                    );
                });
            }
        });
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
            Stroke::new(3.0, p.accent_soft),
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
            Color32::from_rgb(0x1d, 0x1e, 0x24)
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

// ----- toasts ---------------------------------------------------------------------------------

struct Toast {
    title: String,
    detail: Option<String>,
    error: bool,
    action: Option<(String, Action)>,
    at: Instant,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn info(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        #[cfg(debug_assertions)]
        eprintln!("DEVTEST LOG: toast info: {msg}");
        self.push(msg, None, false, None);
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        #[cfg(debug_assertions)]
        eprintln!("DEVTEST LOG: toast error: {msg}");
        self.push(msg, None, true, None);
    }

    /// A toast with a title, a line under it, and an action button.
    pub fn push(
        &mut self,
        title: String,
        detail: Option<String>,
        error: bool,
        action: Option<(String, Action)>,
    ) {
        // A message "Title: detail" or "Title. Detail" splits into its two lines.
        let (title, detail) = match detail {
            Some(d) => (title, Some(d)),
            None => split_message(&title),
        };
        self.items.push(Toast {
            title,
            detail,
            error,
            action,
            at: Instant::now(),
        });
    }

    /// Draws the toasts in the bottom right corner of the canvas; returns an action clicked.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Action> {
        let now = Instant::now();
        self.items.retain(|t| {
            now.duration_since(t.at)
                < Duration::from_secs(if t.error || t.action.is_some() { 9 } else { 5 })
        });
        if self.items.is_empty() {
            return None;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        let mut chosen = None;
        egui::Area::new(egui::Id::new("toasts"))
            // Above the status strip, inside the canvas.
            .anchor(Align2::RIGHT_BOTTOM, vec2(-20.0, -44.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let mut dismiss = None;
                let p = style::pal(ui);
                ui.spacing_mut().item_spacing.y = 8.0;
                for (i, toast) in self.items.iter().enumerate() {
                    egui::Frame::NONE
                        .fill(p.raised)
                        .corner_radius(CornerRadius::same(style::RADIUS_LG))
                        .shadow(ui.visuals().popup_shadow)
                        .stroke(ui.visuals().window_stroke)
                        .inner_margin(egui::Margin {
                            left: 14,
                            right: 12,
                            top: 12,
                            bottom: 12,
                        })
                        .show(ui, |ui| {
                            ui.set_width(360.0 - 26.0);
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                let (icon, color) = if toast.error {
                                    (icons::WARNING_CIRCLE, p.err)
                                } else {
                                    (icons::CHECK_CIRCLE, p.ok)
                                };
                                kit::icon(ui, icon, 18.0, color);
                                let text_w = ui.available_width() - 22.0 - 10.0;
                                ui.allocate_ui_with_layout(
                                    vec2(text_w, 10.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        ui.set_width(text_w);
                                        ui.spacing_mut().item_spacing.y = 2.0;
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&toast.title)
                                                    .font(style::medium(style::BODY))
                                                    .color(p.text),
                                            )
                                            .wrap(),
                                        );
                                        if let Some(d) = &toast.detail {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(d)
                                                        .font(style::regular(style::SMALL))
                                                        .color(p.text_muted),
                                                )
                                                .wrap(),
                                            );
                                        }
                                        if let Some((label, action)) = &toast.action {
                                            ui.add_space(6.0);
                                            if kit::secondary_sm(ui, None, label).clicked() {
                                                chosen = Some(action.clone());
                                                dismiss = Some(i);
                                            }
                                        }
                                    },
                                );
                                if kit::icon_button_ex(ui, icons::X, t("Dismiss"), false, 22.0)
                                    .clicked()
                                {
                                    dismiss = Some(i);
                                }
                            });
                        });
                }
                if let Some(i) = dismiss {
                    self.items.remove(i);
                }
            });
        chosen
    }
}

/// "Uploaded 4 file(s). 1 transfer(s) failed." → title and detail; short messages stay whole.
fn split_message(msg: &str) -> (String, Option<String>) {
    if let Some((a, b)) = msg.split_once(": ")
        && a.len() <= 48
        && !b.is_empty()
    {
        return (a.to_string(), Some(b.to_string()));
    }
    if let Some((a, b)) = msg.split_once(". ")
        && a.len() <= 60
        && !b.trim().is_empty()
    {
        return (format!("{a}."), Some(b.to_string()));
    }
    (msg.to_string(), None)
}
