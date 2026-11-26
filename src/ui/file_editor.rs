//! A small editor for remote text files (configs, scripts): opens over SFTP, saves back with
//! ⌘S, keeps the file's line endings, and warns when the file changed on the server meanwhile.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use eframe::egui::text::{CCursor, CCursorRange, CharIndex};
use eframe::egui::text_edit::TextEditState;
use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, FontId, Id, Key, Modifiers, Rect, RichText, Sense,
    Stroke, TextStyle, Ui, pos2, vec2,
};

use super::style::{self, Tone};
use super::{icons, kit};
use crate::i18n::{t, tf};
use crate::sftp::edit::{self, LoadError, Loaded, SaveError, Stamp};
use crate::sftp::entry::remote_name;
use crate::sftp::remote::Sftp;

enum State {
    Loading,
    Ready,
    Failed(LoadError),
}

enum Msg {
    Loaded(Result<Loaded, LoadError>),
    Saved(Result<Stamp, SaveError>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiscardNext {
    Close,
    Reload,
    Download,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Confirm {
    /// Lose unsaved changes to close, reload, or download?
    Discard(DiscardNext),
    /// The file changed on the server: overwrite it?
    Overwrite,
    /// The file was deleted on the server: create it again?
    Recreate,
}

pub enum EditorOutput {
    Open,
    /// Closed; `saved` when the file was written at least once (the listing should refresh).
    Close {
        saved: bool,
    },
    /// The file can't be edited here; the user asked to download it.
    Download,
}

enum Status {
    Saving,
    Saved,
    /// Local copy saved: stays visible even while the remote buffer is dirty, cleared on next edit.
    Copied(String),
    Info(String),
    Error(String),
}

static NEXT_EDITOR_ID: AtomicU64 = AtomicU64::new(1);

fn next_editor_id() -> u64 {
    NEXT_EDITOR_ID.fetch_add(1, Ordering::Relaxed)
}

pub struct FileEditor {
    instance_id: u64,
    pub path: String,
    state: State,
    text: String,
    /// The text as last loaded or saved.
    saved_text: String,
    /// The text being written right now.
    saving_text: Option<String>,
    crlf: bool,
    bom: bool,
    stamp: Option<Stamp>,
    pending: Option<mpsc::Receiver<Msg>>,
    saved_once: bool,
    status: Option<Status>,
    confirm: Option<Confirm>,
    focus: bool,
    /// Line and column of the cursor (1-based), for the status bar.
    cursor: (usize, usize),
    /// The file's permission bits.
    mode: Option<u32>,
    /// The server's name (the chip in the header).
    host: String,
    /// Close once the save in progress succeeds (Save in the discard question).
    close_after_save: bool,
    /// The laid out text and the cache key it was laid out from: typing lays the text
    /// out once per change, not once per frame.
    galley_cache: Option<(u64, Arc<egui::Galley>)>,
    /// The changed-line marks of the gutter, keyed by `hash((text, saved_text))`.
    marks: Option<(u64, Vec<u8>)>,
    /// The cursor's character index and text hash the line and column were computed for.
    cursor_char: Option<(usize, u64)>,
}

impl FileEditor {
    pub fn open(
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        sftp: Sftp,
        path: String,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (ctx2, p) = (ctx.clone(), path.clone());
        rt.spawn(async move {
            let _ = tx.send(Msg::Loaded(edit::load(&sftp, &p).await));
            ctx2.request_repaint();
        });
        Self {
            instance_id: next_editor_id(),
            path,
            state: State::Loading,
            text: String::new(),
            saved_text: String::new(),
            saving_text: None,
            crlf: false,
            bom: false,
            stamp: None,
            pending: Some(rx),
            saved_once: false,
            status: None,
            confirm: None,
            focus: true,
            cursor: (1, 1),
            mode: None,
            host: String::new(),
            close_after_save: false,
            galley_cache: None,
            marks: None,
            cursor_char: None,
        }
    }

    /// The server's name, shown in the header.
    pub fn with_host(mut self, host: String) -> Self {
        self.host = host;
        self
    }

    /// Loads the file again from the server, dropping the text in the editor.
    fn reload(&mut self, rt: &tokio::runtime::Handle, ctx: &egui::Context, sftp: Option<Sftp>) {
        let Some(sftp) = sftp else {
            self.status = Some(Status::Error(t("Not connected.").into()));
            return;
        };
        let (tx, rx) = mpsc::channel();
        let (ctx2, p) = (ctx.clone(), self.path.clone());
        rt.spawn(async move {
            let _ = tx.send(Msg::Loaded(edit::load(&sftp, &p).await));
            ctx2.request_repaint();
        });
        self.instance_id = next_editor_id();
        self.pending = Some(rx);
        self.saving_text = None;
        self.close_after_save = false;
        self.state = State::Loading;
        self.status = None;
        self.confirm = None;
        self.galley_cache = None;
        self.marks = None;
        self.cursor_char = None;
    }

    pub fn name(&self) -> &str {
        remote_name(&self.path)
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self.state, State::Ready) && self.text != self.saved_text
    }

    /// Whether the editor currently shows a blocking confirmation modal (Discard changes).
    pub fn modal_open(&self) -> bool {
        matches!(self.confirm, Some(Confirm::Discard(_)))
    }

    /// Closes now if nothing would be lost; otherwise asks first. Returns true when closed.
    pub fn request_close(&mut self) -> bool {
        if matches!(self.confirm, Some(Confirm::Discard(_))) {
            self.confirm = None;
            return false;
        }
        if self.is_dirty() {
            self.confirm = Some(Confirm::Discard(DiscardNext::Close));
            false
        } else {
            true
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        let msg = match rx.try_recv() {
            Ok(msg) => msg,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                self.close_after_save = false;
                if self.saving_text.take().is_some() {
                    self.status = Some(Status::Error(t("Save interrupted.").into()));
                } else if matches!(self.state, State::Loading) {
                    self.state =
                        State::Failed(LoadError::Failed(t("Could not open the file.").into()));
                }
                return;
            }
        };
        self.pending = None;
        match msg {
            Msg::Loaded(Ok(l)) => {
                self.text = l.text.clone();
                self.saved_text = l.text;
                self.crlf = l.crlf;
                self.bom = l.bom;
                self.stamp = Some(l.stamp);
                self.mode = l.mode;
                self.state = State::Ready;
                self.focus = true;
                self.marks = None;
                self.galley_cache = None;
                self.cursor_char = None;
            }
            Msg::Loaded(Err(e)) => self.state = State::Failed(e),
            Msg::Saved(result) => {
                let written = self.saving_text.take();
                match result {
                    Ok(stamp) => {
                        self.stamp = Some(stamp);
                        self.saved_once = true;
                        if let Some(text) = written {
                            self.saved_text = text;
                        }
                        self.marks = None;
                        if self.is_dirty() {
                            self.close_after_save = false;
                        }
                        self.status = Some(Status::Saved);
                    }
                    Err(SaveError::Changed) => {
                        self.close_after_save = false;
                        self.status = None;
                        self.confirm = Some(Confirm::Overwrite);
                    }
                    Err(SaveError::Deleted) => {
                        self.close_after_save = false;
                        self.status = None;
                        self.confirm = Some(Confirm::Recreate);
                    }
                    // The text stays in the editor, so nothing typed is lost.
                    Err(SaveError::Failed(e)) => {
                        self.close_after_save = false;
                        self.status = Some(Status::Error(tf("Could not save: {}", &[&e])))
                    }
                }
            }
        }
    }

    fn save(
        &mut self,
        rt: &tokio::runtime::Handle,
        ctx: &egui::Context,
        sftp: Option<Sftp>,
        overwrite: bool,
    ) {
        if self.saving_text.is_some() || !matches!(self.state, State::Ready) {
            return;
        }
        let Some(sftp) = sftp else {
            self.close_after_save = false;
            self.status = Some(Status::Error(t("Not connected.").into()));
            return;
        };
        let data = edit::encode(&self.text, self.crlf, self.bom);
        let expected = if overwrite { None } else { self.stamp };
        let (tx, rx) = mpsc::channel();
        let (ctx2, path) = (ctx.clone(), self.path.clone());
        rt.spawn(async move {
            let _ = tx.send(Msg::Saved(edit::save(&sftp, &path, &data, expected).await));
            ctx2.request_repaint();
        });
        self.saving_text = Some(self.text.clone());
        self.pending = Some(rx);
        self.status = Some(Status::Saving);
    }

    /// Indentation inserted by Tab: the file's own style (tabs if it indents with tabs).
    fn indent_unit(&self) -> &'static str {
        if self.text.lines().any(|l| l.starts_with('\t')) {
            "\t"
        } else if self.path.ends_with(".py") {
            "    "
        } else {
            "  "
        }
    }

    /// Tab and Enter, handled before the text field sees them: Tab indents with spaces (YAML
    /// forbids tabs), Enter keeps the current line's indentation. Each key press becomes the
    /// text it stands for, in place, so fast typing keeps its order.
    fn handle_keys(&mut self, ctx: &egui::Context, id: Id) {
        if !ctx.memory(|m| m.has_focus(id)) {
            return;
        }
        let pressed = |e: &Event, key: Key| matches!(e, Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && modifiers.is_none());
        if !ctx.input(|i| {
            i.events
                .iter()
                .any(|e| pressed(e, Key::Tab) || pressed(e, Key::Enter))
        }) {
            return;
        }
        let cursor = TextEditState::load(ctx, id)
            .and_then(|s| s.cursor.char_range())
            .map_or(self.text.chars().count(), |r| {
                r.as_sorted_char_range().start.0
            });
        let start = self
            .text
            .char_indices()
            .nth(cursor)
            .map_or(self.text.len(), |(b, _)| b);
        let line_start = self.text[..start].rfind('\n').map_or(0, |i| i + 1);
        let indent: String = self.text[line_start..start]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let unit = self.indent_unit();
        ctx.input_mut(|i| {
            for e in &mut i.events {
                if pressed(e, Key::Tab) {
                    *e = Event::Text(unit.to_string());
                } else if pressed(e, Key::Enter) {
                    *e = Event::Text(format!("\n{indent}"));
                }
            }
        });
    }

    /// Writes the text to a file on this Mac (when the server can't take it).
    fn save_copy(&mut self) {
        let Some(dest) = rfd::FileDialog::new()
            .set_title(t("Save a Copy"))
            .set_file_name(self.name())
            .save_file()
        else {
            return;
        };
        let data = edit::encode(&self.text, self.crlf, self.bom);
        self.status = Some(match std::fs::write(&dest, data) {
            Ok(()) => Status::Copied(tf("Saved a copy to {}.", &[&dest.display().to_string()])),
            Err(e) => Status::Error(tf("Could not save: {}", &[&e.to_string()])),
        });
    }

    /// The editor filling `rect` (the tab's canvas): the header (← Terminal, the path, the
    /// host, Save), a banner when the file changed or vanished on the server, the text with its
    /// gutter, and the status bar.
    pub fn show_in(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        rt: &tokio::runtime::Handle,
        sftp: Option<Sftp>,
        overlay_open: bool,
    ) -> EditorOutput {
        self.poll();
        let ctx = ui.ctx().clone();
        let mut out = EditorOutput::Open;
        let text_id = Id::new(("file-editor-text", self.instance_id));
        let dirty = self.is_dirty();
        let text_before_hash = egui::util::hash(&self.text);
        let mut save_clicked = false;
        let mut close_clicked = false;
        let mut copy_clicked = false;
        let mut reload_clicked = false;
        let mut force_reload = false;
        let mut download_clicked = false;
        let mut overwrite_clicked = false;

        if !overlay_open && !matches!(self.confirm, Some(Confirm::Discard(_))) {
            let (save_key, escape) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, Key::S),
                    i.key_pressed(Key::Escape),
                )
            });
            save_clicked |= save_key;
            close_clicked |= escape && !egui::Popup::is_any_open(&ctx);
            if matches!(self.state, State::Ready) {
                self.handle_keys(&ctx, text_id);
            }
        }
        if self.close_after_save && !self.is_dirty() && matches!(self.status, Some(Status::Saved)) {
            return EditorOutput::Close { saved: true };
        }

        let p = style::pal(ui);
        let dark = p.dark;
        ui.painter().rect_filled(rect, 0.0, p.canvas);
        let mut root = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let ui = &mut root;
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let ready = matches!(self.state, State::Ready);
        let failed = matches!(self.state, State::Failed(_));

        // ---- Header (44 px).
        let head = Rect::from_min_size(rect.min, vec2(rect.width(), HEAD_H));
        ui.painter().hline(
            head.x_range(),
            head.bottom() - 0.5,
            Stroke::new(1.0, p.border),
        );
        let cy = head.center().y;
        let mut right = head.right() - 10.0;
        let mut place = |w: f32, h: f32| {
            let r = Rect::from_min_max(pos2(right - w, cy - h / 2.0), pos2(right, cy + h / 2.0));
            right = r.left() - 8.0;
            r
        };
        if failed {
            let r = place(24.0, 24.0);
            let resp = ui
                .interact(r, Id::new(("editor-x", self.instance_id)), Sense::click())
                .on_hover_text(t("Close"));
            kit::paint_icon_button(ui, &resp, r, icons::X, t("Close"), false, false);
            if resp.clicked() {
                close_clicked = true;
            }
        } else {
            let saving = self.saving_text.is_some();
            let r = place(24.0, 24.0);
            let more = ui
                .interact(
                    r,
                    Id::new(("editor-more", self.instance_id)),
                    Sense::click(),
                )
                .on_hover_text(t("More"));
            kit::paint_icon_button(ui, &more, r, icons::DOTS_THREE, t("More"), false, false);
            egui::Popup::menu(&more).show(|ui| {
                kit::menu_body(ui);
                ui.set_min_width(220.0);
                if ui
                    .add_enabled_ui(ready, |ui| {
                        kit::menu_item(ui, Some(icons::FLOPPY_DISK), t("Save a Copy…"), None)
                    })
                    .inner
                    .on_hover_text(tf(
                        "Save the text to a file on {}",
                        &[crate::platform::this_computer()],
                    ))
                    .clicked()
                {
                    copy_clicked = true;
                    ui.close();
                }
                if ui
                    .add_enabled_ui(!saving, |ui| {
                        kit::menu_item(
                            ui,
                            Some(icons::ARROWS_CLOCKWISE),
                            t("Reload from Server"),
                            None,
                        )
                    })
                    .inner
                    .clicked()
                {
                    reload_clicked = true;
                    ui.close();
                }
                if ui
                    .add_enabled_ui(!saving, |ui| {
                        kit::menu_item(ui, Some(icons::DOWNLOAD_SIMPLE), t("Download"), None)
                    })
                    .inner
                    .clicked()
                {
                    download_clicked = true;
                    ui.close();
                }
            });
            let save_kind = if dirty {
                kit::Kind::Primary
            } else {
                kit::Kind::Secondary
            };
            let label = t("Save");
            let keys = super::keys::Shortcut::Save.label();
            let w = save_width(ui, label);
            let r = place(w, style::TOOLBAR_H);
            let mut b = ui.new_child(egui::UiBuilder::new().max_rect(r));
            let save = b
                .add_enabled_ui(ready && dirty && !saving, |ui| {
                    kit::button_kbd(ui, save_kind, None, label, Some(&keys), true, false)
                })
                .inner;
            if save.clicked() {
                save_clicked = true;
            }
            if saving {
                let r = place(14.0, 14.0);
                kit::paint_spinner(ui, r.center(), 7.0);
            }
        }
        // The host (Redesign 3.1: no label tag).
        let host_g =
            ui.painter()
                .layout_no_wrap(self.host.clone(), style::mono(11.0), p.text_faint);
        let hr = place(host_g.size().x, 16.0);
        ui.painter().galley(
            pos2(hr.left(), cy - host_g.size().y / 2.0),
            host_g,
            p.text_faint,
        );
        // ← Terminal, then the path to the file.
        let mut x = head.left() + 8.0;
        let back_w =
            text_w(ui, t("Terminal"), style::medium(style::SMALL)) + 9.0 * 2.0 + 14.0 + 5.0;
        let back_r = Rect::from_min_size(pos2(x, cy - 13.0), vec2(back_w, 26.0));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(back_r));
        if kit::button_ex(
            &mut b,
            kit::Kind::Ghost,
            Some(icons::ARROW_LEFT),
            t("Terminal"),
            true,
        )
        .on_hover_text(t("Back to the terminal"))
        .clicked()
        {
            close_clicked = true;
        }
        x = back_r.right() + 8.0;
        ui.painter().vline(
            x + 4.0,
            (cy - 8.0)..=(cy + 8.0),
            Stroke::new(1.0, p.border_strong),
        );
        x += 8.0 + 8.0;
        self.paint_crumbs(ui, pos2(x, cy), right - 8.0, dirty, &p);

        // ---- The banner of a conflict with the server.
        let mut top = head.bottom();
        if let Some(c @ (Confirm::Overwrite | Confirm::Recreate)) = self.confirm {
            let h = 82.0;
            let band = Rect::from_min_size(pos2(rect.left(), top), vec2(rect.width(), h));
            let choice = self.banner(ui, band, c, &p);
            match choice {
                Some(Banner::Reload) => force_reload = true,
                Some(Banner::Copy) => copy_clicked = true,
                Some(Banner::Overwrite) => overwrite_clicked = true,
                Some(Banner::Close) => close_clicked = true,
                None => {}
            }
            top = band.bottom();
        }

        // ---- The status bar (28 px).
        let foot = Rect::from_min_max(pos2(rect.left(), rect.bottom() - FOOT_H), rect.max);
        self.status_bar(ui, foot, dirty, &p);

        // ---- The text.
        let body = Rect::from_min_max(pos2(rect.left(), top), pos2(rect.right(), foot.top()));
        let mut b = ui.new_child(egui::UiBuilder::new().max_rect(body));
        match &self.state {
            State::Loading => {
                let c = body.center();
                kit::paint_spinner(&b, c - vec2(60.0, 0.0), 7.0);
                b.painter().text(
                    c - vec2(46.0, 0.0),
                    Align2::LEFT_CENTER,
                    tf("Opening {}…", &[self.name()]),
                    style::regular(style::BODY),
                    p.text_muted,
                );
            }
            State::Failed(e) => {
                let message = e.message();
                let can_download = matches!(e, LoadError::TooLarge(_) | LoadError::NotText);
                b.add_space((body.height() * 0.28).max(8.0));
                b.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // Redesign 3.1: a plain icon, no tile.
                    ui.label(
                        RichText::new(icons::FILE_X)
                            .font(style::icon_font(28.0))
                            .color(p.text_faint),
                    );
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new(tf("{} can't be edited here", &[self.name()]))
                            .font(style::semibold(14.0))
                            .color(p.text),
                    );
                    ui.add_space(8.0);
                    ui.allocate_ui(vec2(340.0, 40.0), |ui| {
                        ui.set_max_width(340.0);
                        ui.vertical_centered(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&message)
                                        .font(style::regular(style::SMALL))
                                        .color(p.text_faint),
                                )
                                .wrap()
                                .halign(egui::Align::Center),
                            );
                        });
                    });
                    ui.add_space(18.0);
                    let back = t("Back to Files");
                    let dl = t("Download");
                    let w = text_w(ui, back, style::medium(style::BODY))
                        + 24.0
                        + if can_download {
                            8.0 + text_w(ui, dl, style::medium(style::BODY)) + 24.0 + 15.0 + 6.0
                        } else {
                            0.0
                        };
                    ui.allocate_ui_with_layout(
                        vec2(w, style::CONTROL_H),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            if kit::secondary(ui, None, back).clicked() {
                                close_clicked = true;
                            }
                            if can_download
                                && kit::button_ex(
                                    ui,
                                    kit::Kind::Primary,
                                    Some(icons::DOWNLOAD_SIMPLE),
                                    dl,
                                    false,
                                )
                                .clicked()
                            {
                                out = EditorOutput::Download;
                            }
                        },
                    );
                });
            }
            State::Ready => self.text_area(&mut b, body, text_id, &p, dark),
        }

        if egui::util::hash(&self.text) != text_before_hash
            && matches!(self.status, Some(Status::Copied(_)))
        {
            self.status = None;
        }
        if copy_clicked {
            self.save_copy();
        }
        if force_reload {
            self.reload(rt, &ctx, sftp.clone());
        } else if reload_clicked {
            if self.is_dirty() {
                self.confirm = Some(Confirm::Discard(DiscardNext::Reload));
            } else {
                self.reload(rt, &ctx, sftp.clone());
            }
        }
        if download_clicked {
            if self.is_dirty() {
                self.confirm = Some(Confirm::Discard(DiscardNext::Download));
            } else {
                out = EditorOutput::Download;
            }
        }
        if overwrite_clicked {
            self.confirm = None;
            self.save(rt, &ctx, sftp.clone(), true);
        }
        if save_clicked {
            if self.confirm.is_some_and(|c| c == Confirm::Recreate) {
                self.confirm = None;
                self.save(rt, &ctx, sftp.clone(), true);
            } else if dirty {
                self.save(rt, &ctx, sftp.clone(), false);
            } else if ready {
                self.status = Some(Status::Info(t("No changes to save.").into()));
            }
        }
        // "Saved" only while it is true (a local copy message stays until the next edit).
        if self.is_dirty() && matches!(self.status, Some(Status::Saved | Status::Info(_))) {
            self.status = None;
        }

        if let Some(Confirm::Discard(next)) = self.confirm {
            let choice = std::cell::Cell::new(None::<u8>);
            let host = if self.host.is_empty() {
                t("the server").to_string()
            } else {
                self.host.clone()
            };
            style::modal(&ctx, Id::new(("file-editor-discard", self.instance_id))).show(
                &ctx,
                |ui| {
                    ui.set_width(380.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    dialog_head(
                        ui,
                        &tf("Discard changes to {}?", &[self.name()]),
                        &tf("Your edits haven't been saved to {}.", &[&host]),
                    );
                    ui.add_space(14.0);
                    let keys = super::keys::Shortcut::Save.label();
                    kit::dialog_footer(
                        ui,
                        |ui| {
                            if discard_button(ui).clicked() {
                                choice.set(Some(1));
                            }
                        },
                        |ui| {
                            if next == DiscardNext::Close
                                && kit::button_kbd(
                                    ui,
                                    kit::Kind::Primary,
                                    None,
                                    t("Save"),
                                    Some(&keys),
                                    false,
                                    false,
                                )
                                .clicked()
                            {
                                choice.set(Some(2));
                            }
                            if kit::secondary(ui, None, t("Keep Editing")).clicked() {
                                choice.set(Some(0));
                            }
                        },
                    );
                },
            );
            if !overlay_open {
                let (esc, save_key) = ctx.input_mut(|i| {
                    (
                        i.key_pressed(Key::Escape),
                        next == DiscardNext::Close && i.consume_key(Modifiers::COMMAND, Key::S),
                    )
                });
                if esc {
                    choice.set(Some(0));
                }
                if save_key {
                    choice.set(Some(2));
                }
            }
            match choice.get() {
                Some(1) => {
                    self.confirm = None;
                    match next {
                        DiscardNext::Close => {
                            return EditorOutput::Close {
                                saved: self.saved_once,
                            };
                        }
                        DiscardNext::Reload => self.reload(rt, &ctx, sftp),
                        DiscardNext::Download => return EditorOutput::Download,
                    }
                }
                Some(2) => {
                    self.confirm = None;
                    self.close_after_save = true;
                    self.save(rt, &ctx, sftp, false);
                }
                Some(_) => self.confirm = None,
                None => {}
            }
        } else if close_clicked && self.request_close() {
            return EditorOutput::Close {
                saved: self.saved_once,
            };
        }
        out
    }

    /// The path to the file in the header: its folders (the first ones give way when it is
    /// long), then the file's icon, name and, with unsaved changes, a dot.
    fn paint_crumbs(
        &self,
        ui: &mut Ui,
        left: egui::Pos2,
        right: f32,
        dirty: bool,
        p: &style::Palette,
    ) {
        let painter = ui.painter();
        let folders: Vec<&str> = self
            .path
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(d, _)| d)
            .unwrap_or("")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();
        let name = self.name();
        let file_g = painter.layout_no_wrap(name.to_string(), style::semibold(style::BODY), p.text);
        let file_w = 16.0 + 7.0 + file_g.size().x + if dirty { 7.0 + 7.0 } else { 0.0 };
        let font = style::regular(style::BODY);
        let sep = 12.0 + 4.0;
        let widths: Vec<f32> = folders
            .iter()
            .map(|f| {
                painter
                    .layout_no_wrap(f.to_string(), font.clone(), p.text)
                    .size()
                    .x
                    + 12.0
            })
            .collect();
        let avail = right - left.x - file_w;
        let mut first = 0;
        let total = |from: usize| -> f32 {
            widths[from..].iter().sum::<f32>()
                + sep * (folders.len() - from) as f32
                + if from > 0 { 16.0 + sep } else { 0.0 }
        };
        while first < folders.len() && total(first) > avail {
            first += 1;
        }
        let cy = left.y;
        let mut x = left.x;
        if first > 0 {
            painter.text(
                pos2(x + 6.0, cy),
                Align2::LEFT_CENTER,
                "…",
                font.clone(),
                p.text_faint,
            );
            x += 16.0;
            painter.text(
                pos2(x + 8.0, cy),
                Align2::CENTER_CENTER,
                icons::CARET_RIGHT,
                style::icon_font(10.0),
                p.text_ghost,
            );
            x += sep;
        }
        for (i, f) in folders.iter().enumerate().skip(first) {
            painter.text(
                pos2(x + 6.0, cy),
                Align2::LEFT_CENTER,
                *f,
                font.clone(),
                p.text_muted,
            );
            x += widths[i];
            painter.text(
                pos2(x + 8.0, cy),
                Align2::CENTER_CENTER,
                icons::CARET_RIGHT,
                style::icon_font(10.0),
                p.text_ghost,
            );
            x += sep;
        }
        let kind = icons::FileKind::of(name, false, false, false);
        icons::paint_file_icon(painter, pos2(x + 8.0, cy), 16.0, kind, p);
        x += 16.0 + 7.0;
        let clip = Rect::from_min_max(pos2(x, cy - 12.0), pos2(right, cy + 12.0));
        let gw = file_g.size().x;
        painter
            .with_clip_rect(clip)
            .galley(pos2(x, cy - file_g.size().y / 2.0), file_g, p.text);
        if dirty && x + gw + 10.0 < right {
            painter.circle_filled(pos2(x + gw + 7.0 + 3.5, cy), 3.5, p.text_muted);
        }
    }

    /// The banner under the header when the file changed or was deleted on the server.
    fn banner(
        &self,
        ui: &mut Ui,
        band: Rect,
        confirm: Confirm,
        p: &style::Palette,
    ) -> Option<Banner> {
        let mut out = None;
        let (tone, icon, title, sub) = if confirm == Confirm::Overwrite {
            (
                Tone::Warn,
                icons::ARROWS_CLOCKWISE,
                tf("{} changed on the server", &[self.name()]),
                format!(
                    "{} {}",
                    t("Someone saved it after you opened it."),
                    t("Saving now replaces their version with yours.")
                ),
            )
        } else {
            (
                Tone::Err,
                icons::FILE_X,
                tf("{} was deleted on the server", &[self.name()]),
                format!(
                    "{} {}",
                    t("It was removed after you opened it."),
                    t("Saving creates it again.")
                ),
            )
        };
        let painter = ui.painter();
        painter.rect_filled(band, 0.0, p.tone_soft(tone));
        painter.hline(
            band.x_range(),
            band.bottom() - 0.5,
            Stroke::new(1.0, p.border),
        );
        // Redesign 3.1: the tint says it; no icon tile.
        let _ = icon;
        let x = band.left() + 14.0;
        let w = band.right() - 12.0 - x;
        let g = painter.layout_no_wrap(title, style::semibold(style::BODY), p.text);
        painter.galley(pos2(x, band.top() + 10.0), g, p.text);
        let mut job = egui::text::LayoutJob::simple_singleline(
            sub,
            style::regular(style::SMALL),
            p.text_muted,
        );
        job.wrap.max_width = w;
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some('…');
        let g = painter.layout_job(job);
        painter.galley(pos2(x, band.top() + 28.0), g, p.text_muted);
        let row = Rect::from_min_max(
            pos2(x, band.top() + 47.0),
            pos2(band.right() - 12.0, band.top() + 73.0),
        );
        let mut r = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        r.spacing_mut().item_spacing.x = 6.0;
        if confirm == Confirm::Overwrite {
            if kit::button_ex(
                &mut r,
                kit::Kind::Secondary,
                Some(icons::DOWNLOAD_SIMPLE),
                t("Reload theirs"),
                true,
            )
            .clicked()
            {
                out = Some(Banner::Reload);
            }
            if kit::button_ex(&mut r, kit::Kind::Secondary, None, t("Save a Copy…"), true).clicked()
            {
                out = Some(Banner::Copy);
            }
            r.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::button_ex(ui, kit::Kind::DangerQuiet, None, t("Overwrite"), true).clicked()
                {
                    out = Some(Banner::Overwrite);
                }
            });
        } else {
            if kit::button_ex(&mut r, kit::Kind::Secondary, None, t("Save a Copy…"), true).clicked()
            {
                out = Some(Banner::Copy);
            }
            if kit::button_ex(&mut r, kit::Kind::Ghost, None, t("Close"), true).clicked() {
                out = Some(Banner::Close);
            }
            let keys = super::keys::Shortcut::Save.label();
            r.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::button_kbd(
                    ui,
                    kit::Kind::Primary,
                    None,
                    t("Save"),
                    Some(&keys),
                    true,
                    false,
                )
                .clicked()
                {
                    out = Some(Banner::Overwrite);
                }
            });
        }
        out
    }

    /// The status bar: saved or not, the host and path; position, indentation, line endings,
    /// encoding and language on the right.
    fn status_bar(&self, ui: &mut Ui, foot: Rect, dirty: bool, p: &style::Palette) {
        let painter = ui.painter();
        painter.rect_filled(foot, 0.0, p.canvas);
        painter.hline(foot.x_range(), foot.top() + 0.5, Stroke::new(1.0, p.border));
        let cy = foot.center().y;
        let xs = style::regular(style::XS);
        let mut x = foot.left() + 8.0;
        // A segment: optional lead (dot, icon), text; 6 px inside, 2 apart.
        let segment = |x: &mut f32,
                       lead: Option<(&str, Color32)>,
                       dot: Option<Color32>,
                       text: &str,
                       font: FontId,
                       color: Color32| {
            let g = painter.layout_no_wrap(text.to_string(), font, color);
            let mut cx = *x + 6.0;
            if let Some(c) = dot {
                painter.circle_filled(pos2(cx + 3.5, cy), 3.5, c);
                cx += 7.0 + 6.0;
            }
            if let Some((icon, c)) = lead {
                painter.text(
                    pos2(cx + 6.5, cy),
                    Align2::CENTER_CENTER,
                    icon,
                    style::icon_font(13.0),
                    c,
                );
                cx += 13.0 + 6.0;
            }
            let w = g.size().x;
            painter.galley(pos2(cx, cy - g.size().y / 2.0), g, color);
            *x = cx + w + 6.0 + 2.0;
        };
        let ready = matches!(self.state, State::Ready);
        match &self.status {
            Some(Status::Saving) => {
                segment(&mut x, None, None, t("Saving…"), xs.clone(), p.text_muted)
            }
            Some(Status::Error(m)) => segment(&mut x, None, Some(p.err), m, xs.clone(), p.err),
            Some(Status::Info(m) | Status::Copied(m)) => {
                segment(&mut x, None, None, m, xs.clone(), p.text_muted)
            }
            _ if dirty => segment(
                &mut x,
                None,
                Some(p.warn),
                t("Unsaved changes"),
                xs.clone(),
                p.warn,
            ),
            _ if ready => {
                icons::paint_badge_circle(
                    painter,
                    pos2(x + 6.0 + 6.5, cy),
                    15.0,
                    icons::CHECK,
                    p.ok,
                    p.canvas,
                );
                let g = painter.layout_no_wrap(t("Saved").to_string(), xs.clone(), p.text_muted);
                let w = g.size().x;
                painter.galley(
                    pos2(x + 6.0 + 13.0 + 6.0, cy - g.size().y / 2.0),
                    g,
                    p.text_muted,
                );
                x += 6.0 + 13.0 + 6.0 + w + 8.0;
            }
            _ => {}
        }
        let path = short_path(&self.path);
        let where_ = if self.host.is_empty() {
            path
        } else {
            format!("{} · {}", self.host, path)
        };
        if !ready {
            segment(
                &mut x,
                Some((icons::HARD_DRIVES, p.text_faint)),
                None,
                &where_,
                xs.clone(),
                p.text_muted,
            );
            return;
        }
        // The right side, laid out from the right edge.
        let indent = match self.indent_unit() {
            "\t" => t("Tabs").to_string(),
            unit => tf("Spaces: {}", &[&unit.len().to_string()]),
        };
        let parts: Vec<(String, FontId)> = vec![
            (
                tf(
                    "{} {}, {} {}",
                    &[
                        t("Ln"),
                        &self.cursor.0.to_string(),
                        t("Col"),
                        &self.cursor.1.to_string(),
                    ],
                ),
                style::mono(11.0),
            ),
            (indent, xs.clone()),
            (
                (if self.crlf { "CRLF" } else { "LF" }).to_string(),
                xs.clone(),
            ),
            (
                (if self.bom { "UTF-8 BOM" } else { "UTF-8" }).to_string(),
                xs.clone(),
            ),
            (Syntax::language(&self.path).to_string(), xs.clone()),
        ];
        let mut rx = foot.right() - 8.0;
        let mut right_x = Vec::new();
        for (text, font) in parts.iter().rev() {
            let g = painter.layout_no_wrap(text.clone(), font.clone(), p.text_muted);
            let w = g.size().x + 12.0;
            rx -= w;
            right_x.push(rx);
            painter.galley(pos2(rx + 6.0, cy - g.size().y / 2.0), g, p.text_muted);
            rx -= 2.0;
        }
        // The host and path in what is left.
        let lead_x = x + 6.0;
        painter.text(
            pos2(lead_x + 6.5, cy),
            Align2::CENTER_CENTER,
            icons::HARD_DRIVES,
            style::icon_font(13.0),
            p.text_faint,
        );
        let tx = lead_x + 13.0 + 6.0;
        let mut job = egui::text::LayoutJob::simple_singleline(where_, xs, p.text_muted);
        job.wrap.max_width = (rx - 8.0 - tx).max(10.0);
        job.wrap.max_rows = 1;
        job.wrap.overflow_character = Some('…');
        let g = painter.layout_job(job);
        painter.galley(pos2(tx, cy - g.size().y / 2.0), g, p.text_muted);
    }

    /// The text with its gutter: line numbers (only the visible ones are drawn), a mark beside
    /// each line changed since the file was opened or saved, and the current line's wash.
    fn text_area(&mut self, ui: &mut Ui, body: Rect, text_id: Id, p: &style::Palette, dark: bool) {
        let font = FontId::new(13.0, egui::FontFamily::Monospace);
        let marks_hash = egui::util::hash((&self.text, &self.saved_text));
        if self.marks.as_ref().is_none_or(|(h, _)| *h != marks_hash) {
            self.marks = Some((marks_hash, line_marks(&self.text, &self.saved_text)));
        }
        let lines = self.text.split('\n').count().max(1);
        let current = self.cursor.0;
        let syntax = Syntax::of(&self.path, dark);
        let mut cache = self.galley_cache.take();
        let marks_owned = self.marks.take();
        let marks: &[u8] = marks_owned.as_ref().map_or(&[], |(_, m)| m.as_slice());
        let mut cursor_at = None;
        let ppp_bits = ui.ctx().pixels_per_point().to_bits();
        let font_gen = super::fonts::generation();
        egui::ScrollArea::both()
            .id_salt(("file-editor-scroll", self.instance_id))
            .auto_shrink([false, false])
            .show_viewport(ui, |ui, viewport| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                ui.visuals_mut().text_cursor.stroke = Stroke::new(2.0, p.accent_text);
                let origin = ui.max_rect().min;
                let content_h = lines as f32 * LINE_H + PAD_Y * 2.0;
                let width = ui.available_width().max(body.width());
                // The current line's wash, behind the gutter and the text.
                let band_y = origin.y + PAD_Y + (current as f32 - 1.0) * LINE_H;
                ui.painter().rect_filled(
                    Rect::from_min_size(
                        pos2(origin.x, band_y),
                        vec2(width.max(body.width()), LINE_H),
                    ),
                    0.0,
                    p.accent_softer,
                );
                ui.horizontal_top(|ui| {
                    let (g_rect, _) = ui.allocate_exact_size(
                        vec2(GUTTER_W, content_h.max(body.height())),
                        Sense::hover(),
                    );
                    // Only the lines in view.
                    let first = (((viewport.min.y - PAD_Y) / LINE_H).floor().max(0.0)) as usize;
                    let last = ((((viewport.max.y - PAD_Y) / LINE_H).ceil()) as usize).min(lines);
                    let painter = ui.painter();
                    let num_font = FontId::new(12.0, egui::FontFamily::Monospace);
                    for n in first..last {
                        let y = g_rect.top() + PAD_Y + n as f32 * LINE_H;
                        let color = if n + 1 == current {
                            p.text_muted
                        } else {
                            p.text_ghost
                        };
                        painter.text(
                            pos2(g_rect.right() - 16.0, y + LINE_H / 2.0 - 2.0),
                            Align2::RIGHT_CENTER,
                            (n + 1).to_string(),
                            num_font.clone(),
                            color,
                        );
                        if let Some(m @ (1 | 2)) = marks.get(n).copied() {
                            painter.rect_filled(
                                Rect::from_min_max(
                                    pos2(g_rect.right() - 9.0, y + 2.0),
                                    pos2(g_rect.right() - 7.0, y + LINE_H - 2.0),
                                ),
                                CornerRadius::same(1),
                                if m == 1 { p.accent } else { p.ok },
                            );
                        }
                    }
                    let fg = syntax.fg;
                    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                        let text = buf.as_str();
                        let key = egui::util::hash((text, dark, ppp_bits, font_gen));
                        if let Some((k, g)) = &cache
                            && *k == key
                        {
                            return g.clone();
                        }
                        let job = syntax.layout(text, &font, LINE_H, fg);
                        let g = ui.fonts_mut(|f| f.layout_job(job));
                        cache = Some((key, g.clone()));
                        g
                    };
                    let output = egui::TextEdit::multiline(&mut self.text)
                        .id(text_id)
                        .font(TextStyle::Monospace)
                        .code_editor()
                        .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                            left: 0,
                            right: 16,
                            top: PAD_Y as i8,
                            bottom: PAD_Y as i8,
                        }))
                        .desired_width(f32::INFINITY)
                        .desired_rows(((body.height() - PAD_Y * 2.0) / LINE_H).max(1.0) as usize)
                        .layouter(&mut layouter)
                        .show(ui);
                    if self.focus {
                        // Start at the top of the file, like any editor.
                        let mut state = output.state.clone();
                        state
                            .cursor
                            .set_char_range(Some(CCursorRange::one(CCursor::new(CharIndex(0)))));
                        state.store(ui.ctx(), text_id);
                        output.response.request_focus();
                        self.focus = false;
                    }
                    if let Some(range) = output.cursor_range {
                        cursor_at = Some(range.primary.index.0);
                    }
                });
            });
        self.galley_cache = cache;
        self.marks = marks_owned;
        let text_hash = egui::util::hash(&self.text);
        if let Some(at) = cursor_at
            && self.cursor_char != Some((at, text_hash))
        {
            self.cursor_char = Some((at, text_hash));
            let before = self
                .text
                .char_indices()
                .nth(at)
                .map_or(self.text.len(), |(b, _)| b);
            let head = &self.text[..before];
            let line = head.matches('\n').count() + 1;
            let col = head.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            self.cursor = (line, col);
        }
    }
}

