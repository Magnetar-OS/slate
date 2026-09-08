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
    fn history_is_capped() {
        let mut history = History::default();
        for i in 0..(CAP + 10) {
            history.record(vec![entry(None, Some(&i.to_string()))]);
        }
        assert_eq!(history.undo.len(), CAP);
    }
}
