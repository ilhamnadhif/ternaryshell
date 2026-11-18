//! Pieces of the connection cards: the card frame, the steps, fingerprints with their
//! randomart, the one-time-code boxes, and one sentence on what to check when a connection
//! fails. Everything here is drawn with the painter and repaints only while a step is in
//! progress. Redesign 3.1: no icon tiles, no route diagram, no checklist box.

use std::time::Duration;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use super::StepState;
use crate::bookmarks::Bookmark;
use crate::i18n::{t, tf};
use crate::ssh::types::{ConnError, ProbeReason};
use crate::ui::editor::Section;
use crate::ui::style::{self, Palette};
use crate::ui::{icons, kit};

// ----- the card ---------------------------------------------------------------------------

/// The frame of a connection card: the surface fill, 14 px corners, the popover shadow and a
/// hairline ring (a faint red when the card warns of danger).
pub fn frame(p: &Palette, danger: bool) -> egui::Frame {
    let ring = if danger {
        style::soft(p.err, 0.30)
    } else if p.dark {
        Color32::from_white_alpha(15)
    } else {
        Color32::from_black_alpha(20)
    };
    egui::Frame::NONE
        .fill(p.surface)
        .corner_radius(CornerRadius::same(style::RADIUS_XL))
        .shadow(egui::Shadow {
            offset: [0, if danger { 20 } else { 10 }],
            blur: if danger { 50 } else { 30 },
            spread: 0,
            color: Color32::from_black_alpha(if p.dark { 125 } else { 31 }),
        })
        .stroke(Stroke::new(1.0, ring))
}

/// The card's header: the title (15 px semibold) and a line under it.
pub fn head(ui: &mut Ui, title: &str, sub: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 18,
            right: 18,
            top: 18,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let text_w = ui.available_width();
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.set_max_width(text_w);
                    let p = style::pal(ui);
                    ui.add(
                        egui::Label::new(
                            RichText::new(title)
                                .font(style::semibold(15.0))
                                .color(p.text)
                                .extra_letter_spacing(-0.15),
                        )
                        .wrap(),
                    );
                    sub(ui);
                });
            });
        });
}

/// A line under the card's title: 12.5 px muted text, wrapping.
pub fn sub_text(ui: &mut Ui, text: &str) {
    let p = style::pal(ui);
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .font(style::regular(12.5))
                .color(p.text_muted),
        )
        .wrap(),
    );
}

/// A line under the title: an address in mono, then plain text (" · through bastion").
pub fn sub_address(ui: &mut Ui, address: &str, rest: &str) {
    let p = style::pal(ui);
    let mut job = egui::text::LayoutJob::default();
    job.append(
        address,
        0.0,
        egui::TextFormat {
            font_id: style::mono(11.5),
            color: p.text_muted,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    if !rest.is_empty() {
        job.append(
            rest,
            0.0,
            egui::TextFormat {
                font_id: style::regular(12.5),
                color: p.text_muted,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
    }
    ui.add(egui::Label::new(job).wrap());
}

/// The card's body: 18 px at the sides, 12 px between its parts.
pub fn body(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 18,
            right: 18,
            top: 0,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 12.0;
            add(ui);
        });
}

/// The card's footer: a quiet action on the left, the buttons on the right (no band, no
/// keyboard hints).
pub fn foot(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: 4,
            bottom: 16,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_height(style::CONTROL_H);
                ui.spacing_mut().item_spacing.x = 6.0;
                left(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    right(ui);
                });
            });
        });
}

/// A quiet small button (26 px, no frame until hovered).
pub fn ghost_sm(ui: &mut Ui, icon: Option<&str>, text: &str) -> egui::Response {
    kit::button_ex(ui, kit::Kind::Ghost, icon, text, true)
}

// ----- the steps --------------------------------------------------------------------------