/// A choice in the conflict banner.
#[derive(Clone, Copy)]
enum Banner {
    Reload,
    Copy,
    Overwrite,
    Close,
}

const HEAD_H: f32 = 44.0;
const FOOT_H: f32 = 28.0;
const LINE_H: f32 = 20.0;
const PAD_Y: f32 = 8.0;
const GUTTER_W: f32 = 56.0;

fn text_w(ui: &Ui, text: &str, font: FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_string(), font, Color32::WHITE)
        .size()
        .x
}

/// A path for the status bar: its last folders when it is long ("…/srv/app/storefront.yml").
fn short_path(path: &str) -> String {
    const MAX: usize = 56;
    if path.chars().count() <= MAX {
        return path.to_string();
    }
    let mut tail = String::new();
    for part in path.rsplit('/').filter(|p| !p.is_empty()) {
        let next = if tail.is_empty() {
            part.to_string()
        } else {
            format!("{part}/{tail}")
        };
        if next.chars().count() > MAX - 2 && !tail.is_empty() {
            break;
        }
        tail = next;
    }
    format!("…/{tail}")
}

/// How wide the small Save button is (Redesign 3.1: no key cap in it).
fn save_width(ui: &Ui, label: &str) -> f32 {
    text_w(ui, label, style::medium(style::SMALL)) + 9.0 * 2.0
}

