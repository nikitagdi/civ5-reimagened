//! The integrated path on a small `ui` set built in memory: the asset loader over a fake server, the boot,
//! the layer, the draw list. Always runs; `game_set.rs` does the same on the real set.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use asset_format::{
    ContentHash, LocaleTable, TextureBlob, TextureFormat, UiAtlas, UiFileInfo, UiFileKind, UiFont,
    UiGroup, UiIndex, UiLayerInfo, UiTexDim,
};
use assets::{AssetLoader, HttpSource, LogicalId, Manifest, PrefetchGroup, Variant, VariantFormat};
use draw_list::{TextureId, UiDrawList};
use platform::{HttpClient, HttpError, HttpOp, HttpResponse};
use ui_client::{BootPlan, BootState, LayerConfig, UiBoot, UiLayer};

const BASE: &str = "http://test/assets/";

#[derive(Default)]
struct FakeHttp {
    routes: RefCell<HashMap<String, Vec<u8>>>,
    asked: RefCell<Vec<String>>,
}

struct Done(Option<Result<HttpResponse, HttpError>>);

impl HttpOp for Done {
    fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
        self.0.take()
    }
}

impl HttpClient for FakeHttp {
    fn get(&self, url: &str) -> Box<dyn HttpOp> {
        self.asked.borrow_mut().push(url.to_owned());
        let resp = match self.routes.borrow().get(url) {
            Some(body) => HttpResponse {
                status: 200,
                body: body.clone(),
            },
            None => HttpResponse {
                status: 404,
                body: Vec::new(),
            },
        };
        Box::new(Done(Some(Ok(resp))))
    }
}

/// The glyphs `A`..`Z`, `a`..`z` and space, 8 by 10, in a 128 by 64 atlas.
const FONT: &str = r#"<glyphgen>
 <textures width="128" height="64">
  <texture name="" src="TestFont.dds" allowcolor="1"/>
 </textures>
 <styles count="1"><style name=""><layers><layer tex=""/></layers></style></styles>
 <glyphs spacing="0" ascent="12" ascadj="2" descent="4">
  <glyph ch="32" u="0" v="0" width="0" height="0" a="0" b="4" c="0" originy="0"/>
  <glyph ch="72" u="0" v="8" width="8" height="10" a="1" b="9" c="0" originy="10"/>
  <glyph ch="101" u="10" v="8" width="8" height="10" a="1" b="9" c="0" originy="10"/>
  <glyph ch="108" u="20" v="8" width="8" height="10" a="1" b="9" c="0" originy="10"/>
  <glyph ch="111" u="30" v="8" width="8" height="10" a="1" b="9" c="0" originy="10"/>
 </glyphs>
</glyphgen>"#;

const WINDOW_XML: &str = r#"<Context ID="T">
  <Image ID="Art" Texture="Art.dds" Size="64,64" Offset="10,50"/>
  <Label ID="Greeting" Font="TwCenMT16" String="TXT_KEY_HELLO" Offset="10,10"/>
</Context>"#;

struct Server {
    http: Rc<FakeHttp>,
    n: u8,
    manifest: Manifest,
}

impl Server {
    fn new() -> Server {
        Server {
            http: Rc::new(FakeHttp::default()),
            n: 0,
            manifest: Manifest::new(),
        }
    }

    fn put(&mut self, id: &str, format: VariantFormat, bytes: Vec<u8>) {
        self.n += 1;
        // The loader checks a blob against its hash.
        let hash = ContentHash(*blake3::hash(&bytes).as_bytes());
        self.http
            .routes
            .borrow_mut()
            .insert(format!("{BASE}{hash}"), bytes.clone());
        self.manifest.insert(
            LogicalId::new(id),
            Variant {
                format,
                hash,
                size: bytes.len() as u64,
                prefetch: PrefetchGroup(0),
            },
        );
    }

