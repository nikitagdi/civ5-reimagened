//! Games back from the disk (4.2 "Подъём партий"): at gateway start every game that was `Running`
//! gets a worker with its newest save, as long as `max_games` and the free memory allow; the others sleep.
//! A sleeping game wakes when one of its players logs in or somebody takes a slot with `JoinGame`
//! ([`wake`]). The seats come from the stored meta: the nicks are held for their owners (`Conn::None`)
//! until they log in. The saves are indexed from the file names of `ring/` and `keep/`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use game_host::saves::{self as c5sv, SaveInfo};
use protocol::{CivPick, GameId, GameSetup, SaveKind, SeatControl, SlotControl, SlotSetup};

use super::actor::{GameInit, GameLoad};
use super::{GameHandle, SeatSnap};
use crate::Shared;
use crate::lobby::LobbyMsg;
use crate::lobby::saves::{SaveEntry, SaveSeat, save_id};
use crate::state::meta::{self, GameMeta, MetaStatus};
use crate::supervisor::policy::{SaveBook, SaveEntry as BookEntry};

/// Free memory in MiB (Linux `MemAvailable`); `None` where it is not known.
pub fn free_mem_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

/// Why no game can start now: `max_games` or the memory.
pub fn no_room_for_a_game(shared: &Shared) -> Option<&'static str> {
    if shared.games.running() >= shared.config.max_games as usize {
        return Some("too many games are running");
    }
    if let Some(free) = free_mem_mb()
        && free < shared.config.min_free_mem_mb
    {
        return Some("the server is short of memory");
    }
    None
}

/// The worker's input for a stored game: its settings and who sits where.
pub fn setup_of(meta: &GameMeta) -> GameSetup {
    GameSetup {
        settings: meta.settings.clone(),
        slots: meta
            .seats
            .iter()
            .map(|s| SlotSetup {
                player: s.player,
                civ: s.civ.clone(),
                control: match &s.nick {
                    Some(nick) => SlotControl::Human { nick: nick.clone() },
                    None => SlotControl::Ai,
                },
            })
            .collect(),
    }
}

fn game_dir(shared: &Shared, id: GameId) -> Option<PathBuf> {
    shared
        .config
        .state_dir
        .as_ref()
        .map(|d| d.join("games").join(id.to_string()))
}

/// The seats a sleeping game shows in the lobby list.
pub fn seat_snaps(meta: &GameMeta, choices: &protocol::LobbyChoices) -> Vec<SeatSnap> {
    meta.seats
        .iter()
        .map(|s| SeatSnap {
            player: s.player,
            civ_name: civ_name(choices, &s.civ),
            control: match &s.nick {
                Some(nick) => SeatControl::Human { nick: nick.clone() },
                None => SeatControl::Ai {
                    kicked: s.kicked.clone(),
                },
            },
            online: false,
            conn: "none",
        })
        .collect()
}

/// The saves of a game on disk whose C5SV files check out (every checksum, the version), newest first.
/// A file that does not is left out with a line in the log: the list of saves and the restart never offer it.
pub fn read_saves(dir: &std::path::Path) -> Vec<(BookEntry, SaveInfo)> {
    let mut out = Vec::new();
    for (sub, kind) in [
        (c5sv::RING_DIR, SaveKind::Ring),
        (c5sv::KEEP_DIR, SaveKind::Keep),
    ] {
        let Ok(read) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let Some((seq, turn)) = c5sv::parse_file_name(&name) else {
                continue;
            };
            let path = item.path();
            let info = std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| c5sv::read_header(&bytes).map_err(|e| e.to_string()));
            match info {
                Ok(info) => out.push((
                    BookEntry {
                        seq,
                        file: format!("{sub}/{name}"),
                        turn,
                        kind,
                    },
                    info,
                )),
                Err(why) => {
                    tracing::warn!(save = %path.display(), %why, "a save is not usable, it is left out");
                }
            }
        }
    }
    out.sort_by_key(|(e, _)| std::cmp::Reverse(e.seq));
    out
}

