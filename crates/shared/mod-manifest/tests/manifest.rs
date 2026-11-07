//! `mod.toml` parsing: a valid manifest, then one test row per kind of invalid manifest.

use mod_manifest::{
    CHANNEL_PAYLOAD_CEILING, ClientPolicy, Manifest, ManifestError, ModId, Part, SyncClass,
    Version, VersionReq,
};

const FULL: &str = r#"
[mod]
id        = "voxel-demo"
name      = "VoxelDemo"
version   = "0.1.0"
api       = "^1"
license   = "LicenseRef-Proprietary"
authors   = ["Example Author"]
sync      = "cosmetic"
client    = "optional"
parts     = ["server-rs", "client-rs", "assets", "lua-ui"]

[deps]
some-lib = ">=1.2, <2"

[optional]
nice-to-have = "^0.3"

[conflicts]
old-mine = "*"

[order]
after  = ["some-lib"]
before = ["late-mod"]

[rust]
server    = { crate = "voxel-demo-server", entry = "voxel_demo_server::register" }
client    = { crate = "voxel-demo-client", entry = "voxel_demo_client::register" }
caps      = ["net.channel", "world.read", "input.claim", "camera", "render.pipeline", "hud", "kv"]
state_max = "4MiB"

[[channels]]
id     = 0
name   = "edits"
c2s_max = 2048
s2c_max = 16384
rate   = "30/s"
burst  = 60

[[channels]]
id   = 1
name = "poses"

[content]
assets = "assets/"
lua_ui = [{ entry = "InGameUIAddin", file = "lua/ui/Foo.lua" }]
"#;

const GAMEPLAY: &str = r#"
[mod]
id      = "coretweak"
name    = "Core tweak"
version = "1.0.0"
api     = "^1"
license = "MIT"
sync    = "gameplay"
parts   = ["data", "lua-gameplay", "core-patch"]

[content]
data         = ["data/*.sql"]
loc          = ["loc/*.sql"]
lua_gameplay = ["lua/gameplay/*.lua"]

[core]
base    = "reimagened/v34.15@abc123"
patches = ["core/*.patch"]
defines = ["LEKMOD_MODCORE_FOO"]
"#;

fn with(from: &str, to: &str) -> String {
    assert!(FULL.contains(from), "fixture has no {from:?}");
    FULL.replacen(from, to, 1)
}

fn parse_err(src: &str) -> ManifestError {
    match Manifest::parse(src) {
        Ok(_) => panic!("expected an error for:\n{src}"),
        Err(e) => e,
    }
}

/// The manifest must fail with an error about exactly this key path.
#[track_caller]
fn assert_path(src: &str, path: &str) {
    let e = parse_err(src);
    assert_eq!(e.path(), Some(path), "wrong path in: {e}");
}

#[test]
fn full_manifest_parses() {
    let m = Manifest::parse(FULL).unwrap();
    assert_eq!(m.id.as_str(), "voxel-demo");
    assert_eq!(m.version, Version::new(0, 1, 0));
    assert_eq!(m.sync, SyncClass::Cosmetic);
    assert_eq!(m.client, Some(ClientPolicy::Optional));
    assert_eq!(m.client_policy(), ClientPolicy::Optional);
    assert_eq!(m.authors, ["Example Author"]);
    assert_eq!(m.parts.len(), 4);
    assert!(m.has_part(Part::ServerRs) && m.has_part(Part::LuaUi) && !m.has_part(Part::Data));
    let lib: ModId = "some-lib".parse().unwrap();
    assert_eq!(m.deps[&lib], ">=1.2, <2".parse::<VersionReq>().unwrap());
    assert!(
        m.optional
            .contains_key(&"nice-to-have".parse::<ModId>().unwrap())
    );
    assert!(m.conflicts[&"old-mine".parse::<ModId>().unwrap()].is_any());
    assert!(m.order.after.contains(&lib));
    assert_eq!(
        m.rust.server.as_ref().unwrap().entry,
        "voxel_demo_server::register"
    );
    assert_eq!(
        m.rust.client.as_ref().unwrap().crate_name,
        "voxel-demo-client"
    );
    assert_eq!(m.rust.caps.len(), 7);
    assert_eq!(m.rust.state_max, 4 * 1024 * 1024);
    assert_eq!(m.channels.len(), 2);
    assert_eq!(
        (
            m.channels[0].c2s_max,
            m.channels[0].s2c_max,
            m.channels[0].rate_per_s,
            m.channels[0].burst
        ),
        (2048, 16384, 30, 60)
    );
    assert_eq!(m.content.assets.as_deref(), Some("assets/"));
    assert_eq!(m.content.lua_ui[0].entry, "InGameUIAddin");
    assert!(m.core.is_none());
}

