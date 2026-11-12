//! `cargo xtask dev --mods <file> [--mod-packs <dir>] [dev flags]`: the dev loop over the dist. The dist server and the
//! dist web client are built (debug, single-threaded as `dev` does), the mods are installed into a directory the
//! server scans, and the server is started by `dev` with the flags it knows.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::build::{self, assemble};
use super::check::{self, Prepared};
use super::dist::{self, Mode, toml_path};

/// What `dev` runs when `--mods` is given.
#[derive(Debug)]
pub struct DevDist {
    /// `--manifest-path` of the dist workspace (package `dist-server`): `target/dist`, or `target/dist-wsl` (Linux
    /// paths inside) for a run in WSL.
    pub manifest: PathBuf,
    /// The web bundle (`--web-root`).
    pub web: PathBuf,
    /// `--config` of the server: the `--config` file of the caller (if any) with `mods_dir` of the installed mods.
    /// A file under `target/dist`; for a run in WSL the paths inside are Linux paths.
    pub config: PathBuf,
}

/// What `dev` takes out of its arguments when `--mods` is given.
#[derive(Debug, Default)]
pub struct ModsArgs {
    /// `--mods <file>`.
    pub list: Option<PathBuf>,
    /// `--mod-packs <dir>`.
    pub packs: Option<PathBuf>,
    /// `--config <file>`: merged with `mods_dir` (without `--mods` it stays among the server flags).
    pub config: Option<PathBuf>,
    /// The other arguments.
    pub rest: Vec<String>,
}

/// `--mods <file>`, `--mod-packs <dir>` and (with `--mods`) `--config <file>` taken out of the `dev` arguments.
pub fn take_args(args: &[String]) -> Result<ModsArgs> {
    let mut list = None;
    let mut packs = None;
    let mut config = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        let slot = match key {
            "--mods" => &mut list,
            "--mod-packs" => &mut packs,
            "--config" => &mut config,
            _ => {
                rest.push(a.clone());
                continue;
            }
        };
        let v = match inline {
            Some(v) => v,
            None => it
                .next()
                .cloned()
                .with_context(|| format!("`{key}` needs a value"))?,
        };
        *slot = Some(PathBuf::from(v));
    }
    if packs.is_some() && list.is_none() {
        bail!("`--mod-packs` needs `--mods <file>`");
    }
    if list.is_none()
        && let Some(c) = config.take()
    {
        // No mods: `--config` is the server's own flag.
        rest.extend(["--config".to_string(), c.to_string_lossy().into_owned()]);
    }
    Ok(ModsArgs {
        list,
        packs,
        config,
        rest,
    })
}

/// Installs the mods of the resolved set into `dir` (`<dir>/<id>/`): each mod directory as the server hashes it (see
/// [`check::copy_without`]), so the installed hash is the one in `mods.lock`. The pack of a mod comes with it when it
/// is at `<mod>/<id>.pack`; a pack in `packs` (`<packs>/<id>.pack`, e.g. an assets-out directory) is copied over it.
/// The directory is made anew on every run.
pub fn install_mods(p: &Prepared, dir: &Path, packs: Option<&Path>) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("cannot clear {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir)?;
    for rm in p.resolution.set.mods() {
        let src = p
            .dir_of(&rm.id, rm.version)
            .with_context(|| format!("mod `{}` has no directory", rm.id))?;
        let to = dir.join(rm.id.as_str());
        check::copy_without(src, &to, true)
            .with_context(|| format!("installing mod `{}`", rm.id))?;
        if let Some(packs) = packs {
            let name = format!("{}.pack", rm.id);
            let pack = packs.join(&name);
            if pack.is_file() {
                std::fs::copy(&pack, to.join(&name))
                    .with_context(|| format!("copying {}", pack.display()))?;
                println!("pack of `{}`: {}", rm.id, pack.display());
            }
        }
    }
    Ok(())
}

/// The path keys of the server config a run in WSL needs as Linux paths.
const CONFIG_PATH_KEYS: [&str; 4] = ["state_dir", "web_root", "assets_root", "gamedata_dir"];

