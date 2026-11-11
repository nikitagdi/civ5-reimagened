//! The classic original UI (EUI's `InGame` over the exe API, `crate::m2::M2Ui`) as a frontend of the host.
//!
//! The frame runs the layer after the game session's frame and carries out what the layer asked of the world (the
//! camera, the highlights, the grid, the bridges, the "look here" effect); the input goes to the Lua handlers and
//! the exe's hot keys, and what they leave moves the camera. Removing the classic UI is removing this module, the
//! modules it wraps and its one registration in the app.

use std::rc::Rc;

use client_core::ClientEvent;
use draw_list::UiDrawList;
use platform::{InputEvent, KeyValueStore};
use render::Renderer;

use super::{FrameCx, Frontend, InputCx, PrepareCx, Readiness, Selection, UiKind, UiSoundSource};
use crate::m2::{M2Phase, M2Ui};

/// The classic UI: the M2 layer and the press tracking of its input.
pub struct ClassicFrontend {
    m2: M2Ui,
    /// Whether the press the layers hold moved past the drag threshold (`ui_press.rs`).
    ui_press: crate::ui_press::UiPress,
    /// The layer went to sleep while it was up: the core's events it missed were dropped by the session.
    slept_with_layer: bool,
    /// Wake up the cold way: the layer is made again (`?ui2classic=cold`); else the warm way, the layer stays and
    /// takes the game as a new load.
    cold_wake: bool,
    /// When the last wake-up began (ms), until the layer runs again: `ui.classic.wake_ms` measures it.
    woke_at: Option<f64>,
}

impl ClassicFrontend {
    /// `kv`: the page's store for the options and the user data; `ui_volume`: the page's `?volume=` (0..=1).
    pub fn new(kv: Rc<dyn KeyValueStore>, ui_volume: f32) -> ClassicFrontend {
        ClassicFrontend {
            m2: M2Ui::new(kv, ui_volume),
            ui_press: crate::ui_press::UiPress::default(),
            slept_with_layer: false,
            cold_wake: false,
            woke_at: None,
        }
    }
}

impl Frontend for ClassicFrontend {
    fn kind(&self) -> UiKind {
        UiKind::Classic
    }

    fn needs_replica(&self) -> bool {
        true
    }

    fn prepare(&mut self, _cx: &mut PrepareCx<'_>) -> Readiness {
        match self.m2.phase() {
            M2Phase::Failed => Readiness::Failed(
                self.m2
                    .failure()
                    .unwrap_or("the classic UI failed")
                    .to_owned(),
            ),
            // The layer boots in its own frames (the cold path).
            _ => Readiness::Ready,
        }
    }

    fn activate(&mut self, cx: &mut PrepareCx<'_>) {
        // Measured from here to the layer's first frame (a layer that never came up boots in that time).
        self.woke_at = Some(cx.now_ms);
        if std::mem::take(&mut self.slept_with_layer) {
            // The layer missed the core's events while it slept. The warm way keeps it and announces the game to
            // it again as a new load; the cold way (on request, or without a layer to keep) makes it again.
            // SIMPLIFIED(SIMP-267): the warm way announces every unit and city again, as after a snapshot load.
            if !self.cold_wake && self.m2.force_resync() {
                tracing::info!("the classic UI wakes up: the game is announced to its layer again");
            } else {
                tracing::info!("the classic UI wakes up: its layer is made again");
                self.m2.restart_layer();
            }
        }
    }

    fn deactivate(&mut self, _cx: &mut PrepareCx<'_>) {
        self.slept_with_layer = self.m2.layer().is_some();
        self.woke_at = None;
    }

    fn set_cold_wake(&mut self, cold: bool) {
        self.cold_wake = cold;
    }

    fn options(&mut self) -> Vec<(String, String)> {
        vec![("classic".into(), "Classic".into())]
    }

