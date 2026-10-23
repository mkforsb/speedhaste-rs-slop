//! Speed Haste — native SDL3 frontend.
//!
//! usage: speedhaste [PATH/TO/SPEEDH.JCL] [original options like -nologo]
//!
//! Without a path it plays ./SPEEDH.JCL, or else the built-in shareware
//! version (circuits 1-2, one car, no championship).
//!
//! Port options: `-scale N` (hi-res races), `-gpu` (present through wgpu
//! and draw races on the GPU; Ctrl+G switches races between the GPU and
//! the software renderer, Ctrl+F toggles texture filtering), `-filter`,
//! `-textures DIR` (with `-gpu`: replacement PNGs, see speedhaste-gpu),
//! `-relay ws://HOST:PORT/ROOM` (network races through speedhaste-relay).

use std::sync::{Arc, Mutex};
use std::time::Instant;

mod net;

use sdl3::audio::{AudioCallback, AudioFormat, AudioSpec, AudioStream};
use sdl3::event::{Event, WindowEvent};
use sdl3::gamepad::{Axis, Button, Gamepad};
use sdl3::keyboard::Scancode;
use sdl3::pixels::PixelFormat;
use sdl3::render::FRect;
use sdl3::sys::pixels::SDL_PixelFormat;
use sdl3::video::Window;

use speedhaste_core::audio::Audio;
use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::keys;
use speedhaste_core::pad::{self, Pad};
use speedhaste_core::sys::{Frame, Game, Storage};

const AUDIO_RATE: u32 = 44100;

/// Config and records live as files in $XDG_DATA_HOME/speedhaste.
struct FileStorage(std::path::PathBuf);

impl FileStorage {
    fn new() -> FileStorage {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".local/share")))
            .unwrap_or_else(|| ".".into());
        FileStorage(base.join("speedhaste"))
    }
}

impl Storage for FileStorage {
    fn load(&self, key: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(key)).ok()
    }
    fn save(&mut self, key: &str, value: &str) {
        let _ = std::fs::create_dir_all(&self.0);
        if let Err(e) = std::fs::write(self.0.join(key), value) {
            eprintln!("can't save {key}: {e}");
        }
    }
}

struct AudioOut(Arc<Mutex<Audio>>, Vec<f32>);

impl AudioCallback<f32> for AudioOut {
    fn callback(&mut self, stream: &mut AudioStream, requested: i32) {
        self.1.resize(requested.max(0) as usize & !1, 0.0);
        if let Ok(mut a) = self.0.lock() {
            a.render(&mut self.1);
        }
        let _ = stream.put_data_f32(&self.1);
    }
}

/// SDL scancode → W3C `KeyboardEvent.code`, so both frontends share
/// `keys::from_code`.
fn web_code(sc: Scancode) -> Option<&'static str> {
    use Scancode::*;
    Some(match sc {
        A => "KeyA", B => "KeyB", C => "KeyC", D => "KeyD", E => "KeyE", F => "KeyF", G => "KeyG",
        H => "KeyH", I => "KeyI", J => "KeyJ", K => "KeyK", L => "KeyL", M => "KeyM", N => "KeyN",
        O => "KeyO", P => "KeyP", Q => "KeyQ", R => "KeyR", S => "KeyS", T => "KeyT", U => "KeyU",
        V => "KeyV", W => "KeyW", X => "KeyX", Y => "KeyY", Z => "KeyZ",
        _1 => "Digit1", _2 => "Digit2", _3 => "Digit3", _4 => "Digit4", _5 => "Digit5",
        _6 => "Digit6", _7 => "Digit7", _8 => "Digit8", _9 => "Digit9", _0 => "Digit0",
        Return => "Enter", Escape => "Escape", Backspace => "Backspace", Tab => "Tab", Space => "Space",
        Minus => "Minus", Equals => "Equal", LeftBracket => "BracketLeft", RightBracket => "BracketRight",
        Backslash => "Backslash", Semicolon => "Semicolon", Apostrophe => "Quote", Grave => "Backquote",
        Comma => "Comma", Period => "Period", Slash => "Slash", CapsLock => "CapsLock",
        F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6", F7 => "F7", F8 => "F8",
        F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12",
        PrintScreen => "PrintScreen", ScrollLock => "ScrollLock", Pause => "Pause", Insert => "Insert",
        Home => "Home", PageUp => "PageUp", Delete => "Delete", End => "End", PageDown => "PageDown",
        Right => "ArrowRight", Left => "ArrowLeft", Down => "ArrowDown", Up => "ArrowUp",
        NumLockClear => "NumLock", KpDivide => "NumpadDivide", KpMultiply => "NumpadMultiply",
        KpMinus => "NumpadSubtract", KpPlus => "NumpadAdd", KpEnter => "NumpadEnter",
        Kp1 => "Numpad1", Kp2 => "Numpad2", Kp3 => "Numpad3", Kp4 => "Numpad4", Kp5 => "Numpad5",
        Kp6 => "Numpad6", Kp7 => "Numpad7", Kp8 => "Numpad8", Kp9 => "Numpad9", Kp0 => "Numpad0",
        KpPeriod => "NumpadDecimal", LCtrl => "ControlLeft", LShift => "ShiftLeft", LAlt => "AltLeft",
        RCtrl => "ControlRight", RShift => "ShiftRight", RAlt => "AltRight",
        _ => return None,
    })
}

