//! The key table one screen binds: a key press to the action it produces.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::Action;

/// One key a table binds, with the modifiers it needs and the spelling its hint shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Key {
    code: KeyCode,
    modifiers: KeyModifiers,
    spelling: &'static str,
}

impl Key {
    /// A character key: `spelling` is what the hint shows for it.
    const fn character(character: char, spelling: &'static str) -> Self {
        Self {
            code: KeyCode::Char(character),
            modifiers: KeyModifiers::NONE,
            spelling,
        }
    }

    /// A control character, such as `Ctrl-D`.
    const fn control(character: char, spelling: &'static str) -> Self {
        Self {
            code: KeyCode::Char(character),
            modifiers: KeyModifiers::CONTROL,
            spelling,
        }
    }

    /// A named key, such as an arrow or Enter.
    const fn named(code: KeyCode, spelling: &'static str) -> Self {
        Self {
            code,
            modifiers: KeyModifiers::NONE,
            spelling,
        }
    }

    /// The spelling a hint shows for this key.
    pub(crate) fn spelling(self) -> &'static str {
        self.spelling
    }

    /// Whether the hint row spells this key among a group's keys: a control chord is left to the
    /// plain key a reader reaches for, and an arrow is spelled only when the group holds no other
    /// key.
    pub(crate) fn spelled_in_hints(self) -> bool {
        !matches!(
            self.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
        ) && !self.modifiers.contains(KeyModifiers::CONTROL)
    }
}

/// One binding: the keys that produce an action, and the word a hint names it by.
pub(crate) struct Binding {
    pub(crate) keys: &'static [Key],
    /// The word the hint names the action by.
    pub(crate) label: &'static str,
    pub(crate) action: Action,
}

/// The keys one screen binds, in the order its hints show them.
pub(crate) struct Table {
    bindings: &'static [Binding],
}

impl Table {
    /// The action one key press produces; `None` when this table binds no such key.
    pub(crate) fn action(&self, key: &KeyEvent) -> Option<Action> {
        // Shift spells the same key: `G` and `g` are two bindings, not two modifiers.
        let modifiers = key.modifiers.difference(KeyModifiers::SHIFT);
        self.bindings
            .iter()
            .find(|binding| {
                binding
                    .keys
                    .iter()
                    .any(|bound| bound.code == key.code && bound.modifiers == modifiers)
            })
            .map(|binding| binding.action)
    }

    /// The hint of every binding whose action `actions` accepts, in table order.
    pub(crate) fn hints(
        &self,
        actions: &[Action],
    ) -> impl Iterator<Item = (&'static [Key], &'static str)> {
        self.bindings
            .iter()
            .filter(move |binding| actions.contains(&binding.action))
            .map(|binding| (binding.keys, binding.label))
    }

    /// The spelling of the first key bound to `action`, when the table binds it.
    pub(crate) fn key_spelling(&self, action: Action) -> Option<&'static str> {
        self.bindings
            .iter()
            .find(|binding| binding.action == action)
            .and_then(|binding| binding.keys.first())
            .map(|key| key.spelling())
    }
}

pub(crate) const TEXT_INPUT: Table = Table {
    bindings: &[
        Binding {
            keys: &[Key::named(KeyCode::Enter, "Enter")],
            label: "apply",
            action: Action::Open,
        },
        Binding {
            keys: &[Key::named(KeyCode::Esc, "Esc")],
            label: "cancel",
            action: Action::Dismiss,
        },
    ],
};

pub(crate) const HELP_JUMP: Table = Table {
    bindings: &[Binding {
        keys: &[
            Key::named(KeyCode::Esc, "Esc"),
            Key::named(KeyCode::Tab, "Tab"),
        ],
        label: "cancel",
        action: Action::Dismiss,
    }],
};

/// The keys every screen shares: moving, paging, searching, and leaving.
pub(crate) const DEFAULT: Table = Table {
    bindings: &[
        MOVE_DOWN,
        MOVE_UP,
        Binding {
            keys: &[Key::named(KeyCode::PageUp, "PgUp")],
            label: "page",
            action: Action::PageUp,
        },
        Binding {
            keys: &[
                Key::named(KeyCode::PageDown, "PgDn"),
                Key::character(' ', "Space"),
            ],
            label: "page",
            action: Action::PageDown,
        },
        Binding {
            keys: &[Key::character('g', "g"), Key::named(KeyCode::Home, "Home")],
            label: "first",
            action: Action::First,
        },
        Binding {
            keys: &[Key::character('G', "G"), Key::named(KeyCode::End, "End")],
            label: "last",
            action: Action::Last,
        },
        Binding {
            keys: &[Key::character('/', "/")],
            label: "search",
            action: Action::Search,
        },
        Binding {
            keys: &[Key::character('n', "n")],
            label: "next hit",
            action: Action::NextMatch,
        },
        Binding {
            keys: &[Key::character('N', "N")],
            label: "previous hit",
            action: Action::PreviousMatch,
        },
        Binding {
            keys: &[Key::named(KeyCode::Enter, "Enter")],
            label: "open",
            action: Action::Open,
        },
        Binding {
            keys: &[Key::character('e', "e")],
            label: "export",
            action: Action::Export,
        },
        Binding {
            keys: &[Key::named(KeyCode::Tab, "Tab")],
            label: "next tab",
            action: Action::NextTab,
        },
        Binding {
            keys: &[Key::named(KeyCode::BackTab, "Shift-Tab")],
            label: "previous tab",
            action: Action::PreviousTab,
        },
        PRESET_1,
        PRESET_2,
        PRESET_3,
        Binding {
            keys: &[Key::character('q', "q")],
            label: "quit",
            action: Action::Quit,
        },
        Binding {
            keys: &[Key::named(KeyCode::Esc, "Esc")],
            label: "back",
            action: Action::Dismiss,
        },
    ],
};