#[test]
fn channel_defaults() {
    let m = Manifest::parse(FULL).unwrap();
    let c = &m.channels[1];
    assert_eq!((c.id, c.name.as_str()), (1, "poses"));
    assert_eq!(
        (c.c2s_max, c.s2c_max, c.rate_per_s, c.burst),
        (4096, 16384, 30, 60)
    );
}

#[test]
fn gameplay_manifest_with_core_patch_parses() {
    let m = Manifest::parse(GAMEPLAY).unwrap();
    assert_eq!(m.client_policy(), ClientPolicy::Required);
    let core = m.core.unwrap();
    assert_eq!(core.defines, ["LEKMOD_MODCORE_FOO"]);
    assert_eq!(m.content.data, ["data/*.sql"]);
    assert_eq!(m.content.lua_gameplay.len(), 1);
}

#[test]
fn minimal_manifest_and_policy_defaults() {
    let src = |sync: &str, extra: &str| {
        format!(
            "[mod]\nid=\"ab\"\nname=\"A\"\nversion=\"1.0.0\"\napi=\"*\"\nlicense=\"MIT\"\nsync=\"{sync}\"\nparts=[\"client-rs\"]\n{extra}\n[rust]\nclient={{crate=\"ab-client\",entry=\"ab_client::register\"}}\n"
        )
    };
    let c = Manifest::parse(&src("client", "")).unwrap();
    assert_eq!(c.client_policy(), ClientPolicy::Optional);
    assert!(c.authors.is_empty() && c.deps.is_empty() && c.channels.is_empty());
    // Cosmetic without `client` fails closed.
    let s = Manifest::parse(&src("cosmetic", "")).unwrap();
    assert_eq!(s.client, None);
    assert_eq!(s.client_policy(), ClientPolicy::Required);
}

#[test]
fn part_class_matrix_matches_the_design() {
    use Part::*;
    use SyncClass::*;
    // design-mod-system.md section 1.
    let allowed = [
        (Data, [true, false, false]),
        (LuaGameplay, [true, false, false]),
        (CorePatch, [true, false, false]),
        (LuaUi, [true, true, true]),
        (Assets, [true, true, true]),
        (ServerRs, [true, true, false]),
        (ClientRs, [true, true, true]),
    ];
    for (part, row) in allowed {
        for (class, want) in [Gameplay, Cosmetic, Client].into_iter().zip(row) {
            assert_eq!(part.allowed_in(class), want, "{part:?} in {class:?}");
        }
    }
    assert_eq!(Part::ALL.len(), 7);
    for p in Part::ALL {
        assert_eq!(Part::parse(p.as_str()), Some(p));
    }
}

#[test]
fn syntax_error_is_reported() {
    assert!(matches!(parse_err("[mod\nid="), ManifestError::Syntax(_)));
    assert_eq!(parse_err("not toml at all ===").path(), None);
}

#[test]
fn missing_required_keys() {
    assert!(matches!(parse_err(""), ManifestError::Missing { .. }));
    assert_path("", "mod");
    for key in ["id", "name", "version", "api", "license", "sync", "parts"] {
        let line = FULL
            .lines()
            .find(|l| l.trim_start().starts_with(key) && l.contains('='))
            .unwrap();
        let src = FULL.replacen(line, "", 1);
        assert_path(&src, &format!("mod.{key}"));
        assert!(
            matches!(parse_err(&src), ManifestError::Missing { .. }),
            "{key}"
        );
    }
}

