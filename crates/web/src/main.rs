//! Speed Haste — web frontend: asks for SPEEDH.JCL, then runs the game on a
//! canvas with WebAudio sound. URL query parameters become the original's
//! command-line options (`?nologo&track=3&car=2`), plus the port's `scale`,
//! `gpu` (wgpu on WebGL2, races drawn on the GPU at the canvas' resolution;
//! Ctrl+G / Ctrl+F toggle the race renderer / filtering), `filter`, and
//! `relay=ws://HOST:PORT/ROOM` for network races (speedhaste-relay), and
//! `touch=1` / `touch=0` for the on-screen touch controls.
//!
//! The shareware version is built in; the registered SPEEDH.JCL can be
//! chosen instead.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use speedhaste_core::audio::Audio;
use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::keys;
use speedhaste_core::pad::{self, Pad};
use speedhaste_core::sys::{Game, Storage};

mod touch;

fn main() {
    dioxus::launch(App);
}

// The user's SPEEDH.JCL is kept in this browser's IndexedDB so it needn't be
// picked on every visit. It never leaves the browser.
#[wasm_bindgen(inline_js = r#"
function db() {
    return new Promise((ok, err) => {
        const r = indexedDB.open('speedhaste', 1);
        r.onupgradeneeded = () => r.result.createObjectStore('files');
        r.onsuccess = () => ok(r.result);
        r.onerror = () => err(r.error);
    });
}
function req(mode, f) {
    return db().then(d => new Promise((ok, err) => {
        const r = f(d.transaction('files', mode).objectStore('files'));
        r.onsuccess = () => ok(r.result);
        r.onerror = () => err(r.error);
    }));
}
export function idb_get(k) { return req('readonly', s => s.get(k)); }
export function idb_put(k, v) { return req('readwrite', s => s.put(v, k)); }
export function idb_delete(k) { return req('readwrite', s => s.delete(k)); }

// Audio output: the page mixes ahead and posts interleaved stereo chunks to
// this worklet, which plays them and reports how many frames it played.
const PROC = `class Out extends AudioWorkletProcessor {
    constructor() {
        super();
        this.q = []; this.off = 0; this.played = 0; this.n = 0;
        this.port.onmessage = e => this.q.push(e.data);
    }
    process(inputs, outputs) {
        const [l, r] = outputs[0];
        for (let i = 0; i < l.length; i++) {
            if (this.q.length && this.off >= this.q[0].length) { this.q.shift(); this.off = 0; }
            if (!this.q.length) { l[i] = r[i] = 0; continue; }
            const b = this.q[0];
            l[i] = b[this.off]; r[i] = b[this.off + 1];
            this.off += 2; this.played++;
        }
        if (++this.n % 4 == 0) this.port.postMessage(this.played);
        return true;
    }
}
registerProcessor('speedhaste-out', Out);`;
export async function start_worklet(ctx) {
    const url = URL.createObjectURL(new Blob([PROC], { type: 'text/javascript' }));
    await ctx.audioWorklet.addModule(url);
    const node = new AudioWorkletNode(ctx, 'speedhaste-out', { numberOfInputs: 0, outputChannelCount: [2] });
    node.played = 0;
    node.port.onmessage = e => { node.played = e.data; };
    node.connect(ctx.destination);
    return node;
}
export function worklet_push(node, samples) { node.port.postMessage(samples, [samples.buffer]); }
export function worklet_played(node) { return node.played; }

// Fullscreen, in landscape where the browser can lock it (Android).
export function fullscreen() {
    if (document.fullscreenElement) { document.exitFullscreen(); return; }
    const e = document.documentElement;
    const p = e.requestFullscreen ? e.requestFullscreen() : null;
    if (p) p.then(() => screen.orientation && screen.orientation.lock && screen.orientation.lock('landscape').catch(() => {})).catch(() => {});
}

