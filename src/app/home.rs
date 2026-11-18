//! The pages shown with no tab open: Home (recent connections as cards, groups, hosts of
//! ~/.ssh/config not imported yet) and, with no connections at all, the first-run welcome.

use std::sync::OnceLock;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use uuid::Uuid;

use super::TernaryShellApp;
use super::chrome::ago;
use crate::bookmarks::ssh_config::SshConfigHost;
use crate::i18n::{t, tf};
use crate::ui::actions::Action;
use crate::ui::keys::Shortcut as S;
use crate::ui::sidebar::{Live, key_width, paint_key};
use crate::ui::style::{self, Palette};
use crate::ui::{icons, kit};

/// Recent connections shown on Home (and opened with ⌘1–⌘6 there).
const HOME_CARDS: usize = 6;

/// A card of the welcome page: icon, title, description, key, whether it is the main one, and
/// what it does (none when there is nothing to do).
type WelcomeCard<'a> = (
    &'a str,
    &'a str,
    String,
    Option<&'a str>,
    bool,
    Option<Action>,
);

/// What Home remembers between frames.
#[derive(Default)]
pub struct HomeState {
    /// Hosts of ~/.ssh/config that are not saved connections, computed for this store size and
    /// list of dismissed hosts (so it is parsed again only when one of them changes).
    ssh_new: Option<((usize, usize), Vec<String>)>,
}

/// The first word of the user's full name ("Ilham"), or the account name.
fn first_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let full = full_name().unwrap_or_default();
        let first = full.split([' ', ',']).find(|w| !w.is_empty()).unwrap_or("");
        let name = if first.is_empty() {
            std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_default()
        } else {
            first.to_string()
        };
        let mut c = name.chars();
        match c.next() {
            Some(f) => f.to_uppercase().chain(c).collect(),
            None => String::new(),
        }
    })
}

#[cfg(unix)]
fn full_name() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage (or null); the fields are
    // NUL-terminated C strings that are copied out before any other call.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_gecos.is_null() {
            return None;
        }
        let gecos = std::ffi::CStr::from_ptr((*pw).pw_gecos)
            .to_string_lossy()
            .into_owned();
        Some(gecos.split(',').next().unwrap_or("").trim().to_string())
    }
}

#[cfg(not(unix))]
fn full_name() -> Option<String> {
    None
}

/// "Good morning, Ilham" for the time of day.
fn greeting() -> String {
    use chrono::Timelike;
    let hour = chrono::Local::now().hour();
    let name = first_name();
    let (with, without) = match hour {
        5..=11 => (t("Good morning, {}"), t("Good morning")),
        12..=17 => (t("Good afternoon, {}"), t("Good afternoon")),
        _ => (t("Good evening, {}"), t("Good evening")),
    };
    if name.is_empty() {
        without.to_string()
    } else {
        with.replacen("{}", name, 1)
    }
}

/// "Wednesday, November 18" ("Rabu, 18 November").
fn today() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    let days = [
        t("Monday"),
        t("Tuesday"),
        t("Wednesday"),
        t("Thursday"),
        t("Friday"),
        t("Saturday"),
        t("Sunday"),
    ];
    let months = [
        t("January"),
        t("February"),
        t("March"),
        t("April"),
        t("May"),
        t("June"),
        t("July"),
        t("August"),
        t("September"),
        t("October"),
        t("November"),
        t("December"),
    ];
    let day = days[now.weekday().num_days_from_monday() as usize];
    let month = months[now.month0() as usize];
    if crate::i18n::is_indonesian() {
        format!("{day}, {} {month}", now.day())
    } else {
        format!("{day}, {month} {}", now.day())
    }
}

/// `a`, `a and b`, `a, b and c`, `a, b, c and 2 more`.
fn name_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [a] => a.clone(),
        [a, b] => tf("{} and {}", &[a, b]),
        [a, b, c] => tf("{} and {}", &[&format!("{a}, {b}"), c]),
        [a, b, c, rest @ ..] => tf(
            "{} and {} more",
            &[&format!("{a}, {b}, {c}"), &rest.len().to_string()],
        ),
    }
}

/// The text of a 13 px semibold title with a slightly tighter tracking.
fn title_job(text: &str, size: f32, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: style::semibold(size),
            color,
            extra_letter_spacing: -size * 0.02,
            ..Default::default()
        },
    );
    job
}

