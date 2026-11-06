//! The table of owners for the gateway (`ToWorker::WantOwners`, `WorkerMsg::Owners`, IPC v9): the whole truth about who
//! owns which plot, for server mods. Not a view of a seat, so no fog.
//!
//! The worker keeps the table only after the gateway asked for it; a game whose mods do not read the world never
//! builds it and sees no change in its frames. The table is brought up to date in [`Worker::build_round`] with the
//! list of dirty plots that round already took (the list is single-consumer: the core resets it). The frame puts
//! `Owners` before everything of the seats, so the table the gateway holds is never older than the last `Results`
//! it has seen.
//!
//! Every turn change sends a `Full` whatever the core reported dirty: a safety net for an owner change the
//! dirty list could miss. (The real core lists every owner change, watched by a human team or not: see
//! `SPEC-mod-host-server.md`.)

use protocol::OwnersUpdate;

use crate::core::{Core, CoreError};
use crate::worker::{Fb, Worker};

/// What the worker keeps for the table.
#[derive(Debug, Default)]
pub(crate) struct OwnersState {
    /// The gateway asked: the table is kept and sent.
    on: bool,
    /// Index `y * width + x`; empty until the first `Full`.
    table: Vec<i8>,
    /// Revision of the last update sent; grows by one with each.
    rev: u32,
    /// The game turn of the last `Full`.
    turn: i32,
}

impl<C: Core> Worker<C> {
    /// `ToWorker::WantOwners`: starts keeping the table and answers with the whole of it now.
    pub(crate) fn want_owners(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        self.owners.on = true;
        self.owners_full(fb)
    }

    /// Reads the whole table from the core and queues it as a `Full`.
    fn owners_full(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        let n = self.map.plot_count();
        let mut out = Vec::with_capacity(n);
        self.core.owners(None, &mut out)?;
        if out.len() != n || out.iter().enumerate().any(|(i, (j, _))| i != *j as usize) {
            return Err(CoreError::failed(format!(
                "core: the owners have {} records for {n} plots",
                out.len()
            )));
        }
        let st = &mut self.owners;
        st.table = out.into_iter().map(|(_, o)| o).collect();
        st.rev += 1;
        st.turn = self.turn;
        fb.owners = Some(OwnersUpdate::Full {
            rev: st.rev,
            turn: st.turn,
            owners: st.table.clone(),
        });
        Ok(())
    }

    /// Called by `build_round` with the dirty list it took: a `Full` at a turn change or without a list, else a
    /// `Delta` of the dirty plots whose owner differs from the table (none: nothing is sent).
    pub(crate) fn owners_round(
        &mut self,
        dirty: Option<&[u32]>,
        fb: &mut Fb,
    ) -> Result<(), CoreError> {
        if !self.owners.on {
            return Ok(());
        }
        let st = &self.owners;
        let list = match dirty {
            Some(list) if !st.table.is_empty() && st.turn == self.turn => list,
            _ => return self.owners_full(fb),
        };
        if list.is_empty() {
            return Ok(());
        }
        let mut fresh = Vec::with_capacity(list.len());
        self.core.owners(Some(list), &mut fresh)?;
        let st = &mut self.owners;
        let mut changes = Vec::new();
        for (i, o) in fresh {
            match st.table.get_mut(i as usize) {
                Some(slot) if *slot != o => {
                    *slot = o;
                    changes.push((i, o));
                }
                Some(_) => {}
                None => {
                    return Err(CoreError::failed(format!(
                        "core: owner of plot {i} outside the map"
                    )));
                }
            }
        }
        if changes.is_empty() {
            return Ok(());
        }
        changes.sort_unstable_by_key(|(i, _)| *i);
        changes.dedup_by_key(|(i, _)| *i);
        let base_rev = st.rev;
        st.rev += 1;
        fb.owners = Some(OwnersUpdate::Delta {
            base_rev,
            rev: st.rev,
            changes,
        });
        Ok(())
    }
}