/// One step (30 px): an 18 px circle (✓ done, spinning now, a ring pending, ✗ failed), the
/// label with an optional faint note after it, and how long it took on the right. `after`
/// tells whether a step comes before it, to join them with a line (green once done).
pub fn step(
    ui: &mut Ui,
    state: StepState,
    label: &str,
    note: Option<&str>,
    meta: Option<&str>,
    after: Option<StepState>,
) {
    let p = style::pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    let painter = ui.painter();
    let c = pos2(rect.left() + 11.0, rect.center().y);
    if let Some(prev) = after {
        let color = if prev == StepState::Done {
            style::soft(p.ok, 0.45)
        } else {
            p.border_strong
        };
        let x = rect.left() + 11.25;
        painter.line_segment(
            [pos2(x, rect.top() - 11.0), pos2(x, rect.top() + 3.0)],
            Stroke::new(1.5, color),
        );
    }
    match state {
        StepState::Done => {
            painter.circle_filled(c, 9.0, p.surface);
            painter.circle_filled(c, 9.0, style::soft(p.ok, if p.dark { 0.14 } else { 0.10 }));
            painter.text(
                c,
                Align2::CENTER_CENTER,
                icons::CHECK,
                style::icon_font(11.0),
                p.ok,
            );
        }
        StepState::Failed => {
            painter.circle_filled(c, 9.0, p.surface);
            painter.circle_filled(c, 9.0, style::soft(p.err, if p.dark { 0.14 } else { 0.09 }));
            painter.text(
                c,
                Align2::CENTER_CENTER,
                icons::X,
                style::icon_font(11.0),
                p.err,
            );
        }
        StepState::Pending => {
            painter.circle_filled(c, 9.0, p.surface);
            painter.circle_stroke(c, 8.25, Stroke::new(1.5, p.border_strong));
        }
        StepState::Now => {
            painter.circle_filled(c, 9.0, p.surface);
            painter.circle_stroke(c, 8.0, Stroke::new(2.0, p.accent_tint));
            spinner(painter, ui.input(|i| i.time) as f32, c, 7.0, &p);
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
    }
    let painter = ui.painter();
    let meta_w = meta.map_or(0.0, |m| {
        let g = painter.layout_no_wrap(m.to_string(), style::regular(style::SMALL), p.text_faint);
        let w = g.size().x;
        painter.galley(
            pos2(rect.right() - w, rect.center().y - g.size().y / 2.0),
            g,
            p.text_faint,
        );
        w + 10.0
    });
    let color = if state == StepState::Pending {
        p.text_faint
    } else {
        p.text
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(
        label,
        0.0,
        egui::TextFormat {
            font_id: style::regular(style::BODY),
            color,
            valign: egui::Align::Center,
            ..Default::default()
        },
    );
    if let Some(n) = note {
        job.append(
            &format!(" · {n}"),
            0.0,
            egui::TextFormat {
                font_id: style::regular(style::SMALL),
                color: p.text_faint,
                valign: egui::Align::Center,
                ..Default::default()
            },
        );
    }
    let g = painter.layout_job(job);
    let clip = Rect::from_min_max(rect.min, pos2(rect.right() - meta_w, rect.bottom()));
    painter.with_clip_rect(clip).galley(
        pos2(rect.left() + 32.0, rect.center().y - g.size().y / 2.0),
        g,
        color,
    );
}

/// A spinning arc on a faint track (radius `r`), at `time` seconds.
pub fn spinner(painter: &egui::Painter, time: f32, c: egui::Pos2, r: f32, p: &Palette) {
    painter.circle_stroke(c, r - 1.0, Stroke::new(2.0, p.subtle));
    let start = time * 7.5;
    let points: Vec<egui::Pos2> = (0..=16)
        .map(|i| {
            let a = start + i as f32 / 16.0 * std::f32::consts::FRAC_PI_2;
            c + vec2(a.cos(), a.sin()) * (r - 1.0)
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(2.0, p.accent)));
}

// ----- fingerprints -----------------------------------------------------------------------

/// `SHA256:abcd…` with the hash split into groups of 8, which are far easier to compare.
pub fn grouped(fingerprint: &str) -> String {
    let (prefix, hash) = match fingerprint.split_once(':') {
        Some((a, b)) => (format!("{a}:"), b),
        None => (String::new(), fingerprint),
    };
    let mut out = prefix;
    for (i, c) in hash.chars().enumerate() {
        if i > 0 && i % 8 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// How a fingerprint box looks.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fp {
    Plain,
    /// The saved key that no longer matches: faint and struck through.
    Old,
    /// The new key that doesn't match the saved one: on a red well.
    Danger,
}

/// A fingerprint in a mono well (selectable), its hash in groups of 8.
pub fn fingerprint(ui: &mut Ui, text: &str, kind: Fp) {
    let p = style::pal(ui);
    let (fill, stroke) = match kind {
        Fp::Danger => (
            style::soft(p.err, if p.dark { 0.14 } else { 0.09 }),
            style::soft(p.err, 0.4),
        ),
        _ => (p.well, p.border),
    };
    egui::Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut rt = RichText::new(grouped(text))
                .font(style::mono(12.0))
                .line_height(Some(19.0))
                .color(if kind == Fp::Old {
                    p.text_faint
                } else {
                    p.text
                });
            if kind == Fp::Old {
                rt = rt.strikethrough();
            }
            ui.add(egui::Label::new(rt).wrap().selectable(true));
        });
}

/// The randomart of a key (`ssh-keygen -lv`): the drunken bishop's walk over the SHA-256
/// digest the fingerprint names, in the 17 × 9 field. `title` is like `ED25519 256`.
pub fn randomart(fingerprint: &str, title: &str) -> Option<String> {
    use base64::Engine;
    const W: usize = 17;
    const H: usize = 9;
    const SYMBOLS: &[u8] = b" .o+=*BOX@%&#/^SE";
    let hash = fingerprint.strip_prefix("SHA256:")?;
    let digest = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(hash.trim_end_matches('='))
        .ok()?;
    let mut field = [[0u8; H]; W];
    let (mut x, mut y) = (W / 2, H / 2);
    let (sx, sy) = (x, y);
    for byte in digest {
        let mut b = byte;
        for _ in 0..4 {
            x = if b & 1 != 0 {
                (x + 1).min(W - 1)
            } else {
                x.saturating_sub(1)
            };
            y = if b & 2 != 0 {
                (y + 1).min(H - 1)
            } else {
                y.saturating_sub(1)
            };
            let v = &mut field[x][y];
            if (*v as usize) < SYMBOLS.len() - 3 {
                *v += 1;
            }
            b >>= 2;
        }
    }
    let frame = |label: &str| {
        let label = if label.len() > W { &label[..W] } else { label };
        let left = (W - label.len()) / 2;
        format!(
            "+{}{}{}+",
            "-".repeat(left),
            label,
            "-".repeat(W - left - label.len())
        )
    };
    let mut out = frame(&format!("[{title}]"));
    for row in 0..H {
        out.push_str("\n|");
        for (col, column) in field.iter().enumerate() {
            let c = if (col, row) == (x, y) {
                b'E'
            } else if (col, row) == (sx, sy) {
                b'S'
            } else {
                SYMBOLS[column[row] as usize]
            };
            out.push(c as char);
        }
        out.push('|');
    }
    out.push('\n');
    out.push_str(&frame("[SHA256]"));
    Some(out)
}

/// The randomart in its well (computed once per fingerprint, then kept).
pub fn randomart_box(ui: &mut Ui, fingerprint: &str, title: &str) {
    let p = style::pal(ui);
    let id = egui::Id::new(("randomart", fingerprint));
    let art: Option<String> = ui.data_mut(|d| {
        d.get_temp_mut_or_insert_with(id, || randomart(fingerprint, title))
            .clone()
    });
    let Some(art) = art else {
        return;
    };
    egui::Frame::NONE
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(art)
                        .font(style::mono(9.5))
                        .line_height(Some(10.5))
                        .color(p.text_muted),
                )
                .extend(),
            );
        });
}

