//! Game configuration (game/config.c), in the SPEED.CFG text format:
//! one `Key Value` per line, values as decimal, `$hex`, `"strings"` or Yes/No.

use crate::keys;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub music_volume: i32,
    pub music_on: bool,
    pub fx_volume: i32,
    pub fx_on: bool,
    /// 0 = keyboard (plus the same-numbered gamepad unless another player
    /// claims it), 1/2 = joystick A/B, i.e. gamepad 1/2 in this port.
    pub p1_control: u8,
    pub p2_control: u8,
    /// Up, down, left, right, gear — DOS scancodes.
    pub p1_keys: [u8; 5],
    pub p2_keys: [u8; 5],
    pub num_laps: i32,
    /// Songs available for races, as indices into haste1..9.s3m.
    pub songs: Vec<i32>,
    /// 0 = F1-style cars, 1 = stock cars.
    pub car_type: i32,
    pub decoration_detail: i32,
    pub floor_detail: i32,
    pub walls_detail: i32,
    pub polygon_detail: i32,
    pub background_detail: i32,
    pub svga_on: bool,
    pub timed_race: bool,
}

impl Default for Config {
    /// The values of the SPEED.CFG shipped with the game.
    fn default() -> Self {
        Config {
            music_volume: 256,
            music_on: true,
            fx_volume: 256,
            fx_on: true,
            p1_control: 0,
            p2_control: 0,
            p1_keys: [keys::UARROW, keys::DARROW, keys::LARROW, keys::RARROW, keys::SPACE],
            p2_keys: [keys::KEYPAD8, keys::KEYPAD2, keys::KEYPAD4, keys::KEYPAD6, keys::KEYPAD0],
            num_laps: 5,
            songs: (0..8).collect(),
            car_type: 1,
            decoration_detail: 16,
            floor_detail: 1,
            walls_detail: 1,
            polygon_detail: 1,
            background_detail: 10,
            svga_on: false,
            timed_race: false,
        }
    }
}

const KEY_NAMES: [&str; 5] = ["Up", "Dn", "Lt", "Rt", "Ge"];

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        let mut nsongs = c.songs.len();
        let mut mus = c.songs.clone();
        mus.resize(8, 0);
        for line in text.lines() {
            let line = clean_line(line);
            let tok = split_line(&line, 2);
            if tok.len() < 2 {
                continue;
            }
            let (k, v) = (tok[0].to_ascii_lowercase(), tok[1].as_str());
            let n = read_word(v);
            let yes = v.eq_ignore_ascii_case("yes") || n != 0;
            match k.as_str() {
                "musicvolume" => c.music_volume = n,
                "musicon" => c.music_on = yes,
                "fxvolume" => c.fx_volume = n,
                "fxon" => c.fx_on = yes,
                "p1control" => c.p1_control = control(v),
                "p2control" => c.p2_control = control(v),
                "numlaps" => c.num_laps = n,
                "nsongs" => nsongs = n.clamp(0, 8) as usize,
                "cartype" => c.car_type = n,
                "decorationdetail" => c.decoration_detail = n,
                "floordetail" => c.floor_detail = n,
                "wallsdetail" => c.walls_detail = n,
                "polygondetail" => c.polygon_detail = n,
                "backgrounddetail" => c.background_detail = n,
                "svgaon" => c.svga_on = yes,
                "timedrace" => c.timed_race = yes,
                _ => {
                    if let Some(i) = k.strip_prefix("mus").and_then(|i| i.parse::<usize>().ok()) {
                        if (1..=8).contains(&i) {
                            mus[i - 1] = n;
                        }
                    } else {
                        for (p, keys) in [("p1key", &mut c.p1_keys), ("p2key", &mut c.p2_keys)] {
                            if let Some(rest) = k.strip_prefix(p)
                                && let Some(i) = KEY_NAMES.iter().position(|kn| kn.eq_ignore_ascii_case(rest))
                            {
                                keys[i] = n as u8;
                            }
                        }
                    }
                }
            }
        }
        c.songs = mus[..nsongs].to_vec();
        c
    }

    pub fn to_text(&self) -> String {
        let yn = |b: bool| if b { "Yes" } else { "No" };
        let ctl = |c: u8| ["\"Keyboard\"", "\"Joystick A\"", "\"Joystick B\""][c.min(2) as usize];
        let mut s = String::from("; Speed Haste configuration file\n");
        let mut kv = |k: &str, v: String| s.push_str(&format!("{k:<30}{v}\n"));
        kv("MusicVolume", self.music_volume.to_string());
        kv("MusicOn", yn(self.music_on).into());
        kv("FXVolume", self.fx_volume.to_string());
        kv("FXOn", yn(self.fx_on).into());
        kv("P1Control", ctl(self.p1_control).into());
        kv("P2Control", ctl(self.p2_control).into());
        for (p, keys) in [("P1Key", &self.p1_keys), ("P2Key", &self.p2_keys)] {
            for (n, k) in KEY_NAMES.iter().zip(keys) {
                kv(&format!("{p}{n}"), format!("${k:02X}"));
            }
        }
        kv("NumLaps", self.num_laps.to_string());
        kv("NSongs", self.songs.len().to_string());
        for i in 0..8 {
            kv(&format!("Mus{}", i + 1), self.songs.get(i).copied().unwrap_or(i as i32).to_string());
        }
        kv("CarType", self.car_type.to_string());
        kv("DecorationDetail", self.decoration_detail.to_string());
        kv("FloorDetail", self.floor_detail.to_string());
        kv("WallsDetail", self.walls_detail.to_string());
        kv("PolygonDetail", self.polygon_detail.to_string());
        kv("BackgroundDetail", self.background_detail.to_string());
        kv("SVGAOn", (self.svga_on as i32).to_string());
        kv("TimedRace", yn(self.timed_race).into());
        s
    }
}