// Rumble the n-th connected gamepad (where the browser supports it).
export function pad_rumble(n, strong, weak, ms) {
    const pads = [...navigator.getGamepads()].filter(g => g && g.connected);
    const a = pads[n] && pads[n].vibrationActuator;
    if (a && a.playEffect) a.playEffect('dual-rumble', { duration: ms, strongMagnitude: strong, weakMagnitude: weak }).catch(() => {});
}
"#)]
extern "C" {
    fn idb_get(key: &str) -> js_sys::Promise;
    fn idb_put(key: &str, value: &js_sys::Uint8Array) -> js_sys::Promise;
    fn idb_delete(key: &str) -> js_sys::Promise;
    fn start_worklet(ctx: &web_sys::AudioContext) -> js_sys::Promise;
    fn worklet_push(node: &JsValue, samples: js_sys::Float32Array);
    fn worklet_played(node: &JsValue) -> f64;
    fn pad_rumble(n: u32, strong: f32, weak: f32, ms: u32);
    fn fullscreen();
}

/// The AudioWorklet output: keeps ~80 ms of mixed audio queued.
struct Worklet {
    node: JsValue,
    written: u64,
    buf: Vec<f32>,
}

impl Worklet {
    fn pump(&mut self, audio: &Mutex<Audio>, rate: u32) {
        let queued = self.written.saturating_sub(worklet_played(&self.node) as u64);
        let target = rate as u64 * 80 / 1000;
        if queued >= target {
            return;
        }
        let n = (target - queued).next_multiple_of(128) as usize;
        self.buf.resize(n * 2, 0.0);
        if let Ok(mut a) = audio.lock() {
            a.render(&mut self.buf);
        }
        worklet_push(&self.node, js_sys::Float32Array::from(&self.buf[..]));
        self.written += n as u64;
    }
}

/// Fallback for browsers/pages without AudioWorklet (it needs a secure
/// context): a ScriptProcessorNode pulling from the mixer.
fn script_processor(actx: &web_sys::AudioContext, audio: Arc<Mutex<Audio>>) {
    let Ok(node) = actx.create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(2048, 0, 2) else {
        return;
    };
    let mut buf = Vec::new();
    let cb = Closure::<dyn FnMut(web_sys::AudioProcessingEvent)>::new(move |e: web_sys::AudioProcessingEvent| {
        let Ok(out) = e.output_buffer() else { return };
        let n = out.length() as usize;
        buf.resize(n * 2, 0.0f32);
        if let Ok(mut a) = audio.lock() {
            a.render(&mut buf);
        }
        let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        let r: Vec<f32> = buf.iter().skip(1).step_by(2).copied().collect();
        let _ = out.copy_to_channel(&l, 0);
        let _ = out.copy_to_channel(&r, 1);
    });
    node.set_onaudioprocess(Some(cb.as_ref().unchecked_ref()));
    cb.forget();
    let _ = node.connect_with_audio_node(&actx.destination());
}

const CACHE_KEY: &str = "SPEEDH.JCL";

async fn cached_jcl() -> Option<Vec<u8>> {
    let v = JsFuture::from(idb_get(CACHE_KEY)).await.ok()?;
    v.dyn_into::<js_sys::Uint8Array>().ok().map(|a| a.to_vec())
}