/// SDL gamepad → the core's pad state.
fn pad_state(g: &Gamepad) -> Pad {
    const BUTTONS: [(Button, u32); 12] = [
        (Button::South, pad::SOUTH), (Button::East, pad::EAST), (Button::West, pad::WEST), (Button::North, pad::NORTH),
        (Button::LeftShoulder, pad::LB), (Button::RightShoulder, pad::RB), (Button::Back, pad::BACK), (Button::Start, pad::START),
        (Button::DPadUp, pad::UP), (Button::DPadDown, pad::DOWN), (Button::DPadLeft, pad::LEFT), (Button::DPadRight, pad::RIGHT),
    ];
    let axis = |a| g.axis(a) as f32 / 32767.0;
    Pad {
        connected: true,
        lx: axis(Axis::LeftX),
        ly: axis(Axis::LeftY),
        lt: axis(Axis::TriggerLeft),
        rt: axis(Axis::TriggerRight),
        buttons: BUTTONS.iter().filter(|(b, _)| g.button(*b)).fold(0, |m, (_, bit)| m | bit),
    }
}

/// The window system connection SDL opened, for wgpu's instance.
#[derive(Debug)]
struct Display(raw_window_handle::RawDisplayHandle);

// SAFETY: a connection handle (X11 Display*, wl_display*...) that stays
// valid while SDL's video subsystem lives, i.e. for the whole program.
unsafe impl Send for Display {}
unsafe impl Sync for Display {}

impl raw_window_handle::HasDisplayHandle for Display {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        // SAFETY: see above.
        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(self.0) })
    }
}

/// Presentation through wgpu (`-gpu`).
struct GpuOut {
    // Dropped before the window it draws to.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: speedhaste_gpu::Renderer,
    opts: speedhaste_gpu::Options,
    window: Window,
}

