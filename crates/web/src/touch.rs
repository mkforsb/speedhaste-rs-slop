//! On-screen touch controls for phones and tablets, acting as a virtual
//! gamepad (merged into gamepad 1, so the core needs nothing new).
//!
//! The overlay (`Overlay`) tags its controls with `data-touch`:
//! * `steer`: a floating stick on the left half of the screen. Steering is
//!   the sideways distance from where the finger first touched.
//! * `pedal`: gas in the top 65%, brake in the bottom 35% (both are on/off
//!   in the game), following the finger: rock the thumb down to brake.
//! * `btn:NAME`: a held gamepad button (`south`, `lb`, `up`...), shown with
//!   `data-on` while pressed. A tap shorter than a frame still counts.
//!
//! One set of window-level pointer listeners tracks every pointer by id, so
//! one thumb can steer while the other accelerates. Pointer events cover
//! mouse too, so `?touch=1` tries the overlay on a desktop.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::PointerEvent;

use speedhaste_core::pad::{self, Pad};

const PREF_KEY: &str = "speedhaste/touch";
/// Finger travel (CSS px) for full steering lock.
const STEER_RANGE: f64 = 70.0;
/// The pedal's share for the gas, from the top; the rest brakes.
const GAS_SHARE: f64 = 0.65;

/// Whether the overlay starts enabled: `?touch=1` / `?touch=0` wins, then
/// the choice saved on the start page, then whether the device's primary
/// pointer is a finger (phones and tablets, not touchscreen laptops).
pub fn initially_enabled() -> bool {
    let w = web_sys::window();
    let search = w.as_ref().and_then(|w| w.location().search().ok()).unwrap_or_default();
    let param = search.trim_start_matches('?').split('&').find_map(|kv| match kv.split_once('=').unwrap_or((kv, "1")) {
        ("touch", v) => Some(v != "0"),
        _ => None,
    });
    let saved = || w.as_ref()?.local_storage().ok()??.get_item(PREF_KEY).ok()?.map(|v| v == "1");
    let coarse = || w.as_ref()?.match_media("(pointer: coarse)").ok()?.map(|m| m.matches());
    param.or_else(saved).or_else(coarse).unwrap_or(false)
}

pub fn save_preference(on: bool) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(PREF_KEY, if on { "1" } else { "0" });
    }
}

fn button_bit(name: &str) -> Option<u32> {
    Some(match name {
        "south" => pad::SOUTH,
        "east" => pad::EAST,
        "west" => pad::WEST,
        "north" => pad::NORTH,
        "lb" => pad::LB,
        "rb" => pad::RB,
        "back" => pad::BACK,
        "start" => pad::START,
        "up" => pad::UP,
        "down" => pad::DOWN,
        "left" => pad::LEFT,
        "right" => pad::RIGHT,
        _ => return None,
    })
}

enum Ctl {
    /// Where the finger came down.
    Steer { x0: f64, y0: f64 },
    /// Where the pedal was when the finger came down.
    Pedal { top: f64, height: f64 },
    Button(u32),
}

struct Pointer {
    id: i32,
    ctl: Ctl,
    el: web_sys::Element,
    x: f64,
    y: f64,
}

impl Pointer {
    /// The gamepad button this pointer holds, if any.
    fn button(&self) -> Option<u32> {
        match self.ctl {
            Ctl::Pedal { top, height } => Some(if self.y < top + height * GAS_SHARE { pad::SOUTH } else { pad::WEST }),
            Ctl::Button(bit) => Some(bit),
            Ctl::Steer { .. } => None,
        }
    }
}

#[derive(Default)]
struct State {
    pointers: Vec<Pointer>,
    /// Buttons pressed since the last read, so short taps aren't lost.
    pulse: u32,
}

pub struct Touch {
    state: Rc<RefCell<State>>,
    _listeners: Vec<Closure<dyn FnMut(PointerEvent)>>,
}