#[test]
fn unknown_keys_carry_their_path() {
    assert_path(
        &with(
            "name      = \"VoxelDemo\"",
            "name = \"VoxelDemo\"\ncolour = \"red\"",
        ),
        "mod.colour",
    );
    assert_path(&format!("{FULL}\n[bogus]\nx = 1\n"), "bogus");
    assert_path(
        &with("name = \"poses\"", "name = \"poses\"\nfoo = 1"),
        "channels[1].foo",
    );
    assert_path(
        &with(
            "entry = \"voxel_demo_server::register\" }",
            "entry = \"voxel_demo_server::register\", x = 1 }",
        ),
        "rust.server.x",
    );
    assert_path(
        &with("[order]\n", "[order]\nbetween = []\n"),
        "order.between",
    );
    assert_path(
        &with(
            "assets = \"assets/\"",
            "assets = \"assets/\"\nmusic = \"m/\"",
        ),
        "content.music",
    );
    assert_path(
        &with(
            "file = \"lua/ui/Foo.lua\" }",
            "file = \"lua/ui/Foo.lua\", z = 1 }",
        ),
        "content.lua_ui[0].z",
    );
}

#[test]
fn wrong_types() {
    let e = parse_err(&with("id        = \"voxel-demo\"", "id = 5"));
    assert!(
        matches!(e, ManifestError::WrongType { ref path, .. } if path == "mod.id"),
        "{e}"
    );
    assert_path(
        &with(
            "parts     = [\"server-rs\", \"client-rs\", \"assets\", \"lua-ui\"]",
            "parts = \"server-rs\"",
        ),
        "mod.parts",
    );
    assert_path(
        &with(
            "parts     = [\"server-rs\", \"client-rs\", \"assets\", \"lua-ui\"]",
            "parts = [\"server-rs\", 3]",
        ),
        "mod.parts[1]",
    );
    assert_path(
        &with("some-lib = \">=1.2, <2\"", "some-lib = 2"),
        "deps.some-lib",
    );
    assert_path(&with("id     = 0\n", "id = \"zero\"\n"), "channels[0].id");
    assert_path(
        &with(
            "authors   = [\"Example Author\"]",
            "authors = \"Example Author\"",
        ),
        "mod.authors",
    );
}

#[test]
fn invalid_id() {
    for bad in [
        "Mine",
        "m",
        "1abc",
        "voxel_demo",
        "voxel demo",
        &"a".repeat(33),
        "-ab",
        "voxel.demo",
        "",
    ] {
        assert_path(
            &with("id        = \"voxel-demo\"", &format!("id = {bad:?}")),
            "mod.id",
        );
    }
    for good in ["ab", "a-b", "a1", "hello-world-2", &"a".repeat(32)] {
        Manifest::parse(&with(
            "id        = \"voxel-demo\"",
            &format!("id = {good:?}"),
        ))
        .unwrap();
    }
}

#[test]
fn invalid_scalars() {
    assert_path(
        &with("version   = \"0.1.0\"", "version = \"0.1\""),
        "mod.version",
    );
    assert_path(
        &with("version   = \"0.1.0\"", "version = \"0.1.0-beta\""),
        "mod.version",
    );
    assert_path(&with("api       = \"^1\"", "api = \"1\""), "mod.api");
    assert_path(&with("api       = \"^1\"", "api = \"\""), "mod.api");
    assert_path(
        &with("license   = \"LicenseRef-Proprietary\"", "license = \"\""),
        "mod.license",
    );
    assert_path(
        &with(
            "license   = \"LicenseRef-Proprietary\"",
            "license = \"MIT; rm -rf\"",
        ),
        "mod.license",
    );
    assert_path(
        &with("sync      = \"cosmetic\"", "sync = \"synced\""),
        "mod.sync",
    );
    assert_path(
        &with("client    = \"optional\"", "client = \"maybe\""),
        "mod.client",
    );
    assert_path(
        &with("name      = \"VoxelDemo\"", "name = \"\""),
        "mod.name",
    );
    assert_path(
        &with("authors   = [\"Example Author\"]", "authors = [\"\"]"),
        "mod.authors[0]",
    );
    Manifest::parse(&with(
        "license   = \"LicenseRef-Proprietary\"",
        "license = \"MIT OR (Apache-2.0 WITH LLVM-exception)\"",
    ))
    .unwrap();
}

#[test]
fn invalid_parts() {
    let parts = "parts     = [\"server-rs\", \"client-rs\", \"assets\", \"lua-ui\"]";
    assert_path(&with(parts, "parts = []"), "mod.parts");
    assert_path(
        &with(parts, "parts = [\"server-rs\", \"client-rs\", \"wasm\"]"),
        "mod.parts[2]",
    );
    assert_path(
        &with(
            parts,
            "parts = [\"server-rs\", \"client-rs\", \"assets\", \"assets\"]",
        ),
        "mod.parts[3]",
    );
}

