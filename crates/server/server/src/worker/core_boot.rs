//! Building the core from `Init.core` (game-host-mp 4.15, 4.16). The only place of the gateway binary that
//! names a core; `xtask check` keeps `game_host::real`, `RealCore` and `gamecore::Game` inside `worker/`.

use std::path::PathBuf;

use game_host::fixture::FixtureCore;
use game_host::fixture::map::{GALLERY_ENV, Gallery};
use game_host::real::RealCore;
use protocol::{CoreKind, WorkerInit};

/// The core of a worker.
pub enum Booted {
    /// The test world, test hooks set.
    Fixture(Box<FixtureCore>),
    /// The brain: gamecore on the game data of `gamedata_dir` (Linux).
    Real(Box<RealCore>),
    /// No core can be built here: `Fatal { code: 5 }` with this text (the game did not start).
    Unavailable(String),
}

/// Builds the core `init` asks for. The game starts later, in `Worker::new`.
pub fn boot(init: &WorkerInit) -> Booted {
    match &init.core {
        CoreKind::Fixture { huge, wrap_x } => {
            let mut core = FixtureCore::new(*huge, *wrap_x);
            // The full set (card N-R2): a development mode, switched on by a directory with the asset
            // reports (`cargo xtask dev --gallery <dir>`); the worker inherits the variable.
            if let Some(dir) = std::env::var_os(GALLERY_ENV).filter(|d| !d.is_empty()) {
                match Gallery::load(std::path::Path::new(&dir)) {
                    Ok(gallery) => core = core.with_gallery(gallery),
                    Err(e) => return Booted::Unavailable(format!("{GALLERY_ENV}: {e}")),
                }
            }
            if let Some(hooks) = init.cfg.fixture_test {
                core.set_test_hooks(hooks);
            }
            Booted::Fixture(Box::new(core))
        }
        // There is no brain for Windows: the library is built for Linux (game-host-mp 4.15).
        CoreKind::Real { .. } if cfg!(windows) => {
            Booted::Unavailable("the real brain runs on Linux (WSL) only".into())
        }
        CoreKind::Real { gamedata_dir } => {
            let dir = PathBuf::from(gamedata_dir);
            if !dir.join("vfs-index.json").is_file() {
                return Booted::Unavailable(format!(
                    "no game data in {gamedata_dir} (vfs-index.json is missing)"
                ));
            }
            let scripts = game_host::map_scripts::map_script_choices_in(&dir);
            match RealCore::boot(Box::new(move |path| read_game_file(&dir, path))) {
                Ok(core) => Booted::Real(Box::new(core.with_map_scripts(scripts))),
                Err(e) => Booted::Unavailable(e.message),
            }
        }
    }
}

/// A file of the game data by its Civ5 VFS path, from the directory of the export. A path that leaves the
/// directory is no file.
fn read_game_file(dir: &std::path::Path, path: &str) -> Option<Vec<u8>> {
    const SEP: [char; 2] = ['/', '\\'];
    if path
        .split(SEP)
        .any(|part| part == ".." || part.contains(':'))
    {
        return None;
    }
    std::fs::read(dir.join(path.trim_start_matches(SEP))).ok()
}