const STYLE: &str = r#"
html, body { margin: 0; height: 100%; background: #000; color: #ddd; font-family: sans-serif; }
#main { height: 100%; display: flex; align-items: center; justify-content: center; }
.loader { text-align: center; max-width: 36em; padding: 2em; }
.loader h1 { color: #e33; font-style: italic; letter-spacing: .1em; }
.loader input { margin: 1.5em; font-size: 1.1em; }
.error { color: #f66; }
.loader button { margin: 0 .5em 1em; font-size: 1.1em; padding: .4em 1.2em; }
.small { font-size: .85em; color: #999; }
canvas { image-rendering: pixelated; width: min(100vw, calc(100vh * 4 / 3)); height: min(75vw, 100vh); outline: none; }
.loader .play { font-size: 1.3em; }
.exit { text-align: center; }
.exit pre { font: min(2vw, 3.4vh) monospace; text-align: left; display: inline-block; margin: 0 0 2em; }
.exit a { display: block; color: #aaa; }
body.touch { overflow: hidden; overscroll-behavior: none; touch-action: none; }
"#;

#[component]
fn App() -> Element {
    let mut running = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut touch_on = use_signal(touch::initially_enabled);
    let pending: Rc<RefCell<Option<Jcl>>> = use_hook(|| Rc::new(RefCell::new(None)));
    // A SPEEDH.JCL remembered from an earlier visit.
    let mut cached = use_signal(|| false);
    let cache: Rc<RefCell<Option<Vec<u8>>>> = use_hook(|| Rc::new(RefCell::new(None)));
    let c = cache.clone();
    use_future(move || {
        let c = c.clone();
        async move {
            if let Some(bytes) = cached_jcl().await {
                *c.borrow_mut() = Some(bytes);
                cached.set(true);
            }
        }
    });

    let p = pending.clone();
    use_effect(move || {
        if running()
            && let Some(jcl) = p.borrow_mut().take()
        {
            start_game(jcl, *touch_on.peek());
        }
    });

    let p = pending.clone();
    let on_file = move |evt: FormEvent| {
        let p = p.clone();
        async move {
            let Some(file) = evt.files().into_iter().next() else { return };
            match file.read_bytes().await {
                Ok(bytes) => match Jcl::parse(bytes.to_vec()) {
                    Ok(jcl) => {
                        let _ = JsFuture::from(idb_put(CACHE_KEY, &js_sys::Uint8Array::from(&bytes[..]))).await;
                        *p.borrow_mut() = Some(jcl);
                        running.set(true);
                    }
                    Err(e) => error.set(Some(format!("{}: {e}", file.name()))),
                },
                Err(e) => error.set(Some(format!("can't read {}: {e}", file.name()))),
            }
        }
    };

    let (p, c) = (pending.clone(), cache.clone());
    let on_play = move |_| match c.borrow_mut().take().map(Jcl::parse) {
        Some(Ok(jcl)) => {
            *p.borrow_mut() = Some(jcl);
            running.set(true);
        }
        Some(Err(e)) => {
            error.set(Some(format!("remembered SPEEDH.JCL: {e}")));
            cached.set(false);
        }
        None => cached.set(false),
    };
    let p = pending.clone();
    let on_shareware = move |_| {
        *p.borrow_mut() = Some(Jcl::shareware());
        running.set(true);
    };
    let c = cache.clone();
    let on_forget = move |_| {
        c.borrow_mut().take();
        cached.set(false);
        wasm_bindgen_futures::spawn_local(async {
            let _ = JsFuture::from(idb_delete(CACHE_KEY)).await;
        });
    };

    rsx! {
        style { {STYLE} {touch::STYLE} }
        if running() {
            canvas { id: "screen", width: "320", height: "200", tabindex: "0" }
            if touch_on() {
                touch::Overlay {
                    on_keyboard: move |_| {
                        if let Some(e) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id("kbd"))
                            && let Ok(e) = e.dyn_into::<web_sys::HtmlElement>()
                        {
                            let _ = e.focus();
                        }
                    },
                    on_fullscreen: move |_| fullscreen(),
                }
                // Typing names for the record tables, through the phone's keyboard.
                input { id: "kbd", r#type: "text", autocomplete: "off", autocapitalize: "characters", "autocorrect": "off", spellcheck: false }
            }
        } else {
            div { class: "loader",
                h1 { "SPEED HASTE" }
                p { "A port of the 1995 DOS racing game by Javier Arévalo and Noriaworks Entertainment." }
                if cached() {
                    p { "Your " b { "SPEEDH.JCL" } " is remembered in this browser." }
                    button { class: "play", onclick: on_play, autofocus: true, "Play" }
                    button { onclick: on_forget, "Forget it" }
                    p { class: "small", "Or choose another copy:" }
                } else {
                    p { "The shareware version (two circuits, one car) is built in:" }
                    button { class: "play", onclick: on_shareware.clone(), autofocus: true, "Play Shareware Version" }
                    p { "For the full game, choose your copy of " b { "SPEEDH.JCL" } " from the registered version:" }
                }
                input { r#type: "file", accept: ".jcl,.JCL", onchange: on_file }
                p { class: "small", "The file stays on your computer: it is only kept in this browser's storage." }
                if cached() {
                    button { onclick: on_shareware, "Play Shareware Version" }
                }
                p {
                    button {
                        onclick: move |_| {
                            let on = !touch_on();
                            touch_on.set(on);
                            touch::save_preference(on);
                        },
                        if touch_on() { "Touch controls: on" } else { "Touch controls: off" }
                    }
                }
                if let Some(e) = error() {
                    p { class: "error", "{e}" }
                }
                p { "Keys: arrows to drive, space to change gear, F1-F4 cameras, F9 HUD, F11 floor detail, F12 hi-res, Pause to pause, ESC to quit." }
                p { "Gamepad: stick or d-pad steers, RT/A accelerates, LT/X brakes, RB/LB shift gears, Y changes camera, Start pauses, Back quits." }
                p { class: "small", "Add " b { "?gpu" } " to the address for GPU-drawn races at your screen's resolution (Ctrl+G: GPU/original renderer, Ctrl+F: texture filtering), " b { "?scale=4" } " for sharper original rendering, " b { "?relay=ws://HOST:8790/ROOM" } " for network races through a speedhaste-relay server (Multiplayer Race in the menu)." }
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn canvas_target(c: &web_sys::HtmlCanvasElement) -> Result<wgpu::SurfaceTarget<'static>, String> {
    Ok(wgpu::SurfaceTarget::Canvas(c.clone()))
}

/// (The crate also builds natively, for checks; it only runs in browsers.)
#[cfg(not(target_arch = "wasm32"))]
fn canvas_target(_: &web_sys::HtmlCanvasElement) -> Result<wgpu::SurfaceTarget<'static>, String> {
    Err("not a browser".into())
}

/// Presentation through wgpu's WebGL2 backend (`?gpu`).
struct WebGpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: speedhaste_gpu::Renderer,
    opts: speedhaste_gpu::Options,
    canvas: web_sys::HtmlCanvasElement,
    /// Races drawn on the GPU (else by the software renderer).
    races: bool,
}

impl WebGpu {
    async fn new(canvas: web_sys::HtmlCanvasElement, filter: bool) -> Result<WebGpu, String> {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::GL;
        let instance = wgpu::Instance::new(desc);
        let surface = instance.create_surface(canvas_target(&canvas)?).map_err(|e| e.to_string())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&surface), ..Default::default() })
            .await
            .map_err(|e| e.to_string())?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;
        let mut config = surface.get_default_config(&adapter, canvas.width().max(1), canvas.height().max(1)).ok_or("surface not supported")?;
        if let Some(&f) = surface.get_capabilities(&adapter).formats.iter().find(|f| !f.is_srgb()) {
            config.format = f;
        }
        surface.configure(&device, &config);
        let renderer = speedhaste_gpu::Renderer::new(&device, &queue, config.format);
        Ok(WebGpu { surface, device, queue, config, renderer, opts: speedhaste_gpu::Options { filter }, canvas, races: true })
    }

    fn present(&mut self, frame: Option<&speedhaste_core::sys::Frame>) {
        // Draw at the canvas' size on screen, in device pixels.
        let dpr = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio());
        let w = ((self.canvas.client_width() as f64 * dpr).round() as u32).max(1);
        let h = ((self.canvas.client_height() as f64 * dpr).round() as u32).max(1);
        if (w, h) != (self.config.width, self.config.height) {
            self.canvas.set_width(w);
            self.canvas.set_height(h);
            (self.config.width, self.config.height) = (w, h);
            self.surface.configure(&self.device, &self.config);
        }
        let tex = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        if let Some(f) = frame {
            let view = tex.texture.create_view(&Default::default());
            // The page keeps the canvas at 4:3.
            self.renderer.render(f, &view, (w, h), [0.0, 0.0, w as f32, h as f32], &self.opts);
        }
        self.queue.present(tex);
    }
}

/// The relay connection for network races: a browser WebSocket.
struct WebPipe {
    ws: web_sys::WebSocket,
    inbox: Rc<RefCell<std::collections::VecDeque<Vec<u8>>>>,
    failed: Rc<RefCell<Option<String>>>,
    _callbacks: Vec<Closure<dyn FnMut(web_sys::Event)>>,
}

impl WebPipe {
    fn connect(url: &str) -> Result<WebPipe, String> {
        let ws = web_sys::WebSocket::new(url).map_err(|e| format!("{e:?}"))?;
        ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
        let inbox: Rc<RefCell<std::collections::VecDeque<Vec<u8>>>> = Rc::default();
        let failed: Rc<RefCell<Option<String>>> = Rc::default();
        let i = inbox.clone();
        let on_msg = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
            if let Some(e) = e.dyn_ref::<web_sys::MessageEvent>()
                && let Ok(buf) = e.data().dyn_into::<js_sys::ArrayBuffer>()
            {
                i.borrow_mut().push_back(js_sys::Uint8Array::new(&buf).to_vec());
            }
        });
        let (f1, f2) = (failed.clone(), failed.clone());
        let on_err = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            f1.borrow_mut().get_or_insert_with(|| "connection error".into());
        });
        let on_close = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            f2.borrow_mut().get_or_insert_with(|| "closed".into());
        });
        ws.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
        ws.set_onerror(Some(on_err.as_ref().unchecked_ref()));
        ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        Ok(WebPipe { ws, inbox, failed, _callbacks: vec![on_msg, on_err, on_close] })
    }
}