/// Sections and screenfuls are separate movements in the help reader.
pub(crate) const HELP: Table = Table {
    bindings: &[
        Binding {
            keys: &[Key::named(KeyCode::Down, "↓")],
            label: "scroll",
            action: Action::Down,
        },
        Binding {
            keys: &[Key::named(KeyCode::Up, "↑")],
            label: "scroll",
            action: Action::Up,
        },
        Binding {
            keys: &[Key::character('f', "f")],
            label: "section",
            action: Action::NextSection,
        },
        Binding {
            keys: &[Key::character('b', "b")],
            label: "section",
            action: Action::PreviousSection,
        },
        Binding {
            keys: &[
                Key::character(' ', "Space"),
                Key::named(KeyCode::PageDown, "PgDn"),
            ],
            label: "page down",
            action: Action::PageDown,
        },
        Binding {
            keys: &[Key::named(KeyCode::PageUp, "PgUp")],
            label: "page up",
            action: Action::PageUp,
        },
        Binding {
            keys: &[Key::character('d', "d"), Key::control('d', "Ctrl-D")],
            label: "half page down",
            action: Action::HalfPageDown,
        },
        Binding {
            keys: &[Key::character('u', "u"), Key::control('u', "Ctrl-U")],
            label: "half page up",
            action: Action::HalfPageUp,
        },
        Binding {
            keys: &[Key::character('g', "g"), Key::named(KeyCode::Home, "Home")],
            label: "first",
            action: Action::First,
        },
        Binding {
            keys: &[Key::character('G', "G"), Key::named(KeyCode::End, "End")],
            label: "last",
            action: Action::Last,
        },
        Binding {
            keys: &[Key::character('/', "/")],
            label: "search",
            action: Action::Search,
        },
        Binding {
            keys: &[Key::character('n', "n")],
            label: "next hit",
            action: Action::NextMatch,
        },
        Binding {
            keys: &[Key::character('N', "N")],
            label: "previous hit",
            action: Action::PreviousMatch,
        },
        Binding {
            keys: &[Key::named(KeyCode::Tab, "Tab")],
            label: "jump",
            action: Action::Label,
        },
        Binding {
            keys: &[Key::named(KeyCode::Left, "←")],
            label: "back",
            action: Action::Back,
        },
        Binding {
            keys: &[Key::named(KeyCode::Right, "→")],
            label: "forward",
            action: Action::Forward,
        },
        Binding {
            keys: &[Key::character('q', "q")],
            label: "quit",
            action: Action::Quit,
        },
        Binding {
            keys: &[Key::named(KeyCode::Esc, "Esc")],
            label: "back",
            action: Action::Dismiss,
        },
    ],
};

/// `↓` — moving on.
const MOVE_DOWN: Binding = Binding {
    keys: &[Key::named(KeyCode::Down, "↓")],
    label: "move",
    action: Action::Down,
};

/// `↑` — moving back.
const MOVE_UP: Binding = Binding {
    keys: &[Key::named(KeyCode::Up, "↑")],
    label: "move",
    action: Action::Up,
};

/// `Space` — checking or unchecking what the reader is on.
const TOGGLE: Binding = Binding {
    keys: &[Key::character(' ', "Space")],
    label: "check",
    action: Action::Toggle,
};

/// `y` — using the selection the screen shows.
const USE: Binding = Binding {
    keys: &[Key::character('y', "y")],
    label: "use",
    action: Action::Quit,
};

/// `q`/`Esc`/`Ctrl-D` — cancelling the session.
const CANCEL: Binding = Binding {
    keys: &[
        Key::character('q', "q"),
        Key::named(KeyCode::Esc, "Esc"),
        Key::control('d', "Ctrl-D"),
    ],
    label: "cancel",
    action: Action::Dismiss,
};

/// `1` — the richest offered preset.
const PRESET_1: Binding = Binding {
    keys: &[Key::character('1', "1")],
    label: "preset",
    action: Action::ApplyPreset(0),
};

/// `2` — the middle offered preset.
const PRESET_2: Binding = Binding {
    keys: &[Key::character('2', "2")],
    label: "preset",
    action: Action::ApplyPreset(1),
};

