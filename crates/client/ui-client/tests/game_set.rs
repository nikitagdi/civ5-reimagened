//! The integrated path on the real `ui` set: the asset loader (over a directory standing in for the
//! server), the boot, the layer with the four EUI contexts, the draw list. Skipped, with a message, when
//! the built set is absent (`civ5-assets build --set ui --out <dir>`; set `CIV5_UI_ASSETS` to the folder).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;

use assets::{AssetLoader, HttpSource, ManifestStatus};
use draw_list::{TextureId, UiDrawList};
use platform::{HttpClient, HttpError, HttpOp, HttpResponse};
use ui_client::{BootPlan, BootState, LayerConfig, UiBoot, UiLayer};

/// Whether `dir` is a built set whose manifest lists `ui:index` (a folder with another set, e.g. the model set, is not
/// this one).
fn has_ui_index(dir: &std::path::Path) -> bool {
    let Ok(pointer) = std::fs::read_to_string(dir.join("manifest")) else {
        return false;
    };
    let Ok(blob) = std::fs::read(dir.join(pointer.trim())) else {
        return false;
    };
    assets::Manifest::decode(&blob)
        .map(|m| !m.variants(&assets::LogicalId::new("ui:index")).is_empty())
        .unwrap_or(false)
}

fn set_dir() -> Option<PathBuf> {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        std::env::var_os("CIV5_UI_ASSETS").map(PathBuf::from),
        Some(here.join("../../../../b4-ui-assets")),
        Some(here.join("../../../../../b4-ui-assets")),
        Some(here.join("../../../../assets-out")),
    ];
    for dir in candidates.into_iter().flatten() {
        if has_ui_index(&dir) {
            return Some(dir);
        }
    }
    eprintln!("SKIPPED: built ui set not found (set CIV5_UI_ASSETS)");
    None
}

struct DirHttp(PathBuf);

struct Done(Option<Result<HttpResponse, HttpError>>);

impl HttpOp for Done {
    fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
        self.0.take()
    }
}

impl HttpClient for DirHttp {
    fn get(&self, url: &str) -> Box<dyn HttpOp> {
        let name = url.rsplit('/').next().unwrap_or("");
        let resp = match std::fs::read(self.0.join(name)) {
            Ok(body) => HttpResponse { status: 200, body },
            Err(_) => HttpResponse {
                status: 404,
                body: Vec::new(),
            },
        };
        Box::new(Done(Some(Ok(resp))))
    }
}

fn boot(dir: PathBuf) -> Rc<ui_client::UiStore> {
    let http: Rc<dyn HttpClient> = Rc::new(DirHttp(dir));
    let source = HttpSource::new(Rc::clone(&http), "http://test/assets/");
    let mut loader = AssetLoader::new(http, "http://test/assets/", Box::new(source));
    loader.load_manifest(None);
    let mut boot = UiBoot::new(BootPlan::m1());
    for _ in 0..10_000 {
        assert_ne!(loader.poll_manifest(), ManifestStatus::Failed);
        match boot.update(&mut loader) {
            BootState::Ready => return boot.store().expect("store"),
            BootState::Failed(m) => panic!("boot failed: {m}"),
            BootState::Loading { .. } => {}
        }
    }
    panic!("boot did not finish");
}