/// A card: the surface (a faint white wash in the dark theme) and a hairline; raised with a
/// stronger line and a soft shadow while hovered.
fn paint_card(painter: &egui::Painter, rect: Rect, hovered: bool, p: &Palette) {
    let radius = CornerRadius::same(10);
    if hovered {
        let shadow = egui::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(46),
        };
        painter.add(shadow.as_shape(rect, radius));
    }
    let fill = if p.dark {
        style::soft(Color32::WHITE, 0.025)
    } else {
        p.card
    };
    painter.rect_filled(rect, radius, p.canvas);
    painter.rect_filled(rect, radius, fill);
    if hovered {
        painter.rect_filled(rect, radius, p.hover);
    }
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, if hovered { p.border_strong } else { p.border }),
        StrokeKind::Inside,
    );
}

/// An icon tile (32 px, 9 px corners) in `color` on a 14 % wash of it, or neutral.
fn paint_tile(
    painter: &egui::Painter,
    rect: Rect,
    icon: &str,
    color: Option<Color32>,
    p: &Palette,
) {
    let (fill, ink) = match color {
        Some(c) => (style::soft(c, 0.14), c),
        None => (p.hover, p.text_muted),
    };
    painter.rect_filled(rect, CornerRadius::same(9), fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        icon,
        style::icon_font(17.0),
        ink,
    );
}

