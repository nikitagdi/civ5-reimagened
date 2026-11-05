//! Switches of a raw peer: what a real client never does.

use serde_json::Value;

/// What the bot names in `resume.basis` and `JoinGame.basis` when it enters with `Fresh` (NP-D22).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClaimBasis {
    /// The hash of the basis in memory.
    #[default]
    Own,
    /// No basis in memory, as after a reload of the page.
    None,
    /// The hash of the first basis the bot ever held (it is stale after a change of the basis).
    Stale,
    /// A game that is not this one.
    WrongGame,
}

/// How the bot goes back after a drop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Reconnect {
    /// 0.5 s, doubling to 10 s.
    #[default]
    Simple,
    /// The `net-client` policy of fresh probe sockets (NC-D17): not built yet.
    Probe,
}

#[derive(Debug, Clone, Default)]
pub struct Knobs {
    /// Extra copies of every `Cmd` frame sent right after the first (`dup_cmds`).
    pub dup_cmds: u32,
    /// Do not send the unanswered orders again after a resume (`no_resend`).
    pub no_resend: bool,
    /// Lift the limit of 32 orders in the air (`window`).
    pub window: bool,
    /// At this turn spoil the own view and recompute its hash (`corrupt_view`).
    pub corrupt_view_at_turn: Option<i32>,
    /// Do not send the application `Ping` (`no_ping`).
    pub no_ping: bool,
    /// Pause before every order of the policy, in ms (`hold_ms`).
    pub hold_ms: u64,
    pub claim_basis: ClaimBasis,
    /// At this turn throw the basis away and ask for it again (`drop_basis`).
    pub drop_basis_at_turn: Option<i32>,
    pub reconnect: Reconnect,
}

impl Knobs {
    /// `freeze_ms` is an action, not a setting: it is handled by the bot (`Bot::freeze`), not here.
    pub fn set(&mut self, name: &str, value: &Value) -> Result<(), String> {
        let num = || {
            value
                .as_u64()
                .ok_or_else(|| format!("knob {name}: expected a non-negative number"))
        };
        let turn = || -> Result<Option<i32>, String> {
            if value.is_null() {
                return Ok(None);
            }
            let n = value
                .as_i64()
                .ok_or_else(|| format!("knob {name}: expected a turn number or null"))?;
            i32::try_from(n)
                .map(Some)
                .map_err(|_| format!("knob {name}: turn out of range"))
        };
        let flag = || {
            value
                .as_bool()
                .ok_or_else(|| format!("knob {name}: expected true or false"))
        };
        match name {
            "dup_cmds" => self.dup_cmds = u32::try_from(num()?).map_err(|e| e.to_string())?,
            "no_resend" => self.no_resend = flag()?,
            "window" => self.window = flag()?,
            "corrupt_view" => self.corrupt_view_at_turn = turn()?,
            "no_ping" => self.no_ping = flag()?,
            "hold_ms" => self.hold_ms = num()?,
            "drop_basis" => self.drop_basis_at_turn = turn()?,
            "claim_basis" => {
                self.claim_basis = match value.as_str() {
                    Some("own") => ClaimBasis::Own,
                    Some("none") => ClaimBasis::None,
                    Some("stale") => ClaimBasis::Stale,
                    Some("wrong_game") => ClaimBasis::WrongGame,
                    _ => return Err("knob claim_basis: own, none, stale or wrong_game".into()),
                }
            }
            "reconnect" => {
                self.reconnect = match value.as_str() {
                    Some("simple") => Reconnect::Simple,
                    Some("probe") => {
                        return Err(
                            "knob reconnect: the probe policy waits for net-client (NC-D17)".into(),
                        );
                    }
                    _ => return Err("knob reconnect: simple or probe".into()),
                }
            }
            other => return Err(format!("unknown knob {other}")),
        }
        Ok(())
    }
}