/// The `data-touch` element under the event, and its control name.
fn target(e: &PointerEvent) -> Option<(web_sys::Element, String)> {
    let el = e.target()?.dyn_into::<web_sys::Element>().ok()?.closest("[data-touch]").ok()??;
    let name = el.get_attribute("data-touch")?;
    Some((el, name))
}

impl Touch {
    pub fn new() -> Touch {
        let state = Rc::new(RefCell::new(State::default()));
        let window = web_sys::window().expect("no window");
        let mut listeners = Vec::new();
        let st = state.clone();
        listeners.push(("pointerdown", Closure::<dyn FnMut(PointerEvent)>::new(move |e: PointerEvent| {
            let Some((el, name)) = target(&e) else { return };
            e.prevent_default();
            let (x, y) = (e.client_x() as f64, e.client_y() as f64);
            let ctl = match name.strip_prefix("btn:").map(button_bit) {
                Some(Some(bit)) => Ctl::Button(bit),
                Some(None) => return,
                None if name == "steer" => Ctl::Steer { x0: x, y0: y },
                None if name == "pedal" => {
                    let r = el.get_bounding_client_rect();
                    Ctl::Pedal { top: r.top(), height: r.height() }
                }
                None => return,
            };
            // Keep the moves (and the release) when the finger slides off.
            let _ = el.set_pointer_capture(e.pointer_id());
            if let Ctl::Button(_) = ctl {
                let _ = el.set_attribute("data-on", "1");
            }
            let p = Pointer { id: e.pointer_id(), ctl, el, x, y };
            let mut st = st.borrow_mut();
            st.pulse |= p.button().unwrap_or(0);
            st.pointers.retain(|q| q.id != p.id);
            st.pointers.push(p);
        })));
        let st = state.clone();
        listeners.push(("pointermove", Closure::new(move |e: PointerEvent| {
            if let Some(p) = st.borrow_mut().pointers.iter_mut().find(|p| p.id == e.pointer_id()) {
                (p.x, p.y) = (e.client_x() as f64, e.client_y() as f64);
            }
        })));
        for ev in ["pointerup", "pointercancel", "lostpointercapture"] {
            let st = state.clone();
            listeners.push((ev, Closure::new(move |e: PointerEvent| {
                st.borrow_mut().pointers.retain(|p| {
                    let keep = p.id != e.pointer_id();
                    if !keep {
                        let _ = p.el.remove_attribute("data-on");
                    }
                    keep
                });
            })));
        }
        for (ev, cb) in &listeners {
            let _ = window.add_event_listener_with_callback(ev, cb.as_ref().unchecked_ref());
        }
        Touch { state, _listeners: listeners.into_iter().map(|(_, cb)| cb).collect() }
    }

    /// The virtual pad's state for this frame.
    pub fn read(&self) -> Pad {
        let mut st = self.state.borrow_mut();
        let mut p = Pad { connected: true, buttons: std::mem::take(&mut st.pulse), ..Pad::default() };
        for ptr in &st.pointers {
            if let Ctl::Steer { x0, .. } = ptr.ctl {
                p.lx = ((ptr.x - x0) / STEER_RANGE).clamp(-1.0, 1.0) as f32;
            }
            p.buttons |= ptr.button().unwrap_or(0);
        }
        p
    }

    /// Show the steering stick under the finger (or at rest) and which
    /// part of the pedal is pressed.
    pub fn update_visuals(&self) {
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        let style = |id: &str, css: &str| {
            if let Some(el) = doc.get_element_by_id(id) {
                let _ = el.set_attribute("style", css);
            }
        };
        let st = self.state.borrow();
        match st.pointers.iter().find_map(|p| match p.ctl {
            Ctl::Steer { x0, y0 } => Some((x0, y0, p.x)),
            _ => None,
        }) {
            Some((x0, y0, x)) => {
                let dx = (x - x0).clamp(-STEER_RANGE, STEER_RANGE);
                style("t-stick", &format!("left: {x0}px; top: {y0}px; opacity: 1"));
                style("t-knob", &format!("transform: translate(calc(-50% + {dx}px), -50%)"));
            }
            None => {
                style("t-stick", "");
                style("t-knob", "");
            }
        }
        let pedal = st.pointers.iter().filter(|p| matches!(p.ctl, Ctl::Pedal { .. })).fold(0, |m, p| m | p.button().unwrap_or(0));
        for (id, bit) in [("t-gas", pad::SOUTH), ("t-brake", pad::WEST)] {
            if let Some(el) = doc.get_element_by_id(id) {
                let _ = if pedal & bit != 0 { el.set_attribute("data-on", "1") } else { el.remove_attribute("data-on") };
            }
        }
    }

