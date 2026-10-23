//! Race map data: floor tiles, backgrounds, map things, start positions,
//! static cameras (game/racemap.c), sector map (game/sectors.c) and the AI
//! path (.PTH).

use crate::config::{clean_line, scan_int, split_line};
use crate::data::{Data, DataError, Result};
use crate::fx::{fp_mult_div, get_angle, pow2};
use crate::is2::Sprite;

/// Sector coordinates from map coordinates and back.
#[inline]
pub fn to_sec(c: u32) -> u32 {
    c.wrapping_sub(0x4000_0000) >> 15
}

#[inline]
pub fn to_map(c: u32) -> u32 {
    (c << 15).wrapping_add(0x4000_0000)
}

#[derive(Clone, Copy, Debug)]
pub struct MapThing {
    pub x: u32,
    pub y: u32,
    pub angle: u16,
    pub ty: u16,
}

#[derive(Clone, Copy, Debug)]
pub struct StartPos {
    pub x: u32,
    pub y: u32,
    pub angle: u16,
    pub used: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct StaticCamera {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PathPoint {
    pub x: u32,
    pub y: u32,
    pub dir: u32,
    pub speed: i32,
    pub ncars: i32,
}

pub struct Floor {
    /// Unique (tile, rotation) graphics, 64x64 each.
    pub grafs: Vec<u8>,
    /// 128x128 cells (y * 128 + x), each an index into `grafs` (in tiles).
    pub map: Vec<u32>,
    /// Per `grafs` tile: the GRAFS.DAT tile | quarter turns << 14.
    pub tiles: Vec<u16>,
}

impl Floor {
    #[inline]
    pub fn texel(&self, x: u32, y: u32) -> u8 {
        let t = self.map[((y >> 25) * 128 + (x >> 25)) as usize] as usize;
        self.grafs[t * 4096 + (((y >> 19) & 63) * 64 + ((x >> 19) & 63)) as usize]
    }
}

pub struct Backgrounds {
    /// Background set number (NUBESn.PIX, MOUNTn.PIX).
    pub n: i32,
    /// Clouds (far layer) and mountains (near layer, colour 0 transparent),
    /// both 320 pixels wide.
    pub sky: Vec<u8>,
    pub sky_h: i32,
    pub mount: Vec<u8>,
    pub mount_h: i32,
}

/// MAP_Load: floor tiles + backgrounds; returns the raw map things too.
pub fn load_map(d: &Data, name: &str, nback: i32) -> Result<(Floor, Backgrounds, Vec<MapThing>)> {
    let sky = d.load(&format!("nubes{nback}.pix"))?.to_vec();
    let mount = d.load(&format!("mount{nback}.pix"))?.to_vec();
    let bg = Backgrounds { n: nback, sky_h: sky.len() as i32 / 320, mount_h: mount.len() as i32 / 320, sky, mount };

    let b = d.load(name)?;
    const HDR: usize = 4 + 64 * 64 * 2 + 64 * 64;
    if b.len() < HDR {
        return Err(DataError::Format(name.into(), "short map"));
    }
    let word = |i: usize| u16::from_le_bytes([b[4 + i * 2], b[5 + i * 2]]);
    let angle = |i: usize| b[4 + 8192 + i] as u16;
    let mut charmap: Vec<u16> = Vec::new();
    let mut cells = vec![0u32; 64 * 64];
    for i in 0..64 * 64 {
        let c = word(i) | (angle(i) << 14);
        let k = charmap.iter().position(|&x| x == c).unwrap_or_else(|| {
            charmap.push(c);
            charmap.len() - 1
        });
        cells[i] = k as u32;
    }
    let src = d.load("grafs.dat")?;
    let mut grafs = vec![0u8; charmap.len() * 4096];
    for (k, &c) in charmap.iter().enumerate() {
        let off = (c & 0x3FFF) as usize * 4096;
        let g = src.get(off..off + 4096).ok_or(DataError::Format("grafs.dat".into(), "tile out of range"))?;
        let p = &mut grafs[k * 4096..(k + 1) * 4096];
        for i in 0..64 {
            for j in 0..64 {
                p[i * 64 + j] = match c >> 14 {
                    0 => g[i * 64 + j],
                    1 => g[(63 - j) * 64 + i],
                    2 => g[(63 - i) * 64 + 63 - j],
                    _ => g[j * 64 + 63 - i],
                };
            }
        }
    }
    // The 64x64 disk map is stored [x][y] and centred in a 128x128 world.
    let mut map = vec![0u32; 128 * 128];
    for i in 32..96 {
        for j in 32..96 {
            map[j * 128 + i] = cells[(i - 32) * 64 + (j - 32)];
        }
    }
    let things = b[HDR..]
        .chunks_exact(8)
        .map(|t| {
            let w = |o: usize| u16::from_le_bytes([t[o], t[o + 1]]);
            MapThing { x: w(0) as u32, y: w(2) as u32, angle: 0x4000u16.wrapping_sub(w(4)), ty: w(6) }
        })
        .collect();
    Ok((Floor { grafs, map, tiles: charmap }, bg, things))
}

/// MAP_FindStaticCamera: the camera nearest to (x, y).
pub fn find_static_camera(cams: &[StaticCamera], x: u32, y: u32) -> Option<StaticCamera> {
    let mut best = None;
    let mut mindist = u32::MAX;
    for c in cams {
        let d = pow2(((c.x >> 20) as i32).wrapping_sub((x >> 20) as i32))
            .wrapping_add(pow2(((c.y >> 20) as i32).wrapping_sub((y >> 20) as i32))) as u32;
        if d < mindist {
            best = Some(*c);
            mindist = d;
        }
    }
    best
}

/// PATH_Init.
pub fn load_path(d: &Data, name: &str) -> Result<Vec<PathPoint>> {
    let b = d.load(name)?;
    if b.len() < 256 || &b[..4] != b"PATH" {
        return Err(DataError::Format(name.into(), "bad path header"));
    }
    let n = i32::from_le_bytes(b[196..200].try_into().unwrap()).max(0) as usize;
    let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    (0..n)
        .map(|i| {
            let o = 256 + i * 16;
            if o + 16 > b.len() {
                return Err(DataError::Format(name.into(), "truncated path"));
            }
            Ok(PathPoint { x: u(o), y: u(o + 4), dir: u(o + 8), speed: u(o + 12) as i32, ncars: 0 })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Sectors

#[derive(Clone, Debug)]
pub struct Side {
    pub v0: usize,
    pub v1: usize,
    /// Wall texture (index into `SectorMap::textures`), if any.
    pub tex: Option<usize>,
    /// Sector on the other side.
    pub other: usize,
    pub angle: u16,
}

#[derive(Clone, Debug)]
pub struct Sector {
    /// Nonzero for drivable road; sector 0 is the default "off road" one.
    pub flags: u32,
    pub sides: Vec<Side>,
}

pub struct SectorMap {
    pub verts: Vec<(u32, u32)>,
    pub secs: Vec<Sector>,
    pub textures: Vec<Sprite>,
    /// Their IS2 names (without extension).
    pub tex_names: Vec<String>,
}

impl SectorMap {
    /// SEC_LoadMap.
    pub fn load(d: &Data, name: &str) -> Result<SectorMap> {
        let bad = |why| DataError::Format(name.into(), why);
        let text = String::from_utf8_lossy(d.load(name)?).into_owned();
        let mut lines = text.split(['\r', '\n']).map(clean_line).filter(|l| !l.is_empty() && !l.starts_with(';'));
        let mut next = || lines.next().ok_or(bad("unexpected end"));
        let hdr = split_line(&next()?, 3);
        let nv = hdr.first().and_then(|s| scan_int(s)).ok_or(bad("header"))? as usize;
        let ns = hdr.get(1).and_then(|s| scan_int(s)).ok_or(bad("header"))? as usize + 1;
        if !next()?.eq_ignore_ascii_case("Vertices") {
            return Err(bad("expected Vertices"));
        }
        let mut verts = Vec::with_capacity(nv);
        for _ in 0..nv {
            let t = split_line(&next()?, 2);
            let x = t.first().and_then(|s| scan_int(s)).unwrap_or(0) as u32;
            let y = t.get(1).and_then(|s| scan_int(s)).unwrap_or(0) as u32;
            verts.push((x, y));
        }
        if !next()?.eq_ignore_ascii_case("Sectors") {
            return Err(bad("expected Sectors"));
        }
        let mut secs = vec![Sector { flags: 0, sides: Vec::new() }];
        let mut textures: Vec<Sprite> = Vec::new();
        let mut tex_names: Vec<String> = Vec::new();
        for _ in 1..ns {
            let t = split_line(&next()?, 2);
            let n = t.first().and_then(|s| scan_int(s)).unwrap_or(0).max(0) as usize;
            let flags = t.get(1).and_then(|s| scan_int(s)).unwrap_or(0) as u32;
            let mut sides = Vec::with_capacity(n);
            for _ in 0..n {
                let t = split_line(&next()?, 4);
                if t.len() != 4 {
                    return Err(bad("side needs 4 tokens"));
                }
                let v0 = scan_int(&t[0]).filter(|&v| v >= 0 && (v as usize) < nv).ok_or(bad("vertex"))? as usize;
                let v1 = scan_int(&t[1]).filter(|&v| v >= 0 && (v as usize) < nv).ok_or(bad("vertex"))? as usize;
                let tex = if t[2].is_empty() {
                    None
                } else {
                    let tname: String = t[2].chars().take(8).collect::<String>().to_ascii_uppercase();
                    Some(match tex_names.iter().position(|n| *n == tname) {
                        Some(i) => i,
                        None => {
                            textures.push(Sprite::load(d, &format!("{tname}.is2"))?);
                            tex_names.push(tname);
                            textures.len() - 1
                        }
                    })
                };
                let other = scan_int(&t[3]).filter(|&k| k >= -1 && (k as i64) < ns as i64 - 1).ok_or(bad("sector"))?;
                let (a, b) = (verts[v0], verts[v1]);
                let angle = get_angle(b.0.wrapping_sub(a.0) as i32, (b.1.wrapping_sub(a.1) as i32).wrapping_neg());
                sides.push(Side { v0, v1, tex, other: (other + 1) as usize, angle });
            }
            secs.push(Sector { flags, sides });
        }
        Ok(SectorMap { verts, secs, textures, tex_names })
    }

    /// SEC_IsInSector (x, y in sector units). Comparisons are unsigned like
    /// the original's uint32 maths.
    pub fn is_in(&self, s: usize, x: u32, y: u32) -> bool {
        let mut hits = 0;
        for side in &self.secs[s].sides {
            let (v0, v1) = (self.verts[side.v0], self.verts[side.v1]);
            if (x >= v0.0 && x < v1.0) || (x < v0.0 && x >= v1.0) {
                if v0.1 < y && v1.1 < y {
                    hits += 1;
                } else if v0.1 < y || v1.1 < y {
                    let dy = if v0.0 < v1.0 {
                        (v0.1 as i32).wrapping_add(fp_mult_div(
                            x.wrapping_sub(v0.0) as i32,
                            v1.1.wrapping_sub(v0.1) as i32,
                            v1.0.wrapping_sub(v0.0) as i32,
                        ))
                    } else {
                        (v1.1 as i32).wrapping_add(fp_mult_div(
                            x.wrapping_sub(v1.0) as i32,
                            v0.1.wrapping_sub(v1.1) as i32,
                            v0.0.wrapping_sub(v1.0) as i32,
                        ))
                    };
                    if (dy as u32) < y {
                        hits += 1;
                    }
                }
            }
        }
        hits & 1 == 1
    }

    /// SEC_FindSector: try `hint` first, then every sector; 0 if none.
    pub fn find(&self, hint: Option<usize>, x: u32, y: u32) -> usize {
        if let Some(s) = hint
            && self.is_in(s, x, y)
        {
            return s;
        }
        (1..self.secs.len()).find(|&s| Some(s) != hint && self.is_in(s, x, y)).unwrap_or(0)
    }
}
