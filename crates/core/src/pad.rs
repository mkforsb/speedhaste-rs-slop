//! Modern gamepads (Xbox / DualShock "standard" layout). Not the original's
//! analog joystick model: the aim is a pad that just works. The platforms
//! report raw state; this module turns it into menu keys and race controls.
//!
//! Race: left stick or d-pad steers, RT / A accelerates, LT / X brakes,
//! RB / LB shift up / down, Y cycles the camera, Start pauses, Back quits.
//! Menus: d-pad or stick moves, A / Start confirms, B / Back goes back.

use crate::keys;

pub const SOUTH: u32 = 1 << 0;
pub const EAST: u32 = 1 << 1;
pub const WEST: u32 = 1 << 2;
pub const NORTH: u32 = 1 << 3;
pub const LB: u32 = 1 << 4;
pub const RB: u32 = 1 << 5;
pub const BACK: u32 = 1 << 6;
pub const START: u32 = 1 << 7;
pub const UP: u32 = 1 << 8;
pub const DOWN: u32 = 1 << 9;
pub const LEFT: u32 = 1 << 10;
pub const RIGHT: u32 = 1 << 11;

/// One gamepad's state. Axes are -1..1 (stick y is negative upwards),
/// triggers 0..1.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Pad {
    pub connected: bool,
    pub lx: f32,
    pub ly: f32,
    pub lt: f32,
    pub rt: f32,
    pub buttons: u32,
}

const STICK_DEAD: f32 = 0.15;
const TRIGGER_ON: f32 = 0.3;
const MENU_STICK: f32 = 0.6;

impl Pad {
    fn held(&self, b: u32) -> bool {
        self.buttons & b != 0
    }

    pub fn accelerate(&self) -> bool {
        self.rt > TRIGGER_ON || self.held(SOUTH)
    }

    pub fn brake(&self) -> bool {
        self.lt > TRIGGER_ON || self.held(WEST)
    }

    /// Stick steering in the race's analog range (-16..16). The race
    /// ignores |x| <= 4, so the stick's own dead zone maps to 5 and up.
    pub fn steer(&self) -> i8 {
        let a = self.lx.abs();
        if a < STICK_DEAD {
            return 0;
        }
        let v = 5.0 + ((a - STICK_DEAD) / (1.0 - STICK_DEAD)).min(1.0) * 11.0;
        (v.round() as i8) * self.lx.signum() as i8
    }

    /// Scancodes this pad holds down. In menus it navigates; during a race
    /// only Start (pause) and Back (quit) act as keys, so a pad never
    /// drives a keyboard player's keys.
    pub fn keys(&self, in_race: bool, out: &mut Vec<u8>) {
        if !self.connected {
            return;
        }
        if in_race {
            if self.held(START) {
                out.push(keys::PAUSE);
            }
            if self.held(BACK) {
                out.push(keys::ESC);
            }
            return;
        }
        let dirs = [
            (UP, self.ly < -MENU_STICK, keys::UARROW),
            (DOWN, self.ly > MENU_STICK, keys::DARROW),
            (LEFT, self.lx < -MENU_STICK, keys::LARROW),
            (RIGHT, self.lx > MENU_STICK, keys::RARROW),
        ];
        for (b, stick, k) in dirs {
            if self.held(b) || stick {
                out.push(k);
            }
        }
        if self.held(SOUTH | START) {
            out.push(keys::ENTER);
        }
        if self.held(EAST | BACK) {
            out.push(keys::ESC);
        }
    }
}