impl speedhaste_core::net::Pipe for WebPipe {
    fn status(&self) -> Result<bool, String> {
        if let Some(e) = self.failed.borrow().clone() {
            return Err(e);
        }
        Ok(self.ws.ready_state() == web_sys::WebSocket::OPEN)
    }
    fn send(&mut self, msg: Vec<u8>) {
        let _ = self.ws.send_with_u8_array(&msg);
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        self.inbox.borrow_mut().pop_front()
    }
}

impl Drop for WebPipe {
    fn drop(&mut self) {
        self.ws.set_onmessage(None);
        self.ws.set_onerror(None);
        self.ws.set_onclose(None);
        let _ = self.ws.close();
    }
}

/// A pipe that failed to open.
struct DeadPipe(String);

impl speedhaste_core::net::Pipe for DeadPipe {
    fn status(&self) -> Result<bool, String> {
        Err(self.0.clone())
    }
    fn send(&mut self, _: Vec<u8>) {}
    fn recv(&mut self) -> Option<Vec<u8>> {
        None
    }
}

/// Config and records in the browser's localStorage.
struct LocalStorage;

impl Storage for LocalStorage {
    fn load(&self, key: &str) -> Option<String> {
        web_sys::window()?.local_storage().ok()??.get_item(&format!("speedhaste/{key}")).ok()?
    }
    fn save(&mut self, key: &str, value: &str) {
        if let Some(Ok(Some(s))) = web_sys::window().map(|w| w.local_storage()) {
            let _ = s.set_item(&format!("speedhaste/{key}"), value);
        }
    }
}