impl GpuOut {
    fn new(window: Window, filter: bool) -> Result<GpuOut, Box<dyn std::error::Error>> {
        use speedhaste_gpu::block_on;
        use raw_window_handle::HasDisplayHandle;
        let display = Display(window.display_handle()?.as_raw());
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(display)));
        // SAFETY: the window outlives the surface (see the field order).
        let surface = unsafe { instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&window)?)? };
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let info = adapter.get_info();
        eprintln!("gpu: {} ({:?})", info.name, info.backend);
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))?;
        let (w, h) = window.size_in_pixels();
        let mut config = surface.get_default_config(&adapter, w.max(1), h.max(1)).ok_or("surface not supported")?;
        let caps = surface.get_capabilities(&adapter);
        // Prefer a non-sRGB format: the palette is already in display space.
        if let Some(&f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        let renderer = speedhaste_gpu::Renderer::new(&device, &queue, config.format);
        Ok(GpuOut { surface, device, queue, config, renderer, opts: speedhaste_gpu::Options { filter }, window })
    }

    fn present(&mut self, frame: Option<&Frame>) {
        let (w, h) = self.window.size_in_pixels();
        if w == 0 || h == 0 {
            return;
        }
        if (w, h) != (self.config.width, self.config.height) {
            (self.config.width, self.config.height) = (w, h);
            self.surface.configure(&self.device, &self.config);
        }
        let tex = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        if let Some(f) = frame {
            let view = tex.texture.create_view(&Default::default());
            self.renderer.render(f, &view, (w, h), speedhaste_gpu::fit_4_3(w, h), &self.opts);
        }
        self.queue.present(tex);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // The first argument that is neither an option nor an option's value.
    let takes_value = ["-scale", "-textures", "-track", "-car", "-seed", "-relay"];
    let path_at = (0..args.len()).find(|&i| !args[i].starts_with('-') && (i == 0 || !takes_value.contains(&args[i - 1].as_str())));
    // Without a path: ./SPEEDH.JCL if there is one, else the built-in
    // shareware data.
    let jcl_path = match path_at {
        Some(i) => Some(args.remove(i)),
        None => std::path::Path::new("SPEEDH.JCL").exists().then(|| "SPEEDH.JCL".to_string()),
    };
    let jcl = match &jcl_path {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| format!("can't read {path}: {e}\nusage: speedhaste [PATH/TO/SPEEDH.JCL] [options]"))?;
            Jcl::parse(bytes).map_err(|e| format!("{path}: {e}"))?
        }
        None => {
            eprintln!("No SPEEDH.JCL given: playing the built-in shareware version.\nusage: speedhaste [PATH/TO/SPEEDH.JCL] [options]");
            Jcl::shareware()
        }
    };
    let data = Data::new(jcl);
    let exit_screen = speedhaste_core::textscr::TextScreen::exit_screen(&data);

    let sdl = sdl3::init()?;
    let video = sdl.video()?;
    let flag = |name: &str| args.iter().any(|a| a == name);
    let mut window = video.window("Speed Haste", 960, 720);
    window.position_centered().resizable();
    let (mut gpu, mut canvas) = if flag("-gpu") {
        let mut g = GpuOut::new(window.high_pixel_density().build()?, flag("-filter"))?;
        if let Some(dir) = args.iter().position(|a| a == "-textures").and_then(|i| args.get(i + 1)) {
            let repl = speedhaste_gpu::Image::load_dir(std::path::Path::new(dir)).map_err(|e| format!("{dir}: {e}"))?;
            eprintln!("{} texture replacements from {dir}", repl.len());
            g.renderer.set_replacements(repl);
        }
        (Some(g), None)
    } else {
        let canvas = window.build()?.into_canvas();
        unsafe {
            sdl3::sys::render::SDL_SetRenderVSync(canvas.raw(), 1);
        }
        (None, Some(canvas))
    };
    let tc = canvas.as_ref().map(|c| c.texture_creator());
    let rgba = unsafe { PixelFormat::from_ll(SDL_PixelFormat::RGBA32) };

    let audio = Arc::new(Mutex::new(Audio::new(AUDIO_RATE)));
    let audio_sys = sdl.audio()?;
    let spec = AudioSpec { freq: Some(AUDIO_RATE as i32), channels: Some(2), format: Some(AudioFormat::f32_sys()) };
    let stream = audio_sys.open_playback_stream(&spec, AudioOut(audio.clone(), Vec::new()));
    match &stream {
        Ok(s) => s.resume()?,
        Err(e) => eprintln!("audio disabled: {e}"),
    }

    if !args.iter().any(|a| a == "-seed") {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u32);
        args.extend(["-seed".to_string(), t.to_string()]);
    }
    let relay = args.iter().position(|a| a == "-relay").and_then(|i| args.get(i + 1)).cloned();
    let mut game = Game::new(data, audio, Box::new(FileStorage::new()), args, speedhaste_core::game::run);
    if let Some(url) = relay {
        game.set_connector(Box::new(move || Box::new(net::connect(url.clone()))));
    }
    // SPEEDHASTE_NET_DEBUG=1: report the network desync checks.
    let net_debug = std::env::var_os("SPEEDHASTE_NET_DEBUG").is_some();
    let mut last_checks = (0, 0);
    let mut gpu_races = gpu.is_some();
    game.set_gpu(gpu_races);
    let mut texture = None;
    let mut tex_size = (0, 0);
    let mut rgba_buf = Vec::new();
    let start = Instant::now();
    let gamepads = sdl.gamepad()?;
    // Open pads in connection order: the first two are gamepads 1 and 2.
    let mut pads: Vec<Gamepad> = Vec::new();
    let mut events = sdl.event_pump()?;
    'main: loop {
        for ev in events.poll_iter() {
            match ev {
                Event::Quit { .. } => break 'main,
                Event::KeyDown { scancode: Some(sc), keymod, .. }
                    if gpu.is_some() && keymod.intersects(sdl3::keyboard::Mod::LCTRLMOD | sdl3::keyboard::Mod::RCTRLMOD)
                        && matches!(sc, Scancode::G | Scancode::F) =>
                {
                    // Port hotkeys, not passed to the game.
                    let g = gpu.as_mut().unwrap();
                    if sc == Scancode::G {
                        gpu_races = !gpu_races;
                        game.set_gpu(gpu_races);
                        eprintln!("races: {} renderer", if gpu_races { "GPU" } else { "software" });
                    } else {
                        g.opts.filter = !g.opts.filter;
                        eprintln!("filtering {}", if g.opts.filter { "on" } else { "off" });
                    }
                }
                Event::KeyDown { scancode: Some(sc), repeat: false, .. } => {
                    if let Some(k) = web_code(sc).and_then(keys::from_code) {
                        game.key(k, true);
                    }
                }
                Event::KeyUp { scancode: Some(sc), .. } => {
                    if let Some(k) = web_code(sc).and_then(keys::from_code) {
                        game.key(k, false);
                    }
                }
                Event::GamepadAdded { which, .. } => match gamepads.open(which) {
                    Ok(g) => pads.push(g),
                    Err(e) => eprintln!("can't open gamepad: {e}"),
                },
                Event::GamepadRemoved { which, .. } => pads.retain(|g| g.id().is_ok_and(|id| id != which)),
                Event::Window { win_event: WindowEvent::FocusLost, .. } => {
                    // Avoid stuck keys when alt-tabbing.
                    for k in 0..=255u8 {
                        game.key(k, false);
                    }
                }
                _ => {}
            }
        }
        for i in 0..2 {
            game.pad(i, pads.get(i).map(pad_state).unwrap_or_default());
        }
        if !game.frame(start.elapsed().as_secs_f64() * 1000.0) {
            break;
        }
        if net_debug && game.net_checks() != last_checks {
            last_checks = game.net_checks();
            if last_checks.0 % 20 == 1 || last_checks.1 > 0 {
                eprintln!("network desync checks: {} done, {} failed", last_checks.0, last_checks.1);
            }
        }
        for (i, g) in pads.iter_mut().take(2).enumerate() {
            if let Some(r) = game.take_rumble(i) {
                // Low-frequency (strong) and high-frequency (weak) motors.
                let _ = g.set_rumble((r.strong * 65535.0) as u16, (r.weak * 65535.0) as u16, r.ms);
            }
        }
        if let Some(g) = gpu.as_mut() {
            g.present(game.front().as_deref());
            continue;
        }
        let (Some(canvas), Some(tc)) = (canvas.as_mut(), tc.as_ref()) else { break };
        if let Some(f) = game.front() {
            if texture.is_none() || tex_size != (f.w, f.h) {
                let mut t = tc.create_texture_streaming(rgba, f.w as u32, f.h as u32)?;
                t.set_scale_mode(sdl3::render::ScaleMode::Nearest);
                texture = Some(t);
                tex_size = (f.w, f.h);
            }
            let lut = f.pal.to_rgba_table();
            rgba_buf.clear();
            rgba_buf.extend(f.pix.iter().flat_map(|&c| lut[c as usize]));
            texture.as_mut().unwrap().update(None, &rgba_buf, f.w * 4)?;
        }
        canvas.set_draw_color(sdl3::pixels::Color::BLACK);
        canvas.clear();
        if let Some(t) = &texture {
            // Show at 4:3 like a CRT, letterboxed.
            let (ww, wh) = canvas.output_size()?;
            let (ww, wh) = (ww as f32, wh as f32);
            let (w, h) = if ww * 3.0 > wh * 4.0 { (wh * 4.0 / 3.0, wh) } else { (ww, ww * 3.0 / 4.0) };
            canvas.copy(t, None, Some(FRect::new((ww - w) / 2.0, (wh - h) / 2.0, w, h)))?;
        }
        canvas.present();
    }
    // As the original left it in the DOS text screen.
    use std::io::IsTerminal;
    print!("{}", exit_screen.to_terminal(std::io::stdout().is_terminal()));
    Ok(())
}