/// The ssh-keygen command that prints the fingerprint of this kind of key on the server.
pub fn compare_command(algorithm: &str) -> String {
    let a = algorithm.to_ascii_lowercase();
    let kind = if a.contains("ed25519") {
        "ed25519"
    } else if a.contains("ecdsa") {
        "ecdsa"
    } else {
        "rsa"
    };
    format!("ssh-keygen -lf /etc/ssh/ssh_host_{kind}_key.pub")
}

/// The randomart title of a key type: `ED25519 256`, `ECDSA 384`, `RSA`.
pub fn art_title(algorithm: &str) -> String {
    let a = algorithm.to_ascii_lowercase();
    if a.contains("ed25519") {
        "ED25519 256".into()
    } else if a.contains("ecdsa") {
        let bits = if a.contains("384") {
            384
        } else if a.contains("521") {
            521
        } else {
            256
        };
        format!("ECDSA {bits}")
    } else if a.contains("rsa") {
        "RSA".into()
    } else {
        algorithm.to_uppercase()
    }
}

// ----- one-time codes ---------------------------------------------------------------------

/// Digits in a one-time code.
pub const CODE_LEN: usize = 6;

/// Whether a keyboard-interactive question asks for a one-time code (Google Authenticator,
/// TOTP): then the answer gets one box per digit and goes off after the last digit.
pub fn is_code_prompt(message: &str, echo: bool) -> bool {
    if echo {
        return false;
    }
    let m = message.to_lowercase();
    [
        "verification code",
        "one-time",
        "one time",
        "otp",
        "authenticator",
        "2fa",
        "totp",
        "token code",
    ]
    .iter()
    .any(|k| m.contains(k))
        && !m.contains("password")
}