/// The first two connected gamepads, read through the W3C "standard"
/// mapping (same layout for Xbox and DualShock pads).
fn read_pads() -> [Pad; 2] {
    let mut pads = [Pad::default(); 2];
    let Some(list) = web_sys::window().and_then(|w| w.navigator().get_gamepads().ok()) else { return pads };
    let connected = list.iter().filter_map(|g| g.dyn_into::<web_sys::Gamepad>().ok()).filter(|g| g.connected());
    for (slot, g) in pads.iter_mut().zip(connected) {
        let buttons = g.buttons();
        let value = |i: u32| buttons.get(i).dyn_into::<web_sys::GamepadButton>().map_or(0.0, |b| b.value() as f32);
        let axes = g.axes();
        let axis = |i: u32| axes.get(i).as_f64().unwrap_or(0.0) as f32;
        const BUTTONS: [(u32, u32); 12] = [
            (0, pad::SOUTH), (1, pad::EAST), (2, pad::WEST), (3, pad::NORTH), (4, pad::LB), (5, pad::RB),
            (8, pad::BACK), (9, pad::START), (12, pad::UP), (13, pad::DOWN), (14, pad::LEFT), (15, pad::RIGHT),
        ];
        *slot = Pad {
            connected: true,
            lx: axis(0),
            ly: axis(1),
            lt: value(6),
            rt: value(7),
            buttons: BUTTONS.iter().filter(|(i, _)| value(*i) > 0.5).fold(0, |m, (_, bit)| m | bit),
        };
    }
    pads
}