    fn publish(&self) {
        let body = self.manifest.encode();
        let h = ContentHash(*blake3::hash(&body).as_bytes());
        let mut r = self.http.routes.borrow_mut();
        r.insert(format!("{BASE}manifest"), format!("{h}\n").into_bytes());
        r.insert(format!("{BASE}{h}"), body);
    }
}

fn file(name: &str, kind: UiFileKind, id: &str) -> UiFileInfo {
    UiFileInfo {
        name: name.to_owned(),
        id: id.to_owned(),
        kind,
        layer: 0,
        size: 1,
        path: format!("UI/{name}"),
        hidden: 0,
        index_of: None,
    }
}

/// A set with a font (one R8 atlas), a context with an image and a label, and the texts.
fn server() -> Server {
    server_lua(b"-- empty")
}

/// [`server`] with the Lua of the context `T`. The set also holds an icon atlas (`Icons.dds`, 4 by 4 pixels,
/// two icons of 2 pixels per row and column) that no group has.
fn server_lua(lua: &[u8]) -> Server {
    let mut s = Server::new();
    let index = UiIndex {
        layers: vec![UiLayerInfo {
            name: "base".into(),
            dir: String::new(),
            priority: 0,
            order: 0,
            packages: vec![],
            priority_defaulted: false,
        }],
        files: vec![
            file("art.dds", UiFileKind::Texture, "ui:tx:art.dds"),
            file("icons.dds", UiFileKind::Texture, "ui:tx:icons.dds"),
            file("plain.dds", UiFileKind::Texture, "ui:tx:plain.dds"),
            file("t.lua", UiFileKind::Lua, "ui:f:t.lua"),
            file("t.xml", UiFileKind::Xml, "ui:f:t.xml"),
            file("testfont.dds", UiFileKind::Texture, "ui:tx:testfont.dds"),
            file("testfont.ggxml", UiFileKind::Font, "ui:f:testfont.ggxml"),
        ],
        fonts: vec![UiFont {
            name: "TwCenMT16".into(),
            ggxml: "testfont.ggxml".into(),
            textures: vec!["testfont.dds".into()],
        }],
        tex_dims: vec![UiTexDim {
            file: "plain.dds".into(),
            width: 4,
            height: 4,
        }],
        atlases: vec![UiAtlas {
            atlas: "ICONS".into(),
            icon_size: 2,
            file: "icons.dds".into(),
            icons_per_row: 2,
            icons_per_column: 2,
        }],
        groups: vec![
            UiGroup {
                id: "ui:boot".into(),
                members: vec!["ui:f:testfont.ggxml".into(), "ui:tx:testfont.dds".into()],
                bytes: 1,
                requires: vec![],
            },
            UiGroup {
                id: "ui:win:t".into(),
                members: vec![
                    "ui:f:t.lua".into(),
                    "ui:f:t.xml".into(),
                    "ui:tx:art.dds".into(),
                ],
                bytes: 1,
                requires: vec!["ui:boot".into()],
            },
        ],
        ..UiIndex::default()
    };
    s.put("ui:index", VariantFormat::Generic, index.encode());
    s.put(
        "ui:f:t.xml",
        VariantFormat::Generic,
        WINDOW_XML.as_bytes().to_vec(),
    );
    s.put("ui:f:t.lua", VariantFormat::Generic, lua.to_vec());
    let icons = TextureBlob::new_2d(
        TextureFormat::Rgba8,
        false,
        4,
        4,
        vec![(0..16).flat_map(|i| [10 * i, 0, 0, 255]).collect()],
    );
    s.put(
        "ui:tx:icons.dds",
        VariantFormat::Uncompressed,
        icons.encode(),
    );
    let plain = TextureBlob::new_2d(
        TextureFormat::Rgba8,
        false,
        4,
        4,
        vec![(0..16).flat_map(|i| [0, 7 * i, 0, 255]).collect()],
    );
    s.put(
        "ui:tx:plain.dds",
        VariantFormat::Uncompressed,
        plain.encode(),
    );
    s.put(
        "ui:f:testfont.ggxml",
        VariantFormat::Generic,
        FONT.as_bytes().to_vec(),
    );
    // The glyph atlas is coverage only: 128 by 64, every byte 200.
    let atlas = TextureBlob::new_2d(TextureFormat::R8, false, 128, 64, vec![vec![200; 128 * 64]]);
    s.put(
        "ui:tx:testfont.dds",
        VariantFormat::Uncompressed,
        atlas.encode(),
    );
    let art = TextureBlob::new_2d(
        TextureFormat::Rgba8,
        false,
        4,
        4,
        vec![(0..16).flat_map(|i| [i, 2 * i, 3 * i, 255]).collect()],
    );
    s.put("ui:tx:art.dds", VariantFormat::Uncompressed, art.encode());
    let mut texts = LocaleTable::new("en_US");
    texts.entries.push(("TXT_KEY_HELLO".into(), "Hello".into()));
    s.put("ui:text:en_us", VariantFormat::Generic, texts.encode());
    s.publish();
    s
}

