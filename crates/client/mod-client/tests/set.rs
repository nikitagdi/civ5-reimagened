//! Matching the set a server announced against the mods built into the binary (T35).

use mod_api::{Cap, Caps, Version};
use mod_client::testing::MockClientCtx;
use mod_client::{AttachInfo, ClientCtx, ClientMod, ClientRegistry, ModBuild, SetEntry};
use mod_manifest::ModId;

struct Echo;
impl ClientMod for Echo {
    fn attach(&mut self, cx: &mut dyn ClientCtx, info: &AttachInfo) {
        // The index a message of this mod carries is the game's load index, not a position in the registry.
        cx.send(0, &info.this.mod_idx.to_le_bytes()).unwrap();
    }
}

fn build(v: (u32, u32, u32), caps: &[Cap]) -> ModBuild {
    ModBuild {
        version: Version::new(v.0, v.1, v.2),
        caps: Caps::of(caps),
        code_hash: [1; 16],
    }
}

fn entry(id: &str, idx: u16, available: bool) -> SetEntry {
    SetEntry {
        id: id.into(),
        version: "1.2.3".into(),
        load_idx: idx,
        available,
    }
}

fn id(s: &str) -> ModId {
    ModId::new(s).unwrap()
}

#[test]
fn registered_mods_report_their_build_and_unbuilt_ones_a_default() {
    let mut r = ClientRegistry::new();
    assert!(r.is_empty());
    r.add_built("alpha", build((1, 2, 3), &[Cap::NetChannel]), || {
        Box::new(Echo)
    });
    r.add("beta", || Box::new(Echo));
    assert!(!r.is_empty());
    assert_eq!(
        r.build(&id("alpha")).unwrap().version,
        Version::new(1, 2, 3)
    );
    assert_eq!(r.build(&id("beta")).unwrap(), ModBuild::default());
    assert_eq!(r.build(&id("gamma")), None);
    // A build declared for an id that is not registered is ignored.
    r.set_build("gamma", build((9, 0, 0), &[]));
    assert_eq!(r.build(&id("gamma")), None);
    assert!(r.problems().is_empty());
}

#[test]
fn the_set_keeps_the_load_order_and_the_load_index_and_names_what_stays_out() {
    let mut r = ClientRegistry::new();
    r.add_built("alpha", build((1, 2, 3), &[Cap::NetChannel]), || {
        Box::new(Echo)
    });
    r.add_built("beta", build((1, 2, 3), &[Cap::NetChannel]), || {
        Box::new(Echo)
    });
    let set = [
        entry("beta", 4, true),
        entry("gamma", 5, false),
        entry("alpha", 7, true),
        // In the binary, but the server did not find it in the hello (another version): out.
    ];
    let (mut d, out) = r.instantiate_set(&set);
    assert_eq!(out, [id("gamma")]);
    assert_eq!(d.len(), 2);
    let idx: Vec<u16> = d.mods().map(|m| m.mod_idx).collect();
    assert_eq!(idx, [4, 7]);
    let mut host = MockClientCtx::new();
    d.attach(&mut host);
    assert_eq!(host.sent_on(4, 0), [&4u16.to_le_bytes()[..]]);
    assert_eq!(host.sent_on(7, 0), [&7u16.to_le_bytes()[..]]);
}

#[test]
fn an_unavailable_registered_mod_stays_out_and_no_set_is_an_empty_dispatcher() {
    let mut r = ClientRegistry::new();
    r.add_built("alpha", build((1, 2, 3), &[Cap::NetChannel]), || {
        Box::new(Echo)
    });
    let (d, out) = r.instantiate_set(&[entry("alpha", 0, false)]);
    assert!(d.is_empty());
    assert_eq!(out, [id("alpha")]);
    let (d, out) = r.instantiate_set(&[]);
    assert!(d.is_empty() && out.is_empty());
}