/// The saves the running core build can load; unknown running build: all.
fn of_build<'a>(
    saves: &'a [(BookEntry, SaveInfo)],
    build: Option<&'a str>,
) -> impl Iterator<Item = &'a (BookEntry, SaveInfo)> {
    saves
        .iter()
        .filter(move |(_, i)| build.is_none_or(|b| b == i.header.build_id))
}

/// Starts the worker of a stored game with the newest of `book`. `Err`: why not.
fn spawn_from_meta(
    shared: &Arc<Shared>,
    meta: &GameMeta,
    book: SaveBook,
) -> Result<GameHandle, String> {
    let dir = game_dir(shared, meta.id).ok_or_else(|| "the gateway keeps no state".to_owned())?;
    let newest = book
        .pick(0)
        .cloned()
        .ok_or_else(|| "the game has no save this core build can load".to_owned())?;
    // The mods of the game must be here as they were frozen (S4): else the game stays asleep with a reason.
    shared
        .mod_store
        .restore_set(meta.mods_lock.as_deref(), meta.settings.mods.len())
        .map_err(|e| format!("the game cannot be restored: {e}"))?;
    if meta.mods_lock.is_some() {
        super::modhost::ModHost::build(
            meta.id,
            Arc::clone(&shared.mod_store),
            Arc::clone(&shared.config.mod_registry),
            meta.mods_lock.as_deref(),
        )
        .map_err(|e| format!("the game cannot be restored: {e}"))?;
    }
    let kicked: BTreeMap<_, _> = meta
        .seats
        .iter()
        .filter_map(|s| s.kicked.clone().map(|k| (s.player, k)))
        .collect();
    let init = GameInit {
        id: meta.id,
        setup: setup_of(meta),
        host: meta.host.clone(),
        room: None,
        seed: meta.seed,
        load: Some(GameLoad {
            file: dir.join(&newest.file).to_string_lossy().into_owned(),
            turn: newest.turn,
            book: Some(book),
        }),
        kicked,
        created_ms: Some(meta.created_ms),
        mods_lock: meta.mods_lock.clone(),
    };
    shared.start_game(init, shared.choices())
}

// SIMPLIFIED(SIMP-059): only saves of the running core build are read; the original does not load saves of another build either, and we keep no old worker.
fn book_of(shared: &Shared, meta: &GameMeta) -> SaveBook {
    let build = shared.health.core_build();
    game_dir(shared, meta.id)
        .map(|d| read_saves(&d))
        .map(|saves| {
            SaveBook::from_entries(of_build(&saves, build.as_deref()).map(|(e, _)| e.clone()))
        })
        .unwrap_or_default()
}

/// Wakes a sleeping game: a worker with its newest save. The seats attach by themselves as the game
/// reports ready (`LobbyMsg::GameStarted`).
pub fn wake(shared: &Arc<Shared>, id: GameId) -> Result<GameHandle, String> {
    if let Some(h) = shared.games.get(id) {
        return Ok(h);
    }
    let meta = shared
        .games
        .dormant(id)
        .ok_or_else(|| "no such game".to_owned())?;
    if let Some(why) = no_room_for_a_game(shared) {
        return Err(format!("{why}; try again later"));
    }
    let book = book_of(shared, &meta);
    if book.is_empty() {
        return Err("the game was saved by another core build and cannot be woken".to_owned());
    }
    // From here the game is no longer asleep; a failure puts it back.
    let meta = shared.games.take_dormant(id).unwrap_or(meta);
    match spawn_from_meta(shared, &meta, book) {
        Ok(h) => Ok(h),
        Err(e) => {
            shared.games.add_dormant(meta);
            Err(e)
        }
    }
}

