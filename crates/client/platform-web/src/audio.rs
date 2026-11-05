//! Web Audio output. The context starts suspended; a browser lets it
//! run only after a user gesture, so `resume` must be called from a gesture handler.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use platform::{AudioOutError, AudioOutput, Pcm, SampleId, VoiceId, VoiceParams};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, GainNode,
    HtmlAudioElement, MediaElementAudioSourceNode, StereoPannerNode,
};

fn js_err(e: JsValue) -> AudioOutError {
    AudioOutError::Failed(format!("{e:?}"))
}

/// Shared with the closures that finish a decode or end a voice.
#[derive(Default)]
struct Shared {
    buffers: HashMap<u32, AudioBuffer>,
    live: HashSet<u64>,
}

pub struct WebAudio {
    ctx: AudioContext,
    master: GainNode,
    shared: Rc<RefCell<Shared>>,
    sources: HashMap<u64, AudioBufferSourceNode>,
    /// The gain node of each voice in `sources` and `streams` (a world ambience changes its gain while it sounds, a
    /// volume slider changes every voice of its bus).
    gains: HashMap<u64, GainNode>,
    /// The stereo position of each looping voice (a world ambience moves in the field while it sounds).
    panners: HashMap<u64, StereoPannerNode>,
    /// The voices that play a file while it downloads (the music): an `<audio>` element into the context.
    streams: HashMap<u64, (HtmlAudioElement, MediaElementAudioSourceNode)>,
    next_voice: u64,
}

impl WebAudio {
    /// Creates the context (suspended until a gesture).
    pub fn new() -> Result<Self, AudioOutError> {
        let ctx = AudioContext::new().map_err(js_err)?;
        let master = ctx.create_gain().map_err(js_err)?;
        master
            .connect_with_audio_node(&ctx.destination())
            .map_err(js_err)?;
        // A press anywhere on the page is a gesture, the DOM lobby's too (the canvas does not see those): the context
        // starts then, so the main menu's music can play before the first press on the game.
        if let Some(w) = web_sys::window() {
            let c = ctx.clone();
            let resume = Closure::<dyn FnMut()>::new(move || {
                if c.state() != AudioContextState::Running
                    && let Ok(p) = c.resume()
                {
                    wasm_bindgen_futures::spawn_local(async move {
                        let _ = wasm_bindgen_futures::JsFuture::from(p).await;
                    });
                }
            });
            for kind in ["pointerdown", "keydown"] {
                let _ = w.add_event_listener_with_callback(kind, resume.as_ref().unchecked_ref());
            }
            resume.forget();
        }
        Ok(Self {
            ctx,
            master,
            shared: Rc::new(RefCell::new(Shared::default())),
            sources: HashMap::new(),
            gains: HashMap::new(),
            panners: HashMap::new(),
            streams: HashMap::new(),
            next_voice: 0,
        })
    }

    /// Forgets the nodes of voices that ended by themselves (a one-shot's gain node is kept while it sounds now).
    fn prune(&mut self) {
        let shared = self.shared.borrow();
        self.sources.retain(|id, _| shared.live.contains(id));
        self.gains.retain(|id, _| shared.live.contains(id));
        self.panners.retain(|id, _| shared.live.contains(id));
        let ended: Vec<u64> = self
            .streams
            .keys()
            .filter(|id| !shared.live.contains(id))
            .copied()
            .collect();
        drop(shared);
        for id in ended {
            self.drop_stream(id);
        }
    }

    /// Stops a stream's element and lets its download go.
    fn drop_stream(&mut self, id: u64) {
        if let Some((el, node)) = self.streams.remove(&id) {
            let _ = el.pause();
            let _ = el.remove_attribute("src");
            el.load();
            let _ = node.disconnect();
        }
    }
}

