//! DOS keyboard scancodes as used by the original (wcgsl/h/llkey.h):
//! set-1 make codes, with E0-prefixed (extended) keys OR'ed with 0x80.
//! SPEED.CFG stores key bindings in this form.

macro_rules! keys {
    ($($name:ident = $code:expr, $web:expr;)*) => {
        $(pub const $name: u8 = $code;)*

        /// Map a W3C `KeyboardEvent.code` string to a scancode.
        pub fn from_code(code: &str) -> Option<u8> {
            match code {
                $($web => Some($code),)*
                _ => None,
            }
        }

        /// Human-readable name of a scancode, as shown in the setup screens.
        pub fn name(sc: u8) -> Option<&'static str> {
            match sc {
                $($code => Some(stringify!($name)),)*
                _ => None,
            }
        }
    };
}

keys! {
    ESC = 0x01, "Escape";
    K1 = 0x02, "Digit1";
    K2 = 0x03, "Digit2";
    K3 = 0x04, "Digit3";
    K4 = 0x05, "Digit4";
    K5 = 0x06, "Digit5";
    K6 = 0x07, "Digit6";
    K7 = 0x08, "Digit7";
    K8 = 0x09, "Digit8";
    K9 = 0x0A, "Digit9";
    K0 = 0x0B, "Digit0";
    MINUS = 0x0C, "Minus";
    EQUAL = 0x0D, "Equal";
    BACKSPACE = 0x0E, "Backspace";
    TAB = 0x0F, "Tab";
    Q = 0x10, "KeyQ";
    W = 0x11, "KeyW";
    E = 0x12, "KeyE";
    R = 0x13, "KeyR";
    T = 0x14, "KeyT";
    Y = 0x15, "KeyY";
    U = 0x16, "KeyU";
    I = 0x17, "KeyI";
    O = 0x18, "KeyO";
    P = 0x19, "KeyP";
    LBRACKET = 0x1A, "BracketLeft";
    RBRACKET = 0x1B, "BracketRight";
    ENTER = 0x1C, "Enter";
    LEFTCTRL = 0x1D, "ControlLeft";
    A = 0x1E, "KeyA";
    S = 0x1F, "KeyS";
    D = 0x20, "KeyD";
    F = 0x21, "KeyF";
    G = 0x22, "KeyG";
    H = 0x23, "KeyH";
    J = 0x24, "KeyJ";
    K = 0x25, "KeyK";
    L = 0x26, "KeyL";
    SEMICOLON = 0x27, "Semicolon";
    TICK = 0x28, "Quote";
    APOSTROPHE = 0x29, "Backquote";
    LEFTSHIFT = 0x2A, "ShiftLeft";
    BACKSLASH = 0x2B, "Backslash";
    Z = 0x2C, "KeyZ";
    X = 0x2D, "KeyX";
    C = 0x2E, "KeyC";
    V = 0x2F, "KeyV";
    B = 0x30, "KeyB";
    N = 0x31, "KeyN";
    M = 0x32, "KeyM";
    COMMA = 0x33, "Comma";
    PERIOD = 0x34, "Period";
    SLASH = 0x35, "Slash";
    RIGHTSHIFT = 0x36, "ShiftRight";
    KEYPADSTAR = 0x37, "NumpadMultiply";
    LEFTALT = 0x38, "AltLeft";
    SPACE = 0x39, "Space";
    CAPSLOCK = 0x3A, "CapsLock";
    F1 = 0x3B, "F1";
    F2 = 0x3C, "F2";
    F3 = 0x3D, "F3";
    F4 = 0x3E, "F4";
    F5 = 0x3F, "F5";
    F6 = 0x40, "F6";
    F7 = 0x41, "F7";
    F8 = 0x42, "F8";
    F9 = 0x43, "F9";
    F10 = 0x44, "F10";
    NUMLOCK = 0x45, "NumLock";
    SCROLLLOCK = 0x46, "ScrollLock";
    KEYPAD7 = 0x47, "Numpad7";
    KEYPAD8 = 0x48, "Numpad8";
    KEYPAD9 = 0x49, "Numpad9";
    KEYPADMINUS = 0x4A, "NumpadSubtract";
    KEYPAD4 = 0x4B, "Numpad4";
    KEYPAD5 = 0x4C, "Numpad5";
    KEYPAD6 = 0x4D, "Numpad6";
    KEYPADPLUS = 0x4E, "NumpadAdd";
    KEYPAD1 = 0x4F, "Numpad1";
    KEYPAD2 = 0x50, "Numpad2";
    KEYPAD3 = 0x51, "Numpad3";
    KEYPAD0 = 0x52, "Numpad0";
    KEYPADDEL = 0x53, "NumpadDecimal";
    SYSREQ = 0x54, "SysRq";
    F11 = 0x57, "F11";
    F12 = 0x58, "F12";
    KEYPADENTER = 0x9C, "NumpadEnter";
    RIGHTCTRL = 0x9D, "ControlRight";
    KEYPADSLASH = 0xB5, "NumpadDivide";
    PRTSC = 0xB7, "PrintScreen";
    RIGHTALT = 0xB8, "AltRight";
    PAUSE = 0xC5, "Pause";
    HOME = 0xC7, "Home";
    UARROW = 0xC8, "ArrowUp";
    PGUP = 0xC9, "PageUp";
    LARROW = 0xCB, "ArrowLeft";
    RARROW = 0xCD, "ArrowRight";
    END = 0xCF, "End";
    DARROW = 0xD0, "ArrowDown";
    PGDN = 0xD1, "PageDown";
    INS = 0xD2, "Insert";
    DEL = 0xD3, "Delete";
}