    fn frame(&mut self, cx: &mut FrameCx<'_>, out: &mut UiDrawList) {
        m2_frame(&mut self.m2, cx, out);
        // The camera keys of `WorldView.lua` (SIMP-exe-560).
        for k in self.m2.take_camera_keys() {
            cx.map_controller.handle(&k, cx.camera);
        }
        // The wake-up is over once the layer ran a frame: from the activation to the end of that frame.
        if self.m2.layer().is_some()
            && self.m2.phase() == M2Phase::Running
            && let Some(t0) = self.woke_at.take()
        {
            let ms = cx.clock.now_ms_f64() - t0;
            render::perf::set_counter("ui.classic.wake_ms", ms);
            tracing::info!(ms, cold = self.cold_wake, "the classic UI is back");
        }
    }

    fn input(&mut self, cx: &mut InputCx<'_>, ev: InputEvent) {
        m2_input(&mut self.m2, &mut self.ui_press, cx, ev);
    }

    fn net_event(&mut self, ev: &ClientEvent) {
        // The original UI's end of turn: a refusal brings the end-turn button back (live game 931).
        match ev {
            ClientEvent::OrderAnswered { id, outcome } => {
                use protocol::{CommandOutcome as O, RejectCode};
                match outcome {
                    O::Queued => {}
                    // "Already ended": the turn is ended all the same.
                    O::Applied
                    | O::Rejected {
                        code: RejectCode::TurnEnded,
                        ..
                    } => self.m2.on_order_done(*id, false),
                    O::Rejected { .. } | O::Lost => self.m2.on_order_done(*id, true),
                }
            }
            ClientEvent::OrderRolledBack { id, .. } => self.m2.on_order_done(*id, true),
            _ => {}
        }
    }

    fn frame_started(&mut self, now_ms: f64) {
        self.m2.frame_started(now_ms);
    }

    fn after_render(&mut self, renderer: &mut Renderer, drawn: bool) {
        // The turn timeline's render settle (`c.render_settle`): the pages in view are all there.
        let live = renderer.terrain_live_stats();
        let pending = live.unbuilt + live.unpaged;
        self.m2.frame_done(
            drawn && pending == 0 && live.ring_unbuilt == 0 && live.ring_unpaged == 0,
            pending,
        );
        let wanted = renderer.ui_textures_mut().take_wanted();
        for tex in self.m2.resolve_textures(&wanted) {
            let data = render::UiTextureData {
                width: tex.width,
                height: tex.height,
                format: tex.format,
                mips: vec![tex.bytes],
                content: Some(tex.content),
            };
            if let Err(e) = renderer.ui_textures_mut().upload(tex.id, &data) {
                tracing::warn!(texture = tex.id.0, "ui texture upload: {e}");
            }
        }
    }

    fn new_game(&mut self) {
        self.m2.new_game();
        self.slept_with_layer = false;
    }

    fn turn_processing(&self) -> bool {
        self.m2.turn_processing()
    }

    fn dispatches_core_events(&self) -> bool {
        true
    }

    fn cursor_row(&self) -> Option<i32> {
        self.m2.layer().map(|l| l.cursor())
    }

    fn trade_paths(&self) -> Option<crate::m2::TradePaths> {
        self.m2.trade_paths()
    }

    fn popup_route(&self) -> Option<exe_api::PopupRoute> {
        self.m2.popup_route()
    }

    fn selection(&mut self, ring: Option<(i32, i32)>) -> Option<Selection> {
        // The original UI owns the highlights (`Events.SerialEventHexHighlight`), the path (its path finder) and
        // the selection: the engine draws its ring under the head selected unit of the UI (`UnitSelectionChanged`),
        // not under the interim controller's selection, which the original UI never sets.
        let shown = (self.m2.movement_path(), self.m2.selected_unit_plot());
        self.m2.note_ring(ring);
        Some(shown)
    }

    fn sounds(&self) -> Option<&dyn UiSoundSource> {
        Some(&self.m2)
    }
}

impl UiSoundSource for M2Ui {
    fn take_knobs(&self) -> Vec<(String, f32)> {
        M2Ui::take_knobs(self)
    }

    fn take_button_sounds(&self) -> Vec<(audio::ButtonEvent, audio::ButtonFlags)> {
        self.layer()
            .map(|l| l.take_button_sounds())
            .unwrap_or_default()
    }

