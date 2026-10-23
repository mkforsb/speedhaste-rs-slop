// Speed Haste GPU renderer: palettized textures, shaded through the game's
// colour tables (GRAFS.CLR) via the "lit palette", at any resolution.
// Only textures and uniforms, so it also runs on WebGL2.

struct View {
    // The view on the 320x200 frame: x, y, w, h.
    rect: vec4<f32>,
    // Where the 320x200 frame is on the target: origin x, y, scale x, y.
    map: vec4<f32>,
    // Projection centre cx, cy, focus, camera height.
    proj: vec4<f32>,
    // Camera cos, sin, sky rows, sky tilt.
    dir: vec4<f32>,
    // Camera x, y (map units), cloud and mountain offsets (i32 bits).
    cam: vec4<u32>,
    // Cloud and mountain texture ids, their heights.
    sky: vec4<u32>,
    // Filtering on, overlay width, height, sRGB target.
    misc: vec4<u32>,
}

@group(0) @binding(0) var atlas: texture_2d_array<u32>;
// Per texture id, two texels: x, y, layer, w | h << 16 of the indexed
// texture, then of its replacement (w = 0: none).
@group(0) @binding(1) var texinfo: texture_2d<u32>;
// Rows 0-31: the palette through each colour table row; 32: the palette;
// 33: x < 32 = the translucency factor of each shade level.
@group(0) @binding(2) var litpal: texture_2d<f32>;
// 128x128 floor cells: texture id of the tile.
@group(0) @binding(3) var floormap: texture_2d<u32>;
// The overlay (or a whole software frame): colour, kind.
@group(0) @binding(4) var overlay: texture_2d<u32>;
// Replacement textures (RGBA).
@group(0) @binding(5) var hiatlas: texture_2d_array<f32>;
// Per shade level (x): the colour table row as scale (y = 0) and offset
// (y = 1) of colours, for replacements.
@group(0) @binding(6) var rowfit: texture_2d<f32>;
@group(1) @binding(0) var<uniform> v: View;

const CLEAR: u32 = 0xFFFFu;
const RAW: u32 = 32u;
const NEAR: f32 = 64.0;
const SV_TEXTURE: u32 = 1u;
const SV_HOLES: u32 = 2u;
const SV_CLAMP_V: u32 = 4u;
const SV_CLAMP_U: u32 = 8u;

fn filtering() -> bool {
    return v.misc.x != 0u;
}

fn out_color(c: vec3<f32>) -> vec3<f32> {
    if (v.misc.w != 0u) {
        // The target re-encodes to sRGB: hand it linear values.
        return select(pow((c + 0.055) / 1.055, vec3(2.4)), c / 12.92, c <= vec3(0.04045));
    }
    return c;
}

fn logical(p: vec4<f32>) -> vec2<f32> {
    return (p.xy - v.map.xy) / v.map.zw;
}

struct Tex {
    x: i32,
    y: i32,
    layer: i32,
    w: i32,
    h: i32,
}

fn info(id: u32, k: u32) -> Tex {
    let t = textureLoad(texinfo, vec2<i32>(i32((id & 255u) * 2u + k), i32(id >> 8u)), 0);
    return Tex(i32(t.x), i32(t.y), i32(t.z), i32(t.w & 0xFFFFu), i32(t.w >> 16u));
}

fn tex_info(id: u32) -> Tex {
    return info(id, 0u);
}

// Position (x, y) in `t`, wrapping or clamped (SV_CLAMP_*).
fn wrap(t: Tex, x: i32, y: i32, flags: u32) -> vec2<i32> {
    var wx = ((x % t.w) + t.w) % t.w;
    var wy = ((y % t.h) + t.h) % t.h;
    if ((flags & SV_CLAMP_U) != 0u) {
        wx = clamp(x, 0, t.w - 1);
    }
    if ((flags & SV_CLAMP_V) != 0u) {
        wy = clamp(y, 0, t.h - 1);
    }
    return vec2(t.x + wx, t.y + wy);
}

fn texel(t: Tex, x: i32, y: i32, flags: u32) -> u32 {
    return textureLoad(atlas, wrap(t, x, y, flags), t.layer, 0).r;
}

fn hitexel(t: Tex, x: i32, y: i32, flags: u32) -> vec4<f32> {
    let c = textureLoad(hiatlas, wrap(t, x, y, flags), t.layer, 0);
    return vec4(c.rgb * c.a, c.a);
}

// A true colour at shade `level`, through the rows' affine fits.
fn lit_rgb(level: f32, c: vec3<f32>) -> vec3<f32> {
    if (level >= 32.0) {
        return c;
    }
    let l = clamp(level, 0.0, 31.0);
    let r = i32(l);
    let a = c * textureLoad(rowfit, vec2(r, 0), 0).rgb + textureLoad(rowfit, vec2(r, 1), 0).rgb;
    if (!filtering() || r >= 31) {
        return clamp(a, vec3(0.0), vec3(1.0));
    }
    let b = c * textureLoad(rowfit, vec2(r + 1, 0), 0).rgb + textureLoad(rowfit, vec2(r + 1, 1), 0).rgb;
    return clamp(mix(a, b, fract(l)), vec3(0.0), vec3(1.0));
}