/// The server config of the dev run: the config of the caller (`user`, a TOML text; none: empty) with `mods_dir` set
/// to where the mods are installed. A `mods_dir` of the caller is replaced (`--mods` decides the mods). The caller's
/// file is not written; this is the text of a derived file. For `linux` the path keys that name a drive path are
/// written as `/mnt/<drive>/...` (the server runs in WSL); other paths stay as they are.
pub fn server_config(user: Option<&str>, mods_dir: &Path, linux: bool) -> Result<String> {
    let mut t: toml::Table = match user {
        Some(text) => text.parse().context("cannot parse the --config file")?,
        None => toml::Table::new(),
    };
    if linux {
        for key in CONFIG_PATH_KEYS {
            if let Some(toml::Value::String(v)) = t.get_mut(key)
                && let Ok(l) = crate::windows_to_wsl_path(v)
            {
                *v = l;
            }
        }
    }
    let dir = if linux {
        dist::with_linux_paths(|| toml_path(mods_dir))
    } else {
        toml_path(mods_dir)
    };
    if let Some(old) = t.insert("mods_dir".into(), toml::Value::String(dir))
        && user.is_some()
    {
        println!("note: `mods_dir` of the --config file ({old}) is replaced by the installed mods");
    }
    let body = toml::to_string(&t).context("cannot write the server config")?;
    Ok(format!(
        "# Generated by `cargo xtask dev --mods` ({}); the --config file is not changed.\n{body}",
        if user.is_some() {
            "your --config plus mods_dir"
        } else {
            "mods_dir only"
        }
    ))
}

