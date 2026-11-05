//! Set `b1-sv`: synthetic fixtures always; the real install when `CIV5_ROOT` and
//! `CIV5_DB_SNAPSHOT` are set.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{
    ArtTable, ContentHash, LogicalId, Manifest, PlayerColor, TextureBlob, VariantFormat,
};
use asset_pipeline::db::{PlayerColorRow, StrategicViewRow};
use asset_pipeline::game::GameSource;
use asset_pipeline::models::ModelError;
use asset_pipeline::sets::sv::{Needed, SvInput, build_sv, load_input, plan_sv};
use asset_pipeline::sets::{ART_TABLE_ID, TextureOptions};
use ddsfile::{D3DFormat, Dds, NewD3dParams};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-b1sv-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dds(side: u32) -> Vec<u8> {
    let mut d = Dds::new_d3d(NewD3dParams {
        height: side,
        width: side,
        depth: None,
        format: D3DFormat::A8R8G8B8,
        mipmap_levels: Some(1),
        caps2: None,
    })
    .unwrap();
    for (i, b) in d.data.iter_mut().enumerate() {
        *b = (i * 5 % 251) as u8;
    }
    let mut out = Vec::new();
    d.write(&mut out).unwrap();
    out
}

fn row(tag: &str, tile: &str, asset: &str) -> StrategicViewRow {
    StrategicViewRow {
        tag: tag.into(),
        tile_type: tile.into(),
        asset: asset.into(),
    }
}

fn need(table: &'static str, t: &str, tag: Option<&str>) -> Needed {
    Needed {
        table,
        game_type: t.into(),
        art_tag: tag.map(str::to_owned),
    }
}

fn input() -> SvInput {
    SvInput {
        rows: vec![
            row("ART_DEF_IMPROVEMENT_FARM", "Improvement", "SV_Farm.dds"),
            row(
                "ART_DEF_IMPROVEMENT_BARBARIAN_CAMP",
                "Improvement",
                "SV_BarbarianCamp.dds",
            ),
            row(
                "ART_DEF_IMPROVEMENT_GOODY_HUT",
                "Improvement",
                "SV_AncientRuins.dds",
            ),
            row("ART_DEF_RESOURCE_IRON", "Resource", "SV_Iron.dds"),
            row("ART_DEF_RESOURCE_GONE", "Resource", "SV_NotInstalled.dds"),
            row("ART_DEF_UNIT_WARRIOR", "Unit", "SV_Warrior.dds"),
            // The same file under two tags: one blob set, two records.
            row("ART_DEF_UNIT_BARBARIAN_EURO", "Unit", "SV_Warrior.dds"),
        ],
        needed: vec![
            need(
                "Improvements",
                "IMPROVEMENT_FARM",
                Some("ART_DEF_IMPROVEMENT_FARM"),
            ),
            need(
                "Improvements",
                "IMPROVEMENT_BARBARIAN_CAMP",
                Some("ART_DEF_IMPROVEMENT_BARBARIAN_CAMP"),
            ),
            need(
                "Improvements",
                "IMPROVEMENT_GOODY_HUT",
                Some("ART_DEF_IMPROVEMENT_GOODY_HUT"),
            ),
            need("Resources", "RESOURCE_IRON", Some("ART_DEF_RESOURCE_IRON")),
            need("Resources", "RESOURCE_GONE", Some("ART_DEF_RESOURCE_GONE")),
            need("Resources", "RESOURCE_NUTMEG", Some("ART_DEF_RESOURCE_FUR")),
            need("Resources", "RESOURCE_NOTAG", None),
        ],
        colors: vec![PlayerColorRow {
            color_type: "PLAYERCOLOR_RED".into(),
            color: PlayerColor {
                primary: [255, 0, 0, 255],
                secondary: [1, 2, 3, 255],
                text: [4, 5, 6, 255],
            },
        }],
    }
}

fn fake_read(name: &str) -> Result<Option<Vec<u8>>, ModelError> {
    Ok(match name {
        "sv_notinstalled.dds" => None,
        "sv_ancientruins.dds" => Some(dds(1024)),
        _ => Some(dds(16)),
    })
}

