//! A room before the game starts: slots, host, readiness. Pure data and rules; the
//! lobby actor owns the rooms and says who may do what.

use protocol::{
    CivPick, GameSettings, GameSetup, GameStatus, LobbyChoices, Occupant, RoomId, RoomSlot,
    RoomState, RoomSummary, SlotControl, SlotOccupant, SlotSetup, WorldSizeChoice,
};

use super::saves::{SaveEntry, SaveRef, room_slots};
use super::validate;

/// Most humans in a game, and most slots overall.
pub const MAX_HUMANS: usize = 8;
pub const MAX_PLAYERS: usize = 21;

fn same(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// What `slot` did to a human.
#[derive(Debug, PartialEq, Eq)]
pub enum SlotEffect {
    None,
    /// The human of the slot is out of the room.
    Removed(String),
}

#[derive(Debug, Clone)]
pub struct Room {
    pub id: RoomId,
    pub host: String,
    pub settings: GameSettings,
    pub slots: Vec<RoomSlot>,
    pub status: GameStatus,
    /// The save the room was made from (`LoadSave`): settings, civilizations and the set of players
    /// are the save's and cannot change.
    pub from_save: Option<SaveRef>,
}

fn open_slot(index: u8, default_players: u8) -> RoomSlot {
    RoomSlot {
        index,
        occupant: if index < default_players {
            Occupant::Open
        } else {
            Occupant::Closed
        },
        civ: CivPick::Random,
        reserved_for: None,
    }
}

impl Room {
    /// A room whose only human is the host, in slot 0. The slots follow the map size.
    pub fn new(id: RoomId, host: &str, settings: GameSettings, size: &WorldSizeChoice) -> Self {
        let mut slots: Vec<RoomSlot> = (0..size.max_players)
            .map(|i| open_slot(i, size.default_players))
            .collect();
        if let Some(first) = slots.first_mut() {
            first.occupant = Occupant::Human {
                nick: host.to_owned(),
                ready: false,
            };
        }
        Self {
            id,
            host: host.to_owned(),
            settings,
            slots,
            status: GameStatus::Lobby,
            from_save: None,
        }
    }

    /// A room from a save. The slots of the humans of the save are open and
    /// reserved for their nicks, the other players are AI, the rest of the map's slots stay closed. The
    /// host takes his reserved slot, else the first open one nobody reserved, else the slot of an AI
    /// player; a save that offers him none refuses.
    pub fn from_save(
        id: RoomId,
        host: &str,
        entry: &SaveEntry,
        choices: &LobbyChoices,
    ) -> Result<Self, String> {
        let mut slots = room_slots(entry, choices)?;
        let place = slots
            .iter()
            .position(|s| s.reserved_for.as_deref().is_some_and(|r| same(r, host)))
            .or_else(|| {
                slots
                    .iter()
                    .position(|s| matches!(s.occupant, Occupant::Open) && s.reserved_for.is_none())
            })
            .or_else(|| {
                slots
                    .iter()
                    .position(|s| matches!(s.occupant, Occupant::Ai))
            })
            .ok_or_else(|| "the save has no slot for you".to_owned())?;
        slots[place].occupant = Occupant::Human {
            nick: host.to_owned(),
            ready: false,
        };
        slots[place].reserved_for = None;
        Ok(Self {
            id,
            host: host.to_owned(),
            settings: entry.settings.clone(),
            slots,
            status: GameStatus::Lobby,
            from_save: Some(SaveRef::of(entry)),
        })
    }

    pub fn is_host(&self, nick: &str) -> bool {
        same(&self.host, nick)
    }

    fn slot_of(&self, nick: &str) -> Option<usize> {
        self.slots
            .iter()
            .position(|s| matches!(&s.occupant, Occupant::Human { nick: n, .. } if same(n, nick)))
    }

    pub fn has(&self, nick: &str) -> bool {
        self.slot_of(nick).is_some()
    }

    /// Keeps slot `index` for `nick` again: a human of the save left the room, his slot of the save waits for him
    /// (the host can drop the reservation as before).
    pub fn reserve_again(&mut self, index: usize, nick: &str) {
        if let Some(s) = self.slots.get_mut(index)
            && matches!(s.occupant, Occupant::Open)
            && s.reserved_for.is_none()
        {
            s.reserved_for = Some(nick.to_owned());
        }
    }

    /// A slot of the room (from a save) is kept for `nick`.
    pub fn reserves(&self, nick: &str) -> bool {
        self.slots
            .iter()
            .any(|s| s.reserved_for.as_deref().is_some_and(|r| same(r, nick)))
    }

    /// The humans of the room, in slot order.
    pub fn humans(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter_map(|s| match &s.occupant {
                Occupant::Human { nick, .. } => Some(nick.clone()),
                _ => None,
            })
            .collect()
    }

    fn unready(&mut self) {
        for s in &mut self.slots {
            if let Occupant::Human { ready, .. } = &mut s.occupant {
                *ready = false;
            }
        }
    }

    fn check_waiting(&self) -> Result<(), String> {
        if self.status == GameStatus::Lobby {
            Ok(())
        } else {
            Err("the game is starting".to_owned())
        }
    }

    /// A human takes the first open slot.
    pub fn join(&mut self, nick: &str) -> Result<(), String> {
        self.check_waiting()?;
        let slot = self
            .slots
            .iter_mut()
            .find(|s| {
                matches!(s.occupant, Occupant::Open)
                    && s.reserved_for.as_ref().is_none_or(|r| same(r, nick))
            })
            .ok_or_else(|| "the room is full".to_owned())?;
        slot.occupant = Occupant::Human {
            nick: nick.to_owned(),
            ready: false,
        };
        slot.reserved_for = None;
        self.unready();
        Ok(())
    }

    /// A human leaves. `true`: the room is empty now and goes away. A leaving host hands the room to
    /// the next human after his slot (round the table), one `online` first.
    pub fn leave(&mut self, nick: &str, online: impl Fn(&str) -> bool) -> bool {
        let from = self.slot_of(nick);
        if let Some(i) = from {
            self.slots[i].occupant = Occupant::Open;
            self.unready();
        }
        let start = from.map_or(0, |i| i + 1);
        let n = self.slots.len();
        let after: Vec<String> = (0..n)
            .map(|k| (start + k) % n)
            .filter_map(|i| match &self.slots[i].occupant {
                Occupant::Human { nick, .. } => Some(nick.clone()),
                _ => None,
            })
            .collect();
        if after.is_empty() {
            return true;
        }
        if self.is_host(nick) {
            let next = after.iter().find(|h| online(h)).unwrap_or(&after[0]);
            self.host.clone_from(next);
        }
        false
    }

    /// New settings from the host. A new map size reshapes the slots; a size that would drop a
    /// human is refused.
    pub fn set_settings(
        &mut self,
        nick: &str,
        settings: GameSettings,
        choices: &LobbyChoices,
    ) -> Result<(), String> {
        self.check_waiting()?;
        if !self.is_host(nick) {
            return Err("only the host can change the settings".to_owned());
        }
        if self.from_save.is_some() {
            return Err("the settings of a saved game cannot change".to_owned());
        }
        // A new map script starts from its own option defaults (the old script's ids mean nothing there).
        let mut settings = settings;
        validate::fill_map_options(&mut settings, Some(&self.settings.map_script), choices);
        validate::settings(&settings, choices)?;
        let size = validate::world_size(choices, &settings.world_size)
            .ok_or_else(|| "unknown world size".to_owned())?;
        let want = usize::from(size.max_players);
        if want < self.slots.len()
            && self.slots[want..]
                .iter()
                .any(|s| matches!(s.occupant, Occupant::Human { .. }))
        {
            return Err("that map size has too few slots for the players in the room".to_owned());
        }
        self.slots.truncate(want);
        while self.slots.len() < want {
            let i = self.slots.len() as u8;
            self.slots.push(open_slot(i, size.default_players));
        }
        self.settings = settings;
        self.unready();
        Ok(())
    }

    /// The host sets a slot. `Open` on a slot with another human removes him.
    pub fn slot(
        &mut self,
        nick: &str,
        index: u8,
        occupant: SlotOccupant,
        civ: CivPick,
    ) -> Result<SlotEffect, String> {
        self.check_waiting()?;
        if !self.is_host(nick) {
            return Err("only the host can change the slots".to_owned());
        }
        let in_save = self
            .from_save
            .as_ref()
            .map(|f| f.players.iter().any(|p| i16::from(*p) == i16::from(index)));
        let slot = self
            .slots
            .get_mut(usize::from(index))
            .ok_or_else(|| "no such slot".to_owned())?;
        if let Some(in_save) = in_save {
            validate::save_slot(occupant, in_save)?;
        }
        let human = match &slot.occupant {
            Occupant::Human { nick: n, .. } => Some(n.clone()),
            _ => None,
        };
        let mut effect = SlotEffect::None;
        if let Some(h) = human {
            match occupant {
                SlotOccupant::Open => {
                    if same(&h, nick) {
                        return Err("the host cannot leave its own slot; leave the room".to_owned());
                    }
                    effect = SlotEffect::Removed(h);
                }
                SlotOccupant::Ai | SlotOccupant::Closed => {
                    return Err("remove the player from the slot first".to_owned());
                }
            }
        }
        slot.occupant = match occupant {
            SlotOccupant::Open => Occupant::Open,
            SlotOccupant::Ai => Occupant::Ai,
            SlotOccupant::Closed => Occupant::Closed,
        };
        // The civilizations of a save are the save's.
        if in_save.is_none() {
            slot.civ = civ;
        }
        slot.reserved_for = None;
        self.unready();
        Ok(effect)
    }

    /// A human picks a civilization; a named one only once in the room.
    pub fn pick_civ(&mut self, nick: &str, civ: CivPick) -> Result<(), String> {
        self.check_waiting()?;
        if self.from_save.is_some() {
            return Err("the civilizations of a saved game are fixed".to_owned());
        }
        let i = self
            .slot_of(nick)
            .ok_or_else(|| "not in this room".to_owned())?;
        if let CivPick::Civ(name) = &civ
            && self.slots.iter().enumerate().any(|(j, s)| {
                j != i
                    && matches!(&s.civ, CivPick::Civ(c) if c == name)
                    && !matches!(s.occupant, Occupant::Open | Occupant::Closed)
            })
        {
            return Err("that civilization is taken".to_owned());
        }
        self.slots[i].civ = civ;
        self.unready();
        Ok(())
    }

    /// A human moves to an open slot.
    pub fn take_slot(&mut self, nick: &str, index: u8) -> Result<(), String> {
        self.check_waiting()?;
        let from = self
            .slot_of(nick)
            .ok_or_else(|| "not in this room".to_owned())?;
        let to = usize::from(index);
        let target = self
            .slots
            .get(to)
            .ok_or_else(|| "no such slot".to_owned())?;
        if !matches!(target.occupant, Occupant::Open)
            || target.reserved_for.as_ref().is_some_and(|r| !same(r, nick))
        {
            return Err("the slot is not open".to_owned());
        }
        let moved = std::mem::replace(&mut self.slots[from].occupant, Occupant::Open);
        self.slots[to].occupant = moved;
        self.unready();
        Ok(())
    }

    pub fn set_ready(&mut self, nick: &str, value: bool) -> Result<(), String> {
        self.check_waiting()?;
        let i = self
            .slot_of(nick)
            .ok_or_else(|| "not in this room".to_owned())?;
        if let Occupant::Human { ready, .. } = &mut self.slots[i].occupant {
            *ready = value;
        }
        Ok(())
    }

    /// The conditions of `Start` and the worker's setup. Slot `i` is player `i`.
    pub fn start_setup(&self, nick: &str, choices: &LobbyChoices) -> Result<GameSetup, String> {
        self.check_waiting()?;
        if !self.is_host(nick) {
            return Err("only the host can start the game".to_owned());
        }
        validate::settings(&self.settings, choices)?;
        if self
            .slots
            .iter()
            .any(|s| matches!(s.occupant, Occupant::Open))
        {
            return Err("close or fill the open slots first".to_owned());
        }
        let not_ready = self.slots.iter().any(|s| {
            matches!(&s.occupant, Occupant::Human { nick: n, ready: false } if !same(n, &self.host))
        });
        if not_ready {
            return Err("not every player is ready".to_owned());
        }
        let mut slots = Vec::new();
        let (mut humans, mut all) = (0, 0);
        for s in &self.slots {
            let control = match &s.occupant {
                Occupant::Human { nick, .. } => {
                    humans += 1;
                    SlotControl::Human { nick: nick.clone() }
                }
                Occupant::Ai => SlotControl::Ai,
                Occupant::Open | Occupant::Closed => continue,
            };
            all += 1;
            slots.push(SlotSetup {
                player: s.index as i8,
                civ: s.civ.clone(),
                control,
            });
        }
        if humans == 0 || humans > MAX_HUMANS {
            return Err(format!("a game takes 1 to {MAX_HUMANS} humans"));
        }
        if !(2..=MAX_PLAYERS).contains(&all) {
            return Err(format!("a game takes 2 to {MAX_PLAYERS} players"));
        }
        Ok(GameSetup {
            settings: self.settings.clone(),
            slots,
        })
    }

    pub fn state(&self) -> RoomState {
        RoomState {
            id: self.id,
            name: self.settings.name.clone(),
            host: self.host.clone(),
            status: self.status,
            settings: self.settings.clone(),
            slots: self.slots.clone(),
            from_save: self.from_save.as_ref().map(|f| f.id.clone()),
        }
    }

    pub fn summary(&self) -> RoomSummary {
        RoomSummary {
            id: self.id,
            name: self.settings.name.clone(),
            host: self.host.clone(),
            humans: self.humans().len() as u8,
            slots: self
                .slots
                .iter()
                .filter(|s| !matches!(s.occupant, Occupant::Closed))
                .count() as u8,
            world_size: self.settings.world_size.clone(),
            map_script: self.settings.map_script.clone(),
            status: self.status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lobby::saves::{SaveEntry, SaveSeat};
    use protocol::{AutoChoices, SaveKind, TimerMode, TurnMode};

    fn settings() -> GameSettings {
        GameSettings {
            name: "g".into(),
            world_size: "DUEL".into(),
            map_script: "m".into(),
            map_options: vec![],
            speed: "s".into(),
            start_era: "e".into(),
            handicap: "h".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: vec![],
            seed: None,
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
        }
    }

    fn size() -> WorldSizeChoice {
        WorldSizeChoice {
            id: "DUEL".into(),
            name: "Duel".into(),
            width: 40,
            height: 24,
            default_players: 4,
            max_players: 5,
            default_minor: 0,
            max_minor: 0,
        }
    }

    fn choices() -> LobbyChoices {
        LobbyChoices {
            world_sizes: vec![size()],
            ..LobbyChoices::default()
        }
    }

    fn humans_in(slots: &[(u8, &str)], room: &mut Room) {
        for (i, nick) in slots {
            room.slots[usize::from(*i)].occupant = Occupant::Human {
                nick: (*nick).into(),
                ready: false,
            };
        }
    }

    #[test]
    fn a_leaving_host_hands_the_room_to_the_next_human_round_the_table_preferring_one_online() {
        let mut room = Room::new(1, "a", settings(), &size());
        humans_in(&[(2, "c"), (3, "d")], &mut room);
        // Slot order after "a" in slot 0: c, d. `c` is not online: `d` takes over.
        assert!(!room.leave("a", |n| n != "c"));
        assert_eq!(room.host, "d");
        // Nobody online: the next by slot order.
        let mut room = Room::new(1, "a", settings(), &size());
        humans_in(&[(1, "b"), (4, "e")], &mut room);
        assert!(!room.leave("a", |_| false));
        assert_eq!(room.host, "b");
        // Round the table: the host in the last slot hands on to the first.
        let mut room = Room::new(1, "e", settings(), &size());
        room.slots[0].occupant = Occupant::Human {
            nick: "a".into(),
            ready: false,
        };
        room.slots[4].occupant = Occupant::Human {
            nick: "e".into(),
            ready: false,
        };
        room.slots[1].occupant = Occupant::Human {
            nick: "b".into(),
            ready: false,
        };
        room.host = "e".into();
        assert!(!room.leave("e", |_| true));
        assert_eq!(room.host, "a");
        // The last one closes the room.
        assert!(!room.leave("a", |_| true));
        assert!(room.leave("b", |_| true));
    }

    fn entry() -> SaveEntry {
        let seat = |player, nick: Option<&str>| SaveSeat {
            player,
            civ: CivPick::Civ(format!("CIV{player}")),
            civ_name: format!("civ{player}"),
            nick: nick.map(str::to_owned),
        };
        SaveEntry {
            id: "1-00000001".into(),
            game: 1,
            seq: 1,
            kind: SaveKind::Keep,
            file: "keep/00000001-t0010.c5sv".into(),
            name: "g".into(),
            turn: 10,
            saved_at_ms: 0,
            build_id: "b".into(),
            settings: settings(),
            seats: vec![seat(0, Some("a")), seat(1, Some("b")), seat(2, None)],
            host: "a".into(),
        }
    }

    #[test]
    fn a_room_from_a_save_reserves_the_humans_slots_and_seats_the_host_by_the_rules() {
        // The host owns a slot: he sits there.
        let r = Room::from_save(1, "b", &entry(), &choices()).unwrap();
        assert!(matches!(&r.slots[1].occupant, Occupant::Human { nick, .. } if nick == "b"));
        assert_eq!(r.slots[0].reserved_for.as_deref(), Some("a"));
        assert_eq!(r.slots[0].occupant, Occupant::Open);
        assert_eq!(r.slots[2].occupant, Occupant::Ai);
        assert_eq!(r.slots[3].occupant, Occupant::Closed);
        assert_eq!(r.slots[4].occupant, Occupant::Closed);
        assert_eq!(r.slots[0].civ, CivPick::Civ("CIV0".into()));
        assert_eq!(r.state().from_save.as_deref(), Some("1-00000001"));
        // A stranger takes the AI slot (everything else is somebody's).
        let r = Room::from_save(1, "z", &entry(), &choices()).unwrap();
        assert!(matches!(&r.slots[2].occupant, Occupant::Human { nick, .. } if nick == "z"));
        // A save with no human and no AI: no place.
        let mut e = entry();
        e.seats = vec![];
        assert!(Room::from_save(1, "z", &e, &choices()).is_err());
        // A save whose player does not fit the map is refused.
        let mut e = entry();
        e.seats.push(SaveSeat {
            player: 9,
            civ: CivPick::Random,
            civ_name: "x".into(),
            nick: None,
        });
        assert!(Room::from_save(1, "z", &e, &choices()).is_err());
    }

    #[test]
    fn the_host_changing_the_map_script_gets_the_new_scripts_option_defaults() {
        use protocol::{LobbyChoice, MapOptionChoice, MapScriptOptions};
        let script = |id: &str, default: i32| MapScriptOptions {
            script: id.into(),
            options: vec![MapOptionChoice {
                id: "1".into(),
                name: "Age".into(),
                help: String::new(),
                default,
                values: vec![(1, "a".into()), (2, "b".into()), (3, "c".into())],
                read_only: false,
            }],
        };
        let pick = |t: &str| LobbyChoice {
            type_name: t.into(),
            name: t.into(),
        };
        let mut c = choices();
        c.map_scripts = vec![pick("m"), pick("n")];
        c.speeds = vec![pick("s")];
        c.eras = vec![pick("e")];
        c.handicaps = vec![pick("h")];
        c.map_options = vec![script("m", 2), script("n", 3)];
        let mut r = Room::new(1, "a", settings(), &size());
        // The host picks a value for `m`, then another script: the choice does not carry over.
        let mut s = settings();
        s.map_options = vec![("1".into(), 1)];
        r.set_settings("a", s.clone(), &c).unwrap();
        assert_eq!(r.settings.map_options, vec![("1".to_owned(), 1)]);
        s.map_script = "n".into();
        r.set_settings("a", s, &c).unwrap();
        assert_eq!(r.settings.map_options, vec![("1".to_owned(), 3)]);
        // Only the host may change it.
        assert!(r.set_settings("b", settings(), &c).is_err());
    }

    #[test]
    fn the_owner_of_a_reserved_slot_can_take_it_and_nobody_else() {
        let mut r = Room::from_save(1, "z", &entry(), &choices()).unwrap();
        assert!(r.join("y").is_err());
        r.join("b").unwrap();
        assert!(matches!(&r.slots[1].occupant, Occupant::Human { nick, .. } if nick == "b"));
        r.join("a").unwrap();
        assert!(matches!(&r.slots[0].occupant, Occupant::Human { nick, .. } if nick == "a"));
    }
}