#[test]
fn part_not_allowed_in_class() {
    let parts = "parts     = [\"server-rs\", \"client-rs\", \"assets\", \"lua-ui\"]";
    // data / lua-gameplay / core-patch only in gameplay mods.
    for p in ["data", "lua-gameplay", "core-patch"] {
        let e = parse_err(&with(
            parts,
            &format!("parts = [\"server-rs\", \"client-rs\", {p:?}]"),
        ));
        assert_eq!(e.path(), Some("mod.parts"));
        assert!(e.to_string().contains(p), "{e}");
    }
    // A client-only mod has no server half.
    let src = with(
        "sync      = \"cosmetic\"\nclient    = \"optional\"",
        "sync = \"client\"",
    );
    assert_path(&src, "mod.parts");
}

#[test]
fn client_key_is_for_cosmetic_only() {
    let e = parse_err(&GAMEPLAY.replacen(
        "sync    = \"gameplay\"",
        "sync = \"gameplay\"\nclient = \"optional\"",
        1,
    ));
    assert_eq!(e.path(), Some("mod.client"));
}

#[test]
fn client_only_mod_cannot_have_channels_or_net_cap() {
    let base = "[mod]\nid=\"ui-tweak\"\nname=\"T\"\nversion=\"1.0.0\"\napi=\"^1\"\nlicense=\"MIT\"\nsync=\"client\"\nparts=[\"client-rs\"]\n[rust]\nclient={crate=\"t-client\",entry=\"t_client::register\"}\n";
    Manifest::parse(base).unwrap();
    let with_cap = base.replacen("[rust]\n", "[rust]\ncaps=[\"net.channel\"]\n", 1);
    assert_path(&with_cap, "rust.caps");
    let with_ch = format!("{base}[[channels]]\nid=0\nname=\"x\"\n");
    assert_path(&with_ch, "channels");
    let both = format!("{with_cap}[[channels]]\nid=0\nname=\"x\"\n");
    assert_path(&both, "rust.caps");
    // A harmless client capability is fine.
    Manifest::parse(&base.replacen("[rust]\n", "[rust]\ncaps=[\"hud\"]\n", 1)).unwrap();
}

#[test]
fn channels_and_net_cap_go_together() {
    assert_path(
        &with("caps      = [\"net.channel\", ", "caps      = ["),
        "channels",
    );
    let no_channels = FULL.split("[[channels]]").next().unwrap().to_string()
        + "[content]\nassets = \"assets/\"\n";
    // `[rust]` ends before the first [[channels]] in the fixture; the cap is left without channels.
    assert_path(&no_channels, "rust.caps");
}

#[test]
fn channel_limits() {
    assert_path(
        &with("c2s_max = 2048", "c2s_max = 0"),
        "channels[0].c2s_max",
    );
    assert_path(
        &with(
            "c2s_max = 2048",
            &format!("c2s_max = {}", CHANNEL_PAYLOAD_CEILING + 1),
        ),
        "channels[0].c2s_max",
    );
    assert_path(
        &with("s2c_max = 16384", "s2c_max = 100000"),
        "channels[0].s2c_max",
    );
    assert_path(
        &with("s2c_max = 16384", "s2c_max = \"64KiB\""),
        "channels[0].s2c_max",
    );
    assert_path(
        &with("s2c_max = 16384", "s2c_max = \"lots\""),
        "channels[0].s2c_max",
    );
    assert_path(
        &with("s2c_max = 16384", "s2c_max = -5"),
        "channels[0].s2c_max",
    );
    let m = Manifest::parse(&with(
        "s2c_max = 16384",
        &format!("s2c_max = {CHANNEL_PAYLOAD_CEILING}"),
    ))
    .unwrap();
    assert_eq!(m.channels[0].s2c_max, CHANNEL_PAYLOAD_CEILING);
    let m = Manifest::parse(&with("s2c_max = 16384", "s2c_max = \"56KiB\"")).unwrap();
    assert_eq!(m.channels[0].s2c_max, CHANNEL_PAYLOAD_CEILING);
    for bad in ["30", "0/s", "30/m", "/s", "x/s", "10001/s", "-3/s", "3.5/s"] {
        assert_path(
            &with("rate   = \"30/s\"", &format!("rate = {bad:?}")),
            "channels[0].rate",
        );
    }
    assert_path(&with("burst  = 60", "burst = 0"), "channels[0].burst");
    assert_path(&with("burst  = 60", "burst = 1000000"), "channels[0].burst");
    assert_path(&with("id     = 0\n", "id = 256\n"), "channels[0].id");
    assert_path(&with("id     = 0\n", "id = -1\n"), "channels[0].id");
    assert_path(&with("id   = 1\n", "id = 0\n"), "channels[1].id");
    assert_path(
        &with("name = \"poses\"", "name = \"edits\""),
        "channels[1].name",
    );
    assert_path(
        &with("name   = \"edits\"", "name = \"Edits\""),
        "channels[0].name",
    );
}

