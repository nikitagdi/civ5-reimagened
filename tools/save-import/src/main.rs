//! `save-import --save FILE.Civ5Save --state DIR --nick NICK --civ CIVILIZATION_X --ai CIVILIZATION_Y
//!   --world WORLDSIZE_DUEL --map-script PATH --speed GAMESPEED_QUICK --handicap HANDICAP_PRINCE
//!   --build-id-from ref.c5sv --civsim <Lekmod headless/tools> [--game 1]`
//!
//! The B2 import of the original's saves (`golden world`, `tools/golden/src/world.rs`) as a tool of its own: the
//! `.Civ5Save` is split by `civsim.saveio extract` (python), its pregame block and inflated body go into the core's
//! GCS1 import container, that into a `C5SV` file, and the state dir gets `games/<id>/meta.json` (a sleeping game
//! whose seat 0 is `NICK`) and the save under `ring/`. A server started on that state dir lists the game; the login of
//! `NICK` wakes it from the save. `--build-id-from` is any `.c5sv` of the same server build (its header names the
//! core build the save has to carry).
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use protocol::{
    AutoChoices, CivPick, GameSettings, SaveKind, SlotControl, SlotSetup, TimerMode, TurnMode,
};

fn extract(civsim: &Path, save: &Path, dir: &Path) -> Result<(Vec<u8>, Vec<u8>, i32)> {
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
    Ok((pregame, read("body.bin")?, turn))
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
    let (civ, ai) = (get("civ")?, get("ai")?);
    let game: u32 = flags.get("game").map_or(Ok(1), |g| g.parse())?;
    let civsim = PathBuf::from(get("civsim")?);
    let (world, script, speed, handicap) = (
        get("world")?,
        get("map-script")?,
        get("speed")?,
        get("handicap")?,
    );
    let era = flags
        .get("era")
        .cloned()
        .unwrap_or_else(|| "ERA_ANCIENT".into());

    let ref_bytes = std::fs::read(get("build-id-from")?)?;
    let build_id = game_host::saves::read_header(&ref_bytes)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?
        .header
        .build_id;

    let work = state.join("import-work");
    let (pregame, body, turn) = extract(&civsim, &save, &work)?;
    // A network game, as the server makes them: the save is the original's multiplayer game, the core runs its
    // human-phase frame (`RtSession::NetworkFrame`) only for a network game.
    let mut setup = gamecore::Setup::network(
        1,
        vec![
            gamecore::SlotSetup::human(civ.clone(), nick.clone()),
            gamecore::SlotSetup::ai(ai.clone()),
        ],
    );
    setup.turn_mode = gamecore::TurnMode::Hybrid;
    setup.world_size = world.clone();
    setup.map_script = script.clone();
    setup.handicap = handicap.clone();
    setup.game_speed = speed.clone();
    setup.start_era = era.clone();
    setup.minor_civs = gamecore::MinorCivs::Count(0);
    let gcs1 = import_gcs1(&setup.to_json(), &pregame, &body, turn);
    let settings = GameSettings {
        name: format!("{nick}'s golden"),
        world_size: world,
        map_script: script,
        map_options: vec![],
        speed,
        start_era: era,
        handicap,
        minor_civs: 0,
        turn_mode: TurnMode::Hybrid,
        timer: TimerMode::Off,
        game_options: vec![],
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
    };
    let slots = vec![
        SlotSetup {
            player: 0,
            civ: CivPick::Civ(civ.clone()),
            control: SlotControl::Human { nick: nick.clone() },
        },
        SlotSetup {
            player: 1,
            civ: CivPick::Civ(ai.clone()),
            control: SlotControl::Ai,
        },
    ];
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
        "seats": [
            {"player": 0, "civ": {"Civ": civ}, "nick": nick, "kicked": null},
            {"player": 1, "civ": {"Civ": ai}, "nick": null, "kicked": null},
        ],
        "seed": 1,
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
    let _ = std::fs::remove_dir_all(&work);
    println!(
        "{}: game {game}, turn {turn}, seat 0 = {nick}",
        path.display()
    );
    Ok(())
}
