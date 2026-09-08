// SPDX-License-Identifier: GPL-3.0-only

//! Keyboard shortcuts.
//!
//! One map serves two purposes, which is why it lives apart from both: the menu
//! bar reads it to draw the accelerator beside each item, and
//! [`AppModel::update`](crate::app::AppModel) walks it on every key press that
//! no widget consumed. A binding added here therefore appears in the menu and
//! starts working in the same commit — they cannot drift.
//!
//! # Why these keys
//!
//! Modifiers on everything. The grid has no text entry of its own, but the
//! editor and the task field do, and an unmodified `n` or `t` reaching the app
//! while someone is typing an event title would be a bug that only shows up in
//! use. libcosmic only forwards keys with `event::Status::Ignored`, so a focused
//! text input already shields these — the modifier is the second line of
//! defence, not the first.
//!
//! `Ctrl+Left`/`Ctrl+Right` for paging rather than bare arrows for the same
//! reason: bare arrows belong to whatever is focused.

use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;
use cosmic::widget::menu::key_bind::{KeyBind, Modifier};
use std::collections::HashMap;

use crate::app::MenuAction;

/// The application's key bindings, keyed the way the menu widget expects.
#[must_use]
pub fn key_binds() -> HashMap<KeyBind, MenuAction> {
    let mut key_binds = HashMap::new();

    macro_rules! bind {
        ([$($modifier:ident),+ $(,)?], $key:expr, $action:ident) => {{
            key_binds.insert(
                KeyBind {
                    modifiers: vec![$(Modifier::$modifier),+],
                    key: $key,
                },
                MenuAction::$action,
            );
        }};
    }

    // File
    bind!([Ctrl], Key::Character("n".into()), NewEvent);
    bind!([Ctrl], Key::Character("z".into()), Undo);
    bind!([Ctrl, Shift], Key::Character("z".into()), Redo);
    bind!([Ctrl, Shift], Key::Character("n".into()), QuickAdd);
    bind!([Ctrl], Key::Character("i".into()), Import);
    bind!([Ctrl], Key::Character("e".into()), Export);
    bind!([Ctrl], Key::Character("r".into()), SyncNow);
    bind!([Ctrl], Key::Character(",".into()), Settings);

    // Views. Numbered in the order they appear in the switcher, which is the
    // order `ViewKind::ALL` declares.
    bind!([Ctrl], Key::Character("1".into()), Month);
    bind!([Ctrl], Key::Character("2".into()), Week);
    bind!([Ctrl], Key::Character("3".into()), Day);
    bind!([Ctrl], Key::Character("4".into()), Agenda);
    bind!([Ctrl], Key::Character("5".into()), Year);
    bind!([Ctrl], Key::Character("6".into()), Tasks);

    // Sidebar. Ctrl+B is the near-universal sidebar toggle, and nothing in
    // this map wants it.
    bind!([Ctrl], Key::Character("b".into()), ToggleSidebar);

    // Search
    bind!([Ctrl], Key::Character("f".into()), Find);

    // Navigation
    bind!([Ctrl], Key::Character("t".into()), Today);
    bind!([Ctrl], Key::Named(Named::ArrowLeft), Previous);
    bind!([Ctrl], Key::Named(Named::ArrowRight), Next);

    key_binds
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::iced::keyboard::Modifiers;

    #[test]
    fn every_binding_is_distinct() {
        // A duplicated `bind!` would silently overwrite the earlier action
        // rather than failing, and the menu would show an accelerator that
        // fires something else.
        let binds = key_binds();
        let actions: std::collections::HashSet<_> = binds.values().collect();
        assert_eq!(actions.len(), binds.len(), "two keys share one action");
    }

    #[test]
    fn a_bare_letter_never_matches() {
        // The whole point of the modifier: typing an event title must not page
        // the calendar out from under the editor.
        for bind in key_binds().keys() {
            assert!(
                !bind.matches(Modifiers::empty(), &bind.key, None),
                "{bind:?} fires without a modifier"
            );
        }
    }

    #[test]
    fn ctrl_n_opens_a_new_event() {
        let binds = key_binds();
        let hit = binds
            .iter()
            .find(|(bind, _)| bind.matches(Modifiers::CTRL, &Key::Character("n".into()), None));
        assert_eq!(hit.map(|(_, action)| *action), Some(MenuAction::NewEvent));
    }
}
