//! `save-import --save FILE.Civ5Save --state DIR --nick NICK --civsim <Lekmod headless/tools>
//!   (--build-id-from ref.c5sv | --build-id ID) [--human-slot N | --civ CIVILIZATION_X] [--ai CIV_Y[,CIV_Z..]]
//!   [--world WORLDSIZE_X] [--map-script PATH] [--speed GAMESPEED_X] [--handicap HANDICAP_X] [--era ERA_X] [--game 1]`
//!
//! The B2 import of the original's saves (`golden world`, `tools/golden/src/world.rs`) as a tool of its own: the
//! `.Civ5Save` is split by `civsim.saveio extract` (python), its pregame block and inflated body go into the core's
//! GCS1 import container, that into a `C5SV` file, and the state dir gets `games/<id>/meta.json` (a sleeping game
//! with one human seat, `NICK`) and the save under `ring/`. A server started on that state dir lists the game; the
//! login of `NICK` wakes it from the save. `--build-id-from` is any `.c5sv` of the same server build (its header names
//! the core build the save has to carry); `--build-id` gives that id as text (the `core_build` of `/admin/metrics`).
//!
//! The seats are the major civs of the save (its slots below 22 that are taken by a human or the computer), each with
//! its own civ. One of them is `NICK`'s: `--human-slot`, else the slot of `--civ`, else the save's first human; every
//! other seat, a human one of the save too, is an AI. `--ai` (optional) is checked against the AI seats. The world
//! size, map script, speed, handicap and start era default to the save's own; the city-states, game options, seeds,
//! max turns and the turn mode (simultaneous, else hybrid) are the save's (its pregame block).
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use protocol::{
    AutoChoices, CivPick, GameSettings, SaveKind, SlotControl, SlotSetup, TimerMode, TurnMode,
};

/// The major civs of the save: slots below this one (`MAX_MAJOR_CIVS`).
const MAX_MAJOR_CIVS: u64 = 22;

/// Game options the core sets itself in a network game; a setup that names them is refused.
const BRAIN_OPTIONS: [&str; 8] = [
    "GAMEOPTION_PITBOSS",
    "GAMEOPTION_END_TURN_TIMER_ENABLED",
    "GAMEOPTION_QUICK_COMBAT",
    "GAMEOPTION_QUICK_MOVEMENT",
    "GAMEOPTION_FAST_HAND",
    "GAMEOPTION_DYNAMIC_TURNS",
    "GAMEOPTION_SIMULTANEOUS_TURNS",
    "GAMEOPTION_FIRSTMOVE",
];

/// One major civ of the save (`envelope.json` `slots`): its slot, its civ, whether a human holds it.
struct Major {
    slot: protocol::PlayerId,
    civ: String,
    human: bool,
}

/// The major civs of `env` in slot order. They have to be slots 0..n with no gap: the core's setup names slots by
/// their index.
fn majors(env: &serde_json::Value) -> Result<Vec<Major>> {
    let slots = env["slots"]
        .as_array()
        .context("envelope.json has no slots")?;
    let mut out = Vec::new();
    for s in slots {
        let slot = s["slot"].as_u64().context("slot number")?;
        let status = s["status"].as_str().unwrap_or("");
        let civ = s["civ"].as_str().unwrap_or("");
        if slot >= MAX_MAJOR_CIVS
            || !matches!(status, "TAKEN" | "COMPUTER")
            || civ.is_empty()
            || civ == "CIVILIZATION_MINOR"
            || civ == "CIVILIZATION_BARBARIAN"
        {
            continue;
        }
        if slot != out.len() as u64 {
            bail!(
                "the save's major civs are not slots 0..n (slot {slot} after {} of them)",
                out.len()
            );
        }
        out.push(Major {
            slot: protocol::PlayerId::try_from(slot)?,
            civ: civ.to_string(),
            human: status == "TAKEN",
        });
    }
    if out.is_empty() {
        bail!("the save has no major civ");
    }
    Ok(out)
}