// Premultiplied colour of replacement `h` for texture `t` at `t`'s texel
// coordinates `uv`.
fn sample_hi(h: Tex, t: Tex, uv: vec2<f32>, level: f32, flags: u32) -> vec4<f32> {
    let p = uv * vec2(f32(h.w) / f32(t.w), f32(h.h) / f32(t.h));
    var c: vec4<f32>;
    if (!filtering()) {
        c = hitexel(h, i32(floor(p.x)), i32(floor(p.y)), flags);
    } else {
        let q = p - 0.5;
        let b = vec2<i32>(floor(q));
        let f = fract(q);
        let top = mix(hitexel(h, b.x, b.y, flags), hitexel(h, b.x + 1, b.y, flags), f.x);
        let bot = mix(hitexel(h, b.x, b.y + 1, flags), hitexel(h, b.x + 1, b.y + 1, flags), f.x);
        c = mix(top, bot, f.y);
    }
    if (c.a <= 0.0) {
        return vec4(0.0);
    }
    return vec4(lit_rgb(level, c.rgb / c.a) * c.a, c.a);
}

fn row_color(row: u32, c: u32) -> vec3<f32> {
    return textureLoad(litpal, vec2<i32>(i32(c), i32(row)), 0).rgb;
}

// Colour `c` at shade `level` (truncated like the software, or blended
// between levels when filtering).
fn lit(level: f32, c: u32) -> vec3<f32> {
    if (level >= 32.0) {
        return row_color(RAW, c);
    }
    let l = clamp(level, 0.0, 31.0);
    let r = u32(l);
    if (!filtering() || r >= 31u) {
        return row_color(r, c);
    }
    return mix(row_color(r, c), row_color(r + 1u, c), fract(l));
}

fn opaque(i: u32, flags: u32) -> bool {
    return i != CLEAR && !((flags & SV_HOLES) != 0u && i == 0u);
}

// Premultiplied colour of texture `id` at texel coordinates `uv`.
fn sample(id: u32, uv: vec2<f32>, level: f32, flags: u32) -> vec4<f32> {
    let t = tex_info(id);
    let h = info(id, 1u);
    if (h.w > 0) {
        return sample_hi(h, t, uv, level, flags);
    }
    if (!filtering()) {
        let i = texel(t, i32(floor(uv.x)), i32(floor(uv.y)), flags);
        if (!opaque(i, flags)) {
            return vec4(0.0);
        }
        return vec4(lit(level, i), 1.0);
    }
    let p = uv - 0.5;
    let b = vec2<i32>(floor(p));
    let f = fract(p);
    var acc = vec4(0.0);
    for (var k = 0; k < 4; k++) {
        let d = vec2<i32>(k & 1, k >> 1u);
        let i = texel(t, b.x + d.x, b.y + d.y, flags);
        if (opaque(i, flags)) {
            let w = mix(1.0 - f.x, f.x, f32(d.x)) * mix(1.0 - f.y, f.y, f32(d.y));
            acc += vec4(lit(level, i), 1.0) * w;
        }
    }
    return acc;
}

// ---- Triangles ----------------------------------------------------------

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) level: f32,
    @location(3) depth: f32,
    @location(4) tex: u32,
    @location(5) color: u32,
    @location(6) flags: u32,
}

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) level: f32,
    @location(2) @interpolate(flat, either) tex: u32,
    @location(3) @interpolate(flat, either) color: u32,
    @location(4) @interpolate(flat, either) flags: u32,
}

@vertex
fn vs_tris(i: VIn) -> VOut {
    // (cx + f x / y, cy - f z / y) on the 320x200 frame, which the viewport
    // maps to the target; reversed depth NEAR / depth, where objects use
    // their painter's depth (constant over each face).
    let y = i.pos.y;
    let f = v.proj.z;
    var z = NEAR;
    if (i.depth > 0.0) {
        z = NEAR * y / max(i.depth, NEAR);
    }
    var o: VOut;
    o.pos = vec4(
        (v.proj.x * y + f * i.pos.x) / 160.0 - y,
        y - (v.proj.y * y - f * i.pos.z) / 100.0,
        z,
        y,
    );
    o.uv = i.uv;
    o.level = i.level;
    o.tex = i.tex;
    o.color = i.color;
    o.flags = i.flags;
    return o;
}

@fragment
fn fs_tris(i: VOut) -> @location(0) vec4<f32> {
    if ((i.flags & SV_TEXTURE) == 0u) {
        return vec4(out_color(lit(i.level, i.color)), 1.0);
    }
    let c = sample(i.tex, i.uv, i.level, i.flags);
    if (c.a < 0.5) {
        discard;
    }
    return vec4(out_color(c.rgb / c.a), 1.0);
}

// Translucent faces darken what is behind them (multiplied in).
@fragment
fn fs_trans(i: VOut) -> @location(0) vec4<f32> {
    let k = textureLoad(litpal, vec2<i32>(i32(clamp(i.level, 0.0, 31.0)), 33), 0).rgb;
    return vec4(out_color(k), 1.0);
}