/// Which lines differ from the saved text: 1 changed, 2 added (0 unchanged). The lines that
/// match at the start and at the end are skipped; the lines in between are compared with a
/// longest-common-subsequence diff, so two edits far apart mark two lines, not everything
/// between them. In each run of differing lines, as many as were removed count as changed and
/// the rest as added. A very large middle (beyond `LCS_CELLS`) is marked as a whole instead.
fn line_marks(text: &str, saved: &str) -> Vec<u8> {
    const LCS_CELLS: usize = 1_000_000;
    let new: Vec<&str> = text.split('\n').collect();
    let old: Vec<&str> = saved.split('\n').collect();
    let prefix = new.iter().zip(&old).take_while(|(a, b)| a == b).count();
    let max_suffix = new.len().min(old.len()) - prefix;
    let suffix = new
        .iter()
        .rev()
        .zip(old.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    let mut marks = vec![0u8; new.len()];
    let nm = &new[prefix..new.len() - suffix];
    let om = &old[prefix..old.len() - suffix];
    let (n, m) = (nm.len(), om.len());
    // One run of differing lines: `added` new lines against `removed` old ones.
    let mut mark_run = |start: usize, added: usize, removed: usize| {
        for k in 0..added {
            marks[prefix + start + k] = if k < removed { 1 } else { 2 };
        }
    };
    if n == 0 {
        return marks;
    }
    if m == 0 || (n + 1) * (m + 1) > LCS_CELLS {
        mark_run(0, n, m);
        return marks;
    }
    // lcs[i][j]: the longest common subsequence of nm[i..] and om[j..].
    let w = m + 1;
    let mut lcs = vec![0u32; (n + 1) * w];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i * w + j] = if nm[i] == om[j] {
                lcs[(i + 1) * w + j + 1] + 1
            } else {
                lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let (mut run_start, mut removed) = (0, 0);
    while i < n && j < m {
        if nm[i] == om[j] {
            mark_run(run_start, i - run_start, removed);
            i += 1;
            j += 1;
            run_start = i;
            removed = 0;
        } else if lcs[(i + 1) * w + j] >= lcs[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
            removed += 1;
        }
    }
    mark_run(run_start, n - run_start, removed + (m - j));
    marks
}

/// The quiet red "Discard" of the discard question.
fn discard_button(ui: &mut Ui) -> egui::Response {
    let p = style::pal(ui);
    let label = t("Discard");
    let g = ui
        .painter()
        .layout_no_wrap(label.to_string(), style::medium(style::BODY), p.err);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(g.size().x + 24.0, style::CONTROL_H), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if resp.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(style::RADIUS),
            p.tone_soft(Tone::Err),
        );
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(style::RADIUS + 2),
            Stroke::new(2.0, p.accent_line),
            egui::StrokeKind::Outside,
        );
    }
    ui.painter()
        .galley(rect.center() - g.size() / 2.0, g, p.err);
    resp
}