fn extract(
    civsim: &Path,
    save: &Path,
    dir: &Path,
) -> Result<(Vec<u8>, Vec<u8>, i32, serde_json::Value)> {
    std::fs::create_dir_all(dir)?;
    let code = format!(
        "import sys; sys.path.insert(0, {:?}); from civsim.saveio.__main__ import main; \
         sys.exit(main(['extract', {:?}, '--out', {:?}, '--allow-outside-out']))",
        civsim.to_string_lossy(),
        save.to_string_lossy(),
        dir.to_string_lossy()
    );
    let st = Command::new("python")
        .args(["-I", "-c", &code])
        .stdout(Stdio::null())
        .status()
        .context("cannot start python (civsim.saveio extract)")?;
    if !st.success() {
        bail!("civsim.saveio extract {} failed ({st})", save.display());
    }
    let read = |n: &str| {
        std::fs::read(dir.join(n)).with_context(|| format!("no {n} in {}", dir.display()))
    };
    let env: serde_json::Value = serde_json::from_slice(&read("envelope.json")?)?;
    let mut pregame = read("pregame.bin")?;
    // The exe writes `GameMapTypes` of `writeArchive` as 1 byte, the portable core reads every enum as 4.
    let span = env["spans"]
        .as_array()
        .and_then(|s| s.iter().find(|x| x["name"] == "archive.game_map_type"));
    if let (Some(span), Some(base)) = (span, env["layout"]["pregame"]["offset"].as_u64())
        && span["length"].as_u64() == Some(1)
    {
        let at = usize::try_from(span["offset"].as_u64().context("span offset")? + 1 - base)?;
        if at > pregame.len() {
            bail!("archive.game_map_type is past the pregame");
        }
        pregame.splice(at..at, [0u8; 3]);
    }
    let turn = env["summary"]["turn"].as_i64().unwrap_or(0) as i32;
    Ok((pregame, read("body.bin")?, turn, env))
}