#[test]
fn state_max_belongs_to_rust() {
    assert_path(
        &with("name = \"poses\"", "name = \"poses\"\nstate_max = 10"),
        "channels[1].state_max",
    );
    assert_path(
        &with("state_max = \"4MiB\"", "state_max = \"65MiB\""),
        "rust.state_max",
    );
    assert_path(
        &with("state_max = \"4MiB\"", "state_max = 0"),
        "rust.state_max",
    );
    let m = Manifest::parse(&with("state_max = \"4MiB\"", "state_max = 512")).unwrap();
    assert_eq!(m.rust.state_max, 512);
    // State of a mod without a server half makes no sense.
    let client_only = "[mod]\nid=\"ab\"\nname=\"A\"\nversion=\"1.0.0\"\napi=\"*\"\nlicense=\"MIT\"\nsync=\"client\"\nparts=[\"client-rs\"]\n[rust]\nstate_max=\"1MiB\"\nclient={crate=\"a-client\",entry=\"a_client::register\"}\n";
    assert_path(client_only, "rust.state_max");
}

#[test]
fn tick_hz_is_0_to_30_and_needs_a_server_half() {
    // Absent: never ticks.
    let m = Manifest::parse(&with("state_max = \"4MiB\"", "state_max = 512")).unwrap();
    assert_eq!(m.rust.tick_hz, 0);
    for hz in [0u32, 1, 20, 30] {
        let m = Manifest::parse(&with(
            "state_max = \"4MiB\"",
            &format!("state_max = 512\ntick_hz = {hz}"),
        ))
        .unwrap();
        assert_eq!(m.rust.tick_hz, hz);
    }
    for bad in ["31", "-1", "\"20\"", "2.5"] {
        assert_path(
            &with("state_max = \"4MiB\"", &format!("tick_hz = {bad}")),
            "rust.tick_hz",
        );
    }
    let client_only = "[mod]\nid=\"ab\"\nname=\"A\"\nversion=\"1.0.0\"\napi=\"*\"\nlicense=\"MIT\"\nsync=\"client\"\nparts=[\"client-rs\"]\n[rust]\ntick_hz=10\nclient={crate=\"a-client\",entry=\"a_client::register\"}\n";
    assert_path(client_only, "rust.tick_hz");
    // 0 on a client-only mod is the default written out: fine.
    let zero = client_only.replace("tick_hz=10", "tick_hz=0");
    assert_eq!(Manifest::parse(&zero).unwrap().rust.tick_hz, 0);
}

#[test]
fn rust_section_matches_parts() {
    assert_path(
        &with(
            "server    = { crate = \"voxel-demo-server\", entry = \"voxel_demo_server::register\" }\n",
            "",
        ),
        "rust.server",
    );
    assert_path(
        &with(
            "client    = { crate = \"voxel-demo-client\", entry = \"voxel_demo_client::register\" }\n",
            "",
        ),
        "rust.client",
    );
    let no_server_part = with(
        "parts     = [\"server-rs\", \"client-rs\", \"assets\", \"lua-ui\"]",
        "parts = [\"client-rs\", \"assets\"]",
    );
    assert_path(&no_server_part, "rust.server");
    assert_path(
        &with(
            "entry = \"voxel_demo_server::register\"",
            "entry = \"register\"",
        ),
        "rust.server.entry",
    );
    assert_path(
        &with(
            "entry = \"voxel_demo_server::register\"",
            "entry = \"a b::register\"",
        ),
        "rust.server.entry",
    );
    assert_path(
        &with("crate = \"voxel-demo-server\"", "crate = \"Mine Server\""),
        "rust.server.crate",
    );
    assert_path(
        &with("\"hud\", \"kv\"]", "\"hud\", \"Kv\"]"),
        "rust.caps[6]",
    );
    assert_path(
        &with("\"hud\", \"kv\"]", "\"hud\", \"hud\"]"),
        "rust.caps[6]",
    );
}