impl TernaryShellApp {
    /// The recent connections Home shows, newest first (saved connections only).
    pub(super) fn home_recent(&self) -> Vec<(Uuid, Option<i64>)> {
        let mut out: Vec<(Uuid, Option<i64>)> = self
            .settings
            .recent
            .iter()
            .filter(|r| self.store.bookmark(r.id).is_some())
            .take(HOME_CARDS)
            .map(|r| (r.id, Some(r.at)))
            .collect();
        if out.is_empty() {
            out = self
                .store
                .bookmarks
                .iter()
                .take(HOME_CARDS)
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

    /// Home, or the welcome page when there are no connections yet.
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
        let content_w = (area.width() - 128.0).clamp(320.0, 880.0);
        let x0 = area.center().x - content_w / 2.0;
        let x1 = x0 + content_w;
        let mut y = area.top() + 56.0;

        // Date and greeting; New connection on the right, its bottom on the title's.
        // Line boxes as in the design: 13 px × 1.4, 4 px, 26 px × 1.4.
        let painter = ui.painter();
        let date = painter.layout_no_wrap(today(), style::regular(13.0), p.text_faint);
        painter.galley(
            pos2(x0, y + (18.2 - date.size().y) / 2.0),
            date,
            p.text_faint,
        );
        let title = painter.layout_job(title_job(&greeting(), 26.0, p.text));
        let title_top = y + 18.2 + 4.0;
        painter.galley(
            pos2(x0, title_top + (36.4 - title.size().y) / 2.0),
            title,
            p.text,
        );
        let head_bottom = title_top + 36.4;
        let new_r = Rect::from_min_max(
            pos2(x1 - new_button_w(ui), head_bottom - style::CONTROL_H),
            pos2(x1, head_bottom),
        );
        if new_button(ui, new_r).clicked() {
            self.actions.push(Action::NewBookmark { group: None });
        }
        y = head_bottom + 26.0;

        // The search: opens the palette.
        let field = Rect::from_min_size(pos2(x0, y), vec2(content_w, 40.0));
        let resp = ui.interact(field, egui::Id::new("home-search"), Sense::click());
        let painter = ui.painter();
        let shadow = egui::Shadow {
            offset: [0, 1],
            blur: 2,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 102 } else { 15 }),
        };
        painter.add(shadow.as_shape(field, CornerRadius::same(8)));
        painter.rect(
            field,
            CornerRadius::same(8),
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
        let hint = {
            let mut job = egui::text::LayoutJob::default();
            let fmt = |font: egui::FontId, color| egui::TextFormat {
                font_id: font,
                color,
                valign: egui::Align::Center,
                ..Default::default()
            };
            job.append(
                t("Search connections and commands, or type"),
                0.0,
                fmt(style::regular(14.0), p.text_faint),
            );
            job.append("user@host:port", 5.0, fmt(style::mono(12.0), p.text_muted));
            painter.layout_job(job)
        };
        let key = S::QuickConnect.label();
        let kw = key_width(painter, &key);
        let clip = Rect::from_min_max(
            pos2(field.left() + 35.0, field.top()),
            pos2(field.right() - 12.0 - kw - 8.0, field.bottom()),
        );
        painter.with_clip_rect(clip).galley(
            pos2(field.left() + 35.0, field.center().y - hint.size().y / 2.0),
            hint,
            p.text_faint,
        );
        paint_key(
            painter,
            pos2(field.right() - 12.0 - kw, field.center().y),
            &key,
            &p,
            false,
        );
        if resp.clicked() {
            self.actions.push(Action::QuickConnect);
        }
        resp.on_hover_cursor(egui::CursorIcon::Text);
        y += 40.0 + 26.0;

        // The footer of key hints sits at the bottom; the sections stop above it.
        let foot_y = area.bottom() - 22.0 - 9.0;
        let limit = foot_y - 24.0;

        // Recent connections.
        let recent = self.home_recent();
        let from_recent = recent.first().is_some_and(|r| r.1.is_some());
        if !recent.is_empty() {
            let painter = ui.painter();
            painter.text(
                pos2(x0, y + 8.0),
                Align2::LEFT_CENTER,
                if from_recent {
                    t("Recent").to_uppercase()
                } else {
                    t("Connections").to_uppercase()
                },
                style::semibold(style::XS),
                p.text_faint,
            );
            if from_recent {
                let show = painter.layout_no_wrap(
                    t("Show all").to_string(),
                    style::regular(style::SMALL),
                    p.text_faint,
                );
                let r = Rect::from_min_size(
                    pos2(x1 - show.size().x, y + 8.0 - show.size().y / 2.0),
                    show.size(),
                );
                let sr = ui.interact(
                    r.expand(3.0),
                    egui::Id::new("home-show-all"),
                    Sense::click(),
                );
                ui.painter().galley(
                    r.min,
                    show,
                    if sr.hovered() { p.text } else { p.text_faint },
                );
                if sr.clicked() {
                    self.actions.push(Action::QuickConnect);
                }
            }
            y += 16.0 + 10.0;
            let card_w = (content_w - 24.0) / 3.0;
            let card_h = 96.0;
            let live = self.live_map();
            let key_prefix = home_key_prefix();
            let rows = recent.len().div_ceil(3);
            for (i, (id, at)) in recent.iter().enumerate() {
                let (col, row) = (i % 3, i / 3);
                let rect = Rect::from_min_size(
                    pos2(
                        x0 + col as f32 * (card_w + 12.0),
                        y + row as f32 * (card_h + 12.0),
                    ),
                    vec2(card_w, card_h),
                );
                if rect.bottom() > limit {
                    break;
                }
                let key = format!("{key_prefix}{}", i + 1);
                self.home_card(ui, rect, *id, *at, &key, live.get(id).copied());
            }
            y += rows as f32 * card_h + (rows.saturating_sub(1)) as f32 * 12.0 + 26.0;
        }

        // Groups, and the hosts of ~/.ssh/config not imported yet.
        let ssh_new = self.ssh_new_hosts();
        let top_groups: Vec<(Uuid, String, usize)> = self
            .store
            .child_groups(None)
            .map(|g| {
                let n = self
                    .store
                    .bookmarks
                    .iter()
                    .filter(|b| {
                        b.group
                            .is_some_and(|bg| self.store.group_and_descendants(g.id).contains(&bg))
                    })
                    .count();
                (g.id, g.name.clone(), n)
            })
            .collect();
        let shown_groups = top_groups.len().min(5);
        let groups_h = if shown_groups > 0 {
            14.0 + 16.0 + 10.0 + shown_groups as f32 * 30.0 + 14.0
        } else {
            0.0
        };
        let suggest_h = 14.0 + 18.0 + 3.0 + 34.0 + 10.0 + 26.0 + 14.0;
        let has_suggest = !ssh_new.is_empty();
        let block_h = groups_h.max(if has_suggest { suggest_h } else { 0.0 });
        if block_h > 0.0 && y + block_h <= limit {
            let (gw, sw) = match (shown_groups > 0, has_suggest) {
                (true, true) => {
                    let unit = (content_w - 12.0) / 2.25;
                    (unit * 1.25, unit)
                }
                (true, false) => (content_w, 0.0),
                (false, true) => (0.0, content_w),
                (false, false) => (0.0, 0.0),
            };
            if shown_groups > 0 {
                let rect = Rect::from_min_size(pos2(x0, y), vec2(gw, block_h));
                self.home_groups(ui, rect, &top_groups[..shown_groups]);
            }
            if has_suggest {
                let left = if shown_groups > 0 { x0 + gw + 12.0 } else { x0 };
                let rect = Rect::from_min_size(pos2(left, y), vec2(sw, block_h));
                self.home_suggestion(ui, rect, &ssh_new);
            }
        }

        // Key hints along the bottom.
        let hints: [(Vec<String>, &str); 4] = [
            (vec![S::QuickConnect.label()], t("search")),
            (
                vec![
                    format!("{}1", home_key_prefix()),
                    format!("{}9", home_key_prefix()),
                ],
                t("open recent"),
            ),
            (vec![S::NewConnection.label()], t("new connection")),
            (vec![S::Settings.label()], t("settings")),
        ];
        let painter = ui.painter();
        let mut parts = Vec::new();
        let mut total = 0.0;
        let dots = painter.layout_no_wrap("…".into(), style::regular(style::SMALL), p.text_faint);
        for (keys, label) in &hints {
            let g = painter.layout_no_wrap(
                label.to_string(),
                style::regular(style::SMALL),
                p.text_faint,
            );
            let mut w = 0.0;
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    w += 4.0 + dots.size().x + 4.0;
                }
                w += key_width(painter, k);
            }
            total += w + 6.0 + g.size().x;
            parts.push((keys.clone(), g));
        }
        total += 18.0 * (hints.len() - 1) as f32;
        if foot_y > y + 10.0 {
            let mut x = area.center().x - total / 2.0;
            for (keys, g) in parts {
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        x += 4.0;
                        painter.galley(
                            pos2(x, foot_y - dots.size().y / 2.0),
                            dots.clone(),
                            p.text_faint,
                        );
                        x += dots.size().x + 4.0;
                    }
                    x += paint_key(painter, pos2(x, foot_y), k, &p, false);
                }
                x += 6.0;
                let w = g.size().x;
                painter.galley(pos2(x, foot_y - g.size().y / 2.0), g, p.text_faint);
                x += w + 18.0;
            }
        }
    }

    /// One recent connection: its glyph, name and label, address, the key that opens it, when
    /// it was used; Files and Connect while hovered.
    fn home_card(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        id: Uuid,
        at: Option<i64>,
        key: &str,
        live: Option<Live>,
    ) {
        let p = style::pal(ui);
        let Some(b) = self.store.bookmark(id) else {
            return;
        };
        let resp = ui.interact(rect, egui::Id::new(("home-card", id)), Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        let painter = ui.painter();
        paint_card(painter, rect, hovered, &p);
        let label = self.store.label_of(b).cloned();
        let label_color = label.as_ref().map(|l| style::label_color(l.color, p.dark));
        let x = rect.left() + 14.0;
        let top = rect.top() + 14.0;
        let row1_c = top + 17.8;
        let glyph = Rect::from_center_size(pos2(x + 14.0, row1_c), vec2(28.0, 28.0));
        let status = live.map(|l| match l {
            Live::Connected => p.ok,
            Live::Connecting => p.warn,
            Live::Failed => p.err,
        });
        let card_fill = if hovered {
            blend_hover(blend_hover(p.canvas, &p), &p)
        } else {
            p.canvas
        };
        kit::paint_host_glyph(
            painter,
            glyph,
            kit::host_icon(b.host_icon()),
            label_color,
            status,
            card_fill,
            &p,
        );
        // The key that opens it, on the right of the first row.
        let kw = key_width(painter, key);
        let key_x = rect.right() - 14.0 - kw;
        paint_key(painter, pos2(key_x, row1_c), key, &p, !hovered);
        let tx = glyph.right() + 10.0;
        let name = painter.layout_no_wrap(b.display_name(), style::semibold(style::BODY), p.text);
        let mut tag_w = 0.0;
        let tag = label.as_ref().map(|l| l.tag()).filter(|s| !s.is_empty());
        if let Some(tag) = &tag {
            tag_w = kit::env_tag_width(ui, tag, false) + 6.0;
        }
        let name_max = (key_x - 8.0 - tx - tag_w).max(20.0);
        let name_w = name.size().x.min(name_max);
        // Line boxes: the name 18.2 px, 2 px, the address 15.4 px.
        let name_y = top + 1.0 + (18.2 - name.size().y) / 2.0;
        let painter = ui.painter();
        painter
            .with_clip_rect(Rect::from_min_max(
                pos2(tx, rect.top()),
                pos2(tx + name_max, rect.bottom()),
            ))
            .galley(pos2(tx, name_y), name.clone(), p.text);
        if let (Some(tag), Some(c)) = (&tag, label_color) {
            kit::paint_env_tag(
                painter,
                pos2(tx + name_w + 6.0, name_y + name.size().y / 2.0),
                tag,
                c,
                false,
            );
        }
        painter
            .with_clip_rect(Rect::from_min_max(
                pos2(tx, rect.top()),
                pos2(key_x - 8.0, rect.bottom()),
            ))
            .text(
                pos2(tx, top + 18.2 + 2.0 + 7.7),
                Align2::LEFT_CENTER,
                super::chrome::address_with_port(b),
                style::mono(11.0),
                p.text_faint,
            );
        // Second row: when, then the actions.
        let row2 = Rect::from_center_size(
            pos2((x + rect.right() - 14.0) / 2.0, top + 35.6 + 12.0 + 10.0),
            vec2(rect.right() - 14.0 - x, 26.0),
        );
        let when = match at {
            Some(at) => ago(at),
            None => {
                let path = self.store.group_path(b.group);
                if path.is_empty() {
                    "—".to_string()
                } else {
                    path
                }
            }
        };
        painter.text(
            pos2(row2.left(), row2.center().y),
            Align2::LEFT_CENTER,
            when,
            style::regular(style::SMALL),
            p.text_faint,
        );
        let mut files = false;
        let mut connect = false;
        let mut actions = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row2)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        actions.spacing_mut().item_spacing.x = 6.0;
        if hovered {
            connect = kit::button_ex(&mut actions, kit::Kind::Primary, None, t("Connect"), true)
                .clicked();
            files = kit::button_ex(
                &mut actions,
                kit::Kind::Secondary,
                Some(icons::FOLDER_SIMPLE),
                t("Files"),
                true,
            )
            .clicked();
        } else {
            kit::icon_button_ex(&mut actions, icons::ARROW_RIGHT, "", false, 20.0);
            kit::icon_button_ex(&mut actions, icons::FOLDER_SIMPLE, "", false, 20.0);
        }
        if files {
            self.actions.push(Action::OpenFiles(id));
        } else if connect || resp.clicked() {
            self.actions.push(Action::Connect(id));
        }
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }

    /// The groups card: each top-level group with its label and how many hosts it has.
    fn home_groups(&mut self, ui: &mut Ui, rect: Rect, groups: &[(Uuid, String, usize)]) {
        let p = style::pal(ui);
        paint_card(ui.painter(), rect, false, &p);
        let x = rect.left() + 16.0;
        let right = rect.right() - 16.0;
        let mut y = rect.top() + 14.0;
        ui.painter().text(
            pos2(x, y + 8.0),
            Align2::LEFT_CENTER,
            t("Groups").to_uppercase(),
            style::semibold(style::XS),
            p.text_faint,
        );
        y += 16.0 + 10.0;
        for (gid, name, n) in groups {
            let row = Rect::from_min_max(pos2(x - 6.0, y), pos2(right + 6.0, y + 30.0));
            let resp = ui.interact(row, egui::Id::new(("home-group", gid)), Sense::click());
            let painter = ui.painter();
            if resp.hovered() {
                painter.rect_filled(row, CornerRadius::same(style::RADIUS), p.hover);
            }
            let cy = row.center().y;
            let label = self.store.group_label(Some(*gid)).cloned();
            let color = label
                .as_ref()
                .map_or(p.text_ghost, |l| style::label_color(l.color, p.dark));
            painter.circle_filled(pos2(x + 4.0, cy), 4.0, color);
            let ng = painter.layout_no_wrap(name.clone(), style::medium(style::BODY), p.text);
            let nx = x + 8.0 + 8.0;
            let nw = ng.size().x;
            painter.galley(pos2(nx, cy - ng.size().y / 2.0), ng, p.text);
            if let Some(l) = &label {
                let tag = l.tag();
                if !tag.is_empty() {
                    kit::paint_env_tag(painter, pos2(nx + nw + 8.0, cy), &tag, color, false);
                }
            }
            painter.text(
                pos2(right, cy),
                Align2::RIGHT_CENTER,
                icons::CARET_RIGHT,
                style::icon_font(16.0),
                p.text_faint,
            );
            painter.text(
                pos2(right - 16.0 - 8.0, cy),
                Align2::RIGHT_CENTER,
                if *n == 1 {
                    t("1 host").to_string()
                } else {
                    tf("{} hosts", &[&n.to_string()])
                },
                style::regular(style::SMALL),
                p.text_faint,
            );
            if resp.clicked() {
                self.sidebar.reveal_group(&self.store, *gid);
                self.settings.sidebar_visible = true;
            }
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            y += 30.0;
        }
    }

    /// Hosts of ~/.ssh/config that are not saved yet: review and import, or not now.
    fn home_suggestion(&mut self, ui: &mut Ui, rect: Rect, hosts: &[String]) {
        let p = style::pal(ui);
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(10),
            p.accent_softer,
            Stroke::new(1.0, p.accent_tint),
            StrokeKind::Inside,
        );
        let tile = Rect::from_min_size(rect.min + vec2(16.0, 14.0), vec2(32.0, 32.0));
        paint_tile(painter, tile, icons::FILE_CODE, Some(p.accent_text), &p);
        let x = tile.right() + 12.0;
        let w = rect.right() - 16.0 - x;
        let title = if hosts.len() == 1 {
            t("1 new host in ~/.ssh/config").to_string()
        } else {
            tf("{} new hosts in ~/.ssh/config", &[&hosts.len().to_string()])
        };
        let tg = painter.layout(title, style::semibold(style::BODY), p.text, w);
        let rows = tg.rows.len().max(1) as f32;
        let th = 18.2 * rows;
        painter.galley(
            pos2(x, rect.top() + 14.0 + (th - tg.size().y) / 2.0),
            tg,
            p.text,
        );
        let list = name_list(hosts);
        let detail = if hosts.len() == 1 {
            tf("{} isn't in TernaryShell yet.", &[&list])
        } else {
            tf("{} aren't in TernaryShell yet.", &[&list])
        };
        let mut job =
            egui::text::LayoutJob::simple(detail, style::regular(style::SMALL), p.text_faint, w);
        job.sections
            .iter_mut()
            .for_each(|s| s.format.line_height = Some(17.4));
        let dg = painter.layout_job(job);
        let dy = rect.top() + 14.0 + th + 3.0;
        let dh = dg.size().y;
        painter.galley(pos2(x, dy), dg, p.text_faint);
        let by = dy + dh + 10.0;
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    pos2(x, by),
                    pos2(rect.right() - 16.0, by + 26.0),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 6.0;
        if kit::button_ex(
            &mut row,
            kit::Kind::Primary,
            None,
            t("Review & Import"),
            true,
        )
        .clicked()
        {
            self.actions.push(Action::ImportSshConfig);
        }
        if kit::button_ex(&mut row, kit::Kind::Ghost, None, t("Not Now"), true).clicked() {
            self.settings
                .dismissed_ssh_hosts
                .extend(hosts.iter().cloned());
            self.home.ssh_new = None;
        }
    }

    /// No connections at all: three ways to begin.
    fn welcome_page(&mut self, ui: &mut Ui) {
        let p = style::pal(ui);
        let area = ui.max_rect();
        let ssh_hosts = *self.ssh_config_hosts.get_or_insert_with(|| {
            crate::bookmarks::ssh_config::parse_file(&crate::paths::home_dir().join(".ssh/config"))
                .map(|h| {
                    h.iter()
                        .filter(|h| !h.alias.contains(['*', '?', '!']))
                        .count()
                })
                .unwrap_or(0)
        });
        let col_w = 640.0f32.min(area.width() - 48.0);
        let card_w = (col_w - 24.0) / 3.0;
        let card_h = 134.0;
        // Line boxes as in the design: title 26 × 1.4, subtitle 14 × 1.55, note 12 × 1.4.
        let block_h =
            76.0 + 14.0 + 8.0 + 36.4 + 8.0 + 21.7 + 8.0 + 26.0 + card_h + 8.0 + 22.0 + 16.8;
        let mut y = area.center().y - block_h / 2.0;
        let cx = area.center().x;

        // The app icon (as the 76 px icon image shows it: a 66 px tile) with a soft shadow.
        let logo = Rect::from_center_size(pos2(cx, y + 38.0), vec2(66.0, 66.0));
        let shadow = egui::Shadow {
            offset: [0, 10],
            blur: 30,
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 89 } else { 40 }),
        };
        ui.painter()
            .add(shadow.as_shape(logo, CornerRadius::same(15)));
        icons::paint_logo(ui.painter(), logo);
        y += 76.0 + 14.0 + 8.0;
        let painter = ui.painter();
        let title = painter.layout_job(title_job(t("Welcome to TernaryShell"), 26.0, p.text));
        painter.galley(
            pos2(cx - title.size().x / 2.0, y + (36.4 - title.size().y) / 2.0),
            title,
            p.text,
        );
        y += 36.4 + 8.0;
        let sub = painter.layout(
            t("A small, fast SSH and SFTP client. Let's add your first server.").to_string(),
            style::regular(14.0),
            p.text_muted,
            460.0,
        );
        let sub_h = 21.7 * sub.rows.len().max(1) as f32;
        painter.galley(
            pos2(cx - sub.size().x / 2.0, y + (sub_h - sub.size().y) / 2.0),
            sub,
            p.text_muted,
        );
        y += sub_h + 8.0 + 26.0;

        // Three ways in.
        let new_key = S::NewConnection.label();
        let cards: [WelcomeCard; 3] = [
            (
                icons::PLUS,
                t("Add a server"),
                t("Host, user and how to log in.").to_string(),
                Some(new_key.as_str()),
                true,
                Some(Action::NewBookmark { group: None }),
            ),
            (
                icons::FILE_CODE,
                t("From ~/.ssh/config"),
                match ssh_hosts {
                    0 => t("No hosts found in ~/.ssh/config.").to_string(),
                    1 => t("1 host found, ready to import.").to_string(),
                    n => tf("{} hosts found, ready to import.", &[&n.to_string()]),
                },
                None,
                false,
                (ssh_hosts > 0).then_some(Action::ImportSshConfig),
            ),
            (
                icons::TRAY_ARROW_DOWN,
                t("From a file"),
                t("Electerm or a TernaryShell export.").to_string(),
                None,
                false,
                Some(Action::Import),
            ),
        ];
        let x0 = cx - col_w / 2.0;
        for (i, (icon, title, desc, key, main, action)) in cards.into_iter().enumerate() {
            let rect = Rect::from_min_size(
                pos2(x0 + i as f32 * (card_w + 12.0), y),
                vec2(card_w, card_h),
            );
            let resp = ui.interact(rect, egui::Id::new(("welcome-card", i)), Sense::click());
            let hovered = action.is_some() && resp.hovered();
            let painter = ui.painter();
            if main {
                painter.rect_filled(rect, CornerRadius::same(10), p.accent_softer);
                painter.rect_stroke(
                    rect,
                    CornerRadius::same(10),
                    Stroke::new(1.5, p.accent_line),
                    StrokeKind::Outside,
                );
                if hovered {
                    painter.rect_filled(rect, CornerRadius::same(10), p.hover);
                }
            } else {
                paint_card(painter, rect, hovered, &p);
            }
            let tile = Rect::from_min_size(rect.min + vec2(16.0, 16.0), vec2(32.0, 32.0));
            paint_tile(painter, tile, icon, main.then_some(p.accent_text), &p);
            if let Some(k) = key {
                let kw = key_width(painter, k);
                paint_key(
                    painter,
                    pos2(rect.right() - 16.0 - kw, tile.center().y),
                    k,
                    &p,
                    false,
                );
            }
            let ty = tile.bottom() + 12.0;
            let faded = |c: Color32| {
                if action.is_some() {
                    c
                } else {
                    c.gamma_multiply(0.6)
                }
            };
            painter.text(
                pos2(rect.left() + 16.0, ty + 9.1),
                Align2::LEFT_CENTER,
                title,
                style::semibold(style::BODY),
                faded(p.text),
            );
            // The description, 12 px on 17.4 px lines; a leading count ("12 hosts") stands out.
            let mut job = egui::text::LayoutJob::default();
            let fmt = |color: Color32, bold: bool| egui::TextFormat {
                font_id: if bold {
                    style::semibold(style::SMALL)
                } else {
                    style::regular(style::SMALL)
                },
                color,
                line_height: Some(17.4),
                ..Default::default()
            };
            let count = (i == 1 && ssh_hosts > 0).then(|| {
                if ssh_hosts == 1 {
                    t("1 host").to_string()
                } else {
                    tf("{} hosts", &[&ssh_hosts.to_string()])
                }
            });
            match count.as_deref().filter(|c| desc.starts_with(c)) {
                Some(c) => {
                    job.append(c, 0.0, fmt(faded(p.text), true));
                    job.append(&desc[c.len()..], 0.0, fmt(faded(p.text_faint), false));
                }
                None => job.append(&desc, 0.0, fmt(faded(p.text_faint), false)),
            }
            job.wrap.max_width = card_w - 32.0;
            let g = painter.layout_job(job);
            painter.galley(pos2(rect.left() + 16.0, ty + 18.2 + 3.0), g, p.text_faint);
            if let Some(a) = action {
                if resp.clicked() {
                    self.actions.push(a);
                }
                if hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
            }
        }
        y += card_h + 8.0 + 22.0;
        let note = tf(
            "Passwords and keys are saved in {}, not in TernaryShell's files.",
            &[crate::platform::keychain()],
        );
        let painter = ui.painter();
        let ng = painter.layout_no_wrap(note, style::regular(style::SMALL), p.text_faint);
        let total = 16.0 + 8.0 + ng.size().x;
        let nx = cx - total / 2.0;
        let ny = y + 8.4;
        painter.text(
            pos2(nx + 8.0, ny),
            Align2::CENTER_CENTER,
            icons::LOCK_SIMPLE,
            style::icon_font(16.0),
            p.text_faint,
        );
        painter.galley(
            pos2(nx + 16.0 + 8.0, ny - ng.size().y / 2.0),
            ng,
            p.text_faint,
        );
    }
}