    fn take_sound_scripts(&self) -> Vec<String> {
        M2Ui::take_sound_scripts(self)
    }

    fn take_music_events(&self) -> Vec<exe_api::music::MusicEvent> {
        M2Ui::take_music_events(self)
    }

    fn music_state(&self) -> Option<exe_api::music::MusicState> {
        M2Ui::music_state(self)
    }
}

/// One frame of the M2 original UI: the exe API and the layer (frame order of the exe API), then what the layer
/// asked of the world (camera, highlights).
fn m2_frame(m2: &mut M2Ui, cx: &mut FrameCx<'_>, out: &mut UiDrawList) {
    let (size, now) = (cx.size, cx.now);
    let seat = cx.seat.clone().unwrap_or_default();
    let ai_turn = cx.net.and_then(|h| {
        h.peek(|c| {
            c.ai_turn().map(|l| exe_api::AiTurnLog {
                turn: l.turn,
                events: l
                    .events
                    .iter()
                    .map(|e| (i32::from(e.player), e.ended))
                    .collect(),
            })
        })
        .flatten()
    });
    let aspect = size.0 as f32 / size.1.max(1) as f32;
    let projection = Some(crate::m2::ProjectionInfo {
        view_proj: cx.camera.view_proj(aspect),
        ui_scale: ui::UiConfig::scale_for_height(size.1 as f32),
        focus: cx.camera.focus().to_array(),
        level: cx.camera.level(),
    });
    let env = crate::m2::M2Env {
        now_ms: now,
        loader: cx.loader.as_deref_mut(),
        size,
        net: cx.net,
        clock: cx.clock,
        seat,
        ai_turn,
        projection,
    };
    let renderer = &mut *cx.renderer;
    let camera = &mut *cx.camera;
    // `GridToWorld` gives the relief's height as z (turn numbers of a path, yield and resource icons, flags).
    let rev = renderer.ground_revision();
    if rev != m2.plot_heights_revision() {
        match renderer.plot_center_heights() {
            Some((w, h, heights)) => m2.set_plot_heights(rev, w, h, heights),
            None => m2.set_plot_heights(rev, 0, 0, Vec::new()),
        }
    }
    let has_map = renderer.minimap_to_world(32_768, 32_768).is_some();
    m2.set_minimap_size(
        renderer.minimap_view().map_or((0, 0), |(_, i)| i.size),
        has_map,
    );
    m2.set_texture_profile(&renderer.platform_profile());
    m2.begin_frame(env, cx.session, cx.step, out);
    if let Some(on) = m2.take_grid() {
        renderer.set_grid_visible(on);
    }
    m2.set_camera_moving(camera.is_following() || camera.is_gliding());
    if let Some((x, y)) = m2.take_look_at() {
        // Every `lookAt` of the exe sets a target the camera eases to (night answers 1b); the exe has
        // no jump for CAMERALOOKAT_IMMEDIATE either, only StartupGame teleports.
        // SIMPLIFIED(SIMP-exe-750): UI.LookAt's type argument is ignored: CITY_ZOOM_IN/OUT and SHRINE_ZOOM_IN
        // (heights 500/700/800) keep the player's height, IMMEDIATE eases like NORMAL (as in the exe)
        camera.look_at(render::hexgrid::cell_center(x, y));
    }
    // `Events.GameplayFX` (a notification of the seat with a plot, a ruin found among them, or a script): the
    // "look here" effect of the exe's particle system.
    // SIMPLIFIED(SIMP-132): the sounds of the effect (the VisEffect's AS2D_WORLD_RAY_OF_GOD, the EventEffect's
    // AS2D_BUILD_UNIT) are not played; whether the exe plays them for this event is not traced.
    for (x, y) in m2.take_gameplay_fx() {
        renderer.particles_mut().look_at(x, y);
    }
    // Debug (the golden scenario): plot (x, y) at the screen fractions (px, py), at the zoom `level`.
    if let Some((x, y, px, py, level)) = m2.take_camera_place() {
        if level > 0.0 {
            camera.set_level(level);
        }
        let target = render::hexgrid::cell_center(x, y);
        camera.center_on(target);
        let at = render::pixels_to_ndc(
            px * size.0 as f32,
            py * size.1 as f32,
            size.0 as f32,
            size.1 as f32,
        );
        let mid = render::glam::Vec2::ZERO;
        if let (Some(a), Some(b)) = (
            camera.ground_point(at, aspect),
            camera.ground_point(mid, aspect),
        ) {
            camera.center_on(target - render::glam::Vec2::new(a.x - b.x, a.z - b.z));
        }
    }
    // K6: a click on the minimap panel moves the camera (the same way as the interim panel's).
    if let Some((u, v)) = m2.take_minimap_click()
        && let Some(at) = renderer.minimap_to_world(u, v)
    {
        camera.center_on(at);
    }
    // The city screen (K8): the camera takes the city view's zoom and gives the player's zoom back on leaving.
    if let Some(up) = m2.take_city_view() {
        m2.city_camera(up, camera);
    }
    if let Some(set) = m2.take_highlights() {
        renderer.set_highlight_set(set);
    }
    if let Some((bridge, iron)) = m2.take_bridge() {
        renderer.set_bridges(bridge, iron);
    }
    if let Some(sink) = cx.stats
        && let Some(stats) = m2.stats(now, cx.session)
    {
        for (group, values) in &stats.groups {
            sink(group, values);
        }
    }
}

