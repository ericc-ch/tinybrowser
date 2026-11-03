//! One tab's session-history entries and active document identity.

use std::collections::HashMap;

use renderer::HistoryHandling;
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

/// What one commit does to the session history.
pub(crate) enum Commit {
    /// Adds an entry after the active one and drops the forward branch.
    Push {
        url: Url,
        /// The document state's navigable target name, resolved after the
        /// commit's target-name reset
        /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
        name: Option<renderer::ContextName>,
    },
    /// Re-populates the entry a traversal selected. That entry keeps the name
    /// already stored in its document state: the entry, not the live browsing
    /// context, is the source
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>).
    Traverse { entry: EntryId, url: Url },
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
    /// Identity of the document state this entry presents. Contiguous entries
    /// can share one, and every entry that does presents the same `Document`
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#document-state>).
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
    /// The navigable target name of each document state, keyed by the
    /// `document` id its entries share. Storing it once is what makes entries
    /// that share a document state agree about the name by construction; a
    /// missing id means the empty name
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#document-state>,
    /// <https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-name>).
    names: HashMap<u64, renderer::ContextName>,
}

impl SessionHistory {
    pub(crate) fn new(blank: Url, name: Option<renderer::ContextName>) -> Self {
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
            names: name
                .map(|name| HashMap::from([(0, name)]))
                .unwrap_or_default(),
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

    /// Applies one commit, or returns false if a traversal's entry was removed
    /// while its response was being fetched. Entry identities survive list
    /// shifts.
    pub(crate) fn commit(&mut self, commit: Commit) -> bool {
        match commit {
            Commit::Traverse { entry, url } => {
                let Some(index) = self
                    .entries
                    .iter()
                    .position(|candidate| candidate.id == entry)
                else {
                    return false;
                };
                // Repopulating an entry replaces only *its* document state:
                // adjacent entries that shared the old one keep referencing
                // it, so they also keep their document id
                // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#populating-a-session-history-entry>).
                // The new state starts with the name the entry had.
                let document = self.entries[index].document;
                let next = self.next_document;
                let name = self.names.get(&document).cloned();
                self.entries[index].url = url;
                self.entries[index].document = next;
                if let Some(name) = name {
                    self.names.insert(next, name);
                }
                if !self
                    .entries
                    .iter()
                    .any(|candidate| candidate.document == document)
                {
                    self.names.remove(&document);
                }
                self.current = index;
                self.next_document = self.next_document.saturating_add(1);
                true
            }
            Commit::Push { url, name } => {
                self.truncate_forward();
                self.entries.push(Entry {
                    id: EntryId(self.next_entry),
                    url,
                    title: String::new(),
                    state: None,
                    document: self.next_document,
                });
                if let Some(name) = name {
                    self.names.insert(self.next_document, name);
                }
                self.current += 1;
                self.next_document = self.next_document.saturating_add(1);
                self.next_entry = self.next_entry.saturating_add(1);
                true
            }
        }
    }

    /// Writes the active document state's navigable target name.
    ///
    /// A commit calls this directly, and the `window.name` setter reaches it
    /// at the next commit or traversal, when the tab actor folds the registry's
    /// value in
    /// (<https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-name>).
    /// The state is shared by every entry that presents the same `Document`,
    /// so a same-document push reads the new value with no copy of its own.
    pub(crate) fn sync_current_name(&mut self, name: Option<&renderer::ContextName>) {
        let document = self.entries[self.current].document;
        match name {
            Some(name) => {
                self.names.insert(document, name.clone());
            }
            None => {
                self.names.remove(&document);
            }
        }
    }

    /// The target name stored with a session-history entry's document state.
    pub(crate) fn name(&self, entry: EntryId) -> Option<renderer::ContextName> {
        self.entries
            .iter()
            .find(|candidate| candidate.id == entry)
            .and_then(|candidate| self.names.get(&candidate.document).cloned())
    }

    pub(crate) fn update(&mut self, url: Url, state: String, handling: HistoryHandling) {
        match handling {
            HistoryHandling::Replace => {
                let current = &mut self.entries[self.current];
                current.url = url;
                current.state = Some(state);
            }
            HistoryHandling::Push => {
                self.truncate_forward();
                // A same-document push shares the current document state, so
                // it presents the same name with no copy of its own
                // (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#document-state>).
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
    }

    /// Drops the entries after the active one, and the document-state names
    /// only those entries referenced.
    ///
    /// The active entry can share its document state with a removed entry, so
    /// a removed entry's document id does not by itself mean the name is dead.
    fn truncate_forward(&mut self) {
        let removed: Vec<u64> = self
            .entries
            .drain(self.current + 1..)
            .map(|entry| entry.document)
            .collect();
        for document in removed {
            if !self.entries.iter().any(|entry| entry.document == document) {
                self.names.remove(&document);
            }
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
    use super::{Commit, SessionHistory};
    use url::Url;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("valid url")
    }

    fn push(text: &str, name: Option<renderer::ContextName>) -> Commit {
        Commit::Push {
            url: url(text),
            name,
        }
    }

    #[test]
    fn back_then_navigate_truncates_the_forward_branch() {
        let mut history = SessionHistory::new(url("about:blank"), None);
        history.commit(push("https://example.com/a", None));
        history.commit(push("https://example.com/b", None));

        let (index, entry, _, _, same_document) = history.target(-1).expect("back target");
        assert!(!same_document);
        history.traverse_same_document(index);

        history.commit(push("https://example.com/c", None));

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
        let mut history = SessionHistory::new(url("about:blank"), None);
        history.commit(push("https://example.com/a", None));
        history.set_current_title("Alpha".into());
        history.commit(push("https://example.com/b", None));
        history.set_current_title("Beta".into());

        let snapshot = history.snapshot();
        assert_eq!(snapshot.entries()[1].title(), "Alpha");
        assert_eq!(snapshot.entries()[2].title(), "Beta");
    }

    /// The target name belongs to the entry's document state, so traversal
    /// reads it back instead of the live browsing-context name, a
    /// same-document push inherits it, and the `window.name` setter writes
    /// the active document state
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#resetBCName>,
    /// <https://html.spec.whatwg.org/multipage/nav-history-apis.html#dom-name>).
    #[test]
    fn a_target_name_rides_the_entry_it_committed_with() {
        let named = |raw: &str| renderer::ContextName::parse(raw);
        let mut history = SessionHistory::new(url("about:blank"), named("start"));
        history.commit(push("https://example.com/a", named("first")));
        let (_, first, _, _, _) = history.target(0).expect("entry");
        assert_eq!(history.name(first), named("first"));

        // The setter writes the active document state.
        history.sync_current_name(named("renamed").as_ref());
        assert_eq!(history.name(first), named("renamed"));

        // A same-document push shares the document state, so it presents the
        // same name with no copy of its own.
        history.update(
            url("https://example.com/hash"),
            "{}".into(),
            renderer::HistoryHandling::Push,
        );
        let pushed = history.current_entry();
        assert_eq!(history.name(pushed), named("renamed"));
        assert_eq!(history.name(first), named("renamed"));

        // A commit that resets the name stores no name on its entry.
        history.commit(push("https://example.com/b", None));
        let current = history.current_entry();
        assert_eq!(history.name(current), None);
        assert_eq!(history.name(first), named("renamed"));

        // Traversal repopulates only its own entry, keeping the name its
        // document state had.
        assert!(history.commit(Commit::Traverse {
            entry: first,
            url: url("https://example.com/a"),
        }));
        assert_eq!(history.name(first), named("renamed"));
        assert_eq!(history.name(pushed), named("renamed"));

        // The sibling kept the old document state, so the traversed entry
        // now has its own and the setter reaches only the active one.
        history.sync_current_name(None);
        assert_eq!(history.name(first), None);
        assert_eq!(history.name(pushed), named("renamed"));
        assert_eq!(history.name(current), None);
    }

    /// Repopulating an entry must not merge its document state with the
    /// siblings that shared it: a later traversal to a sibling is a new
    /// document, not a same-document step
    /// (<https://html.spec.whatwg.org/multipage/browsing-the-web.html#populating-a-session-history-entry>).
    #[test]
    fn a_traversal_repopulates_only_its_own_entry() {
        let mut history = SessionHistory::new(url("about:blank"), None);
        history.commit(push("https://example.com/a", None));
        history.update(
            url("https://example.com/a#hash"),
            "{}".into(),
            renderer::HistoryHandling::Push,
        );
        let pushed = history.current_entry();
        history.commit(push("https://example.com/b", None));

        let (_, entry, _, _, same_document) = history.target(-1).expect("back target");
        assert_eq!(entry, pushed);
        assert!(!same_document);
        assert!(history.commit(Commit::Traverse {
            entry,
            url: url("https://example.com/a#hash"),
        }));

        // The sibling still presents the pre-traversal document object.
        let (_, sibling, _, _, same_document) = history.target(-1).expect("sibling target");
        assert!(!same_document);
        assert!(history.commit(Commit::Traverse {
            entry: sibling,
            url: url("https://example.com/a"),
        }));
    }
}
