//! What the music asks of the game (re-b15 ITEM 3): the events `cvAudioSimMainGameMusic` subscribes to and the state
//! it reads once a second (the active player's civilization and leader, its era, who its team met and is at war with).
//!
//! The events go to the host as they run ([`ExeHost::music_event`](crate::ExeHost::music_event)); the state is read
//! through the Lua API of the replica ([`crate::ExeApi::music_state`]), the way the exe asks the DLL.

use crate::core::{Arg, Core, Family, Fire, Payload, unpack_lua};

/// A game event the music listens to (`cvAudioSimMainGameMusic::SubscribeToEvents` 0x08b744ba).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicEvent {
    /// `AILeaderMessage` (event 118) of a player: his leader screen is up.
    LeaderView(i32),
    /// `LeavingLeaderViewMode` (the exe's `LeaveLeaderView`, event 39).
    LeaveLeaderView,
    /// `EndGameShow` (the exe's `GameOverScreenShow`, event 136).
    GameOver,
}

/// A player as the music sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MusicPlayer {
    pub id: i32,
    /// `Leaders.Type`.
    pub leader: String,
    /// `Civilizations.Type` and `Civilizations.SoundtrackTag` (empty when not set).
    pub civ_type: String,
    pub soundtrack: String,
    /// The active team met its team (the active player's own entry: true).
    pub met: bool,
    /// The active team is at war with its team (`CvTeam::isAtWar`) and it is alive (`getAtWarCount` counts only those).
    pub at_war: bool,
    /// A major civilization that is alive.
    pub major_alive: bool,
}

/// The game as the music reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MusicState {
    pub active: i32,
    pub human: bool,
    /// The active player's era and the index of `ERA_RENAISSANCE` (-1 when the table has none).
    pub era: i32,
    pub start_era: i32,
    /// `UI.GetLeaderHeadRootUp()` (`None` when the UI does not answer): a leader screen that closed without
    /// `LeavingLeaderViewMode` still ends the leader's theme.
    pub leader_up: Option<bool>,
    /// Every player ever alive below the barbarians (`MAX_CIV_PLAYERS`).
    pub players: Vec<MusicPlayer>,
}

/// One line per call: `active|human|era|start|leader_up;id,leader,civ,soundtrack,met,war,major;...`.
const STATE_LUA: &str = r#"
local a = Game and Game.GetActivePlayer and Game.GetActivePlayer()
if a == nil or a < 0 or not Players or not Players[a] or not Teams then return nil end
local ap = Players[a]
local my = ap:GetTeam()
local at = Teams[my]
if not at then return nil end
local function s(x) local r = tostring(x or ""):gsub("[|;,]", "") return r end
local function b(x) return x and 1 or 0 end
local start = GameInfo and GameInfo.Eras and GameInfo.Eras.ERA_RENAISSANCE
local up = -1
if UI and UI.GetLeaderHeadRootUp then up = b(UI.GetLeaderHeadRootUp()) end
local parts = { table.concat({ a, b(ap:IsHuman()), ap:GetCurrentEra(), start and start.ID or -1, up }, "|") }
local n = (GameDefines and GameDefines.MAX_CIV_PLAYERS) or 63
for p = 0, n - 1 do
	local pl = Players[p]
	if pl and pl:IsEverAlive() then
		local t = pl:GetTeam()
		local civ = GameInfo.Civilizations[pl:GetCivilizationType()]
		local ld = GameInfo.Leaders[pl:GetLeaderType()]
		local met = t == my or at:IsHasMet(t)
		local war = t ~= my and pl:IsAlive() and at:IsAtWar(t)
		local major = pl:IsAlive() and not pl:IsMinorCiv() and not pl:IsBarbarian()
		parts[#parts + 1] = table.concat({ p, s(ld and ld.Type), s(civ and civ.Type), s(civ and civ.SoundtrackTag),
			b(met), b(war), b(major) }, ",")
	end
end
return table.concat(parts, ";")
"#;

/// Reads the line of [`STATE_LUA`].
pub fn parse_state(line: &str) -> Option<MusicState> {
    let mut parts = line.split(';');
    let head: Vec<&str> = parts.next()?.split('|').collect();
    let int = |s: &str| s.trim().parse::<f64>().ok().map(|v| v as i32);
    let mut st = MusicState {
        active: int(head.first()?)?,
        human: head.get(1).is_some_and(|s| s.trim() == "1"),
        era: head.get(2).and_then(|s| int(s)).unwrap_or(0),
        start_era: head.get(3).and_then(|s| int(s)).unwrap_or(-1),
        leader_up: match head.get(4).map(|s| s.trim()) {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        },
        players: Vec::new(),
    };
    for p in parts {
        let f: Vec<&str> = p.split(',').collect();
        if f.len() < 7 {
            continue;
        }
        let Some(id) = int(f[0]) else { continue };
        st.players.push(MusicPlayer {
            id,
            leader: f[1].to_owned(),
            civ_type: f[2].to_owned(),
            soundtrack: f[3].to_owned(),
            met: f[4] == "1",
            at_war: f[5] == "1",
            major_alive: f[6] == "1",
        });
    }
    Some(st)
}

impl Core {
    /// The music's events, told to the host as they run (`Core::run`).
    pub(crate) fn watch_music(&self, f: &Fire) {
        if f.family != Family::Events {
            return;
        }
        let e = match f.name.as_str() {
            "AILeaderMessage" => self.first_int(f).map(MusicEvent::LeaderView),
            "LeavingLeaderViewMode" => Some(MusicEvent::LeaveLeaderView),
            "EndGameShow" => Some(MusicEvent::GameOver),
            _ => None,
        };
        if let Some(e) = e {
            self.host.music_event(e);
        }
    }

    fn first_int(&self, f: &Fire) -> Option<i32> {
        match &f.payload {
            Payload::Args(a) => match a.first() {
                Some(Arg::Num(n)) => Some(*n as i32),
                _ => None,
            },
            Payload::Lua(k) => {
                let ctx = self.lua_ctx()?;
                let v = unpack_lua(ctx.lua(), k).ok()?;
                match v.first() {
                    Some(lua_sys::LuaValue::Number(n)) => Some(*n as i32),
                    _ => None,
                }
            }
        }
    }

    /// See [`crate::ExeApi::music_state`].
    pub(crate) fn music_state(&self) -> Option<MusicState> {
        let ctx = self.lua_ctx()?;
        let line = ctx
            .lua()
            .load(STATE_LUA)
            .set_name("=exe_music_state")
            .eval::<Option<String>>()
            .ok()
            .flatten()?;
        parse_state(&line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_line_reads_back() {
        let st = parse_state(
            "0|1|3|3|0;0,LEADER_WASHINGTON,CIVILIZATION_AMERICA,,1,0,1;1,LEADER_AUGUSTUS,CIVILIZATION_ROME,,0,1,1;bad",
        )
        .unwrap();
        assert_eq!((st.active, st.human, st.era, st.start_era), (0, true, 3, 3));
        assert_eq!(st.leader_up, Some(false));
        assert_eq!(parse_state("0|1|3|3|-1").unwrap().leader_up, None);
        assert_eq!(st.players.len(), 2);
        assert_eq!(st.players[1].leader, "LEADER_AUGUSTUS");
        assert!(st.players[1].at_war && !st.players[1].met && st.players[1].major_alive);
        assert!(parse_state("").is_none());
    }
}