/// `3` — the smallest offered preset.
const PRESET_3: Binding = Binding {
    keys: &[Key::character('3', "3")],
    label: "preset",
    action: Action::ApplyPreset(2),
};

/// `/` — filtering the list by the items' names.
const FILTER: Binding = Binding {
    keys: &[Key::character('/', "/")],
    label: "filter",
    action: Action::Search,
};

/// `Home` — the first row.
const FIRST: Binding = Binding {
    keys: &[Key::named(KeyCode::Home, "Home")],
    label: "",
    action: Action::First,
};

/// `End` — the last row.
const LAST: Binding = Binding {
    keys: &[Key::named(KeyCode::End, "End")],
    label: "",
    action: Action::Last,
};

/// The init screen's keys in the list view: the skill tree, its presets, and its acceptance.
pub(crate) const INIT: Table = Table {
    bindings: &[
        MOVE_DOWN, MOVE_UP, TOGGLE, USE, CANCEL, FILTER, PRESET_1, PRESET_2, PRESET_3, FIRST, LAST,
    ],
};

pub(crate) const DEV: Table = Table {
    bindings: &[
        Binding {
            keys: &[Key::named(KeyCode::Left, "←")],
            label: "round",
            action: Action::PreviousRound,
        },
        Binding {
            keys: &[Key::named(KeyCode::Right, "→")],
            label: "round",
            action: Action::NextRound,
        },
        Binding {
            keys: &[Key::named(KeyCode::Up, "↑")],
            label: "scroll",
            action: Action::Up,
        },
        Binding {
            keys: &[Key::named(KeyCode::Down, "↓")],
            label: "scroll",
            action: Action::Down,
        },
        Binding {
            keys: &[Key::named(KeyCode::PageUp, "PgUp")],
            label: "scroll",
            action: Action::PageUp,
        },
        Binding {
            keys: &[Key::named(KeyCode::PageDown, "PgDn")],
            label: "scroll",
            action: Action::PageDown,
        },
        Binding {
            keys: &[Key::named(KeyCode::Home, "Home")],
            label: "",
            action: Action::First,
        },
        Binding {
            keys: &[Key::named(KeyCode::End, "End")],
            label: "",
            action: Action::Last,
        },
        Binding {
            keys: &[Key::control('c', "Ctrl+C")],
            label: "stop",
            action: Action::Quit,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_bound_once() {
        for table in [&DEFAULT, &HELP, &INIT, &DEV, &TEXT_INPUT, &HELP_JUMP] {
            let mut bound: Vec<(KeyCode, KeyModifiers)> = Vec::new();
            for binding in table.bindings {
                assert!(!binding.keys.is_empty(), "{} binds no key", binding.label);
                for key in binding.keys {
                    assert!(
                        !bound.contains(&(key.code, key.modifiers)),
                        "{:?} with {:?} is bound twice",
                        key.code,
                        key.modifiers
                    );
                    bound.push((key.code, key.modifiers));
                }
            }
        }
    }

    #[test]
    fn bound_keys_spell_themselves() {
        for table in [&DEFAULT, &HELP, &INIT, &DEV, &TEXT_INPUT, &HELP_JUMP] {
            for binding in table.bindings {
                for key in binding.keys {
                    assert!(
                        !key.spelling().is_empty(),
                        "{:?} has no hint spelling",
                        key.code
                    );
                    assert!(
                        table
                            .action(&KeyEvent::new(key.code, key.modifiers))
                            .is_some_and(|action| action == binding.action),
                        "{:?} does not produce {:?}",
                        key.code,
                        binding.action
                    );
                }
            }
        }
    }

    #[test]
    fn letters_keep_their_case() {
        assert_eq!(
            DEFAULT.action(&pressed(KeyCode::Char('G'))),
            Some(Action::Last)
        );
        assert_eq!(
            DEFAULT.action(&pressed(KeyCode::Char('g'))),
            Some(Action::First)
        );
    }

    #[test]
    fn escape_maps_to_dismiss() {
        assert_eq!(
            DEFAULT.action(&pressed(KeyCode::Esc)),
            Some(Action::Dismiss)
        );
    }

    #[test]
    fn q_maps_to_quit() {
        assert_eq!(
            DEFAULT.action(&pressed(KeyCode::Char('q'))),
            Some(Action::Quit)
        );
    }

    #[test]
    fn default_binds_no_control_keys() {
        let control = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert_eq!(DEFAULT.action(&control), None);
        assert_eq!(DEFAULT.action(&pressed(KeyCode::F(5))), None);
    }

    #[test]
    fn hints_follow_the_accepted_actions() {
        let hints = DEFAULT
            .hints(&[Action::Quit, Action::Open])
            .map(|(keys, label)| {
                let spelling = keys
                    .iter()
                    .map(|key| key.spelling())
                    .collect::<Vec<_>>()
                    .join("/");
                (spelling, label)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            hints,
            [("Enter".to_owned(), "open"), ("q".to_owned(), "quit")]
        );
    }

    fn pressed(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
}