fn plan() -> BootPlan {
    BootPlan {
        lang: "en_US".into(),
        contexts: vec!["T".into()],
        groups: vec![],
        memory: vec![],
        lenient_groups: false,
    }
}

fn boot(s: &Server, plan: BootPlan) -> Result<Rc<ui_client::UiStore>, String> {
    let http: Rc<dyn HttpClient> = s.http.clone();
    let source = HttpSource::new(Rc::clone(&http), BASE);
    let mut loader = AssetLoader::new(http, BASE, Box::new(source));
    loader.load_manifest(None);
    let mut boot = UiBoot::new(plan);
    for _ in 0..1000 {
        match boot.update(&mut loader) {
            BootState::Ready => return Ok(boot.store().expect("store")),
            BootState::Failed(m) => return Err(m),
            BootState::Loading { .. } => {}
        }
    }
    Err("did not finish".into())
}

fn cfg() -> LayerConfig {
    LayerConfig {
        lang: "en_US".into(),
        root: "T".into(),
        stubs: None,
        start_event: None,
        extra_roots: Vec::new(),
        small_assets: false,
    }
}

#[test]
fn the_boot_fetches_the_closure_and_the_layer_draws_text_and_a_sprite() {
    let s = server();
    let store = boot(&s, plan()).expect("boot");
    // The index, the boot group, the window group and the texts: nothing else was asked for.
    assert_eq!(
        s.http.asked.borrow().len(),
        2 + 1 + 5 + 1,
        "{:?}",
        s.http.asked
    );

    let mut layer = UiLayer::new(Rc::clone(&store), &cfg(), (1920, 1080)).expect("layer");
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

    let font = store.texture_id_of_name("TestFont.dds").unwrap();
    let art = store.texture_id_of_name("Art.dds").unwrap();
    let quads = |t: TextureId| -> usize {
        list.commands()
            .iter()
            .filter(|c| c.texture == t)
            .map(|c| c.index_count as usize / 6)
            .sum()
    };
    assert_eq!(quads(art), 1, "the sprite");
    assert_eq!(quads(font), 5, "H e l l o");

    // What the renderer asks for after the frame comes back as bytes it can take.
    let wanted: Vec<TextureId> = list.commands().iter().map(|c| c.texture).collect();
    let ready = layer.resolve_textures(&wanted);
    assert_eq!(ready.len(), wanted.len());
    let atlas = ready.iter().find(|t| t.id == font).unwrap();
    assert_eq!(
        (atlas.format, atlas.width, atlas.height),
        (TextureFormat::Rgba8, 128, 64)
    );
    assert_eq!(
        &atlas.bytes[..4],
        &[255, 255, 255, 200],
        "coverage becomes alpha"
    );
    let sprite = ready.iter().find(|t| t.id == art).unwrap();
    assert_eq!(&sprite.bytes[4..8], &[1, 2, 3, 255]);
    assert!(layer.resolve_textures(&[TextureId(999_999)]).is_empty());
    assert_eq!(stats.tex_missing, 0);
}