impl AudioOutput for WebAudio {
    fn load_pcm(&mut self, sample: SampleId, pcm: &Pcm) -> Result<(), AudioOutError> {
        let channels = u32::from(pcm.channels.max(1));
        let frames = pcm.frames() as u32;
        if frames == 0 {
            return Err(AudioOutError::Failed("empty sound".into()));
        }
        let buf = self
            .ctx
            .create_buffer(channels, frames, pcm.rate as f32)
            .map_err(js_err)?;
        let mut plane = vec![0f32; frames as usize];
        // `copyToChannel` rejects a view of a SharedArrayBuffer ("must not be shared"), which is what a slice of the wasm memory
        // is in the threaded build: copy each plane into a Float32Array of its own (a plain ArrayBuffer) first.
        let staging = js_sys::Float32Array::new_with_length(frames);
        for c in 0..channels {
            for (i, f) in plane.iter_mut().enumerate() {
                *f = f32::from(pcm.samples[i * channels as usize + c as usize]) / 32768.0;
            }
            staging.copy_from(&plane);
            buf.copy_to_channel_with_f32_array(&staging, c as i32)
                .map_err(js_err)?;
        }
        self.shared.borrow_mut().buffers.insert(sample.0, buf);
        Ok(())
    }

    fn load_encoded(&mut self, sample: SampleId, bytes: &[u8]) -> Result<(), AudioOutError> {
        let array = js_sys::Uint8Array::from(bytes);
        let promise = self
            .ctx
            .decode_audio_data(&array.buffer())
            .map_err(js_err)?;
        let shared = self.shared.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(v) => {
                    if let Ok(buf) = v.dyn_into::<AudioBuffer>() {
                        shared.borrow_mut().buffers.insert(sample.0, buf);
                    }
                }
                Err(e) => tracing::warn!(?e, "decodeAudioData failed"),
            }
        });
        Ok(())
    }

    fn is_loaded(&self, sample: SampleId) -> bool {
        self.shared.borrow().buffers.contains_key(&sample.0)
    }

    fn play(&mut self, sample: SampleId, gain: f32, looped: bool) -> Option<VoiceId> {
        self.play_with(sample, gain, looped, VoiceParams::default())
    }

    fn play_with(
        &mut self,
        sample: SampleId,
        gain: f32,
        looped: bool,
        params: VoiceParams,
    ) -> Option<VoiceId> {
        if !self.is_running() {
            return None;
        }
        self.prune();
        let buf = self.shared.borrow().buffers.get(&sample.0).cloned()?;

        let src = self.ctx.create_buffer_source().ok()?;
        src.set_buffer(Some(&buf));
        src.set_loop(looped);
        if params.rate.is_finite() && params.rate > 0.0 && params.rate != 1.0 {
            src.playback_rate().set_value(params.rate);
        }
        let g = self.ctx.create_gain().ok()?;
        g.gain().set_value(gain);
        src.connect_with_audio_node(&g).ok()?;
        // One panner per voice, after its gain: a unit's step sounds from its side, a world ambience moves in the field.
        let panner = self.ctx.create_stereo_panner().ok()?;
        panner.pan().set_value(params.pan.clamp(-1.0, 1.0));
        g.connect_with_audio_node(&panner).ok()?;
        panner.connect_with_audio_node(&self.master).ok()?;
        self.next_voice += 1;
        let id = self.next_voice;
        let shared = self.shared.clone();
        let ended = Closure::once_into_js(move || {
            shared.borrow_mut().live.remove(&id);
        });
        let scheduled: &web_sys::AudioScheduledSourceNode = src.as_ref();
        scheduled.set_onended(Some(ended.unchecked_ref()));
        scheduled.start().ok()?;
        self.shared.borrow_mut().live.insert(id);
        self.sources.insert(id, src);
        // The nodes are kept while the voice sounds: a loop changes its gain, and so does any voice whose volume slider
        // moves; a world ambience moves in the stereo field (`prune` lets them go when the voice ended).
        self.gains.insert(id, g);
        self.panners.insert(id, panner);
        Some(VoiceId(id))
    }

    fn set_voice_pan(&mut self, voice: VoiceId, pan: f32) {
        if let Some(p) = self.panners.get(&voice.0) {
            let _ = p
                .pan()
                .set_target_at_time(pan.clamp(-1.0, 1.0), self.ctx.current_time(), 0.25);
        }
    }
    fn stop(&mut self, voice: VoiceId) {
        if let Some(src) = self.sources.remove(&voice.0) {
            let scheduled: &web_sys::AudioScheduledSourceNode = src.as_ref();
            let _ = scheduled.stop();
        }
        self.drop_stream(voice.0);
        self.gains.remove(&voice.0);
        self.panners.remove(&voice.0);
        self.shared.borrow_mut().live.remove(&voice.0);
    }

    fn can_stream(&self) -> bool {
        true
    }

    fn play_stream(&mut self, url: &str, gain: f32, looped: bool) -> Option<VoiceId> {
        if !self.is_running() {
            return None;
        }
        self.prune();
        let el = HtmlAudioElement::new().ok()?;
        // (the assets may come from another origin: without CORS the context would hear silence)
        el.set_cross_origin(Some("anonymous"));
        el.set_preload("auto");
        el.set_loop(looped);
        el.set_src(url);
        let node = self.ctx.create_media_element_source(&el).ok()?;
        let g = self.ctx.create_gain().ok()?;
        g.gain().set_value(gain);
        node.connect_with_audio_node(&g).ok()?;
        g.connect_with_audio_node(&self.master).ok()?;
        self.next_voice += 1;
        let id = self.next_voice;
        // The end of the file or a failed load ends the voice (the music then picks its next song).
        for kind in ["ended", "error"] {
            let shared = self.shared.clone();
            let done = Closure::once_into_js(move || {
                shared.borrow_mut().live.remove(&id);
            });
            let _ = el.add_event_listener_with_callback(kind, done.unchecked_ref());
        }
        if let Ok(p) = el.play() {
            let shared = self.shared.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(e) = wasm_bindgen_futures::JsFuture::from(p).await {
                    tracing::warn!(?e, "a music stream did not start");
                    shared.borrow_mut().live.remove(&id);
                }
            });
        }
        self.shared.borrow_mut().live.insert(id);
        self.gains.insert(id, g);
        self.streams.insert(id, (el, node));
        Some(VoiceId(id))
    }

    fn pause(&mut self, voice: VoiceId) {
        match self.streams.get(&voice.0) {
            Some((el, _)) => {
                let _ = el.pause();
            }
            None => self.stop(voice),
        }
    }

    fn unpause(&mut self, voice: VoiceId) -> bool {
        let Some((el, _)) = self.streams.get(&voice.0) else {
            return false;
        };
        if !self.shared.borrow().live.contains(&voice.0) {
            return false;
        }
        if let Ok(p) = el.play() {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(p).await;
            });
        }
        true
    }

    fn fade_out(&mut self, voice: VoiceId, secs: f32) {
        // A buffer voice glides to silence and ends then; a stream (the music, which fades itself) just stops.
        self.panners.remove(&voice.0);
        if self.streams.contains_key(&voice.0) {
            self.stop(voice);
            return;
        }
        let Some(g) = self.gains.remove(&voice.0) else {
            self.stop(voice);
            return;
        };
        let Some(src) = self.sources.remove(&voice.0) else {
            return;
        };
        let now = self.ctx.current_time();
        let param = g.gain();
        let _ = param.cancel_scheduled_values(now);
        let _ = param.set_value_at_time(param.value(), now);
        let _ = param.linear_ramp_to_value_at_time(0.0, now + f64::from(secs));
        let scheduled: &web_sys::AudioScheduledSourceNode = src.as_ref();
        let _ = scheduled.stop_with_when(now + f64::from(secs));
        self.shared.borrow_mut().live.remove(&voice.0);
    }

    fn set_voice_gain(&mut self, voice: VoiceId, gain: f32) {
        // (a short glide, so a gain that moves once a second does not click)
        if let Some(g) = self.gains.get(&voice.0) {
            let _ = g
                .gain()
                .set_target_at_time(gain, self.ctx.current_time(), 0.25);
        }
    }

    fn set_master_gain(&mut self, gain: f32) {
        self.master.gain().set_value(gain);
    }

    fn is_running(&self) -> bool {
        self.ctx.state() == AudioContextState::Running
    }

    fn resume(&mut self) {
        if let Ok(p) = self.ctx.resume() {
            wasm_bindgen_futures::spawn_local(async move {
                let _ = wasm_bindgen_futures::JsFuture::from(p).await;
            });
        }
    }

    fn is_playing(&self, voice: VoiceId) -> bool {
        self.shared.borrow().live.contains(&voice.0)
    }
}
