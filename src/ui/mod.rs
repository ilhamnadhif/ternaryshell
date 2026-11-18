//! egui user interface.

pub mod about;
pub mod actions;
pub mod confirm;
pub mod dialogs;
pub mod editor;
pub mod file_editor;
pub mod fonts;
pub mod forms;
pub mod icon_codes;
pub mod icons;
pub mod keys;
pub mod kit;
pub mod layout;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod menu;
pub mod sftp_panel;
pub mod sidebar;
pub mod style;
pub mod tabs;
pub mod terminal;
pub mod toasts;
pub mod update_ui;
