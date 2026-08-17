//! Top menu bar (File / Edit / Help), copied in spirit from
//! seqterm's `menu.rs` and adapted to choz's actual actions. Fully keyboard- and
//! mouse-driven; every item maps to a [`MenuAction`] the app dispatches.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    File,
    Edit,
    Help,
}

impl MenuKind {
    pub const ALL: &'static [MenuKind] = &[MenuKind::File, MenuKind::Edit, MenuKind::Help];

    /// Space-padded bar label, e.g. " FILE " — translated.
    pub fn label(self) -> String {
        let key = match self {
            MenuKind::File => "FILE",
            MenuKind::Edit => "EDIT",
            MenuKind::Help => "HELP",
        };
        format!(" {} ", crate::i18n::t(key))
    }

    pub fn items(self) -> &'static [MenuItem] {
        match self {
            MenuKind::File => FILE_MENU,
            MenuKind::Edit => EDIT_MENU,
            MenuKind::Help => HELP_MENU,
        }
    }

    /// Widest item label — used to size the dropdown.
    ///
    /// Measured on the **translated** label, in characters: "Import Max
    /// patch…" is shorter than "Importer un patch Max…", and sizing from the
    /// English would clip the French. Characters, not bytes, or every accent
    /// would widen the box by one.
    pub fn width(self) -> u16 {
        let w = self
            .items()
            .iter()
            .map(|i| crate::i18n::t(i.label).chars().count() + i.shortcut.len() + 4)
            .max()
            .unwrap_or(10);
        w as u16
    }
}

pub struct MenuItem {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub action: MenuAction,
    pub separator: bool,
}

impl MenuItem {
    const fn item(label: &'static str, shortcut: &'static str, action: MenuAction) -> Self {
        Self {
            label,
            shortcut,
            action,
            separator: false,
        }
    }
    const fn sep() -> Self {
        Self {
            label: "",
            shortcut: "",
            action: MenuAction::None,
            separator: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    None,
    OpenWav,
    OpenSf2,
    Quit,
    PluginPaths,
    RescanPlugins,
    SaveProject,
    LoadProject,
    ImportMax,
    About,
}

static FILE_MENU: &[MenuItem] = &[
    MenuItem::item("Open WAV\u{2026}", "", MenuAction::OpenWav),
    MenuItem::item("Open SF2\u{2026}", "", MenuAction::OpenSf2),
    MenuItem::sep(),
    MenuItem::item("Save project\u{2026}", "", MenuAction::SaveProject),
    MenuItem::item("Open project\u{2026}", "", MenuAction::LoadProject),
    MenuItem::sep(),
    MenuItem::item("Import Max patch\u{2026}", "", MenuAction::ImportMax),
    MenuItem::sep(),
    MenuItem::item("Quit", "q", MenuAction::Quit),
];

static EDIT_MENU: &[MenuItem] = &[
    MenuItem::item("Settings\u{2026}", "", MenuAction::PluginPaths),
    MenuItem::item("Rescan plugin paths", "", MenuAction::RescanPlugins),
];

static HELP_MENU: &[MenuItem] = &[MenuItem::item("About choz", "", MenuAction::About)];

/// Open-menu state: which menu and the highlighted item (real item index,
/// separators skipped during navigation).
#[derive(Clone, Copy)]
pub struct MenuState {
    pub kind: MenuKind,
    pub cursor: usize,
}

impl MenuState {
    pub fn open(kind: MenuKind) -> Self {
        let mut s = Self { kind, cursor: 0 };
        s.cursor = s.first_selectable();
        s
    }

    fn first_selectable(&self) -> usize {
        self.kind
            .items()
            .iter()
            .position(|i| !i.separator)
            .unwrap_or(0)
    }

    pub fn move_down(&mut self) {
        let items = self.kind.items();
        let mut i = self.cursor;
        for _ in 0..items.len() {
            i = (i + 1) % items.len();
            if !items[i].separator {
                self.cursor = i;
                return;
            }
        }
    }

    pub fn move_up(&mut self) {
        let items = self.kind.items();
        let n = items.len();
        let mut i = self.cursor;
        for _ in 0..n {
            i = (i + n - 1) % n;
            if !items[i].separator {
                self.cursor = i;
                return;
            }
        }
    }

    /// Switch to the previous/next top-level menu, keeping it open.
    pub fn cycle_menu(&mut self, forward: bool) {
        let all = MenuKind::ALL;
        let idx = all.iter().position(|k| *k == self.kind).unwrap_or(0);
        let n = all.len();
        let next = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        *self = MenuState::open(all[next]);
    }

    pub fn current_action(&self) -> MenuAction {
        self.kind
            .items()
            .get(self.cursor)
            .map(|i| i.action)
            .unwrap_or(MenuAction::None)
    }
}