/// Start of the gateway: the saves go into the lobby's list, the games that were
/// `Running` come back (as far as there is room), the others sleep. Runs inside the tokio runtime.
pub fn restore_all(shared: &Arc<Shared>) -> anyhow::Result<()> {
    // The lobby runs from the start, not from the first login: it closes the sleepers nobody comes back to.
    shared.ensure_actors();
    let Some(state_dir) = shared.config.state_dir.clone() else {
        return Ok(());
    };
    let metas = meta::read_all(&state_dir)?;
    let build = shared.health.core_build();
    for mut m in metas {
        shared.ids.seen_game(m.id);
        let saves = game_dir(shared, m.id)
            .map(|d| read_saves(&d))
            .unwrap_or_default();
        index_saves(shared, &m, &saves, build.as_deref());
        if matches!(m.status, MetaStatus::Finished | MetaStatus::Faulted) {
            continue;
        }
        if saves.is_empty() {
            // Nothing to continue from: the game is let go, its players are free.
            tracing::warn!(game = m.id, "the game has no usable save: it is dropped");
            m.status = MetaStatus::Faulted;
            if let Some(d) = game_dir(shared, m.id) {
                m.write_logged(&d);
            }
            continue;
        }
        // The nicks of its players are theirs from the first moment.
        shared.lobby.send(LobbyMsg::Seated {
            game: m.id,
            seats: m.humans(),
        });
        let book =
            SaveBook::from_entries(of_build(&saves, build.as_deref()).map(|(e, _)| e.clone()));
        let up = if m.status == MetaStatus::Running && !book.is_empty() {
            match no_room_for_a_game(shared) {
                None => match spawn_from_meta(shared, &m, book) {
                    Ok(_) => true,
                    Err(e) => {
                        tracing::warn!(game = m.id, error = %e, "the game cannot be brought back, it sleeps");
                        false
                    }
                },
                Some(why) => {
                    tracing::info!(game = m.id, why, "no room for the game, it sleeps");
                    false
                }
            }
        } else {
            if book.is_empty() {
                tracing::warn!(game = m.id, "no save of this core build: the game sleeps");
            }
            false
        };
        if !up {
            if m.status != MetaStatus::Hibernating {
                m.status = MetaStatus::Hibernating;
                if let Some(d) = game_dir(shared, m.id) {
                    m.write_logged(&d);
                }
            }
            shared.games.add_dormant(m);
        }
    }
    shared.lobby.send(LobbyMsg::GamesChanged);
    Ok(())
}

/// The saves of a stored game go into the lobby's list, oldest first (the index keeps the newest
/// `ring/` file per game). Saves of another core build are left out with a line in the log (GS-D12).
fn index_saves(
    shared: &Arc<Shared>,
    m: &GameMeta,
    saves: &[(BookEntry, SaveInfo)],
    build: Option<&str>,
) {
    let choices = shared.choices();
    let mut listed: Vec<&(BookEntry, SaveInfo)> = Vec::new();
    for s in saves {
        if build.is_some_and(|b| b != s.1.header.build_id) {
            tracing::info!(
                save = %s.0.file,
                build = %s.1.header.build_id,
                "a save of another core build is not listed"
            );
        } else {
            listed.push(s);
        }
    }
    listed.sort_by_key(|(e, _)| e.seq);
    for (e, info) in listed {
        let seats: Vec<SaveSeat> = info
            .seats
            .iter()
            .map(|s| SaveSeat {
                player: s.player,
                civ: s.civ.clone(),
                civ_name: civ_name(&choices, &s.civ),
                nick: match &s.control {
                    SlotControl::Human { nick } => Some(nick.clone()),
                    SlotControl::Ai => None,
                },
            })
            .collect();
        shared.lobby.send(LobbyMsg::SaveWritten(Box::new(SaveEntry {
            id: save_id(m.id, e.seq),
            game: m.id,
            seq: e.seq,
            kind: e.kind,
            file: e.file.clone(),
            name: info.settings.name.clone(),
            turn: info.header.turn,
            saved_at_ms: info.header.saved_at_ms,
            build_id: info.header.build_id.clone(),
            settings: info.settings.clone(),
            seats,
            host: m.host.clone(),
        })));
    }
}

fn civ_name(choices: &protocol::LobbyChoices, civ: &CivPick) -> String {
    match civ {
        CivPick::Random => "Random".to_owned(),
        CivPick::Civ(id) => choices
            .civs
            .iter()
            .find(|c| &c.id == id)
            .map_or_else(|| id.clone(), |c| c.name.clone()),
    }
}
