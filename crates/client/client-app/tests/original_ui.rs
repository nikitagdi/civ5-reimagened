//! M1 through the app's integrated path, headless: the asset loader over a directory, `OriginalUi`, a
//! real `Renderer` (a software adapter, when there is one), the `UiGame` pass, the texture upload loop,
//! and the mouse. Skipped, with a message, without the built `ui` set (`CIV5_UI_ASSETS`, the folder
//! `civ5-assets build --set ui --out <dir>` wrote) or without an adapter.
//!
//! `CIV5_SCREENSHOT_DIR=<dir>` also writes the frame as `m1.png`.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use assets::{AssetLoader, HttpSource};
use client_app::original_ui::{OriginalUi, Phase};
use draw_list::UiDrawList;
use platform::{
    HttpClient, HttpError, HttpOp, HttpResponse, InputEvent, Modifiers, MouseButton,
    PhysicalPosition, PointerEvent, PointerId, PointerKind, PointerPhase,
};
use render::glam::Vec2;
use render::{BackendPref, FrameSource, GameCamera, Renderer, Scene};

const SIZE: (u32, u32) = (1920, 1080);

fn set_dir() -> Option<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        std::env::var_os("CIV5_UI_ASSETS").map(PathBuf::from),
        Some(here.join("../../../../b4-ui-assets")),
        Some(here.join("../../../../../b4-ui-assets")),
    ];
    for dir in candidates.into_iter().flatten() {
        if dir.join("manifest").is_file() {
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

fn pointer(phase: PointerPhase, x: f64, y: f64, button: Option<MouseButton>) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        id: PointerId::MOUSE,
        kind: PointerKind::Mouse,
        phase,
        position: PhysicalPosition { x, y },
        button,
        modifiers: Modifiers::default(),
    })
}

fn counter(r: &Renderer, name: &str) -> u64 {
    r.last_frame_stats()
        .counters
        .iter()
        .find(|(n, _)| *n == name)
        .map_or(0, |(_, v)| *v)
}

/// PNG, 8-bit RGBA, stored (uncompressed) zlib blocks.
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(body);
        out.extend_from_slice(&c);
        out.extend_from_slice(&crc(&c).to_be_bytes());
    }
    let stride = width as usize * 4;
    let mut raw = Vec::new();
    for row in rgba.chunks_exact(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    while let Some(blk) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&(blk.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(blk.len() as u16)).to_le_bytes());
        z.extend_from_slice(blk);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[test]
fn m1_is_drawn_by_the_renderer_and_answers_the_mouse() {
    let Some(dir) = set_dir() else { return };
    let mut renderer =
        match platform_native::block_on(Renderer::new_headless(SIZE, BackendPref::Auto)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("SKIPPED: no renderer: {e}");
                return;
            }
        };
    let http: Rc<dyn HttpClient> = Rc::new(DirHttp(dir));
    let source = HttpSource::new(Rc::clone(&http), "http://test/assets/");
    let mut loader = AssetLoader::new(http, "http://test/assets/", Box::new(source));
    loader.load_manifest(None);

    let mut original = OriginalUi::m1();
    let mut list = UiDrawList::new();
    let camera = GameCamera::new(Vec2::ZERO);
    let scene = Scene::test(0.0);
    let mut now = 0u64;
    let mut frames = 0;
    // Boot, then enough frames for the textures to go up.
    while frames < 400 {
        let drawn = original.begin_frame(now, Some(&mut loader), SIZE, &mut list);
        renderer
            .render(&scene, &camera, &list, None)
            .expect("frame");
        let up = original.end_frame(&mut renderer);
        assert_ne!(original.phase(), Phase::Failed);
        now += 16;
        frames += 1;
        if drawn && up.uploaded == 0 && frames > 3 && counter(&renderer, "ui.tex_missing") == 0 {
            break;
        }
    }
    assert_eq!(original.phase(), Phase::Running);
    let layer = original.layer().expect("layer");
    let stats = layer.stats();
    eprintln!("{stats:?}");
    assert_eq!(
        stats.core.lua_errors,
        0,
        "{:?}",
        layer.core().runtime().recent_errors()
    );
    assert!(stats.draw_cmds > 0 && stats.vertices > 0);
    assert!(stats.text_quads > 0, "no glyph quads were laid out");
    assert_eq!(
        stats.tex_missing,
        0,
        "{:?}",
        layer.store().missing_textures()
    );
    assert_eq!(
        counter(&renderer, "ui.tex_missing"),
        0,
        "textures the renderer never got"
    );
    assert!(counter(&renderer, "ui.cmds_drawn") > 0);
    assert!(
        counter(&renderer, "ui.quads") > 6,
        "text and sprites were drawn"
    );

    // Let the notification slide in and its textures go up before the frame is taken.
    for _ in 0..200 {
        original.begin_frame(now, Some(&mut loader), SIZE, &mut list);
        renderer
            .render(&scene, &camera, &list, None)
            .expect("frame");
        original.end_frame(&mut renderer);
        now += 16;
    }

    // The frame as the window would show it.
    let readback = renderer.read_frame(FrameSource::WithUi);
    original.begin_frame(now, Some(&mut loader), SIZE, &mut list);
    renderer
        .render(&scene, &camera, &list, None)
        .expect("frame");
    let image = readback.wait().expect("readback");
    // The top bar is not the world: it is a dark strip with lighter text pixels.
    let bar: Vec<&[u8]> = (0..SIZE.0).step_by(4).map(|x| image.pixel(x, 12)).collect();
    let distinct: std::collections::BTreeSet<[u8; 3]> =
        bar.iter().map(|p| [p[0], p[1], p[2]]).collect();
    assert!(distinct.len() > 3, "the top bar looks flat: {distinct:?}");
    if let Some(out) = std::env::var_os("CIV5_SCREENSHOT_DIR") {
        let out = PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        let rgba: Vec<u8> = if matches!(
            image.format,
            render::wgpu::TextureFormat::Bgra8Unorm | render::wgpu::TextureFormat::Bgra8UnormSrgb
        ) {
            image
                .bytes
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect()
        } else {
            image.bytes.clone()
        };
        std::fs::write(out.join("m1.png"), png(image.size.0, image.size.1, &rgba)).unwrap();
    }

    // The mouse: sweeping the top bar, some control takes the pointer; a click on it runs its Lua.
    let mut taken = 0;
    for x in (0..SIZE.0).step_by(6) {
        if original.handle_input(&pointer(PointerPhase::Move, f64::from(x), 14.0, None)) {
            taken += 1;
        }
    }
    assert!(taken > 0, "no control of the top bar took the pointer");
    assert!(
        !original.handle_input(&pointer(PointerPhase::Move, 900.0, 700.0, None)),
        "the open map is the world's"
    );
    let before = original.layer().unwrap().stats().core.lua_errors;
    // (Not the menu button at the far right: it queues /InGame/GameMenu, which the M1 set does not have.)
    for x in (1252..1840).step_by(10) {
        let (x, y) = (f64::from(x), 14.0);
        original.handle_input(&pointer(PointerPhase::Move, x, y, None));
        original.handle_input(&pointer(PointerPhase::Down, x, y, Some(MouseButton::Left)));
        original.handle_input(&pointer(PointerPhase::Up, x, y, Some(MouseButton::Left)));
    }
    original.begin_frame(now + 16, Some(&mut loader), SIZE, &mut list);
    let layer = original.layer().unwrap();
    assert_eq!(
        layer.stats().core.lua_errors,
        before,
        "a click raised a Lua error: {:?}",
        layer.core().runtime().recent_errors()
    );
}
