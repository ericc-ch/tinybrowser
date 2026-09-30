//! One tab's session-history entries and active document identity.

use renderer::HistorySnapshot;
use url::Url;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntryId(u64);

#[derive(Clone)]
struct Entry {
    id: EntryId,
    url: Url,
    state: Option<String>,
    document: u64,
}

/// The tab coordinator owns the list. Renderer realms only receive a snapshot
/// and submit changes after a script turn.
#[derive(Clone)]
pub(crate) struct SessionHistory {
    entries: Vec<Entry>,
    current: usize,
    next_document: u64,
    next_entry: u64,
}

impl SessionHistory {
    pub(crate) fn new(blank: Url) -> Self {
        Self {
            entries: vec![Entry { id: EntryId(0), url: blank, state: None, document: 0 }],
            current: 0,
            next_document: 1,
            next_entry: 1,
        }
    }

    pub(crate) fn snapshot(&self) -> HistorySnapshot {
        HistorySnapshot {
            length: self.entries.len(),
            index: self.current,
            state: self.entries[self.current].state.clone(),
        }
    }

    /// Commits a navigation, or returns false if its traversal entry was removed
    /// while the response was being fetched. Entry identities survive list shifts.
    /// <https://html.spec.whatwg.org/multipage/browsing-the-web.html#apply-the-history-step>
    pub(crate) fn navigate(&mut self, url: Url, traversal: Option<EntryId>) -> bool {
        if let Some(entry) = traversal {
            let Some(index) = self.entries.iter().position(|candidate| candidate.id == entry) else {
                return false;
            };
            // Traversal loads a new document object for the entry.
            self.current = index;
            self.entries[index].url = url;
            self.entries[index].document = self.next_document;
            self.next_document = self.next_document.saturating_add(1);
            return true;
        }
        self.entries.truncate(self.current + 1);
        self.entries.push(Entry {
            id: EntryId(self.next_entry),
            url,
            state: None,
            document: self.next_document,
        });
        self.current += 1;
        self.next_document = self.next_document.saturating_add(1);
        self.next_entry = self.next_entry.saturating_add(1);
        true
    }

    pub(crate) fn update(&mut self, url: Url, state: String, replace: bool) {
        if replace {
            let current = &mut self.entries[self.current];
            current.url = url;
            current.state = Some(state);
        } else {
            self.entries.truncate(self.current + 1);
            self.entries.push(Entry {
                id: EntryId(self.next_entry),
                url,
                state: Some(state),
                document: self.entries[self.current].document,
            });
            self.current += 1;
            self.next_entry = self.next_entry.saturating_add(1);
        }
    }

    pub(crate) fn target(&self, delta: i32) -> Option<(usize, EntryId, &Url, Option<&str>, bool)> {
        let index = self.current.checked_add_signed(delta as isize)?;
        let entry = self.entries.get(index)?;
        let same_document = entry.document == self.entries[self.current].document;
        Some((index, entry.id, &entry.url, entry.state.as_deref(), same_document))
    }

    pub(crate) fn traverse_same_document(&mut self, index: usize) {
        self.current = index;
    }

    /// The active entry, for reloads that keep their history position.
    pub(crate) fn current_entry(&self) -> EntryId {
        self.entries[self.current].id
    }
}