/// One box per digit (44 × 48), a gap after the third, over a hidden text field that has the
/// keyboard. Returns the field's response; `text` keeps only digits.
pub fn code_boxes(ui: &mut Ui, text: &mut String, id: egui::Id) -> egui::Response {
    let p = style::pal(ui);
    let (bw, bh, gap, sep) = (44.0, 48.0, 8.0, 8.0);
    let total = bw * CODE_LEN as f32 + gap * (CODE_LEN - 1) as f32 + sep;
    let (rect, _) = ui.allocate_exact_size(vec2(total, bh), Sense::hover());
    // The field: transparent, over the boxes, so clicks and keys go to it.
    let resp = {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let v = child.visuals_mut();
        v.text_cursor.stroke = Stroke::NONE;
        v.selection.bg_fill = Color32::TRANSPARENT;
        v.selection.stroke = Stroke::NONE;
        child.add(
            egui::TextEdit::singleline(text)
                .id(id)
                .frame(egui::Frame::NONE)
                .text_color(Color32::TRANSPARENT)
                .char_limit(CODE_LEN)
                .desired_width(total),
        )
    };
    text.retain(|c| c.is_ascii_digit());
    text.truncate(CODE_LEN);
    let focused = resp.has_focus();
    let painter = ui.painter();
    let digits: Vec<char> = text.chars().collect();
    let mut x = rect.left();
    for i in 0..CODE_LEN {
        if i == 3 {
            x += sep;
        }
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(bw, bh));
        let on = focused && i == digits.len().min(CODE_LEN - 1);
        if on {
            painter.rect_stroke(
                r,
                CornerRadius::same(9),
                Stroke::new(3.0, p.accent_tint),
                StrokeKind::Outside,
            );
        }
        painter.rect(
            r,
            CornerRadius::same(9),
            p.field,
            Stroke::new(1.0, if on { p.accent } else { p.border_strong }),
            StrokeKind::Inside,
        );
        if let Some(d) = digits.get(i) {
            painter.text(
                r.center(),
                Align2::CENTER_CENTER,
                d,
                egui::FontId::monospace(20.0),
                p.text,
            );
        } else if on {
            painter.vline(
                r.center().x,
                (r.center().y - 10.0)..=(r.center().y + 10.0),
                Stroke::new(1.5, p.accent_text),
            );
        }
        x += bw + gap;
    }
    resp
}

// ----- when it fails ----------------------------------------------------------------------