/// Checks and assembles the dist, builds the web client and the server, installs the mods. The caller starts the
/// server (`cargo run --manifest-path <manifest> -p dist-server --bin civ5-server -- --config <config> ...`).
///
/// `wsl`: the server is built and run inside WSL, so the Windows server build is skipped; `target/dist-wsl` (Linux
/// paths) is generated for it. The mods are installed once, into `target/dist/installed`, which WSL sees as
/// `/mnt/<drive>/.../target/dist/installed`. `user_config` is the `--config` file of the caller.
pub fn prepare_dev(
    root: &Path,
    list: &Path,
    packs: Option<&Path>,
    wsl: bool,
    user_config: Option<&Path>,
) -> Result<DevDist> {
    let (prepared, dist) = assemble(root, list, Mode::All)?;
    let web = build::build_web(root, &dist, false, false)?;
    // The UI v2 panels of the mods and the skins they mount in (`&ui=v2` of the page); `CIV5_SKIP_UI2=1` skips both.
    if super::ui::sections_count(&prepared) > 0 {
        build::build_mod_panels(root, &prepared, &web)?;
        crate::ui::web::build_skins(root, &web)?;
    }
    let mods_dir = dist.dir.join("installed");
    let (manifest, config) = if wsl {
        // Built by `cargo run` inside WSL in the caller; the Windows server build would be for nothing.
        let wsl_dist = dist::generate_wsl(root, &prepared)?;
        (wsl_dist.manifest, dist.dir.join("dev-server.wsl.toml"))
    } else {
        // The server is built by `cargo run` in the caller; build it here so a failure comes before the run.
        build::build_server(root, &dist, false)?;
        (dist.manifest.clone(), dist.dir.join("dev-server.toml"))
    };
    install_mods(&prepared, &mods_dir, packs)?;
    let user = match user_config {
        Some(f) => Some(
            std::fs::read_to_string(f)
                .with_context(|| format!("cannot read the --config file {}", f.display()))?,
        ),
        None => None,
    };
    std::fs::write(&config, server_config(user.as_deref(), &mods_dir, wsl)?)
        .with_context(|| format!("cannot write {}", config.display()))?;
    Ok(DevDist {
        manifest,
        web,
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mods_flags_are_taken_out_and_the_rest_stays() {
        let a = take_args(&v(&[
            "--core",
            "fixture",
            "--mods",
            "x/mods.toml",
            "--mod-packs=packs",
            "--config",
            "c.toml",
            "--assets",
            "a",
        ]))
        .unwrap();
        assert_eq!(a.list.as_deref(), Some(Path::new("x/mods.toml")));
        assert_eq!(a.packs.as_deref(), Some(Path::new("packs")));
        assert_eq!(a.config.as_deref(), Some(Path::new("c.toml")));
        assert_eq!(a.rest, v(&["--core", "fixture", "--assets", "a"]));
        let a = take_args(&v(&["--wsl"])).unwrap();
        assert!(a.list.is_none() && a.packs.is_none());
        assert_eq!(a.rest, v(&["--wsl"]));
        // Without mods `--config` stays a server flag.
        let a = take_args(&v(&["--config=c.toml"])).unwrap();
        assert!(a.config.is_none());
        assert_eq!(a.rest, v(&["--config", "c.toml"]));
        assert!(take_args(&v(&["--mods"])).is_err());
        assert!(take_args(&v(&["--mod-packs", "p"])).is_err());
    }

    #[test]
    fn the_config_names_the_installed_mods() {
        let t: toml::Table = server_config(None, Path::new("dist\\installed"), false)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(t["mods_dir"].as_str(), Some("dist/installed"));
        assert_eq!(t.len(), 1);
    }

    /// A drive path built at run time (the leak scan of the repository reads drive paths in source).
    fn win(rest: &str) -> String {
        format!("{}:/{rest}", 'E')
    }

    #[test]
    fn the_config_of_the_caller_is_merged_with_mods_dir() {
        let user = format!(
            "mods_dir = \"elsewhere\"\nstate_dir = \"{}\"\n[net]\nlisten = \"0.0.0.0:9\"\n",
            win("st")
        );
        let dir = win("w/target/dist/installed");
        let t: toml::Table = server_config(Some(&user), Path::new(&dir), true)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            t["mods_dir"].as_str(),
            Some("/mnt/e/w/target/dist/installed")
        );
        assert_eq!(t["state_dir"].as_str(), Some("/mnt/e/st"));
        assert_eq!(t["net"]["listen"].as_str(), Some("0.0.0.0:9"));
        // On the host the paths stay as they are.
        let t: toml::Table = server_config(Some(&user), Path::new("target/dist/installed"), false)
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(t["mods_dir"].as_str(), Some("target/dist/installed"));
        assert_eq!(t["state_dir"].as_str(), Some(win("st").as_str()));
        assert!(server_config(Some("= bad"), Path::new("x"), false).is_err());
    }

    #[test]
    fn the_wsl_dist_has_linux_paths_and_the_host_dist_keeps_its_own() {
        use crate::mods::gen_server::server_manifest;
        let root = win("w/repo");
        let aa = win("m/aa/aa-server");
        let mods = [("aa-server", Path::new(&aa))];
        let host: toml::Table = server_manifest(Path::new(&root), &mods).parse().unwrap();
        assert_eq!(
            host["dependencies"]["server"]["path"].as_str(),
            Some(win("w/repo/crates/server/server").as_str())
        );
        let linux: toml::Table =
            dist::with_linux_paths(|| server_manifest(Path::new(&root), &mods))
                .parse()
                .unwrap();
        assert_eq!(
            linux["dependencies"]["server"]["path"].as_str(),
            Some("/mnt/e/w/repo/crates/server/server")
        );
        assert_eq!(
            linux["dependencies"]["aa-server"]["path"].as_str(),
            Some("/mnt/e/m/aa/aa-server")
        );
        // The switch is scoped.
        assert_eq!(dist::toml_path(Path::new(&aa)), aa);
    }

    #[test]
    fn an_installed_mod_has_the_hash_of_the_lock() {
        use crate::mods::check::tests::Scratch;
        let s = Scratch::new();
        s.file(
            "m/aa/mod.toml",
            &crate::mods::check::tests::manifest("aa", "0.1.0", "", "MIT"),
        );
        s.file("m/aa/data/x.txt", "x");
        s.file("m/aa/mods.toml", "[[mod]]\npath = \".\"\n");
        s.file("m/aa/mods.lock", "stale");
        s.file("m/aa/target/junk", "junk");
        let list = s.0.join("m/aa/mods.toml");
        let p = check::prepare(&list, &s.0.join("work")).unwrap();
        let out = s.0.join("installed");
        install_mods(&p, &out, None).unwrap();
        assert!(out.join("aa/data/x.txt").is_file());
        assert!(!out.join("aa/mods.toml").exists());
        assert!(!out.join("aa/target").exists());
        let installed = mod_host_server::hash_mod_dir(&out.join("aa")).unwrap();
        let locked = p.resolution.set.mods()[0].content_hash;
        assert_eq!(installed, locked);
    }

    #[test]
    fn a_pack_from_the_packs_directory_is_installed_with_its_mod() {
        use crate::mods::check::tests::Scratch;
        let s = Scratch::new();
        s.file(
            "m/aa/mod.toml",
            &crate::mods::check::tests::manifest("aa", "0.1.0", "", "MIT"),
        );
        s.file("m/aa/mods.toml", "[[mod]]\npath = \".\"\n");
        s.file("packs/aa.pack", "PACK");
        s.file("packs/other.pack", "x");
        let p = check::prepare(&s.0.join("m/aa/mods.toml"), &s.0.join("work")).unwrap();
        let out = s.0.join("installed");
        install_mods(&p, &out, Some(&s.0.join("packs"))).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("aa/aa.pack")).unwrap(),
            "PACK"
        );
        assert!(!out.join("other").exists());
    }
}
