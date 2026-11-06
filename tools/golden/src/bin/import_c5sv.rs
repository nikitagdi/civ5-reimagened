//! `import_c5sv` (B4 M5 WO-I): a `.Civ5Save` of the original as a game of the server (AC-S2 scenes S4 and S5). Writes the
//! game directory the server wakes from: `<out>/meta.json` and `<out>/keep/00000001-t<turn>.c5sv` (the GCS1 import container of the
//! core in a C5SV file, as `golden world --save` makes it for the worker). The server must run the core build `--build-id`
//! (the `core_build` of the `meta.json` of any game of that server) and the human nick logs in with the dev password.
//!
//!   import_c5sv --save S5.Civ5Save --civsim <Lekmod headless/tools> --out <dir> --build-id <id> --nick golden --game 1
//!               --human CIVILIZATION_ETHIOPIA --ai CIVILIZATION_MACEDONIA[,..] --world-size WORLDSIZE_DUEL
//!               --map-script Assets/Maps/Pangaea.lua --handicap HANDICAP_PRINCE --speed GAMESPEED_QUICK --era ERA_ANCIENT
//!               [--minor-civs 0] [--workdir <dir for the extract>]
use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use game_host::saves::{SaveHeader, SaveSections, encode, file_name};
use golden::world::{extract_save, import_gcs1};
use protocol::{
    AutoChoices, CivPick, GameSettings, SaveKind, SlotControl, SlotSetup, TimerMode, TurnMode,
};

fn main() -> Result<()> {
    let mut a: HashMap<String, String> = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let k = k.strip_prefix("--").context("flags only")?.to_owned();
        a.insert(k, it.next().context("a flag needs a value")?);
    }
    let get = |k: &str| {
        a.get(k)
            .cloned()
            .with_context(|| format!("--{k} is required"))
    };
    let save = PathBuf::from(get("save")?);
    let out = PathBuf::from(get("out")?);
    let work = a
        .get("workdir")
        .map(PathBuf::from)
        .unwrap_or_else(|| out.join("extract"));
    let parts = extract_save(&PathBuf::from(get("civsim")?), &save, &work)?;
    let human = get("human")?;
    let ais: Vec<String> = a
        .get("ai")
        .map(|s| {
            s.split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let minor: u32 = a
        .get("minor-civs")
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(0);
    let nick = get("nick")?;
    let game: u32 = a.get("game").map(|s| s.parse()).transpose()?.unwrap_or(1);
    let setup = gamecore::Setup {
        seed: 0,
        map_seed: None,
        // The server runs a network game (the humans are seated by CONNECT); the save is the exe's multiplayer game.
        game_type: if a.contains_key("single") {
            gamecore::GameType::Single
        } else {
            gamecore::GameType::Network
        },
        players: std::iter::once(gamecore::SlotSetup::human(human.clone(), nick.clone()))
            .chain(ais.iter().map(|c| gamecore::SlotSetup::ai(c.clone())))
            .collect(),
        turn_mode: gamecore::TurnMode::Simultaneous,
        human_civ: human.clone(),
        ai_count: ais.len() as u32,
        world_size: get("world-size")?,
        map_script: get("map-script")?,
        handicap: get("handicap")?,
        game_speed: get("speed")?,
        start_era: get("era")?,
        minor_civs: gamecore::MinorCivs::Count(minor),
        ..gamecore::Setup::default()
    };
    let gcs1 = import_gcs1(&setup.to_json(), &parts);
    let settings = GameSettings {
        name: "golden".into(),
        world_size: get("world-size")?,
        map_script: get("map-script")?,
        map_options: vec![],
        speed: get("speed")?,
        start_era: get("era")?,
        handicap: get("handicap")?,
        minor_civs: u8::try_from(minor)?,
        turn_mode: TurnMode::Simultaneous,
        timer: TimerMode::Off,
        game_options: vec![],
        seed: Some(0),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
        mods: Vec::new(),
    };
    let mut slots = vec![SlotSetup {
        player: 0,
        civ: CivPick::Civ(human),
        control: SlotControl::Human { nick },
    }];
    for (i, civ) in ais.iter().enumerate() {
        slots.push(SlotSetup {
            player: (i + 1) as _,
            civ: CivPick::Civ(civ.clone()),
            control: SlotControl::Ai,
        });
    }
    if slots.len() < 2 {
        bail!("--ai needs at least one AI civilization");
    }
    let bytes = encode(
        &SaveHeader {
            seq: 1,
            game,
            turn: parts.turn,
            saved_at_ms: 0,
            kind: SaveKind::Keep,
            build_id: get("build-id")?,
        },
        &gcs1,
        &SaveSections {
            seats: slots,
            settings,
            journals: vec![],
            cmds: vec![],
            city_memo: vec![],
        },
    );
    let keep = out.join("keep");
    std::fs::create_dir_all(&keep)?;
    let name = file_name(1, parts.turn);
    std::fs::write(keep.join(&name), bytes)?;
    println!("turn={} file=keep/{name}", parts.turn);
    Ok(())
}
