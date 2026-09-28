// SPDX-License-Identifier: GPL-3.0-only

//! Undo, at the level the architecture makes trustworthy: whole files.
//!
//! Files are the source of truth, every mutation is an atomic rewrite of one
//! or two `.ics` files, and the substrate preserves foreign bytes verbatim —
//! so "the file as it was" *is* the previous state, exactly. Each user-visible
//! mutation records a [`Group`] of before/after snapshots; undo writes the
//! befores back, redo the afters. A split, a move between calendars, or a
//! scoped edit is one group however many files it touched, and undoes as one.
//!
//! Deliberately not journaled: imports (bulk, better re-imported), sync and
//! conflict resolutions (the server was involved; replaying one side would
//! re-fork what the user just settled), and feed refreshes (derived data).

/// One file's before/after around a mutation. `None` means "absent": a
/// created file has no before, a deleted file no after.
#[derive(Clone, Debug)]
pub struct Entry {
    pub calendar_id: String,
    pub file_name: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// The files one user-visible action touched, undone and redone together.
#[derive(Clone, Debug)]
pub struct Group {
    pub entries: Vec<Entry>,
}

impl Group {
    /// Whether every file still holds what this group left in it — its
    /// `after` side when undoing, its `before` when redoing. `current` holds
    /// each entry's file as it is now, in entry order (`None`: absent).
    ///
    /// Anything else means the file changed since — a sync pulled the
    /// server's edit, another client wrote it — and restoring the journaled
    /// bytes would silently discard that change, then queue the discard for
    /// upload with the server's text as its merge base.
    #[must_use]
    pub fn applies_cleanly(&self, undo: bool, current: &[Option<String>]) -> bool {
        self.entries.len() == current.len()
            && self.entries.iter().zip(current).all(|(entry, now)| {
                let left = if undo { &entry.after } else { &entry.before };
                now == left
            })
    }
}

/// Groups kept per direction; enough for a session, small enough to forget.
const CAP: usize = 50;

#[derive(Default)]
pub struct History {
    undo: Vec<Group>,
    redo: Vec<Group>,
}

impl History {
    /// Records a completed mutation. A new action forks history, so the redo
    /// side empties — the standard contract.
    pub fn record(&mut self, entries: Vec<Entry>) {
        // A no-op (every side identical) is not worth an undo step.
        if entries.iter().all(|e| e.before == e.after) {
            return;
        }
        self.undo.push(Group { entries });
        if self.undo.len() > CAP {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn pop_undo(&mut self) -> Option<Group> {
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<Group> {
        self.redo.pop()
    }

    /// Re-files a group after it was applied in the other direction.
    pub fn shelve(&mut self, group: Group, into_redo: bool) {
        if into_redo {
            self.redo.push(group);
        } else {
            self.undo.push(group);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(before: Option<&str>, after: Option<&str>) -> Entry {
        Entry {
            calendar_id: "personal".into(),
            file_name: "a.ics".into(),
            before: before.map(ToOwned::to_owned),
            after: after.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn a_new_action_clears_redo() {
        let mut history = History::default();
        history.record(vec![entry(Some("v1"), Some("v2"))]);
        let group = history.pop_undo().unwrap();
        history.shelve(group, true);
        assert_eq!(history.redo.len(), 1);

        history.record(vec![entry(Some("v2"), Some("v3"))]);
        assert!(history.pop_redo().is_none(), "redo must not survive a fork");
    }

    #[test]
    fn a_noop_records_nothing() {
        let mut history = History::default();
        history.record(vec![entry(Some("same"), Some("same"))]);
        assert!(history.pop_undo().is_none());
    }

    #[test]
    fn an_untouched_file_undoes_and_redoes() {
        let group = Group {
            entries: vec![entry(Some("v1"), Some("v2"))],
        };
        assert!(group.applies_cleanly(true, &[Some("v2".into())]));
        assert!(group.applies_cleanly(false, &[Some("v1".into())]));
    }

    #[test]
    fn a_file_changed_since_is_not_restored_over() {
        // The user's edit made v2; a sync then pulled the server's v3.
        let group = Group {
            entries: vec![entry(Some("v1"), Some("v2"))],
        };
        assert!(!group.applies_cleanly(true, &[Some("v3".into())]));
    }

    #[test]
    fn a_created_file_that_was_since_removed_is_not_resurrected_by_redo() {
        let group = Group {
            entries: vec![entry(None, Some("new"))],
        };
        // Undo removed it; something then wrote a different file there.
        assert!(!group.applies_cleanly(false, &[Some("other".into())]));
        assert!(group.applies_cleanly(false, &[None]));
    }

    #[test]
    fn history_is_capped() {
        let mut history = History::default();
        for i in 0..(CAP + 10) {
            history.record(vec![entry(None, Some(&i.to_string()))]);
        }
        assert_eq!(history.undo.len(), CAP);
    }
}