/// One sentence on what to look at, for a connection that failed this way, and the page of
/// Edit Connection its [bracketed] part opens.
pub fn diagnosis(e: &ConnError, b: &Bookmark) -> Option<(String, Section)> {
    let port = b.port.to_string();
    Some(match e {
        ConnError::Jump(_, inner) => return diagnosis(inner, b),
        ConnError::Timeout => (
            tf(
                "The server didn't answer on port {}. Check that it is running and reachable from this network; it may need a VPN or a [jump host].",
                &[&port],
            ),
            Section::Network,
        ),
        ConnError::HostNotFound(_) => (
            tf(
                "{} could not be found. [Check the name], or use the server's IP address; some names only resolve over a VPN.",
                &[&b.host],
            ),
            Section::Server,
        ),
        ConnError::Refused => (
            tf(
                "Nothing accepts SSH connections on port {}. The server may use [another port], or a firewall rejects the connection.",
                &[&port],
            ),
            Section::Server,
        ),
        ConnError::Unreachable(_) => (
            tf(
                "The server's network can't be reached from {}. Check that you are online; a server on a private network needs a VPN or a [jump host].",
                &[crate::platform::this_computer()],
            ),
            Section::Network,
        ),
        ConnError::AuthFailed(_) => (
            tf(
                "The server didn't accept the login for {}. Check that your key or password is allowed for this user, or try [another way to log in].",
                &[&b.username],
            ),
            Section::Login,
        ),
        ConnError::KeyFile(_) => (
            t("Check [the key file]: it must exist, be readable by this user, and be the private key (not the .pub file).").into(),
            Section::Login,
        ),
        _ => return None,
    })
}

/// The cause of a lost connection, said in a short sentence.
pub fn lost_cause(e: Option<&ConnError>, name: &str) -> String {
    match e {
        Some(ConnError::NotResponding(ProbeReason::NetworkChanged)) => {
            tf("{} stopped answering after the network changed.", &[name])
        }
        Some(ConnError::NotResponding(ProbeReason::NoReply)) => {
            tf("{} stopped answering.", &[name])
        }
        Some(e) => e.message(),
        None => t("The connection was closed.").into(),
    }
}

/// The diagnosis sentence: muted text with its [bracketed] part as a link. Returns whether the
/// link was clicked (only the link: a click elsewhere on the sentence does nothing).
pub fn diagnosis_text(ui: &mut Ui, text: &str) -> bool {
    let p = style::pal(ui);
    let fmt = |color: Color32| egui::TextFormat {
        font_id: style::regular(12.5),
        color,
        line_height: Some(19.0),
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    // The link's characters, as a range of char indices.
    let link = match (text.find('['), text.find(']')) {
        (Some(a), Some(b)) if a < b => {
            job.append(&text[..a], 0.0, fmt(p.text_muted));
            job.append(&text[a + 1..b], 0.0, fmt(p.accent_text));
            job.append(&text[b + 1..], 0.0, fmt(p.text_muted));
            let start = text[..a].chars().count();
            Some(start..start + text[a + 1..b].chars().count())
        }
        _ => {
            job.append(text, 0.0, fmt(p.text_muted));
            None
        }
    };
    let (pos, galley, resp) = egui::Label::new(job)
        .wrap()
        .sense(if link.is_some() {
            Sense::click()
        } else {
            Sense::hover()
        })
        .layout_in_ui(ui);
    let on_link = link.as_ref().is_some_and(|range| {
        resp.hover_pos().is_some_and(|at| {
            let local = at - pos;
            galley.rect.contains(local.to_pos2())
                && range.contains(&galley.cursor_from_pos(local).index.0)
        })
    });
    ui.painter().galley(pos, galley, p.text_muted);
    #[cfg(debug_assertions)]
    crate::devtest::record("diagnosis", resp.rect);
    if on_link {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    on_link && resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_are_grouped_by_eight() {
        assert_eq!(
            grouped("SHA256:abcdefghijklmnopq"),
            "SHA256:abcdefgh ijklmnop q"
        );
    }

    #[test]
    fn randomart_matches_ssh_keygen() {
        // `ssh-keygen -lv -E sha256` of an Ed25519 host key.
        let art = randomart(
            "SHA256:bKgfrM53sRWLIn2H4+OUFz2KO/gXl9oj/TROyqdnOBo",
            "ED25519 256",
        )
        .unwrap();
        assert_eq!(
            art,
            "+--[ED25519 256]--+
|                 |
|                 |
|                 |
|       o  ..     |
|     .. So.oo.   |
|    .oo.*o=oo.   |
|    ..o=+*E* .+  |
|   . ooo*o+o*=+. |
|   .+..+++.o+Oo  |
+----[SHA256]-----+"
        );
    }

    #[test]
    fn code_prompts() {
        assert!(is_code_prompt("Verification code: ", false));
        assert!(is_code_prompt("Enter your OTP:", false));
        assert!(!is_code_prompt("Password: ", false));
        assert!(!is_code_prompt("Verification code: ", true));
    }
}
