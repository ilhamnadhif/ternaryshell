//! Commands from the menu bar, keyboard shortcuts and buttons all go through `Action`.

use uuid::Uuid;

use crate::bookmarks::transfer::ExportScope;

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    // bookmarks
    NewBookmark {
        group: Option<Uuid>,
    },
    EditBookmark(Uuid),
    DuplicateBookmark(Uuid),
    DeleteBookmark(Uuid),
    Connect(Uuid),
    NewGroup {
        parent: Option<Uuid>,
    },
    RenameGroup(Uuid),
    DeleteGroup(Uuid),
    QuickConnect,
    /// Opens the connection editor for an address typed in the palette.
    SaveAddress(String),
    CollapseAllGroups,
    /// Puts the keyboard in the sidebar's filter field.
    FocusSidebarSearch,
    // tabs and panes
    NewTab,
    /// Tabs are referred to by their id: indices shift when tabs close or move.
    DuplicateTab(u64),
    CloseTabOrPane,
    CloseTab(u64),
    CloseOtherTabs(u64),
    /// Stops a connection that is still being made.
    CancelConnect(u64),
    SelectTab(usize),
    NextTab,
    PrevTab,
    MoveTab {
        from: u64,
        to: u64,
    },
    Reconnect,
    /// Enter in an ended pane: a new shell in that pane (the whole tab reconnects only when
    /// the connection itself is gone).
    RestartPane(u64),
    SplitRight,
    SplitDown,
    NextPane,
    PrevPane,
    // view
    ToggleSidebar,
    ToggleSftp,
    /// Light ⇄ dark.
    ToggleTheme,
    FontBigger,
    FontSmaller,
    FontReset,
    /// Windows and Linux (macOS has it in the menu bar's View menu).
    ToggleFullScreen,
    // edit
    Copy,
    Paste,
    Cut,
    SelectAll,
    ClearScrollback,
    SendQuickCommand(Uuid),
    /// The n-th quick command (⌃1–⌃9).
    SendQuickCommandAt(usize),
    // import / export
    Import,
    ImportSshConfig,
    Export(ExportScope),
    // app
    Settings,
    About,
    CheckForUpdates,
    Shortcuts,
    Quit,
}
