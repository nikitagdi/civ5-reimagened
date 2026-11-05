//! `c5sv_retouch <in.c5sv> <out.c5sv> [--name <text>] [--all-ai] [--game <n>] [--seq <n>] [--kind keep|ring]`:
//! rewrites the lobby-facing parts of a server save (the name in `SETG`, who holds the seats in `SEAT`,
//! the game number, seq and kind in the header) and leaves the core's part (`GCS1`) byte for byte.
//!
//! Made for the "Late Huge demo" save: a long all-AI game whose bot humans are given back to the AI, so that
//! `LoadSave` seats whoever loads it in the first AI slot and nobody's nick is reserved.

use game_host::saves::{self, SaveSections};
use protocol::{SaveKind, SlotControl};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: c5sv_retouch <in> <out> [flags]")?;
    let output = args
        .next()
        .ok_or("usage: c5sv_retouch <in> <out> [flags]")?;
    let (mut name, mut all_ai) = (None, false);
    // Players left out of `SEAT`: their room slots stay closed (a dead civilization nobody can take).
    let mut drop: Vec<i8> = Vec::new();
    let mut humans: Vec<(i8, String)> = Vec::new();
    let (mut game, mut seq, mut kind) = (None, None, None);
    while let Some(a) = args.next() {
        let mut value = |n: &str| args.next().ok_or(format!("{n} needs a value"));
        match a.as_str() {
            "--name" => name = Some(value("--name")?),
            "--all-ai" => all_ai = true,
            "--human" => {
                // `<player>=<nick>`: the seat is kept as a human's, so that its slot is reserved in the room
                // and the first AI slot after it is where whoever loads the save sits.
                let v = value("--human")?;
                let (p, n) = v.split_once('=').ok_or("--human <player>=<nick>")?;
                humans.push((p.parse::<i8>().map_err(|e| e.to_string())?, n.to_owned()));
            }
            "--drop" => drop.push(value("--drop")?.parse::<i8>().map_err(|e| e.to_string())?),
            "--game" => game = Some(value("--game")?.parse::<u32>().map_err(|e| e.to_string())?),
            "--seq" => seq = Some(value("--seq")?.parse::<u64>().map_err(|e| e.to_string())?),
            "--kind" => {
                kind = Some(match value("--kind")?.as_str() {
                    "keep" => SaveKind::Keep,
                    "ring" => SaveKind::Ring,
                    other => return Err(format!("--kind {other}: keep or ring")),
                });
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    let bytes = std::fs::read(&input).map_err(|e| format!("{input}: {e}"))?;
    let save = saves::decode(&bytes).map_err(|e| e.to_string())?;
    let mut header = save.info.header.clone();
    let mut sections = SaveSections {
        seats: save.info.seats.clone(),
        settings: save.info.settings.clone(),
        journals: save.journals.clone(),
        cmds: save.cmds.clone(),
        city_memo: save.city_memo.clone().unwrap_or_default(),
    };
    if let Some(n) = name {
        sections.settings.name = n;
    }
    if all_ai {
        for s in &mut sections.seats {
            s.control = SlotControl::Ai;
        }
        sections.journals.clear();
        sections.cmds.clear();
    }
    sections.seats.retain(|s| !drop.contains(&s.player));
    for s in &mut sections.seats {
        if let Some((_, nick)) = humans.iter().find(|(p, _)| *p == s.player) {
            s.control = SlotControl::Human { nick: nick.clone() };
        }
    }
    header.game = game.unwrap_or(header.game);
    header.seq = seq.unwrap_or(header.seq);
    header.kind = kind.unwrap_or(header.kind);
    let out = saves::encode(&header, &save.gcs1, &sections);
    std::fs::write(&output, &out).map_err(|e| format!("{output}: {e}"))?;
    // Read it back the way the gateway does.
    let info = saves::read_header(&out).map_err(|e| e.to_string())?;
    println!(
        "{}: game {} seq {} turn {} build {} name {:?}, {} seats, {} humans",
        output,
        info.header.game,
        info.header.seq,
        info.header.turn,
        info.header.build_id,
        info.settings.name,
        info.seats.len(),
        info.seats
            .iter()
            .filter(|s| matches!(s.control, SlotControl::Human { .. }))
            .count()
    );
    Ok(())
}
