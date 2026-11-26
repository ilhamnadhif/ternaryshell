//! The pages shown with no tab open: Home (a search field, the recent connections, and the
//! hosts of ~/.ssh/config not imported yet) and, with no connections at all, the first-run page.
//! Redesign 3.1: plain lists and sentences, no cards, greetings or key hints.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use uuid::Uuid;

use super::TernaryShellApp;
use super::chrome::ago;
use crate::bookmarks::ssh_config::SshConfigHost;
use crate::i18n::{t, tf};
use crate::ui::actions::Action;
use crate::ui::sidebar::Live;
use crate::ui::style::{self, Palette};
use crate::ui::{icons, kit};

/// Recent connections shown on Home (and opened with ⌘1–⌘6 there).
const HOME_ROWS: usize = 6;
/// Width of the Home column and of the first-run column.
const HOME_W: f32 = 620.0;
const WELCOME_W: f32 = 460.0;
/// Height of a recent-connection row.
const ROW_H: f32 = 34.0;
/// Width of the name column of a row.
const NAME_W: f32 = 150.0;

/// What Home remembers between frames.
#[derive(Default)]
pub struct HomeState {
    /// Hosts of ~/.ssh/config that are not saved connections, computed for this store size and
    /// list of dismissed hosts (so it is parsed again only when one of them changes).
    ssh_new: Option<((usize, usize), Vec<String>)>,
}

impl TernaryShellApp {
    /// The recent connections Home shows, newest first (saved connections only). With no
    /// history yet, the first saved connections.
    pub(super) fn home_recent(&self) -> Vec<(Uuid, Option<i64>)> {
        let mut out: Vec<(Uuid, Option<i64>)> = self
            .settings
            .recent
            .iter()
            .filter(|r| self.store.bookmark(r.id).is_some())
            .take(HOME_ROWS)
            .map(|r| (r.id, Some(r.at)))
            .collect();
        if out.is_empty() {
            out = self
                .store
                .bookmarks
                .iter()
                .take(HOME_ROWS)
                .map(|b| (b.id, None))
                .collect();
        }
        out
    }

    /// Hosts in ~/.ssh/config that are not saved connections and were not dismissed.
    fn ssh_new_hosts(&mut self) -> Vec<String> {
        let key = (
            self.store.bookmarks.len(),
            self.settings.dismissed_ssh_hosts.len(),
        );
        if let Some((k, list)) = &self.home.ssh_new
            && *k == key
        {
            return list.clone();
        }
        let hosts =
            crate::bookmarks::ssh_config::parse_file(&crate::paths::home_dir().join(".ssh/config"))
                .unwrap_or_default();
        let saved = |h: &SshConfigHost| {
            let host = h.hostname.as_deref().unwrap_or(&h.alias);
            let port = h.port.unwrap_or(crate::bookmarks::DEFAULT_PORT);
            let user = h.user.as_deref().unwrap_or("");
            self.store.bookmarks.iter().any(|b| {
                b.name == h.alias
                    || (b.host.eq_ignore_ascii_case(host)
                        && b.port == port
                        && (user.is_empty() || b.username == user))
            })
        };
        let list: Vec<String> = hosts
            .iter()
            .filter(|h| !h.alias.contains(['*', '?', '!']))
            .filter(|h| !saved(h))
            .map(|h| h.alias.clone())
            .filter(|a| !self.settings.dismissed_ssh_hosts.contains(a))
            .collect();
        self.home.ssh_new = Some((key, list.clone()));
        list
    }

    // ----- no tab open --------------------------------------------------------------------

    /// Home, or the first-run page when there are no connections yet.
    pub(super) fn start_page(&mut self, ui: &mut Ui) {
        if self.store.bookmarks.is_empty() && self.store.groups.is_empty() {
            self.welcome_page(ui);
        } else {
            self.home_page(ui);
        }
    }