#[test]
fn relations_are_checked() {
    assert_path(
        &with("some-lib = \">=1.2, <2\"", "voxel-demo = \"*\""),
        "deps.voxel-demo",
    );
    assert_path(
        &with("old-mine = \"*\"", "voxel-demo = \"*\""),
        "conflicts.voxel-demo",
    );
    assert_path(
        &with("nice-to-have = \"^0.3\"", "some-lib = \"*\""),
        "optional.some-lib",
    );
    assert_path(
        &with("old-mine = \"*\"", "some-lib = \"*\""),
        "conflicts.some-lib",
    );
    assert_path(
        &with("some-lib = \">=1.2, <2\"", "some-lib = \"1.2\""),
        "deps.some-lib",
    );
    assert_path(
        &with("some-lib = \">=1.2, <2\"", "Some_Lib = \"*\""),
        "deps.Some_Lib",
    );
    assert_path(
        &with("after  = [\"some-lib\"]", "after = [\"voxel-demo\"]"),
        "order.after",
    );
    assert_path(
        &with("before = [\"late-mod\"]", "before = [\"some-lib\"]"),
        "order.before",
    );
    assert_path(
        &with("after  = [\"some-lib\"]", "after = [\"BAD\"]"),
        "order.after[0]",
    );
    assert_path(
        &with("after  = [\"some-lib\"]", "after = [\"zz\", \"zz\"]"),
        "order.after[1]",
    );
}

#[test]
fn content_rules() {
    assert_path(
        &with("assets = \"assets/\"", "data = [\"data/*.sql\"]"),
        "content.data",
    );
    assert_path(
        &with("assets = \"assets/\"", "lua_gameplay = [\"a.lua\"]"),
        "content.lua_gameplay",
    );
    assert_path(
        &with("assets = \"assets/\"", "assets = \"../outside\""),
        "content.assets",
    );
    assert_path(
        &with("assets = \"assets/\"", "assets = \"/abs\""),
        "content.assets",
    );
    assert_path(
        &with("assets = \"assets/\"", "assets = \"a\\\\b\""),
        "content.assets",
    );
    assert_path(
        &with("file = \"lua/ui/Foo.lua\"", "file = \"C:/x.lua\""),
        "content.lua_ui[0].file",
    );
    assert_path(
        &with("entry = \"InGameUIAddin\"", "entry = \"in game\""),
        "content.lua_ui[0].entry",
    );
    let no_ui_part = with(
        "parts     = [\"server-rs\", \"client-rs\", \"assets\", \"lua-ui\"]",
        "parts = [\"server-rs\", \"client-rs\", \"assets\"]",
    );
    assert_path(&no_ui_part, "content.lua_ui");
    assert_path(
        &GAMEPLAY.replacen(
            "data         = [\"data/*.sql\"]",
            "data = [\"../up.sql\"]",
            1,
        ),
        "content.data[0]",
    );
}

#[test]
fn core_rules() {
    assert_path(
        &GAMEPLAY.replacen(
            "[core]\nbase    = \"reimagened/v34.15@abc123\"\n",
            "[core]\n",
            1,
        ),
        "core.base",
    );
    assert_path(
        &GAMEPLAY.replacen(
            "defines = [\"LEKMOD_MODCORE_FOO\"]",
            "defines = [\"lower\"]",
            1,
        ),
        "core.defines[0]",
    );
    let no_core = GAMEPLAY.split("[core]").next().unwrap();
    assert_path(no_core, "core");
    let no_part = GAMEPLAY.replacen(", \"core-patch\"", "", 1);
    assert_path(&no_part, "core");
}

#[test]
fn error_messages_are_readable() {
    let e = parse_err(&with("rate   = \"30/s\"", "rate = \"fast\""));
    assert_eq!(
        e.to_string(),
        "channels[0].rate: \"fast\" is not a rate: use \"N/s\" with N from 1 to 10000"
    );
    let e = parse_err(&with(
        "name      = \"VoxelDemo\"",
        "name = \"VoxelDemo\"\ncolour = 1",
    ));
    assert_eq!(e.to_string(), "mod.colour: unknown key");
}