#[test]
fn a_set_without_the_window_group_fails_the_boot_with_the_reason() {
    let s = server();
    let mut p = plan();
    p.contexts = vec!["Nowhere".into()];
    let err = boot(&s, p).err().expect("boot fails");
    assert!(err.contains("ui:win:nowhere"), "{err}");
}

#[test]
fn a_blob_the_server_lacks_fails_the_boot() {
    let s = server();
    // Take the sprite away: the manifest still names it.
    let hash = s.manifest.variants(&LogicalId::new("ui:tx:art.dds"))[0].hash;
    s.http.routes.borrow_mut().remove(&format!("{BASE}{hash}"));
    let err = boot(&s, plan()).err().expect("boot fails");
    assert!(err.contains("ui:tx:art.dds"), "{err}");
}

#[test]
fn an_atlas_no_group_has_is_fetched_when_lua_sets_it_and_shows_when_it_arrives() {
    let s = server_lua(
        b"Controls.Art:SetTexture('Assets/UI/Art/Icons.dds')
Controls.Art:SetTextureOffsetVal(2, 2)",
    );
    let http: Rc<dyn HttpClient> = s.http.clone();
    let source = HttpSource::new(Rc::clone(&http), BASE);
    let mut loader = AssetLoader::new(http, BASE, Box::new(source));
    loader.load_manifest(None);
    let mut boot = UiBoot::new(plan());
    let mut state = BootState::Loading { done: 0, total: 0 };
    for _ in 0..1000 {
        state = boot.update(&mut loader);
        if state == BootState::Ready {
            break;
        }
    }
    assert_eq!(state, BootState::Ready);
    let store = boot.store().expect("store");
    let icons_url_asked = |s: &Server| {
        let hash = s.manifest.variants(&LogicalId::new("ui:tx:icons.dds"))[0].hash;
        s.http
            .asked
            .borrow()
            .iter()
            .any(|u| u.ends_with(&hash.to_string()))
    };
    assert!(!icons_url_asked(&s), "the boot does not fetch the atlas");

    let mut layer = UiLayer::new(Rc::clone(&store), &cfg(), (1920, 1080)).expect("layer");
    assert_eq!(
        layer.stats().core.lua_errors,
        0,
        "{:?}",
        layer.core().runtime().recent_errors()
    );
    let icons = store.texture_id_of_name("Icons.dds").unwrap();
    assert_eq!(
        layer.stats().tex_lazy.pending,
        1,
        "the Lua asked for the atlas"
    );
    // The atlas is not here: the control took it with the size of the rows and draws, the renderer has no bytes yet.
    let mut list = UiDrawList::new();
    layer.frame(0, &mut list);
    let quads_of = |list: &UiDrawList| -> usize {
        list.commands()
            .iter()
            .filter(|c| c.texture == icons)
            .map(|c| c.index_count as usize / 6)
            .sum()
    };
    assert_eq!(
        quads_of(&list),
        1,
        "the control draws the atlas before it has arrived"
    );
    assert!(layer.resolve_textures(&[icons]).is_empty());
    assert!(store.is_loading(icons));

    // The fetch goes out from `update` and comes back.
    for _ in 0..50 {
        boot.update(&mut loader);
        if !store.is_loading(icons) {
            break;
        }
    }
    assert!(icons_url_asked(&s));
    let stats = layer.stats();
    assert_eq!(
        (
            stats.tex_lazy.loaded,
            stats.tex_lazy.pending,
            stats.tex_lazy.failed
        ),
        (1, 0, 0)
    );
    let ready = layer.resolve_textures(&[]);
    assert_eq!(
        ready.len(),
        1,
        "the id the renderer asked for before is answered now"
    );
    assert_eq!(
        (ready[0].id, ready[0].width, ready[0].height),
        (icons, 4, 4)
    );
    layer.frame(16, &mut list);
    assert_eq!(quads_of(&list), 1);
    assert_eq!(layer.stats().core.lua_errors, 0);
}

