//! A small editor for remote text files (configs, scripts): opens over SFTP, saves back with
//! ⌘S, keeps the file's line endings, and warns when the file changed on the server meanwhile.

use std::sync::mpsc;

use eframe::egui::text::{CCursor, CCursorRange, CharIndex};
use eframe::egui::text_edit::TextEditState;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Event, FontId, Id, Key, Modifiers, RichText, Sense,
    Stroke, TextStyle, pos2, vec2,
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
enum Confirm {
    /// Close with unsaved changes?
    Discard,
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
    Info(String),
    Error(String),
}

pub struct FileEditor {
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
    sftp: Option<Sftp>,
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
            sftp: None,
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
        self.pending = Some(rx);
        self.state = State::Loading;
        self.status = None;
        self.confirm = None;
    }

    pub fn name(&self) -> &str {
        remote_name(&self.path)
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self.state, State::Ready) && self.text != self.saved_text
    }

    /// Closes now if nothing would be lost; otherwise asks first. Returns true when closed.
    pub fn request_close(&mut self) -> bool {
        if self.is_dirty() {
            self.confirm = Some(Confirm::Discard);
            false
        } else {
            true
        }
    }

    fn poll(&mut self) {
        let Some(rx) = &self.pending else { return };
        let Ok(msg) = rx.try_recv() else { return };
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
                        self.status = Some(Status::Saved);
                    }
                    Err(SaveError::Changed) => {
                        self.status = None;
                        self.confirm = Some(Confirm::Overwrite);
                    }
                    Err(SaveError::Deleted) => {
                        self.status = None;
                        self.confirm = Some(Confirm::Recreate);
                    }
                    // The text stays in the editor, so nothing typed is lost.
                    Err(SaveError::Failed(e)) => {
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
            Ok(()) => Status::Info(tf("Saved a copy to {}.", &[&dest.display().to_string()])),
            Err(e) => Status::Error(tf("Could not save: {}", &[&e.to_string()])),
        });
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        rt: &tokio::runtime::Handle,
        sftp: Option<Sftp>,
    ) -> EditorOutput {
        self.poll();
        let _ = &self.sftp;
        let mut out = EditorOutput::Open;
        let screen = ctx.content_rect();
        let size = vec2(
            (screen.width() - 80.0).clamp(420.0, 1060.0),
            (screen.height() - 120.0).clamp(260.0, 640.0),
        );
        let text_id = Id::new(("file-editor-text", &self.path));
        let dirty = self.is_dirty();
        let mut save_clicked = false;
        let mut close_clicked = false;
        let mut copy_clicked = false;
        let mut reload_clicked = false;

        if self.confirm.is_none() {
            let (save_key, escape) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::COMMAND, Key::S),
                    i.key_pressed(Key::Escape),
                )
            });
            save_clicked |= save_key;
            close_clicked |= escape;
            if matches!(self.state, State::Ready) {
                self.handle_keys(ctx, text_id);
            }
        }
        if self.close_after_save && !self.is_dirty() && matches!(self.status, Some(Status::Saved)) {
            return EditorOutput::Close { saved: true };
        }

        let dark = ctx.global_style().visuals.dark_mode;
        let syntax = Syntax::of(&self.path, dark);
        style::modal(ctx, Id::new(("file-editor", &self.path))).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.set_height(size.y);
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            let p = style::pal(ui);
            let full = ui.max_rect();
            // Header (48 px): what file, on which server, where; then the actions.
            let head = egui::Rect::from_min_size(full.min, vec2(full.width(), 48.0));
            ui.painter().hline(
                head.x_range(),
                head.bottom() - 0.5,
                Stroke::new(1.0, p.border),
            );
            let mut h = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        pos2(head.left() + 16.0, head.top()),
                        pos2(head.right() - 12.0, head.bottom()),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            h.spacing_mut().item_spacing.x = 10.0;
            let (icon, _) = icons::file_icon(self.name(), false, false, &p);
            kit::icon(&mut h, icon, 16.0, p.text_muted);
            h.label(
                RichText::new(self.name())
                    .font(style::semibold(style::BODY))
                    .color(p.text),
            );
            if self.is_dirty() {
                let (r, _) = h.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());
                h.painter().circle_filled(r.center(), 3.5, p.accent);
            }
            if !self.host.is_empty() {
                kit::chip(&mut h, Some(icons::HARD_DRIVES), &self.host, false);
            }
            let path_w = (h.available_width() - 200.0).max(40.0);
            h.allocate_ui_with_layout(
                vec2(path_w, 20.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_max_width(path_w);
                    ui.add(
                        egui::Label::new(
                            RichText::new(&self.path)
                                .font(style::mono(11.5))
                                .color(p.text_faint),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&self.path);
                },
            );
            h.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if kit::icon_button(ui, icons::X, t("Close")).clicked() {
                    close_clicked = true;
                }
                let saving = self.saving_text.is_some();
                let can_save = matches!(self.state, State::Ready) && !saving;
                let save = ui
                    .add_enabled_ui(can_save, |ui| {
                        kit::button_kbd(
                            ui,
                            kit::Kind::Primary,
                            Some(icons::FLOPPY_DISK),
                            t("Save"),
                            Some(&super::keys::Shortcut::Save.label()),
                            true,
                            false,
                        )
                    })
                    .inner;
                if save.clicked() {
                    save_clicked = true;
                }
                if saving {
                    kit::spinner(ui, 14.0);
                }
                let more = kit::icon_button(ui, icons::DOTS_THREE, t("More"));
                let ready = matches!(self.state, State::Ready);
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
                    if kit::menu_item(
                        ui,
                        Some(icons::ARROWS_CLOCKWISE),
                        t("Reload from Server"),
                        None,
                    )
                    .clicked()
                    {
                        reload_clicked = true;
                        ui.close();
                    }
                    if kit::menu_item(ui, Some(icons::DOWNLOAD_SIMPLE), t("Download"), None)
                        .clicked()
                    {
                        out = EditorOutput::Download;
                        ui.close();
                    }
                });
            });

            // Body.
            let foot_h = 28.0;
            let body = egui::Rect::from_min_max(
                pos2(full.left(), head.bottom()),
                pos2(full.right(), full.bottom() - foot_h),
            );
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
                    let can_download = matches!(e, LoadError::TooLarge(_) | LoadError::NotText);
                    let message = e.message();
                    let icon = if can_download {
                        icons::FILE_ZIP
                    } else {
                        icons::WARNING_CIRCLE
                    };
                    b.add_space((body.height() * 0.22).max(8.0));
                    let title = tf("{} can't be edited here", &[self.name()]);
                    kit::empty_state(&mut b, icon, &title, &message, |ui| {
                        if can_download
                            && kit::secondary_sm(ui, Some(icons::DOWNLOAD_SIMPLE), t("Download"))
                                .clicked()
                        {
                            out = EditorOutput::Download;
                        }
                    });
                }
                State::Ready => {
                    let line_h = 20.0;
                    let font = FontId::new(13.0, egui::FontFamily::Monospace);
                    let lines = self.text.split('\n').count();
                    let current = self.cursor.0;
                    // The gutter: line numbers, the current one brighter.
                    let gutter_w = 52.0f32.max(16.0 + 8.0 * lines.to_string().len() as f32 + 12.0);
                    egui::ScrollArea::both()
                        .id_salt(("file-editor-scroll", &self.path))
                        .auto_shrink([false, false])
                        .show(&mut b, |ui| {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let mut job = egui::text::LayoutJob::default();
                                for n in 1..=lines {
                                    let color = if n == current { p.text } else { p.text_faint };
                                    job.append(
                                        &format!("{n}\n"),
                                        0.0,
                                        egui::TextFormat {
                                            font_id: font.clone(),
                                            color,
                                            line_height: Some(line_h),
                                            ..Default::default()
                                        },
                                    );
                                }
                                job.halign = egui::Align::RIGHT;
                                let galley = ui.fonts_mut(|f| f.layout_job(job));
                                let (g_rect, _) = ui.allocate_exact_size(
                                    vec2(
                                        gutter_w,
                                        galley.size().y.max(body.height() - 20.0) + 20.0,
                                    ),
                                    Sense::hover(),
                                );
                                ui.painter().galley(
                                    pos2(g_rect.right() - 12.0, g_rect.top() + 10.0),
                                    galley,
                                    p.text_faint,
                                );
                                ui.painter().vline(
                                    g_rect.right() - 0.5,
                                    g_rect.y_range(),
                                    Stroke::new(1.0, p.line_soft),
                                );
                                // The text, with light coloring for configs.
                                let fg = syntax.fg;
                                let mut layouter =
                                    |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                                        let job = syntax.layout(buf.as_str(), &font, line_h, fg);
                                        ui.fonts_mut(|f| f.layout_job(job))
                                    };
                                let text_left = ui.cursor().left();
                                let output = egui::TextEdit::multiline(&mut self.text)
                                    .id(text_id)
                                    .font(TextStyle::Monospace)
                                    .code_editor()
                                    .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                                        left: 14,
                                        right: 14,
                                        top: 10,
                                        bottom: 10,
                                    }))
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(((body.height() - 20.0) / line_h) as usize)
                                    .layouter(&mut layouter)
                                    .show(ui);
                                if self.focus {
                                    // Start at the top of the file, like any editor.
                                    let mut state = output.state.clone();
                                    state.cursor.set_char_range(Some(CCursorRange::one(
                                        CCursor::new(CharIndex(0)),
                                    )));
                                    state.store(ui.ctx(), text_id);
                                    output.response.request_focus();
                                    self.focus = false;
                                }
                                if let Some(range) = output.cursor_range {
                                    let at = range.primary.index.0;
                                    let before: String = self.text.chars().take(at).collect();
                                    let line = before.matches('\n').count() + 1;
                                    let col =
                                        before.rsplit('\n').next().map_or(0, |l| l.chars().count())
                                            + 1;
                                    self.cursor = (line, col);
                                }
                                // The current line: a faint wash and an accent edge.
                                let y = output.galley_pos.y + (current as f32 - 1.0) * line_h;
                                let band = egui::Rect::from_min_max(
                                    pos2(text_left, y),
                                    pos2(
                                        output.response.rect.right().max(body.right()),
                                        y + line_h,
                                    ),
                                );
                                ui.painter().rect_filled(band, 0.0, p.accent_softer);
                                ui.painter().vline(
                                    text_left + 1.0,
                                    band.y_range(),
                                    Stroke::new(2.0, p.accent),
                                );
                            });
                        });
                }
            }

            // Status bar (28 px): position, indentation, line endings, encoding, mode, state.
            let foot =
                egui::Rect::from_min_max(pos2(full.left(), full.bottom() - foot_h), full.max);
            ui.painter()
                .hline(foot.x_range(), foot.top() + 0.5, Stroke::new(1.0, p.border));
            let mut f = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        pos2(foot.left() + 14.0, foot.top()),
                        pos2(foot.right() - 10.0, foot.bottom()),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            f.spacing_mut().item_spacing.x = 4.0;
            if matches!(self.state, State::Ready) {
                let indent = match self.indent_unit() {
                    "\t" => t("Tabs").to_string(),
                    unit => tf("Spaces: {}", &[&unit.len().to_string()]),
                };
                let mut parts = vec![
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
                        false,
                    ),
                    (indent, false),
                    ((if self.crlf { "CRLF" } else { "LF" }).to_string(), false),
                    (
                        (if self.bom { "UTF-8 BOM" } else { "UTF-8" }).to_string(),
                        false,
                    ),
                ];
                if let Some(m) = self.mode {
                    parts.push((crate::sftp::entry::mode_string(m), true));
                }
                let n = parts.len();
                for (i, (text, mono)) in parts.into_iter().enumerate() {
                    let font = if mono {
                        style::mono(11.0)
                    } else {
                        style::regular(style::XS)
                    };
                    let g = f.painter().layout_no_wrap(text, font, p.text_muted);
                    let (r, resp) =
                        f.allocate_exact_size(vec2(g.size().x + 12.0, 20.0), Sense::hover());
                    if resp.hovered() {
                        f.painter().rect_filled(r, CornerRadius::same(4), p.hover);
                    }
                    f.painter()
                        .galley(r.center() - g.size() / 2.0, g, p.text_muted);
                    if i + 1 < n {
                        f.label(
                            RichText::new("·")
                                .font(style::regular(style::XS))
                                .color(p.text_faint),
                        );
                    }
                }
            }
            let status = match &self.status {
                Some(Status::Saving) => Some((t("Saving…").to_string(), p.text_muted)),
                Some(Status::Saved) => Some((t("Saved").to_string(), p.ok)),
                Some(Status::Info(m)) => Some((m.clone(), p.text_muted)),
                Some(Status::Error(m)) => Some((m.clone(), p.err)),
                None if dirty => Some((t("Unsaved changes").to_string(), p.accent_text)),
                None => None,
            };
            if let Some((msg, color)) = status {
                f.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(msg)
                                .font(style::regular(style::XS))
                                .color(color),
                        )
                        .truncate(),
                    );
                });
            }
        });

        if copy_clicked {
            self.save_copy();
        }
        if reload_clicked {
            self.reload(rt, ctx, sftp.clone());
        }
        if save_clicked {
            if dirty {
                self.save(rt, ctx, sftp.clone(), false);
            } else if matches!(self.state, State::Ready) {
                self.status = Some(Status::Info(t("No changes to save.").into()));
            }
        }
        // "Saved" only while it is true.
        if self.is_dirty() && matches!(self.status, Some(Status::Saved | Status::Info(_))) {
            self.status = None;
        }

        match self.confirm {
            Some(Confirm::Discard) => {
                let choice = std::cell::Cell::new(None::<u8>);
                let host = if self.host.is_empty() {
                    t("the server").to_string()
                } else {
                    self.host.clone()
                };
                style::modal(ctx, Id::new(("file-editor-discard", &self.path))).show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    kit::dialog_header(
                        ui,
                        Some((icons::PENCIL_SIMPLE, Tone::Warn)),
                        &tf("Discard changes to {}?", &[self.name()]),
                        Some(&tf("Your edits haven't been saved to {}.", &[&host])),
                        false,
                    );
                    ui.add_space(12.0);
                    kit::dialog_footer(
                        ui,
                        |ui| {
                            if kit::danger_quiet(ui, t("Discard")).clicked() {
                                choice.set(Some(1));
                            }
                        },
                        |ui| {
                            if kit::primary(ui, t("Save")).clicked() {
                                choice.set(Some(2));
                            }
                            if kit::secondary(ui, None, t("Cancel")).clicked() {
                                choice.set(Some(0));
                            }
                        },
                    );
                });
                if ctx.input(|i| i.key_pressed(Key::Escape)) {
                    choice.set(Some(0));
                }
                match choice.get() {
                    Some(1) => {
                        return EditorOutput::Close {
                            saved: self.saved_once,
                        };
                    }
                    Some(2) => {
                        self.confirm = None;
                        self.close_after_save = true;
                        self.save(rt, ctx, sftp, false);
                    }
                    Some(_) => self.confirm = None,
                    None => {}
                }
            }
            Some(confirm @ (Confirm::Overwrite | Confirm::Recreate)) => {
                let choice = std::cell::Cell::new(None::<u8>);
                let (icon, title, sub, body, button) = if confirm == Confirm::Overwrite {
                    (
                        icons::ARROWS_CLOCKWISE,
                        tf("{} changed on the server", &[self.name()]),
                        t("Someone saved it after you opened it.").to_string(),
                        t("Saving now replaces their version with yours."),
                        t("Overwrite"),
                    )
                } else {
                    (
                        icons::WARNING,
                        tf("{} was deleted on the server", &[self.name()]),
                        t("It was removed after you opened it.").to_string(),
                        t("Saving creates it again."),
                        t("Save"),
                    )
                };
                style::modal(ctx, Id::new(("file-editor-overwrite", &self.path))).show(ctx, |ui| {
                    ui.set_width(460.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    kit::dialog_header(ui, Some((icon, Tone::Warn)), &title, Some(&sub), false);
                    kit::dialog_body(ui, |ui| {
                        ui.label(body);
                    });
                    kit::dialog_footer(
                        ui,
                        |ui| {
                            if confirm == Confirm::Overwrite
                                && kit::ghost(ui, None, t("Reload theirs")).clicked()
                            {
                                choice.set(Some(2));
                            }
                        },
                        |ui| {
                            let ok = if confirm == Confirm::Overwrite {
                                kit::danger(ui, button)
                            } else {
                                kit::primary(ui, button)
                            };
                            if ok.clicked() {
                                choice.set(Some(1));
                            }
                            if kit::secondary(ui, None, t("Cancel")).clicked() {
                                choice.set(Some(0));
                            }
                        },
                    );
                });
                if ctx.input(|i| i.key_pressed(Key::Escape)) {
                    choice.set(Some(0));
                }
                match choice.get() {
                    Some(1) => {
                        self.confirm = None;
                        self.save(rt, ctx, sftp, true);
                    }
                    Some(2) => self.reload(rt, ctx, sftp),
                    Some(_) => self.confirm = None,
                    None => {}
                }
            }
            None => {
                if close_clicked && self.request_close() {
                    return EditorOutput::Close {
                        saved: self.saved_once,
                    };
                }
            }
        }
        out
    }
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
            fg: c(0xd6d8de, 0x2b2d35),
            key: c(0x6cb6f5, 0x2b62d0),
            string: c(0x9fd07f, 0x2f8a3a),
            number: c(0xecc77d, 0x9a6a00),
            literal: c(0xc993f0, 0x8a3fc0),
            comment: c(0x6b7080, 0x8a8e9c),
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
                job.append(&k[lead..], 0.0, fmt(self.key, false));
                let sep_len = rest.chars().next().map_or(0, |c| c.len_utf8());
                job.append(&rest[..sep_len], 0.0, fmt(self.fg, false));
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
        job.append(dash, 0.0, fmt(self.fg, false));
        let color = if bare.is_empty() || matches!(bare, "{" | "[" | "|" | ">" | "}" | "]") {
            self.fg
        } else if matches!(
            bare.to_ascii_lowercase().as_str(),
            "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
        ) {
            self.literal
        } else if bare.parse::<f64>().is_ok() {
            self.number
        } else {
            self.string
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