/// The original UI sees what egui did not take: the Lua handlers and the exe's hot keys first; what neither takes
/// moves the camera.
fn m2_input(
    m2: &mut M2Ui,
    ui_press: &mut crate::ui_press::UiPress,
    cx: &mut InputCx<'_>,
    ev: InputEvent,
) {
    let at = match &ev {
        InputEvent::Pointer(p) => Some(p.position),
        InputEvent::Wheel(w) => Some(w.position),
        _ => None,
    };
    if let Some(at) = at {
        // `UI.GetMouseOverHex` and `SerialEventMouseOverHex` before the click reaches `WorldView.lua`.
        let ray = cx.map_controller.ray_at(at, cx.camera);
        if let Some(p) = cx.picker.pick(cx.bridge, &ray) {
            let (x, y) = p.cell;
            m2.mouse_over_hex(x, y);
        }
    }
    // A press that became a drag is not a click: its release must not reach a city banner or the world's
    // button-up handler (the exe drops the click after `ProcessCameraDrag`). SIMPLIFIED(SIMP-uic-12).
    // A press the layers took (a banner, the world's button-down) never reaches the map, so its own movement is
    // followed too: any press past the drag threshold is a drag, whoever holds it.
    ui_press.observe(&ev);
    let press_moved = ui_press.moved(&ev);
    let ui_ev =
        crate::app::ui_event_after_drag(&ev, cx.map_controller.is_dragging() || press_moved);
    let taken = m2.handle_input(&ui_ev);
    // The map keeps following a held button over UI controls (a banner), or the drag would stall there.
    if taken
        && matches!(&ev, InputEvent::Pointer(p) if p.phase == platform::PointerPhase::Move)
        && cx.map_controller.is_pressed()
    {
        cx.map_controller.handle(&ev, cx.camera);
    }
    // The camera events the keys fired at once (`WorldView.lua`'s arrows, SIMP-exe-560).
    for k in m2.take_camera_keys() {
        cx.map_controller.handle(&k, cx.camera);
    }
    // A key that neither the Lua UI nor the game's keys used is the mods' hotkey (client-hooks C4).
    if !taken && !cx.hooks.is_empty() {
        let claim = cx.hooks.unclaimed_input(&ev);
        if claim.pointer_lock.is_some() {
            cx.pointer_lock = claim.pointer_lock;
        }
        if claim.taken {
            return;
        }
    }
    if !taken {
        cx.map_controller.handle(&ev, cx.camera);
    } else if crate::app::ends_press(&ev) {
        // WorldView.lua takes the button release (unit selection, orders) after the press went to the map:
        // the drag still has to end, or the camera follows the cursor until the next press. The click is
        // the UI's, not the map's.
        cx.map_controller.handle(&ev, cx.camera);
        cx.map_controller.take_click();
        cx.map_controller.take_right_click();
    }
}