/// The GCS1 import container of the core (Lekmod `rt_save.h`): `header`, `setup`, `pregame`, `body`.
fn import_gcs1(setup_json: &str, pregame: &[u8], body_bytes: &[u8], turn: i32) -> Vec<u8> {
    let header = format!("format=GCS1 2\nimport=civ5save\nturn={turn}\n");
    let sections: [(&str, &[u8]); 4] = [
        ("header", header.as_bytes()),
        ("setup", setup_json.as_bytes()),
        ("pregame", pregame),
        ("body", body_bytes),
    ];
    let mut body = Vec::new();
    for (name, bytes) in sections {
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(bytes);
    }
    let mut out = Vec::with_capacity(32 + body.len());
    out.extend_from_slice(b"GCS1");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&32u32.to_le_bytes());
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&protocol::xxh64(0, &body).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut flags = std::collections::BTreeMap::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a
            .strip_prefix("--")
            .with_context(|| format!("unexpected {a:?}"))?;
        flags.insert(
            name.to_string(),
            it.next()
                .with_context(|| format!("{a} needs a value"))?
                .clone(),
        );
    }
    let get = |k: &str| {
        flags
            .get(k)
            .cloned()
            .with_context(|| format!("--{k} is needed"))
    };
    let save = PathBuf::from(get("save")?);
    let state = PathBuf::from(get("state")?);
    let nick = get("nick")?;
    let game: u32 = flags.get("game").map_or(Ok(1), |g| g.parse())?;
    let civsim = PathBuf::from(get("civsim")?);

    let build_id = match (flags.get("build-id"), flags.get("build-id-from")) {
        (Some(id), None) => id.clone(),
        (None, Some(from)) => {
            let ref_bytes = std::fs::read(from)?;
            game_host::saves::read_header(&ref_bytes)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?
                .header
                .build_id
        }
        _ => bail!("give one of --build-id-from and --build-id"),
    };

    let work = state.join("import-work");
    let (pregame, body, turn, env) = extract(&civsim, &save, &work)?;
    // The extracted files hold the nicknames of the save: nothing of them stays.
    let _ = std::fs::remove_dir_all(&work);
    let summary = &env["summary"];
    // A save of another Lekmod version (its game name carries the tag) has another layout of the DLL's Read: the core
    // would read it wrong (CvGame::Read takes the whole body) and every player would be dead. A save with no tag is let
    // through, as the B4 goldens are.
    if summary["lekmod"].as_bool() == Some(true) && summary["status"] == "unsupported" {
        bail!(
            "the save is not of this gamecore's Lekmod version: {}",
            summary["reason"].as_str().unwrap_or("?")
        );
    }
    // A setting of the save unless the command line names it.
    let setting = |flag: &str, key: &str| -> Result<String> {
        match flags.get(flag) {
            Some(v) => Ok(v.clone()),
            None => summary[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .with_context(|| format!("the save names no {key}: give --{flag}")),
        }
    };
    let world = setting("world", "world_size")?;
    let script = setting("map-script", "map_script")?;
    let speed = setting("speed", "game_speed")?;
    let handicap = setting("handicap", "difficulty")?;
    let era = setting("era", "start_era")?;
    // The pregame block (`archive.*` of the envelope): city-states, game options, seeds, turn mode.
    let fields = &env["fields"];
    let minor_list: Vec<String> = match (
        fields["archive.minor_civ_types"].as_array(),
        fields["archive.minor_nation_civs"].as_array(),
    ) {
        (Some(types), Some(is_minor)) => types
            .iter()
            .zip(is_minor)
            .filter(|(_, m)| m.as_bool() == Some(true))
            .filter_map(|(t, _)| t.as_str().filter(|s| !s.is_empty()).map(str::to_string))
            .collect(),
        _ => Vec::new(),
    };
    let minor_civs = u8::try_from(minor_list.len())?;
    let game_options: Vec<(String, i32)> = fields["archive.game_options"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|p| {
                    let name = p.get(0)?.as_str()?.to_string();
                    let value = i32::try_from(p.get(1)?.as_i64()?).ok()?;
                    Some((name, value))
                })
                .collect()
        })
        .unwrap_or_default();
    let option = |name: &str| {
        game_options
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| *v)
    };
    // The original's simultaneous turns, else its dynamic (hybrid) turns; a sequential save goes hybrid too.
    let simultaneous = option("GAMEOPTION_SIMULTANEOUS_TURNS") == Some(1);
    // The core refuses the options it sets itself in a network game (`kBrainOptions` of the fork's rt_setup.cpp);
    // the save's own values of them stay in its pregame block.
    let game_options: Vec<(String, i32)> = game_options
        .into_iter()
        .filter(|(n, _)| !BRAIN_OPTIONS.contains(&n.as_str()))
        .collect();
    let seed = fields["archive.sync_random_seed"]
        .as_u64()
        .map_or(Ok(1), u32::try_from)?;
    let map_seed = fields["archive.map_random_seed"]
        .as_u64()
        .map(u32::try_from)
        .transpose()?;
    let max_turns = fields["archive.max_turns"]
        .as_u64()
        .map_or(Ok(0), u32::try_from)?;

    let majors = majors(&env)?;
    let human = match (flags.get("human-slot"), flags.get("civ")) {
        (Some(n), _) => {
            let n: protocol::PlayerId = n.parse()?;
            majors
                .iter()
                .position(|m| m.slot == n)
                .with_context(|| format!("slot {n} is not a major civ of the save"))?
        }
        (None, Some(civ)) => majors
            .iter()
            .position(|m| &m.civ == civ)
            .with_context(|| format!("{civ} is not a major civ of the save"))?,
        (None, None) => majors
            .iter()
            .position(|m| m.human)
            .context("the save has no human: give --human-slot")?,
    };
    if let Some(civ) = flags.get("civ")
        && &majors[human].civ != civ
    {
        bail!(
            "--civ {civ}: the human slot {} is {}",
            majors[human].slot,
            majors[human].civ
        );
    }
    if let Some(list) = flags.get("ai") {
        for civ in list.split(',') {
            if !majors
                .iter()
                .enumerate()
                .any(|(i, m)| i != human && m.civ == civ)
            {
                bail!("--ai {civ}: no AI seat of the save has that civ");
            }
        }
    }

    // A network game, as the server makes them: the save is the original's multiplayer game, the core runs its
    // human-phase frame (`RtSession::NetworkFrame`) only for a network game. The core takes the game itself from the
    // pregame block; the setup only has to describe it.
    let mut setup = gamecore::Setup::network(
        seed,
        majors
            .iter()
            .enumerate()
            .map(|(i, m)| {
                if i == human {
                    gamecore::SlotSetup::human(m.civ.clone(), nick.clone())
                } else {
                    gamecore::SlotSetup::ai(m.civ.clone())
                }
            })
            .collect(),
    );
    setup.map_seed = map_seed;
    setup.turn_mode = if simultaneous {
        gamecore::TurnMode::Simultaneous
    } else {
        gamecore::TurnMode::Hybrid
    };
    setup.world_size = world.clone();
    setup.map_script = script.replace('\\', "/");
    setup.handicap = handicap.clone();
    setup.game_speed = speed.clone();
    setup.start_era = era.clone();
    setup.minor_civs = gamecore::MinorCivs::List(minor_list);
    setup.game_options = game_options.clone();
    setup.max_turns = max_turns;
    let gcs1 = import_gcs1(&setup.to_json(), &pregame, &body, turn);
    let settings = GameSettings {
        name: format!("{nick}'s golden"),
        world_size: world,
        map_script: setup.map_script.clone(),
        map_options: vec![],
        speed,
        start_era: era,
        handicap,
        minor_civs,
        turn_mode: if simultaneous {
            TurnMode::Simultaneous
        } else {
            TurnMode::Hybrid
        },
        timer: TimerMode::Off,
        game_options,
        seed: Some(seed),
        map_seed,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
        mods: Vec::new(),
    };
    let slots: Vec<SlotSetup> = majors
        .iter()
        .enumerate()
        .map(|(i, m)| SlotSetup {
            player: m.slot,
            civ: CivPick::Civ(m.civ.clone()),
            control: if i == human {
                SlotControl::Human { nick: nick.clone() }
            } else {
                SlotControl::Ai
            },
        })
        .collect();
    let seats: Vec<serde_json::Value> = slots
        .iter()
        .map(|s| {
            let nick = match &s.control {
                SlotControl::Human { nick } => Some(nick.clone()),
                SlotControl::Ai => None,
            };
            serde_json::json!({"player": s.player, "civ": s.civ, "nick": nick, "kicked": null})
        })
        .collect();
    let file = game_host::saves::encode(
        &game_host::saves::SaveHeader {
            seq: 1,
            game,
            turn,
            saved_at_ms: 1_793_400_000_000,
            kind: SaveKind::Ring,
            build_id: build_id.clone(),
        },
        &gcs1,
        &game_host::saves::SaveSections {
            seats: slots,
            settings: settings.clone(),
            journals: vec![],
            cmds: vec![],
            city_memo: vec![],
        },
    );
    let gdir = state.join("games").join(game.to_string());
    let rel = game_host::saves::rel_path(SaveKind::Ring, 1, turn);
    let path = gdir.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, file)?;
    let meta = serde_json::json!({
        "format": 1,
        "id": game,
        "name": settings.name,
        "status": "Hibernating",
        "settings": serde_json::to_value(&settings)?,
        "host": nick,
        "seats": seats,
        "seed": seed,
        "core_build": build_id,
        "created_ms": 1_793_400_000_000u64,
        "finished_ms": null,
        "turn": turn,
    });
    std::fs::write(
        gdir.join("meta.json"),
        serde_json::to_string_pretty(&meta)? + "\n",
    )?;
    std::fs::write(
        state.join("ids.json"),
        format!(
            "{{\n  \"format\": 1,\n  \"next_game\": {},\n  \"next_room\": 1\n}}\n",
            game + 1
        ),
    )?;
    let ai: Vec<String> = majors
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != human)
        .map(|(_, m)| format!("{} {}", m.slot, m.civ))
        .collect();
    println!(
        "{}: game {game}, turn {turn}, {world}, {minor_civs} minor civs; seat {} {} = {nick}; AI: {}",
        path.display(),
        majors[human].slot,
        majors[human].civ,
        ai.join(", "),
        world = settings.world_size,
    );
    Ok(())
}