#[test]
fn m1_draws_text_and_textures_from_the_built_set() {
    let Some(dir) = set_dir() else { return };
    let store = boot(dir);
    let mut layer =
        UiLayer::new(Rc::clone(&store), &LayerConfig::m1(), (1920, 1080)).expect("layer");
    let mut list = UiDrawList::new();
    layer.frame(0, &mut list);
    layer.frame(16, &mut list);
    let stats = layer.stats();
    assert_eq!(
        stats.core.lua_errors,
        0,
        "{:?}",
        layer.core().runtime().recent_errors()
    );
    assert!(stats.core.controls > 200, "{stats:?}");

    // Which commands draw text: those on a font texture.
    let font_tex: BTreeSet<TextureId> = store
        .index()
        .fonts
        .iter()
        .flat_map(|f| f.textures.iter())
        .filter_map(|t| store.texture_id_of_name(t))
        .collect();
    assert!(!font_tex.is_empty());
    let mut per_tex: BTreeMap<TextureId, usize> = BTreeMap::new();
    for c in list.commands() {
        *per_tex.entry(c.texture).or_default() += c.index_count as usize / 6;
    }
    let glyphs: usize = per_tex
        .iter()
        .filter(|(t, _)| font_tex.contains(t))
        .map(|(_, n)| n)
        .sum();
    let sprites: usize = per_tex
        .iter()
        .filter(|(t, _)| !font_tex.contains(t) && **t != TextureId::WHITE)
        .map(|(_, n)| n)
        .sum();
    eprintln!(
        "controls {} commands {} vertices {} glyph quads {glyphs} sprite quads {sprites} textures used {} missing {:?}",
        stats.core.controls,
        list.commands().len(),
        list.vertices().len(),
        per_tex.len(),
        store.missing_textures()
    );
    assert!(
        list.commands().len() >= 8,
        "{} commands",
        list.commands().len()
    );
    assert!(glyphs > 0, "no text glyph quads");
    assert!(sprites > 0, "no textured sprite quads");

    // Every texture a command names resolves to a blob the renderer can take.
    let wanted: Vec<TextureId> = per_tex
        .keys()
        .copied()
        .filter(|t| *t != TextureId::WHITE)
        .collect();
    let resolved = layer.resolve_textures(&wanted);
    assert_eq!(resolved.len(), wanted.len(), "unresolved textures");
    for tex in &resolved {
        assert!(!tex.bytes.is_empty() && tex.width > 0, "{:?}", tex.format);
        assert_ne!(
            tex.format,
            asset_format::TextureFormat::R8,
            "L8 goes up as Rgba8"
        );
    }
}

#[test]
fn tables_give_icons_colors_and_grammar() {
    use ui::{TextAlign, TextEngine, TextRequest};

    let Some(dir) = set_dir() else { return };
    let store = boot(dir);
    let tables = asset_format::UiTables::decode(&store.bytes("ui:tables").expect("ui:tables"))
        .expect("tables");
    assert!(tables.icons.len() > 500, "{} icons", tables.icons.len());
    assert!(
        tables.colors.iter().any(|(n, _)| n == "COLOR_YELLOW"),
        "colors {}",
        tables.colors.len()
    );
    assert!(tables.languages.iter().any(|l| l.lang == "en_US"));

    let bytes = store.bytes("ui:text:en_us").expect("texts");
    let texts = assets::decode_texts(&bytes).expect("decode");
    assert!(
        texts.grammar.len() > 300,
        "{} grammar rows",
        texts.grammar.len()
    );
    // "an Archer" needs the gender column: the an trait of a noun.
    assert!(
        texts.grammar.iter().any(|g| g.gender == "neuter:an"),
        "no neuter:an gender"
    );

    let mut engine = ui_client::ClientText::new(
        Rc::clone(&store),
        "en_US",
        texts,
        Rc::<std::cell::Cell<ui_text::TextStats>>::default(),
    );
    let request = |text: &str| TextRequest {
        text: text.to_owned(),
        font: "TwCenMT16".to_owned(),
        style: String::new(),
        colors: [[1.0; 4], [0.0, 0.0, 0.0, 1.0], [0.0; 4]],
        wrap_width: -1,
        truncate_width: -1,
        leading_offset: 0,
        align: TextAlign::Left,
        obscured: false,
        parse_markup: true,
    };
    let plain = engine.layout(&request("1"));
    let icon = engine.layout(&request("1 [ICON_GOLD]"));
    assert!(icon.quads.len() > plain.quads.len(), "icon quad missing");
    let red = engine.layout(&request("[COLOR_RED]x[ENDCOLOR]"));
    assert!(
        red.quads
            .iter()
            .any(|q| q.color[0] > 200 && q.color[1] < 80),
        "{:?}",
        red.quads.iter().map(|q| q.color).collect::<Vec<_>>()
    );
}