    /// Release everything (e.g. when the layout switches mid-touch).
    pub fn clear(&self) {
        for p in self.state.borrow_mut().pointers.drain(..) {
            let _ = p.el.remove_attribute("data-on");
        }
    }
}

/// Gamepad 1 with the touch pad's input on top.
pub fn merge(real: Pad, touch: Pad) -> Pad {
    Pad {
        connected: true,
        lx: if touch.lx != 0.0 { touch.lx } else { real.lx },
        ly: real.ly,
        lt: real.lt,
        rt: real.rt,
        buttons: real.buttons | touch.buttons,
    }
}

/// The on-screen controls. `#touch[data-mode]` (set by the frame loop to
/// `race` or `menu`) picks which set shows. They are sized to fit the black
/// bars beside the 4:3 picture on a phone held sideways; held upright, the
/// picture moves to the top and they go below it.
#[component]
pub fn Overlay(on_keyboard: EventHandler<()>, on_fullscreen: EventHandler<()>) -> Element {
    rsx! {
        div { id: "touch", "data-mode": "menu",
            // Race: steer on the left half, gears above it, the pedal right.
            div { class: "t-steer race", "data-touch": "steer",
                div { id: "t-stick", class: "t-stick",
                    div { class: "t-track", span { "◀" } span { "STEER" } span { "▶" } }
                    div { id: "t-knob", class: "t-knob" }
                }
            }
            div { class: "t-gears race",
                div { class: "t-btn", "data-touch": "btn:rb", "▲" }
                span { "GEAR" }
                div { class: "t-btn", "data-touch": "btn:lb", "▼" }
            }
            div { class: "t-pedal race", "data-touch": "pedal",
                div { id: "t-gas", class: "t-gas", span { "GAS" } }
                div { id: "t-brake", class: "t-brake", span { "BRAKE" } }
            }
            div { class: "t-top t-tl race",
                div { class: "t-btn", "data-touch": "btn:start", "❚❚" }
                div { class: "t-btn", "data-touch": "btn:back", "✕" }
            }
            // Menus: d-pad, OK and back.
            div { class: "t-dpad menu",
                div { class: "t-btn t-up", "data-touch": "btn:up", "▲" }
                div { class: "t-btn t-left", "data-touch": "btn:left", "◀" }
                div { class: "t-btn t-right", "data-touch": "btn:right", "▶" }
                div { class: "t-btn t-down", "data-touch": "btn:down", "▼" }
            }
            div { class: "t-ok menu",
                div { class: "t-btn t-back", "data-touch": "btn:east", "BACK" }
                div { class: "t-btn t-enter", "data-touch": "btn:south", "OK" }
            }
            div { class: "t-top t-tr",
                div { class: "t-btn race", "data-touch": "btn:north", "CAM" }
                button { class: "t-btn menu", onclick: move |_| on_keyboard.call(()), "⌨" }
                button { class: "t-btn", onclick: move |_| on_fullscreen.call(()),
                    svg { width: "22", height: "22", view_box: "0 0 24 24",
                        path { d: "M3 9V3h6M15 3h6v6M21 15v6h-6M9 21H3v-6", stroke: "#fff", stroke_width: "2.5", fill: "none" }
                    }
                }
            }
        }
    }
}