/// The game has quit: replace it with the text screen the original left
/// in DOS, and a way back in.
fn show_exit_screen(screen: &speedhaste_core::textscr::TextScreen) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    // (Hidden, not removed: they belong to Dioxus.)
    for id in ["screen", "touch"] {
        if let Some(e) = doc.get_element_by_id(id) {
            let _ = e.set_attribute("style", "display: none");
        }
    }
    let Ok(pre) = doc.create_element("pre") else { return };
    pre.set_id("endtext");
    for row in screen.rows() {
        for (text, a) in row {
            let rgb = |i: u8| {
                let [r, g, b] = speedhaste_core::textscr::VGA_RGB[i as usize];
                format!("#{r:02x}{g:02x}{b:02x}")
            };
            if let Ok(span) = doc.create_element("span") {
                let _ = span.set_attribute("style", &format!("color: {}; background: {}", rgb(a & 15), rgb(a >> 4)));
                span.set_text_content(Some(&text));
                let _ = pre.append_child(&span);
            }
        }
        let _ = pre.append_with_str_1("\n");
    }
    let Ok(again) = doc.create_element("a") else { return };
    let _ = again.set_attribute("href", "");
    again.set_text_content(Some("Play again"));
    let Ok(div) = doc.create_element("div") else { return };
    div.set_class_name("exit");
    let _ = div.append_child(&pre);
    let _ = div.append_child(&again);
    if let Some(main) = doc.get_element_by_id("main") {
        let _ = main.append_child(&div);
    }
}

/// The touch overlay's hidden text field: characters typed on the phone's
/// keyboard become key taps (for names in the record tables). It keeps one
/// space in it, so Backspace on an empty name still makes an input event.
fn type_keys(document: &web_sys::Document, game: Rc<RefCell<Game>>) {
    let Some(input) = document.get_element_by_id("kbd").and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
    input.set_value(" ");
    let tap = move |sc: u8| {
        let mut g = game.borrow_mut();
        g.key(sc, true);
        g.key(sc, false);
    };
    let tap2 = tap.clone();
    let field = input.clone();
    let cb = Closure::<dyn FnMut(web_sys::Event)>::new(move |e: web_sys::Event| {
        let Some(e) = e.dyn_ref::<web_sys::InputEvent>() else { return };
        if e.input_type().starts_with("delete") {
            tap(keys::BACKSPACE);
        }
        for c in e.data().unwrap_or_default().chars() {
            let c = c.to_ascii_uppercase();
            if let Some(sc) = (0..=255u8).find(|&sc| speedhaste_core::records::key_char(sc) == Some(c)) {
                tap(sc);
            }
        }
        field.set_value(" ");
    });
    let _ = input.add_event_listener_with_callback("input", cb.as_ref().unchecked_ref());
    cb.forget();
    // Enter: confirm the name and put the keyboard away.
    let cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
        if e.key() == "Enter" {
            tap2(keys::ENTER);
            if let Some(t) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlElement>().ok()) {
                let _ = t.blur();
            }
        }
    });
    let _ = input.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
    cb.forget();
}

/// `?nologo&track=3` → `["-nologo", "-track", "3"]`.
fn url_args() -> Vec<String> {
    let search = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
    let mut v = Vec::new();
    for part in search.trim_start_matches('?').split('&').filter(|p| !p.is_empty()) {
        let (k, val) = part.split_once('=').unwrap_or((part, ""));
        v.push(format!("-{k}"));
        if !val.is_empty() {
            v.push(js_sys::decode_uri_component(val).map_or(val.to_string(), String::from));
        }
    }
    v
}