// ---- Sky and floor ------------------------------------------------------

struct FOut {
    @builtin(position) pos: vec4<f32>,
}

// A triangle covering the viewport.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FOut {
    let p = vec2(f32(i32(i & 1u) * 4 - 1), f32(i32(i >> 1u) * 4 - 1));
    return FOut(vec4(p, 0.0, 1.0));
}

fn floor_texel(x: u32, y: u32) -> u32 {
    let tile = textureLoad(floormap, vec2<i32>(i32(x >> 25u), i32(y >> 25u)), 0).r;
    return texel(tex_info(tile), i32((x >> 19u) & 63u), i32((y >> 19u) & 63u), 0u);
}

fn sky(l: vec2<f32>) -> vec3<f32> {
    // Clouds, rows from `tilt` on, under mountains (colour 0 clear).
    let row = l.y - v.rect.y + v.dir.w;
    let sh = f32(v.sky.z);
    let mh = f32(v.sky.w);
    let c = sample(v.sky.x, vec2(l.x - f32(bitcast<i32>(v.cam.z)), row), 32.0, SV_CLAMP_V);
    var m = vec4(0.0);
    let mrow = row - (sh - mh);
    if (mrow >= 0.0 && mrow < mh) {
        m = sample(v.sky.y, vec2(l.x - f32(bitcast<i32>(v.cam.w)), mrow), 32.0, SV_HOLES | SV_CLAMP_V);
    }
    return c.rgb * (1.0 - m.a) + m.rgb;
}

// F3D_Draw3D per pixel: the floor point seen through row `yy` below the
// projection centre, and its fog level.
fn ground(l: vec2<f32>) -> vec3<f32> {
    let yy = l.y - 0.5 - v.proj.y;
    if (yy <= 0.0) {
        return row_color(31u, 0u);
    }
    let h = v.proj.w;
    let f = v.proj.z;
    // rad = h * focus / yy in the original's units (16.16 map/256).
    let rad = 4.0 * h * f / yy;
    let fwd = rad * 256.0;
    let lat = (l.x - 0.5 - v.proj.x) * 1024.0 * h / yy;
    let c = v.dir.x;
    let s = v.dir.y;
    let lim = 2.0e9;
    let ox = clamp(fwd * c - lat * s, -lim, lim);
    let oy = clamp(fwd * s + lat * c, -lim, lim);
    let x = v.cam.x + bitcast<u32>(i32(ox));
    let y = v.cam.y + bitcast<u32>(i32(oy));
    let r = rad / 65536.0;
    var t: f32;
    if (r < 4.0) {
        t = 15.0;
    } else if (r >= 36.0) {
        t = 0.0;
    } else if (filtering()) {
        t = 14.0 - 14.0 * (r - 4.0) / 32.0;
    } else {
        t = 14.0 - floor(14.0 * (r - 4.0) / 32.0);
    }
    let level = 31.0 - t;
    let tile = textureLoad(floormap, vec2<i32>(i32(x >> 25u), i32(y >> 25u)), 0).r;
    let hi = info(tile, 1u);
    if (hi.w > 0) {
        let uv = vec2(f32(x & 0x1FFFFFFu), f32(y & 0x1FFFFFFu)) / 524288.0;
        return sample_hi(hi, tex_info(tile), uv, level, SV_CLAMP_U | SV_CLAMP_V).rgb;
    }
    if (!filtering()) {
        return lit(level, floor_texel(x, y));
    }
    let half = 1u << 18u;
    let px = x - half;
    let py = y - half;
    let fx = f32(px & 0x7FFFFu) / 524288.0;
    let fy = f32(py & 0x7FFFFu) / 524288.0;
    let one = 1u << 19u;
    let a = mix(lit(level, floor_texel(px, py)), lit(level, floor_texel(px + one, py)), fx);
    let b = mix(lit(level, floor_texel(px, py + one)), lit(level, floor_texel(px + one, py + one)), fx);
    return mix(a, b, fy);
}

@fragment
fn fs_ground(i: FOut) -> @location(0) vec4<f32> {
    let l = logical(i.pos);
    if (l.y < v.rect.y + v.dir.z) {
        return vec4(out_color(sky(l)), 1.0);
    }
    return vec4(out_color(ground(l)), 1.0);
}

// ---- Overlay ------------------------------------------------------------

@fragment
fn fs_overlay(i: FOut) -> @location(0) vec4<f32> {
    let l = logical(i.pos);
    let size = vec2<f32>(f32(v.misc.y), f32(v.misc.z));
    let p = clamp(vec2<i32>(floor(l * size / vec2(320.0, 200.0))), vec2(0), vec2<i32>(size) - 1);
    let o = textureLoad(overlay, p, 0);
    if (o.g == 0u) {
        discard;
    }
    if (o.g == 1u) {
        return vec4(out_color(row_color(RAW, o.r)), 1.0);
    }
    // Darkened: premultiplied black.
    let k = out_color(textureLoad(litpal, vec2<i32>(24, 33), 0).rgb).r;
    return vec4(0.0, 0.0, 0.0, 1.0 - k);
}