/// A dialog's header: the title (16 px) and what it is about (Redesign 3.1: no icon tile).
fn dialog_head(ui: &mut Ui, title: &str, sub: &str) {
    let p = style::pal(ui);
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 20,
            right: 20,
            top: 18,
            bottom: 4,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(title)
                                .font(style::semibold(16.0))
                                .color(p.text),
                        )
                        .wrap(),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(sub)
                                .font(style::regular(style::BODY))
                                .color(p.text_muted),
                        )
                        .wrap(),
                    );
                });
            });
        });
}

/// Light coloring for config files: keys, strings, numbers, booleans and comments, in the
/// terminal palette's colors.
struct Syntax {
    kind: SyntaxKind,
    fg: Color32,
    key: Color32,
    string: Color32,
    number: Color32,
    literal: Color32,
    comment: Color32,
    /// `:`, `=`, list dashes.
    punct: Color32,
}

#[derive(Clone, Copy, PartialEq)]
enum SyntaxKind {
    Yaml,
    Ini,
    Json,
    Shell,
    Plain,
}

impl Syntax {
    fn of(path: &str, dark: bool) -> Self {
        let ext = path
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        let name = remote_name(path).to_ascii_lowercase();
        let kind = match ext.as_str() {
            "yml" | "yaml" => SyntaxKind::Yaml,
            "ini" | "conf" | "cfg" | "toml" | "env" | "properties" => SyntaxKind::Ini,
            "json" => SyntaxKind::Json,
            "sh" | "bash" | "zsh" => SyntaxKind::Shell,
            _ if name.starts_with(".env") || name == "config" => SyntaxKind::Ini,
            _ if name.ends_with("rc") || name == ".profile" => SyntaxKind::Shell,
            _ => SyntaxKind::Plain,
        };
        let c = |d: u32, l: u32| {
            let v = if dark { d } else { l };
            Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
        };
        Self {
            kind,
            // The ANSI colours of the TernaryShell terminal schemes; comments and punctuation
            // in the faint text colour.
            fg: c(0xd6d6d6, 0x1f1f1f),
            key: c(0x6aa6e8, 0x1f5fbf),
            string: c(0x98c379, 0x2f7d32),
            number: c(0xe0be7a, 0x8a6100),
            literal: c(0xc58ad8, 0x8e44ad),
            comment: c(0x737373, 0x8a8a8a),
            punct: c(0x737373, 0x8a8a8a),
        }
    }

