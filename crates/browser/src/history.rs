//! One tab's session-history entries and active document identity.

use renderer::HistorySnapshot;
use url::Url;

#[derive(Clone)]
struct Entry {
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
}

impl SessionHistory {
    pub(crate) fn new(blank: Url) -> Self {
        Self {
            entries: vec![Entry { url: blank, state: None, document: 0 }],
            current: 0,
            next_document: 1,
        }
    }

    pub(crate) fn snapshot(&self) -> HistorySnapshot {
        HistorySnapshot {
            length: self.entries.len(),
            index: self.current,
            state: self.entries[self.current].state.clone(),
        }
    }

    pub(crate) fn navigate(&mut self, url: Url, traversal: Option<usize>) {
        if let Some(index) = traversal {
            // Traversal loads a new document object for the entry.
            self.current = index;
            self.entries[index].url = url;
            self.entries[index].document = self.next_document;
            self.next_document = self.next_document.saturating_add(1);
            return;
        }
        self.entries.truncate(self.current + 1);
        self.entries.push(Entry {
            url,
            state: None,
            document: self.next_document,
        });
        self.current += 1;
        self.next_document = self.next_document.saturating_add(1);
    }

    pub(crate) fn update(&mut self, url: Url, state: String, replace: bool) {
        if replace {
            let current = &mut self.entries[self.current];
            current.url = url;
            current.state = Some(state);
        } else {
            self.entries.truncate(self.current + 1);
            self.entries.push(Entry {
                url,
                state: Some(state),
                document: self.entries[self.current].document,
            });
            self.current += 1;
        }
    }

    pub(crate) fn target(&self, delta: i32) -> Option<(usize, &Url, Option<&str>, bool)> {
        let index = self.current.checked_add_signed(delta as isize)?;
        let entry = self.entries.get(index)?;
        let same_document = entry.document == self.entries[self.current].document;
        Some((index, &entry.url, entry.state.as_deref(), same_document))
    }

    pub(crate) fn traverse_same_document(&mut self, index: usize) {
        self.current = index;
    }

    /// The active entry, for reloads that keep their history position.
    pub(crate) fn current_index(&self) -> usize {
        self.current
    }
}
