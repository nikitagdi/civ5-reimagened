//! The local state behind the `ui` root: what the player has selected and opened, and the toasts. The
//! actions module owns the typed [`Ui`] value and writes it to the store whole after every change; the store
//! diffs it, so a change of one field is one delta.

use std::collections::BTreeSet;
use std::sync::Arc;

use ui2_model::generated::{RefuseWhy, Toast, ToastKind, Ui};
use ui2_model::{Key, KeyedList, ModelStore, Owner, StoreError};

/// How long a toast stays, in milliseconds.
pub const TOAST_MS: u64 = 6000;
/// The most toasts shown at once; a new one pushes the oldest out.
pub const MAX_TOASTS: usize = 5;

/// The `ui` root as the actions module keeps it.
#[derive(Debug, Default)]
pub struct UiState {
    ui: Ui,
    /// The value was changed since it was last written to the store (or never written).
    dirty: bool,
    next_toast: i32,
    /// When each toast goes, by sequence number.
    toast_until: Vec<(i32, u64)>,
    /// Notifications the player dismissed (the source leaves them out of `notes`).
    pub(crate) dismissed: BTreeSet<Key>,
    /// Orders waiting for an answer (the rows are in `ui.pending`).
    pub(crate) entries: Vec<crate::pending::Entry>,
}

impl UiState {
    pub(crate) fn new() -> UiState {
        UiState {
            dirty: true,
            ..UiState::default()
        }
    }

    /// The current value of the `ui` root.
    #[must_use]
    pub fn ui(&self) -> &Ui {
        &self.ui
    }

    /// The value, for a change; the store gets it at the next [`flush`](Self::flush).
    pub(crate) fn edit(&mut self) -> &mut Ui {
        self.dirty = true;
        &mut self.ui
    }

    /// Replace the value with one the store holds (the store is already up to date).
    pub(crate) fn replace(&mut self, ui: Ui) {
        self.ui = ui;
        self.dirty = false;
    }

    /// Show a toast; it goes after [`TOAST_MS`].
    pub(crate) fn toast(
        &mut self,
        kind: ToastKind,
        why: RefuseWhy,
        detail: Option<&str>,
        now_ms: u64,
    ) {
        let seq = self.next_toast;
        self.next_toast = self.next_toast.wrapping_add(1);
        let mut rows = std::mem::take(&mut self.edit().errors).into_vec();
        rows.push(Toast {
            seq,
            kind,
            why,
            detail: detail.map(Arc::from),
        });
        self.toast_until
            .push((seq, now_ms.saturating_add(TOAST_MS)));
        while rows.len() > MAX_TOASTS {
            let gone = rows.remove(0);
            self.toast_until.retain(|(s, _)| *s != gone.seq);
        }
        // Sequence numbers are unique among live toasts, so this cannot fail.
        self.ui.errors = KeyedList::from_vec(rows).unwrap_or_default();
    }

    /// Remove the toasts whose time is up: one change, however many go.
    pub(crate) fn expire(&mut self, now_ms: u64) {
        if !self.toast_until.iter().any(|(_, at)| *at <= now_ms) {
            return;
        }
        let gone: Vec<i32> = self
            .toast_until
            .iter()
            .filter(|(_, at)| *at <= now_ms)
            .map(|(s, _)| *s)
            .collect();
        self.toast_until.retain(|(_, at)| *at > now_ms);
        let rows: Vec<Toast> = std::mem::take(&mut self.edit().errors)
            .into_vec()
            .into_iter()
            .filter(|t| !gone.contains(&t.seq))
            .collect();
        self.ui.errors = KeyedList::from_vec(rows).unwrap_or_default();
    }

    /// Write the value to the store if it changed since the last write.
    ///
    /// # Errors
    /// The store refused the root, which only a programming error causes.
    pub(crate) fn flush(&mut self, store: &mut ModelStore) -> Result<(), StoreError> {
        if self.dirty {
            store.writer(Owner::Actions).put(self.ui.clone())?;
            self.dirty = false;
        }
        Ok(())
    }
}