fn read_manifest(out: &Path) -> Manifest {
    let hash: ContentHash = std::fs::read_to_string(out.join("manifest"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    Manifest::decode(&std::fs::read(out.join(hash.to_string())).unwrap()).unwrap()
}

fn blob_bytes(out: &Path, m: &Manifest, id: &LogicalId, fmt: VariantFormat) -> Vec<u8> {
    let v = m
        .variants(id)
        .iter()
        .find(|v| v.format == fmt)
        .unwrap_or_else(|| panic!("{id:?} has no {fmt:?} variant"));
    std::fs::read(out.join(v.hash.to_string())).unwrap()
}

fn table_of(out: &Path) -> (Manifest, ArtTable) {
    let m = read_manifest(out);
    let t = ArtTable::decode(&blob_bytes(
        out,
        &m,
        &LogicalId::new(ART_TABLE_ID),
        VariantFormat::Generic,
    ))
    .unwrap();
    (m, t)
}

#[test]
fn plan_lowercases_files_and_dedups() {
    let plan = plan_sv(&input());
    assert_eq!(plan.icons.len(), 7);
    assert_eq!(plan.files.len(), 6);
    assert_eq!(plan.icons["ART_DEF_UNIT_WARRIOR"], "sv_warrior.dds");
}

#[test]
fn build_writes_icons_records_colors_and_report() {
    let tmp = TempDir::new();
    let s = build_sv(&fake_read, &input(), tmp.path(), TextureOptions::default()).unwrap();
    let (m, table) = table_of(tmp.path());
    assert_eq!((s.sv_records, s.color_records, s.textures), (6, 1, 5));
    assert_eq!(table.len(), 7);
    assert_eq!(s.table_entries, 7);

    let camp = table.sv("ART_DEF_IMPROVEMENT_BARBARIAN_CAMP").unwrap();
    assert_eq!(camp.icon.as_str(), "tex:sv_barbariancamp.dds");
    assert_eq!(
        table.sv("ART_DEF_UNIT_WARRIOR").unwrap().icon,
        table.sv("ART_DEF_UNIT_BARBARIAN_EURO").unwrap().icon
    );
    assert!(table.sv("ART_DEF_RESOURCE_GONE").is_none());
    assert_eq!(
        table.player_color("PLAYERCOLOR_RED").unwrap().text,
        [4, 5, 6, 255]
    );

    // Every icon in the table is in the manifest as BC7 and ASTC (SIMP-022), in group 1, capped at 512.
    for (_, rec) in table.iter() {
        if let asset_format::ArtRecord::Sv(sv) = rec {
            let v = m.variants(&sv.icon);
            let formats: Vec<_> = v.iter().map(|v| v.format).collect();
            assert_eq!(formats, [VariantFormat::Bc, VariantFormat::Astc]);
            assert!(v.iter().all(|v| v.prefetch.0 == 1));
        }
    }
    let big = TextureBlob::decode(&blob_bytes(
        tmp.path(),
        &m,
        &LogicalId::new("tex:sv_ancientruins.dds"),
        VariantFormat::Bc,
    ))
    .unwrap();
    assert_eq!((big.width, big.height), (512, 512));

    // Report: missing file, a tag without a row, a type without a tag; nothing else.
    let kinds: Vec<(&str, &str)> = s
        .report
        .iter()
        .map(|e| (e.kind, e.subject.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("sv-file-missing", "Resources RESOURCE_GONE"),
            ("sv-tag-missing", "Resources RESOURCE_NUTMEG"),
            ("sv-tag-missing", "Resources RESOURCE_NOTAG"),
        ]
    );
    assert!(s.report[1].reason.contains("ART_DEF_RESOURCE_FUR"));
}

#[test]
fn rebuild_is_deterministic_and_keeps_other_entries() {
    let tmp = TempDir::new();
    let opts = TextureOptions::default();
    build_sv(&fake_read, &input(), tmp.path(), opts).unwrap();
    // The terrain set's records stay when b1-sv runs after it (shared table).
    let store = asset_pipeline::store::BlobStore::open(tmp.path()).unwrap();
    let (_, mut table) = table_of(tmp.path());
    table.insert_river(asset_format::RiverArt {
        textures: vec![LogicalId::new("tex:river_diffuse.dds")],
    });
    let mut b = asset_pipeline::store::ManifestBuilder::open(&store).unwrap();
    let (hash, size) = store.put(&table.encode()).unwrap();
    b.add(
        LogicalId::new(ART_TABLE_ID),
        asset_format::Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: asset_format::PrefetchGroup(1),
        },
    );
    b.finish(&store).unwrap();
    build_sv(&fake_read, &input(), tmp.path(), opts).unwrap();
    let (_, t2) = table_of(tmp.path());
    assert!(t2.river().is_some());
    assert_eq!(t2.len(), 8);
}

/// Real install and snapshot: every resource, improvement (camp and ruins included) has an
/// `sv/*` record whose icon is in the manifest within the texture cap, or a report entry.
#[test]
fn real_install_every_resource_and_improvement_has_an_icon_or_a_report_entry() {
    let (Some(root), Some(db)) = (
        std::env::var_os("CIV5_ROOT"),
        std::env::var_os("CIV5_DB_SNAPSHOT"),
    ) else {
        println!("skipped: CIV5_ROOT or CIV5_DB_SNAPSHOT not set");
        return;
    };
    let game = GameSource::open(PathBuf::from(root)).unwrap();
    let input = load_input(Path::new(&db)).unwrap();
    assert!(input.rows.len() >= 300, "{}", input.rows.len());
    assert!(input.colors.len() >= 20, "{}", input.colors.len());
    let tmp = TempDir::new();
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(n)?) };
    let s = build_sv(&read, &input, tmp.path(), TextureOptions::default()).unwrap();
    println!("{s:?}");
    for e in &s.report {
        println!("report: {e}");
    }
    let (m, table) = table_of(tmp.path());
    let reported = |subject: &str| s.report.iter().any(|e| e.subject == subject);
    for n in &input.needed {
        let subject = format!("{} {}", n.table, n.game_type);
        let rec = n.art_tag.as_deref().and_then(|t| table.sv(t));
        match rec {
            Some(sv) => {
                let blob =
                    TextureBlob::decode(&blob_bytes(tmp.path(), &m, &sv.icon, VariantFormat::Bc))
                        .unwrap();
                assert!(blob.width.max(blob.height) <= 512, "{subject}");
            }
            None => assert!(
                reported(&subject),
                "{subject} has neither icon nor report entry"
            ),
        }
    }
    for camp_or_ruins in [
        "IMPROVEMENT_BARBARIAN_CAMP",
        "IMPROVEMENT_CITY_RUINS",
        "IMPROVEMENT_GOODY_HUT",
    ] {
        let subject = format!("Improvements {camp_or_ruins}");
        assert!(input.needed.iter().any(|n| n.game_type == camp_or_ruins));
        assert!(!reported(&subject), "{subject} is reported");
    }
    for c in &input.colors {
        assert!(table.player_color(&c.color_type).is_some());
    }
}