/// The pieces of Home's "+ New connection ⌘N" button: the label, the key, and its width.
fn new_button_parts(
    ui: &Ui,
) -> (
    std::sync::Arc<egui::Galley>,
    std::sync::Arc<egui::Galley>,
    f32,
) {
    let painter = ui.painter();
    let label = painter.layout_no_wrap(
        t("New connection").to_string(),
        style::medium(style::BODY),
        Color32::PLACEHOLDER,
    );
    let key = painter.layout_no_wrap(
        S::NewConnection.label(),
        style::medium(style::MICRO),
        Color32::PLACEHOLDER,
    );
    let w = 12.0 + 15.0 + 6.0 + label.size().x + 6.0 + 4.0 + key.size().x + 10.0 + 12.0;
    (label, key, w)
}

fn new_button_w(ui: &Ui) -> f32 {
    new_button_parts(ui).2
}

/// A secondary button (30 px) with a quiet key pill after its label: `+ New connection ⌘N`.
fn new_button(ui: &mut Ui, rect: Rect) -> egui::Response {
    let p = style::pal(ui);
    let (label, key, _) = new_button_parts(ui);
    let resp = ui.interact(rect, egui::Id::new("home-new"), Sense::click());
    let painter = ui.painter();
    let radius = CornerRadius::same(style::RADIUS);
    let shadow = egui::Shadow {
        offset: [0, 1],
        blur: 2,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 102 } else { 15 }),
    };
    painter.add(shadow.as_shape(rect, radius));
    painter.rect_filled(rect, radius, p.canvas);
    painter.rect(
        rect,
        radius,
        if resp.hovered() {
            blend_hover(p.button, &p)
        } else {
            p.button
        },
        Stroke::new(1.0, p.button_border),
        StrokeKind::Inside,
    );
    let cy = rect.center().y;
    let mut x = rect.left() + 12.0;
    painter.text(
        pos2(x + 7.5, cy),
        Align2::CENTER_CENTER,
        icons::PLUS,
        style::icon_font(15.0),
        p.text,
    );
    x += 15.0 + 6.0;
    let lw = label.size().x;
    painter.galley(pos2(x, cy - label.size().y / 2.0), label, p.text);
    x += lw + 6.0 + 4.0;
    let pill = Rect::from_min_size(
        pos2(x, cy - (key.size().y + 2.0) / 2.0),
        vec2(key.size().x + 10.0, key.size().y + 2.0),
    );
    painter.rect_filled(pill, CornerRadius::same(4), style::soft(p.hover, 0.75));
    painter.galley(
        pos2(pill.left() + 5.0, pill.top() + 1.0),
        key,
        p.text_faint.gamma_multiply(0.75),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// `base` with the hover wash over it, as one opaque colour.
fn blend_hover(base: Color32, p: &Palette) -> Color32 {
    let a = p.hover.a() as f32 / 255.0;
    let [tr, tg, tb, _] = p.hover.to_srgba_unmultiplied();
    let mix = |b: u8, t: u8| (b as f32 * (1.0 - a) + t as f32 * a).round() as u8;
    Color32::from_rgb(mix(base.r(), tr), mix(base.g(), tg), mix(base.b(), tb))
}

/// "⌘", "Alt+" or "Ctrl+": the modifiers of ⌘1–⌘9.
fn home_key_prefix() -> &'static str {
    let mods = crate::ui::keys::tab_number_modifiers();
    if cfg!(target_os = "macos") {
        "⌘"
    } else if mods.alt {
        "Alt+"
    } else {
        "Ctrl+"
    }
}