fn control(v: &str) -> u8 {
    let v = v.to_ascii_lowercase();
    if v.contains("joystick a") || v == "1" {
        1
    } else if v.contains("joystick b") || v == "2" {
        2
    } else {
        0
    }
}

/// STRP_ReadWord: `$hex` or C-style `%i` (decimal, 0x hex, 0 octal).
pub fn read_word(s: &str) -> i32 {
    if let Some(h) = s.strip_prefix('$') {
        return i32::from_str_radix(h.trim(), 16).unwrap_or(0);
    }
    scan_int(s).unwrap_or(0)
}

/// sscanf("%i"): optional sign, then 0x hex / 0 octal / decimal prefix.
pub fn scan_int(s: &str) -> Option<i32> {
    let s = s.trim_start();
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let (radix, digits) = if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, h)
    } else if s.len() > 1 && s.starts_with('0') {
        (8, &s[1..])
    } else {
        (10, s)
    };
    let end = digits.find(|c: char| !c.is_digit(radix)).unwrap_or(digits.len());
    if end == 0 {
        return if radix == 8 { Some(0) } else { None };
    }
    let v = u32::from_str_radix(&digits[..end], radix).ok()? as i32;
    Some(if neg { v.wrapping_neg() } else { v })
}

/// STRP_CleanLine: strip control chars, `/* */` and `//` comments, and
/// surrounding whitespace.
pub fn clean_line(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let (mut i, mut incomment, mut started) = (0, 0, false);
    while i < b.len() {
        let c = b[i];
        if c < 32 || c == 127 {
            i += 1;
        } else if incomment > 0 && c == b'*' && b.get(i + 1) == Some(&b'/') {
            i += 2;
            incomment -= 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            incomment += 1;
        } else if incomment == 0 && c == b'/' && b.get(i + 1) == Some(&b'/') {
            break;
        } else if incomment == 0 && (started || c != b' ') {
            started = true;
            out.push(c);
            i += 1;
        } else {
            i += 1;
        }
    }
    while out.last().is_some_and(|c| c.is_ascii_whitespace()) {
        out.pop();
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// STRP_SplitLine: whitespace-separated tokens, `"`/`'` quoting; at most
/// `max` tokens (the rest of the line is ignored).
pub fn split_line(s: &str, max: usize) -> Vec<String> {
    let b = s.as_bytes();
    let mut v = Vec::new();
    let mut p = 0;
    loop {
        while p < b.len() && (b[p] == b' ' || b[p] == b'\t') {
            p += 1;
        }
        if p >= b.len() {
            return v;
        }
        if b[p] == b'"' || b[p] == b'\'' {
            let del = b[p];
            p += 1;
            let end = b[p..].iter().position(|&c| c == del).map_or(b.len(), |e| p + e);
            v.push(s[p..end].to_string());
            p = end + 1;
        } else {
            let end = b[p..].iter().position(|&c| c == b' ' || c == b'\t').map_or(b.len(), |e| p + e);
            v.push(s[p..end].to_string());
            p = end;
        }
        if v.len() >= max {
            return v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shipped_cfg_values() {
        let c = Config::parse("P1KeyUp   $C8\nNumLaps 5\nCarType 1\nTimedRace No\nNSongs 8\nMus3 2\n");
        assert_eq!(c, Config::default());
        assert_eq!(Config::parse(&Config::default().to_text()), Config::default());
        assert_eq!(scan_int("0x0000E000"), Some(0xE000));
        assert_eq!(scan_int("-1"), Some(-1));
        assert_eq!(split_line(r#"0 1 "6" -1"#, 4), vec!["0", "1", "6", "-1"]);
        assert_eq!(split_line(r#"1 6 "" 1"#, 4), vec!["1", "6", "", "1"]);
    }
}