    /// The language shown in the status bar.
    fn language(path: &str) -> &'static str {
        let ext = path
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        let name = remote_name(path).to_ascii_lowercase();
        match ext.as_str() {
            "yml" | "yaml" => "YAML",
            "toml" => "TOML",
            "json" => "JSON",
            "ini" | "cfg" | "properties" => "INI",
            "conf" => "Config",
            "env" => "Env",
            "sh" | "bash" | "zsh" => "Shell",
            "py" => "Python",
            "js" | "mjs" | "cjs" => "JavaScript",
            "ts" => "TypeScript",
            "rs" => "Rust",
            "go" => "Go",
            "md" => "Markdown",
            "xml" => "XML",
            "html" => "HTML",
            "css" => "CSS",
            "sql" => "SQL",
            _ if name.starts_with(".env") => "Env",
            _ if name.ends_with("rc") || name == ".profile" => "Shell",
            _ if name == "dockerfile" => "Dockerfile",
            _ => "Plain Text",
        }
    }

    fn layout(&self, text: &str, font: &FontId, line_h: f32, fg: Color32) -> egui::text::LayoutJob {
        let mut job = egui::text::LayoutJob::default();
        job.wrap.max_width = f32::INFINITY;
        let fmt = |color: Color32, italic: bool| egui::TextFormat {
            font_id: font.clone(),
            color,
            italics: italic,
            line_height: Some(line_h),
            ..Default::default()
        };
        if self.kind == SyntaxKind::Plain {
            job.append(text, 0.0, fmt(fg, false));
            return job;
        }
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                job.append("\n", 0.0, fmt(fg, false));
            }
            self.line(&mut job, line, &fmt);
        }
        job
    }

    fn line(
        &self,
        job: &mut egui::text::LayoutJob,
        line: &str,
        fmt: &dyn Fn(Color32, bool) -> egui::TextFormat,
    ) {
        let trimmed = line.trim_start();
        let comment_start = match self.kind {
            SyntaxKind::Json => None,
            SyntaxKind::Ini => (trimmed.starts_with('#') || trimmed.starts_with(';'))
                .then(|| line.len() - trimmed.len()),
            _ => find_comment(line),
        };
        let (code, comment) = match comment_start {
            Some(at) => (&line[..at], &line[at..]),
            None => (line, ""),
        };
        // A key before ':' (YAML, JSON) or '=' (INI, shell).
        let sep = match self.kind {
            SyntaxKind::Yaml | SyntaxKind::Json => code
                .find(": ")
                .or_else(|| code.strip_suffix(':').map(|c| c.len())),
            SyntaxKind::Ini | SyntaxKind::Shell => code.find('='),
            SyntaxKind::Plain => None,
        };
        let key_ok = |k: &str| {
            let k = k
                .trim()
                .trim_start_matches("- ")
                .trim_start_matches("export ");
            !k.is_empty() && !k.contains(' ') || (k.starts_with('"') && k.ends_with('"'))
        };
        match sep {
            Some(at) if key_ok(&code[..at]) => {
                let (k, rest) = code.split_at(at);
                let lead = k.len() - k.trim_start().len();
                job.append(&k[..lead], 0.0, fmt(self.fg, false));
                let key = &k[lead..];
                let dash = if key.starts_with("- ") { 2 } else { 0 };
                job.append(&key[..dash], 0.0, fmt(self.punct, false));
                job.append(&key[dash..], 0.0, fmt(self.key, false));
                let sep_len = rest.chars().next().map_or(0, |c| c.len_utf8());
                job.append(&rest[..sep_len], 0.0, fmt(self.punct, false));
                self.value(job, &rest[sep_len..], fmt);
            }
            _ if self.kind == SyntaxKind::Ini && trimmed.starts_with('[') => {
                job.append(code, 0.0, fmt(self.literal, false));
            }
            _ => self.value(job, code, fmt),
        }
        if !comment.is_empty() {
            job.append(comment, 0.0, fmt(self.comment, true));
        }
    }

    fn value(
        &self,
        job: &mut egui::text::LayoutJob,
        value: &str,
        fmt: &dyn Fn(Color32, bool) -> egui::TextFormat,
    ) {
        let lead_len = value.len() - value.trim_start().len();
        let (lead, v) = value.split_at(lead_len);
        job.append(lead, 0.0, fmt(self.fg, false));
        let v_trim = v.trim_end().trim_end_matches(',');
        let rest = &v[v_trim.len()..];
        let bare = v_trim.trim_start_matches("- ");
        let dash = &v_trim[..v_trim.len() - bare.len()];
        job.append(dash, 0.0, fmt(self.punct, false));
        let quoted = bare.len() >= 2
            && ((bare.starts_with('"') && bare.ends_with('"'))
                || (bare.starts_with('\'') && bare.ends_with('\'')));
        let color = if bare.is_empty() || matches!(bare, "{" | "[" | "|" | ">" | "}" | "]") {
            self.fg
        } else if matches!(
            bare.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
        ) {
            self.literal
        } else if bare.parse::<f64>().is_ok() {
            self.number
        } else if quoted || self.kind == SyntaxKind::Json {
            self.string
        } else {
            self.fg
        };
        job.append(bare, 0.0, fmt(color, false));
        job.append(rest, 0.0, fmt(self.fg, false));
    }
}

/// Where a `#` comment starts (not inside quotes, and at a word start).
fn find_comment(line: &str) -> Option<usize> {
    let mut quote = None;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '#') if prev.is_whitespace() || i == 0 => return Some(i),
            _ => {}
        }
        prev = c;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::line_marks;

    #[test]
    fn marks_only_the_lines_that_changed() {
        let saved = "a\nb\nc\nd\ne\nf";
        assert_eq!(line_marks(saved, saved), vec![0; 6]);
        // Two edits far apart: two lines, not everything between them.
        assert_eq!(
            line_marks("a\nB\nc\nd\nE\nf", saved),
            vec![0, 1, 0, 0, 1, 0]
        );
        // An added line in the middle.
        assert_eq!(
            line_marks("a\nb\nx\nc\nd\ne\nf", saved),
            vec![0, 0, 2, 0, 0, 0, 0]
        );
        // A removed line marks nothing that is left, a replaced pair plus one more.
        assert_eq!(line_marks("a\nb\nd\ne\nf", saved), vec![0; 5]);
        assert_eq!(
            line_marks("a\nX\nY\nZ\nd\ne\nf", saved),
            vec![0, 1, 1, 2, 0, 0, 0]
        );
    }
}
