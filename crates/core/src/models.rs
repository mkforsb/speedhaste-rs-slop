//! Car and circuit catalogues, CARS.LST / CIRCUITS.LST (game/models.c).

use crate::config::{clean_line, read_word, split_line};
use crate::data::Data;

#[derive(Clone, Debug, Default)]
pub struct CarModel {
    pub name: String,
    pub automatic: bool,
    pub level: i32,
    pub n3d: i32,
    pub max_speed: i32,
}

#[derive(Clone, Debug, Default)]
pub struct Circuit {
    pub name: String,
    pub nmap: i32,
    pub level: i32,
    pub length: i32,
    pub best: i32,
    /// Lap record in seconds for the current car type.
    pub record: i32,
    pub nback: i32,
}

/// Parse a `Section ... Key values ... End` list. `section` starts a new
/// entry; `f(entry, key, tokens)` fills it in.
fn parse_list<T: Default>(text: &str, section: &str, min_tokens: usize, mut f: impl FnMut(&mut T, &str, &[String])) -> Vec<T> {
    let mut lines = text.lines();
    let count = lines.next().map(|l| read_word(&clean_line(l))).unwrap_or(0).max(0) as usize;
    let mut v: Vec<T> = Vec::new();
    for line in lines {
        let line = clean_line(line);
        if line.is_empty() || line.starts_with(';') {
            continue;
        }
        let tok = split_line(&line, 10);
        if tok.is_empty() {
            continue;
        }
        if tok[0].eq_ignore_ascii_case("End") {
            break;
        }
        if tok[0].eq_ignore_ascii_case(section) {
            if v.len() >= count {
                break;
            }
            v.push(T::default());
        } else if let Some(e) = v.last_mut()
            && tok.len() >= min_tokens
        {
            f(e, &tok[0].to_ascii_lowercase(), &tok);
        }
    }
    v
}

/// MDL_InitCars: models for `car_type` (each line has one value per type).
pub fn load_cars(d: &Data, car_type: i32) -> Vec<CarModel> {
    let text = String::from_utf8_lossy(d.get("cars.lst").unwrap_or_default());
    let t = 1 + car_type.max(0) as usize;
    parse_list(&text, "Car", 4, |c: &mut CarModel, k, tok| {
        let v = tok.get(t).map(String::as_str).unwrap_or("0");
        match k {
            "name" => c.name = v.chars().take(39).collect(),
            "number" => c.n3d = read_word(v),
            "level" => c.level = read_word(v),
            "automatic" => c.automatic = read_word(v) != 0,
            "maxspeed" => c.max_speed = read_word(v),
            _ => {}
        }
    })
}

/// MDL_InitCircuits.
pub fn load_circuits(d: &Data, car_type: i32) -> Vec<Circuit> {
    let text = String::from_utf8_lossy(d.get("circuits.lst").unwrap_or_default());
    parse_list(&text, "Circuit", 1, |c: &mut Circuit, k, tok| {
        let arg = |i: usize| tok.get(i).map(String::as_str).unwrap_or("0");
        match k {
            "name" => c.name = arg(1).chars().take(39).collect(),
            "number" => c.nmap = read_word(arg(1)),
            "level" => c.level = read_word(arg(1)),
            "length" => c.length = read_word(arg(1)),
            "best" => c.best = read_word(arg(1)),
            "record" => c.record = read_word(arg(1 + car_type.max(0) as usize)),
            "nback" => c.nback = read_word(arg(1)),
            _ => {}
        }
    })
}