    fn home_page(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let area = ui.max_rect();
        let w = HOME_W.min(area.width() - 64.0).max(320.0);
        let x0 = area.center().x - w / 2.0;
        let mut y = area.top() + 120.0;

        // The search field: opens the palette.
        let field = Rect::from_min_size(pos2(x0, y), vec2(w, 40.0));
        if search_field(ui, field, &p).clicked() {
            self.actions.push(Action::QuickConnect);
        }
        y = field.bottom() + 22.0;

        // Recent (or, before anything was opened, the first connections).
        let recent = self.home_recent();
        let has_history = recent.iter().any(|(_, at)| at.is_some());
        let heading = ui.painter().layout_no_wrap(
            if has_history {
                t("Recent")
            } else {
                t("Connections")
            }
            .to_string(),
            style::semibold(style::SMALL),
            p.text_muted,
        );
        ui.painter()
            .galley(pos2(x0 + 10.0, y), heading, p.text_muted);
        y += 17.0 + 6.0;
        let live = self.live_map();
        for (id, at) in recent {
            let Some(b) = self.store.bookmark(id) else {
                continue;
            };
            let row = Rect::from_min_size(pos2(x0, y), vec2(w, ROW_H));
            y += ROW_H;
            let resp = ui.interact(row, egui::Id::new(("home-row", id)), Sense::click());
            let hovered = !self.modal_open() && ui.rect_contains_pointer(row);
            let painter = ui.painter();
            if hovered {
                painter.rect_filled(row, CornerRadius::same(style::RADIUS), p.hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let cy = row.center().y;
            let glyph =
                Rect::from_center_size(pos2(row.left() + 10.0 + 10.0, cy), vec2(20.0, 20.0));
            let status = live.get(&id).map(|l| match l {
                Live::Connected => p.ok,
                Live::Connecting => p.warn,
                Live::Failed => p.err,
            });
            kit::paint_host_glyph(
                painter,
                glyph,
                kit::host_icon(b.host_icon()),
                None,
                status,
                if hovered {
                    blend(p.canvas, p.hover)
                } else {
                    p.canvas
                },
                &p,
            );
            let name_x = glyph.right() + 12.0;
            let clip = |left: f32, right: f32| {
                painter.with_clip_rect(Rect::from_min_max(
                    pos2(left, row.top()),
                    pos2(right, row.bottom()),
                ))
            };
            clip(name_x, name_x + NAME_W).text(
                pos2(name_x, cy),
                Align2::LEFT_CENTER,
                b.display_name(),
                style::regular(style::BODY),
                p.text,
            );
            let right_w = 84.0;
            let addr_x = name_x + NAME_W + 12.0;
            clip(addr_x, row.right() - 10.0 - right_w - 8.0).text(
                pos2(addr_x, cy),
                Align2::LEFT_CENTER,
                super::address_with_port(b),
                style::mono(11.5),
                p.text_faint,
            );
            let right = Rect::from_min_max(
                pos2(row.right() - 10.0 - right_w, row.top()),
                pos2(row.right() - 10.0, row.bottom()),
            );
            let mut files = false;
            if hovered {
                let g = ui.painter().layout_no_wrap(
                    t("Open Files").to_string(),
                    style::regular(style::SMALL),
                    p.accent_text,
                );
                let r = Rect::from_min_size(
                    pos2(right.right() - g.size().x, cy - g.size().y / 2.0),
                    g.size(),
                );
                let link = ui.interact(
                    r.expand(4.0),
                    egui::Id::new(("home-files", id)),
                    Sense::click(),
                );
                ui.painter().galley(r.min, g, p.accent_text);
                files = link.clicked();
            } else {
                let when = match at {
                    Some(at) => ago(at),
                    None => self.store.group_path(b.group),
                };
                ui.painter().text(
                    pos2(right.right(), cy),
                    Align2::RIGHT_CENTER,
                    when,
                    style::regular(style::SMALL),
                    p.text_faint,
                );
            }
            if files {
                self.actions.push(Action::OpenFiles(id));
            } else if resp.clicked() {
                self.actions.push(Action::Connect(id));
            }
        }

        // One sentence about ~/.ssh/config, with Import… and Hide.
        let new = self.ssh_new_hosts();
        if !new.is_empty() {
            y += 22.0;
            let text = if new.len() == 1 {
                tf("{} in ~/.ssh/config isn't in TernaryShell yet.", &[&new[0]])
            } else {
                tf(
                    "{} hosts in ~/.ssh/config aren't in TernaryShell yet.",
                    &[&new.len().to_string()],
                )
            };
            let painter = ui.painter();
            let g = painter.layout_no_wrap(text, style::regular(12.5), p.text_faint);
            let mut x = x0 + 10.0;
            let line_h = g.size().y;
            painter.galley(pos2(x, y), g.clone(), p.text_faint);
            x += g.size().x + 4.0;
            if text_link(ui, pos2(x, y), t("Import…"), p.accent_text, "home-import").clicked() {
                self.actions.push(Action::ImportSshConfig);
            }
            x += ui
                .painter()
                .layout_no_wrap(
                    t("Import…").to_string(),
                    style::regular(12.5),
                    p.accent_text,
                )
                .size()
                .x
                + 12.0;
            if text_link(ui, pos2(x, y), t("Hide"), p.text_faint, "home-hide").clicked() {
                self.settings.dismissed_ssh_hosts.extend(new);
            }
            let _ = line_h;
        }
    }

    fn welcome_page(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let area = ui.max_rect();
        let w = WELCOME_W.min(area.width() - 64.0).max(280.0);
        let x0 = area.center().x - w / 2.0;
        let mut y = area.top() + (area.height() * 0.28).clamp(80.0, 200.0);
        let painter = ui.painter();
        let title = painter.layout_no_wrap(
            t("Add your first server").to_string(),
            style::semibold(20.0),
            p.text,
        );
        painter.galley(pos2(x0, y), title.clone(), p.text);
        y += title.size().y + 10.0;
        let body = painter.layout(
            tf(
                "Connections you add show up in the sidebar. Passwords and keys are kept in {}.",
                &[crate::platform::keychain()],
            ),
            style::regular(style::BODY),
            p.text_muted,
            w,
        );
        let body_h = body.size().y;
        painter.galley(pos2(x0, y), body, p.text_muted);
        y += body_h + 20.0;

        let ssh_hosts = self.ssh_new_hosts().len();
        let row = Rect::from_min_size(pos2(x0, y), vec2(w + 200.0, style::CONTROL_H));
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 14.0;
        if kit::button_ex(
            &mut child,
            kit::Kind::Primary,
            None,
            t("New Connection…"),
            false,
        )
        .clicked()
        {
            self.actions.push(Action::NewBookmark { group: None });
        }
        if ssh_hosts > 0
            && link_widget(
                &mut child,
                &if ssh_hosts == 1 {
                    t("Import ~/.ssh/config (1 host)").to_string()
                } else {
                    tf("Import ~/.ssh/config ({} hosts)", &[&ssh_hosts.to_string()])
                },
                p.accent_text,
            )
            .clicked()
        {
            self.actions.push(Action::ImportSshConfig);
        }
        if link_widget(&mut child, t("Import a File…"), p.accent_text).clicked() {
            self.actions.push(Action::Import);
        }
    }
}

/// The search field of Home: 40 px, a magnifier, the hint with `user@host:port` in mono.
fn search_field(ui: &mut Ui, field: Rect, p: &Palette) -> egui::Response {
    let resp = ui.interact(field, egui::Id::new("home-search"), Sense::click());
    let painter = ui.painter();
    painter.rect(
        field,
        CornerRadius::same(style::RADIUS_MD),
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
        pos2(field.left() + 12.0 + 7.5, field.center().y),
        Align2::CENTER_CENTER,
        icons::MAGNIFYING_GLASS,
        style::icon_font(15.0),
        p.text_faint,
    );
    let mut job = egui::text::LayoutJob::default();
    let fmt = |font: egui::FontId, color| egui::TextFormat {
        font_id: font,
        color,
        valign: egui::Align::Center,
        ..Default::default()
    };
    job.append(
        t("Search or type"),
        0.0,
        fmt(style::regular(14.0), p.text_faint),
    );
    job.append("user@host:port", 5.0, fmt(style::mono(12.0), p.text_faint));
    let g = painter.layout_job(job);
    painter.galley(
        pos2(field.left() + 36.0, field.center().y - g.size().y / 2.0),
        g,
        p.text_faint,
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
    }
    resp
}

/// A text link painted at `pos` (top left), 12.5 px.
fn text_link(ui: &mut Ui, pos: egui::Pos2, text: &str, color: Color32, id: &str) -> egui::Response {
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), style::regular(12.5), color);
    let r = Rect::from_min_size(pos, g.size());
    let resp = ui.interact(r.expand(3.0), egui::Id::new(id), Sense::click());
    let c = if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        style::pal(ui).text
    } else {
        color
    };
    ui.painter().galley(r.min, g, c);
    resp
}

/// A text link laid out in `ui` (13 px).
fn link_widget(ui: &mut Ui, text: &str, color: Color32) -> egui::Response {
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), style::regular(style::BODY), color);
    let (rect, resp) = ui.allocate_exact_size(g.size(), Sense::click());
    let c = if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        style::pal(ui).text
    } else {
        color
    };
    ui.painter().galley(rect.min, g, c);
    resp
}

/// `top` drawn over `base` (translucent hover fills on the canvas colour).
fn blend(base: Color32, top: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    let [tr, tg, tb, _] = top.to_srgba_unmultiplied();
    Color32::from_rgb(mix(base.r(), tr), mix(base.g(), tg), mix(base.b(), tb))
}