fn start_game(jcl: Jcl, touch_on: bool) {
    let window = web_sys::window().expect("no window");
    let document = window.document().expect("no document");
    let canvas: web_sys::HtmlCanvasElement = document.get_element_by_id("screen").expect("no canvas").dyn_into().unwrap();
    let _ = canvas.focus();
    let mut args = url_args();
    let flag = |name: &str| args.iter().any(|a| a == name);
    // With ?gpu the canvas belongs to WebGL2 once wgpu is up; until then (or
    // if that fails) nothing / the 2D context draws.
    let gpu: Rc<RefCell<Option<WebGpu>>> = Rc::new(RefCell::new(None));
    let ctx2d = |c: &web_sys::HtmlCanvasElement| c.get_context("2d").ok().flatten().and_then(|c| c.dyn_into::<web_sys::CanvasRenderingContext2d>().ok());
    let ctx: Rc<RefCell<Option<web_sys::CanvasRenderingContext2d>>> = Rc::new(RefCell::new(None));
    if flag("-gpu") {
        let (g, c, cv, filter) = (gpu.clone(), ctx.clone(), canvas.clone(), flag("-filter"));
        wasm_bindgen_futures::spawn_local(async move {
            match WebGpu::new(cv.clone(), filter).await {
                Ok(w) => *g.borrow_mut() = Some(w),
                Err(e) => {
                    web_sys::console::log_1(&format!("GPU renderer unavailable: {e}").into());
                    *c.borrow_mut() = ctx2d(&cv);
                }
            }
        });
    } else {
        *ctx.borrow_mut() = ctx2d(&canvas);
    }

    // Audio: an AudioWorklet fed from the frame loop, else a ScriptProcessorNode.
    let actx = web_sys::AudioContext::new().ok();
    let rate = actx.as_ref().map_or(44100.0, |a| a.sample_rate()) as u32;
    let audio = Arc::new(Mutex::new(Audio::new(rate)));
    let worklet: Rc<RefCell<Option<Worklet>>> = Rc::new(RefCell::new(None));
    if let Some(actx) = actx.clone() {
        let (w, a) = (worklet.clone(), audio.clone());
        wasm_bindgen_futures::spawn_local(async move {
            match JsFuture::from(start_worklet(&actx)).await {
                Ok(node) => *w.borrow_mut() = Some(Worklet { node, written: 0, buf: Vec::new() }),
                Err(e) => {
                    web_sys::console::log_2(&"AudioWorklet unavailable, using ScriptProcessorNode:".into(), &e);
                    script_processor(&actx, a);
                }
            }
        });
    }
    let mixer = audio.clone();

    if !args.iter().any(|a| a == "-seed") {
        args.extend(["-seed".to_string(), (js_sys::Date::now() as u64 as u32).to_string()]);
    }
    let relay = args.iter().position(|a| a == "-relay").and_then(|i| args.get(i + 1)).cloned();
    let data = Data::new(jcl);
    let exit_screen = speedhaste_core::textscr::TextScreen::exit_screen(&data);
    let game = Rc::new(RefCell::new(Game::new(data, audio, Box::new(LocalStorage), args, speedhaste_core::game::run)));
    if let Some(url) = relay {
        game.borrow_mut().set_connector(Box::new(move || match WebPipe::connect(&url) {
            Ok(p) => Box::new(p),
            Err(e) => Box::new(DeadPipe(e)),
        }));
    }

    // Keyboard.
    for (ev, down) in [("keydown", true), ("keyup", false)] {
        let g = game.clone();
        let actx = actx.clone();
        let gpu = gpu.clone();
        let cb = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
            if let Some(a) = &actx
                && a.state() == web_sys::AudioContextState::Suspended
            {
                let _ = a.resume();
            }
            // Typing into the touch overlay's text field (see type_keys).
            if e.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()).is_some_and(|t| t.id() == "kbd") {
                return;
            }
            // Port hotkeys with ?gpu: Ctrl+G race renderer, Ctrl+F filtering.
            if e.ctrl_key()
                && let Some(w) = gpu.borrow_mut().as_mut()
                && matches!(e.code().as_str(), "KeyG" | "KeyF")
            {
                e.prevent_default();
                if down && !e.repeat() {
                    if e.code() == "KeyG" {
                        w.races = !w.races;
                    } else {
                        w.opts.filter = !w.opts.filter;
                    }
                }
                return;
            }
            if let Some(k) = keys::from_code(&e.code()) {
                e.prevent_default();
                if !(down && e.repeat()) {
                    g.borrow_mut().key(k, down);
                }
            }
        });
        let _ = window.add_event_listener_with_callback(ev, cb.as_ref().unchecked_ref());
        cb.forget();
    }

    let touch = touch_on.then(|| {
        let _ = document.body().map(|b| b.class_list().add_1("touch"));
        // Touches unlock the audio as key presses do.
        for ev in ["pointerup", "touchend"] {
            let actx = actx.clone();
            let cb = Closure::<dyn FnMut()>::new(move || {
                if let Some(a) = &actx
                    && a.state() == web_sys::AudioContextState::Suspended
                {
                    let _ = a.resume();
                }
            });
            let _ = window.add_event_listener_with_callback(ev, cb.as_ref().unchecked_ref());
            cb.forget();
        }
        type_keys(&document, game.clone());
        touch::Touch::new()
    });
    let overlay = document.get_element_by_id("touch");
    let mut was_in_race = false;

    // Frame loop.
    let perf = window.performance().expect("no performance");
    let start = perf.now();
    let tick: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let tick2 = tick.clone();
    let mut rgba = Vec::new();
    *tick.borrow_mut() = Some(Closure::new(move || {
        let mut pads = read_pads();
        if let Some(t) = &touch {
            let in_race = game.borrow().in_race();
            if in_race != was_in_race {
                was_in_race = in_race;
                t.clear();
                if let Some(o) = &overlay {
                    let _ = o.set_attribute("data-mode", if in_race { "race" } else { "menu" });
                }
            }
            pads[0] = touch::merge(pads[0], t.read());
            t.update_visuals();
        }
        if let Some(a) = &actx
            && a.state() == web_sys::AudioContextState::Suspended
            && pads.iter().any(|p| p.buttons != 0)
        {
            let _ = a.resume();
        }
        for (i, p) in pads.into_iter().enumerate() {
            game.borrow_mut().pad(i, p);
        }
        if let Some(w) = gpu.borrow().as_ref() {
            game.borrow_mut().set_gpu(w.races);
        }
        let alive = game.borrow_mut().frame(perf.now() - start);
        for i in 0..2 {
            if let Some(r) = game.borrow_mut().take_rumble(i) {
                pad_rumble(i as u32, r.strong, r.weak, r.ms);
            }
        }
        if let Some(w) = worklet.borrow_mut().as_mut() {
            w.pump(&mixer, rate);
        }
        if let Some(w) = gpu.borrow_mut().as_mut() {
            w.present(game.borrow().front().as_deref());
        } else if let Some(ctx) = ctx.borrow().as_ref()
            && let Some(f) = game.borrow().front()
        {
            if canvas.width() != f.w as u32 || canvas.height() != f.h as u32 {
                canvas.set_width(f.w as u32);
                canvas.set_height(f.h as u32);
            }
            let lut = f.pal.to_rgba_table();
            rgba.clear();
            rgba.extend(f.pix.iter().flat_map(|&c| lut[c as usize]));
            if let Ok(img) = web_sys::ImageData::new_with_u8_clamped_array_and_sh(wasm_bindgen::Clamped(&rgba), f.w as u32, f.h as u32) {
                let _ = ctx.put_image_data(&img, 0.0, 0.0);
            }
        }
        if alive {
            if let Some(cb) = tick2.borrow().as_ref() {
                let _ = web_sys::window().unwrap().request_animation_frame(cb.as_ref().unchecked_ref());
            }
        } else {
            show_exit_screen(&exit_screen);
        }
    }));
    let _ = window.request_animation_frame(tick.borrow().as_ref().unwrap().as_ref().unchecked_ref());
}
