//! One tab's session-history entries and active document identity.

use renderer::HistorySnapshot as RendererHistorySnapshot;
use url::Url;

/// Stable identity of one session-history entry.
///
/// The value survives list shifts, so a traversal can name its target without
/// depending on an index
/// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#session-history>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryId(u64);

impl EntryId {
    /// The wire value protocol adapters put on their surface.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One committed session-history entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    id: EntryId,
    url: Url,
    title: String,
}

impl HistoryEntry {
    /// Stable identity that survives list shifts.
    #[must_use]
    pub fn id(&self) -> EntryId {
        self.id
    }

    /// URL committed for this entry.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Document title recorded when the entry committed.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }
}

/// The committed history of one tab: entries in traversal order and the active
/// position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySnapshot {
    entries: Vec<HistoryEntry>,
    current_index: usize,
}

impl HistorySnapshot {
    /// Entries in traversal order, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Zero-based position of the active entry.
    #[must_use]
    pub fn current_index(&self) -> usize {
        self.current_index
    }

    /// Total entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Always false: a snapshot carries at least the initial blank entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone)]
struct Entry {
    id: EntryId,
    url: Url,
    title: String,
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
            entries: vec![Entry {
                id: EntryId(0),
                url: blank,
                title: String::new(),
                state: None,
                document: 0,
            }],
            current: 0,
            next_document: 1,
            next_entry: 1,
        }
    }

    /// State the renderer needs for `history.length`, `history.state`, and
    /// traversal.
    pub(crate) fn renderer_snapshot(&self) -> RendererHistorySnapshot {
        RendererHistorySnapshot {
            length: self.entries.len(),
            index: self.current,
            state: self.entries[self.current].state.clone(),
        }
    }

    /// Committed entries and the active index for protocol adapters.
    pub(crate) fn snapshot(&self) -> HistorySnapshot {
        HistorySnapshot {
            entries: self
                .entries
                .iter()
                .map(|entry| HistoryEntry {
                    id: entry.id,
                    url: entry.url.clone(),
                    title: entry.title.clone(),
                })
                .collect(),
            current_index: self.current,
        }
    }

    /// Records the committed document's title on the active entry.
    pub(crate) fn set_current_title(&mut self, title: String) {
        self.entries[self.current].title = title;
    }

    /// Commits a navigation, or returns false if its traversal entry was removed
    /// while the response was being fetched. Entry identities survive list shifts.
    /// <https://html.spec.whatwg.org/multipage/browsing-the-web.html#apply-the-history-step>
    pub(crate) fn navigate(&mut self, url: Url, traversal: Option<EntryId>) -> bool {
        if let Some(entry) = traversal {
            let Some(index) = self
                .entries
                .iter()
                .position(|candidate| candidate.id == entry)
            else {
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
            title: String::new(),
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
                title: self.entries[self.current].title.clone(),
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
        Some((
            index,
            entry.id,
            &entry.url,
            entry.state.as_deref(),
            same_document,
        ))
    }

    pub(crate) fn traverse_same_document(&mut self, index: usize) {
        self.current = index;
    }

    /// The active entry, for reloads that keep their history position.
    pub(crate) fn current_entry(&self) -> EntryId {
        self.entries[self.current].id
    }
}

#[cfg(test)]
mod tests {
    use super::SessionHistory;
    use url::Url;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("valid url")
    }

    #[test]
    fn back_then_navigate_truncates_the_forward_branch() {
        let mut history = SessionHistory::new(url("about:blank"));
        history.navigate(url("https://example.com/a"), None);
        history.navigate(url("https://example.com/b"), None);

        let (index, entry, _, _, same_document) = history.target(-1).expect("back target");
        assert!(!same_document);
        history.traverse_same_document(index);

        history.navigate(url("https://example.com/c"), None);

        let snapshot = history.snapshot();
        let urls: Vec<&str> = snapshot
            .entries()
            .iter()
            .map(|entry| entry.url().as_str())
            .collect();
        assert_eq!(
            urls,
            [
                "about:blank",
                "https://example.com/a",
                "https://example.com/c"
            ]
        );
        assert_eq!(snapshot.current_index(), 2);
        // The traversal entry keeps its identity at index 1.
        assert_eq!(snapshot.entries()[1].id(), entry);
    }

    #[test]
    fn titles_are_recorded_per_entry() {
        let mut history = SessionHistory::new(url("about:blank"));
        history.navigate(url("https://example.com/a"), None);
        history.set_current_title("Alpha".into());
        history.navigate(url("https://example.com/b"), None);
        history.set_current_title("Beta".into());

        let snapshot = history.snapshot();
        assert_eq!(snapshot.entries()[1].title(), "Alpha");
        assert_eq!(snapshot.entries()[2].title(), "Beta");
    }
}
