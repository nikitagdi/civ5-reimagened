//! The `Events.*` the exe registers and the container each one dispatches through
//! (`data/events.tsv`, extracted statically from `cvLuaEventLibrary::lRegisterAll*Events`).
//!
//! SIMPLIFIED(SIMP-exe-1024): the eight `UnitMember*` events (ids 68-75, LocalMachineContainer) are registered and nothing
//! raises them; the exe raises them nowhere either (RB10 item 4a2: only `Events.UnitMember*.Call` from Lua would).

use std::collections::HashMap;
use std::sync::OnceLock;

const TABLE: &str = include_str!("../data/events.tsv");

/// How the exe's event container hands an event to its listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// `SerialContainer`: queued, listeners run when the queue is processed (once per frame).
    Serial,
    /// `LocalMachineContainer`, `NonSerialContainer`, `SequencerContainer`: listeners run at once.
    Immediate,
}

/// Registry and container of an `Events.<name>`; `None` for a name the exe does not register.
pub fn container_of(name: &str) -> Option<Container> {
    static MAP: OnceLock<HashMap<&'static str, Container>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        for line in TABLE.lines().filter(|l| !l.starts_with('#')) {
            let mut c = line.split('\t');
            let (Some(_registry), Some(name), Some(container)) = (c.next(), c.next(), c.next())
            else {
                continue;
            };
            let kind = match container {
                "SerialContainer" => Container::Serial,
                // SIMPLIFIED(SIMP-exe-31): NonSerialContainer and SequencerContainer are taken for immediate
                // (their bodies are not read); no event of K1-K3 depends on it.
                _ => Container::Immediate,
            };
            m.insert(name, kind);
        }
        m
    })
    .get(name)
    .copied()
}

/// Every registered name (for the coverage report and tests).
pub fn all_names() -> Vec<&'static str> {
    TABLE
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').nth(1))
        .collect()
}