#[test]
fn an_atlas_the_server_lacks_is_marked_failed_and_not_asked_again() {
    let s = server_lua(b"Controls.Art:SetTexture('Icons.dds')");
    let hash = s.manifest.variants(&LogicalId::new("ui:tx:icons.dds"))[0].hash;
    s.http.routes.borrow_mut().remove(&format!("{BASE}{hash}"));
    let http: Rc<dyn HttpClient> = s.http.clone();
    let source = HttpSource::new(Rc::clone(&http), BASE);
    let mut loader = AssetLoader::new(http, BASE, Box::new(source));
    loader.load_manifest(None);
    let mut boot = UiBoot::new(plan());
    while boot.update(&mut loader) != BootState::Ready {}
    let store = boot.store().unwrap();
    let layer = UiLayer::new(Rc::clone(&store), &cfg(), (1920, 1080)).expect("layer");
    for _ in 0..100 {
        boot.update(&mut loader);
    }
    let st = layer.stats().tex_lazy;
    assert_eq!((st.failed, st.pending), (1, 0), "{st:?}");
    // From now on an ask answers `None`, so nothing is fetched again.
    assert!(store.texture_info("Icons.dds").is_none());
    assert_eq!(store.lazy_stats().pending, 0);
}

#[test]
fn a_texture_that_is_no_atlas_is_taken_with_the_size_of_the_index_and_shows_when_it_arrives() {
    let s = server_lua(b"Controls.Art:SetTexture('Plain.dds')");
    let http: Rc<dyn HttpClient> = s.http.clone();
    let source = HttpSource::new(Rc::clone(&http), BASE);
    let mut loader = AssetLoader::new(http, BASE, Box::new(source));
    loader.load_manifest(None);
    let mut boot = UiBoot::new(plan());
    while boot.update(&mut loader) != BootState::Ready {}
    let store = boot.store().unwrap();
    let layer = UiLayer::new(Rc::clone(&store), &cfg(), (1920, 1080)).expect("layer");
    let plain = store.texture_id_of_name("Plain.dds").unwrap();
    // No atlas row names it: the size comes from `tex_dims`, the control has the texture at once.
    assert!(store.is_loading(plain));
    assert!(layer.resolve_textures(&[plain]).is_empty());
    for _ in 0..50 {
        boot.update(&mut loader);
    }
    assert_eq!(
        layer.stats().tex_lazy.loaded,
        1,
        "{:?}",
        layer.stats().tex_lazy
    );
    assert_eq!(
        layer.stats().tex_missing,
        0,
        "the ask was answered, not missing"
    );
    assert_eq!(layer.resolve_textures(&[]).len(), 1);
}

/// A compressed variant padded to whole blocks: the layout keeps the size of the index, and the
/// renderer gets the image part of the stored texture.
#[test]
fn a_padded_ui_texture_keeps_its_layout_size_and_names_its_image_part() {
    let index = UiIndex {
        layers: vec![UiLayerInfo {
            name: "base".into(),
            dir: String::new(),
            priority: 0,
            order: 0,
            packages: vec![],
            priority_defaulted: false,
        }],
        files: vec![file("odd.dds", UiFileKind::Texture, "ui:tx:odd.dds")],
        tex_dims: vec![UiTexDim {
            file: "odd.dds".into(),
            width: 30,
            height: 17,
        }],
        ..UiIndex::default()
    };
    let store = ui_client::UiStore::new(index, Vec::new()).unwrap();
    let padded = TextureBlob::new_2d(
        TextureFormat::Bc7,
        false,
        32,
        20,
        vec![vec![0; TextureFormat::Bc7.mip_size(32, 20)]],
    );
    store.put("ui:tx:odd.dds", padded.encode()).unwrap();
    let id = store.texture_id_of_name("odd.dds").unwrap();
    let info = store.texture_info("odd.dds").unwrap();
    assert_eq!((info.width, info.height), (30, 17));
    let blob = store.texture_blob(id).unwrap();
    assert_eq!((blob.width, blob.height), (32, 20));
    assert_eq!(store.texture_content(id, &blob), (30, 17));
}
