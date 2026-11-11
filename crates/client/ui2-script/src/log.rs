//! "Once" for the script log: the same error of the same handler is written one time until the skin changes,
//! however often the handler fails again (a restarted worker switches the handler on again and it fails anew).

use std::collections::HashSet;

use ui2_model::wire::ScriptCtxId;

/// The lines already written, by context and key. The key of a line is what the worker says it is about (the
/// handler and the message, without the varying parts); lines without a key are always written.
#[derive(Debug, Default)]
pub(crate) struct LogOnce {
    seen: HashSet<(ScriptCtxId, String)>,
}

impl LogOnce {
    /// Whether a line with this key is to be written (the first time it is seen for the context).
    pub fn first(&mut self, ctx: &ScriptCtxId, key: &str) -> bool {
        if self.seen.contains(&(ctx.clone(), key.to_string())) {
            return false;
        }
        self.seen.insert((ctx.clone(), key.to_string()));
        true
    }

    /// Forget the lines of a context (it starts with a new skin or mod).
    pub fn clear(&mut self, ctx: &ScriptCtxId) {
        self.seen.retain(|(c, _)| c != ctx);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn once_per_context_and_key_until_cleared() {
        let mut log = LogOnce::default();
        let skin = ScriptCtxId::Skin;
        let m = ScriptCtxId::Mod(Arc::from("m"));
        assert!(log.first(&skin, "on(x)|boom"));
        assert!(!log.first(&skin, "on(x)|boom"));
        assert!(log.first(&m, "on(x)|boom"));
        assert!(log.first(&skin, "on(y)|boom"));
        log.clear(&skin);
        assert!(log.first(&skin, "on(x)|boom"));
        assert!(!log.first(&m, "on(x)|boom"));
    }
}