pub const STYLE: &str = r#"
#touch { position: fixed; inset: 0; pointer-events: none; touch-action: none; user-select: none;
    -webkit-user-select: none; -webkit-touch-callout: none; font-family: sans-serif; font-weight: bold; color: #fff; }
#touch [data-touch], #touch button { pointer-events: auto; touch-action: none; }
#touch[data-mode=race] .menu, #touch[data-mode=menu] .race { display: none; }
.t-btn { display: flex; align-items: center; justify-content: center; box-sizing: border-box;
    min-width: 48px; height: 48px; padding: 0 8px; border-radius: 12px; font: inherit; font-size: 17px; color: #fff;
    background: rgba(40, 50, 90, .45); border: 2px solid rgba(255, 255, 255, .35); }
.t-btn[data-on] { background: rgba(230, 60, 50, .7); }
.t-top { position: absolute; top: 8px; display: flex; gap: 8px; }
.t-tl { left: 8px; } .t-tr { right: 8px; }
.t-steer { position: absolute; left: 0; top: 64px; bottom: 0; width: 50%; }
.t-stick { position: fixed; left: 96px; top: calc(100% - 44px); opacity: .45; }
.t-track { position: absolute; width: 176px; height: 48px; transform: translate(-50%, -50%); display: flex;
    align-items: center; justify-content: space-between; padding: 0 12px; box-sizing: border-box; font-size: 13px;
    letter-spacing: .2em; border-radius: 24px; background: rgba(40, 50, 90, .45); border: 2px solid rgba(255, 255, 255, .35); }
.t-knob { position: absolute; width: 60px; height: 60px; border-radius: 50%; transform: translate(-50%, -50%);
    background: radial-gradient(circle at 35% 35%, #fff, #bbb); }
.t-gears { position: absolute; left: 8px; top: 64px; display: flex; flex-direction: column; align-items: center;
    gap: 4px; font-size: 11px; }
.t-gears .t-btn { width: 60px; height: 56px; }
.t-pedal { position: absolute; right: 8px; bottom: 8px; width: 120px; height: 70vh; max-height: 360px;
    min-height: 160px; display: flex; flex-direction: column; border-radius: 14px; overflow: hidden;
    border: 2px solid rgba(255, 255, 255, .35); font-size: 13px; letter-spacing: .1em; }
.t-gas, .t-brake { display: flex; align-items: flex-end; justify-content: center; padding-bottom: 10px; box-sizing: border-box; }
.t-gas { flex: 65; background: rgba(30, 120, 50, .45); border-bottom: 2px solid rgba(255, 255, 255, .35); }
.t-brake { flex: 35; background: rgba(120, 40, 30, .45); }
.t-gas[data-on] { background: rgba(40, 200, 70, .7); } .t-brake[data-on] { background: rgba(230, 60, 50, .7); }
.t-dpad { position: absolute; left: 8px; bottom: 8px; display: grid; gap: 4px;
    grid-template: ". u ." 48px "l . r" 48px ". v ." 48px / 48px 48px 48px; }
.t-up { grid-area: u; } .t-left { grid-area: l; } .t-right { grid-area: r; } .t-down { grid-area: v; }
.t-ok { position: absolute; right: 8px; bottom: 8px; display: flex; flex-direction: column; align-items: center; gap: 12px; }
.t-ok .t-btn { width: 72px; height: 72px; border-radius: 50%; }
.t-ok .t-enter { width: 92px; height: 92px; background: rgba(30, 120, 50, .5); }
#kbd { position: fixed; left: 0; top: 0; width: 1px; height: 1px; opacity: 0; border: 0; padding: 0; }
/* Upright: the picture (75vw high) at the top, everything below it. */
@media (orientation: portrait) {
    body.touch #main { align-items: flex-start; }
    .t-top { top: calc(75vw + 10px); }
    .t-gears { top: calc(75vw + 74px); flex-direction: row; }
    .t-steer { top: calc(75vw + 140px); }
    .t-pedal { height: 40vh; }
}
"#;
