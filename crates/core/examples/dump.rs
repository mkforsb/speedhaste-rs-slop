//! Dev tool: decode assets from SPEEDH.JCL into PPM images for inspection.
//! usage: cargo run -p speedhaste-core --example dump -- SPEEDH.JCL OUTDIR

use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::gfx::{Font, Palette, Screen};
use speedhaste_core::is2::{Sprite, TRANSPARENT};
use std::io::Write;

fn write_ppm(path: &str, w: usize, h: usize, pix: &[u8], pal: &Palette) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    write!(f, "P6\n{w} {h}\n255\n").unwrap();
    for &c in &pix[..w * h] {
        f.write_all(&pal.rgba(c)[..3]).unwrap();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = Data::new(Jcl::parse(std::fs::read(&args[1]).unwrap()).unwrap());
    let out = &args[2];
    std::fs::create_dir_all(out).unwrap();
    let game_pal = Palette::load(&data, "grafs.pal").unwrap();
    if args.get(3).map(String::as_str) == Some("meshes") { meshes(&data); return; }
    if args.get(3).map(String::as_str) == Some("songs") { songs(&data, out); return; }

    // Full-screen pictures with their own palettes where they exist.
    for (pix, pal) in [("noria.pix", "noria.pal"), ("sphlogo.pix", "sphlogo.pal"), ("rload00.pix", "rload00.pal"),
                       ("rload10.pix", "rload10.pal"), ("friend.pix", "friend.pal"), ("mbg_prin.pix", "grafs.pal"),
                       ("mbg_car.pix", "grafs.pal"), ("credflag.pix", "grafs.pal"), ("nubes0.pix", "grafs.pal"),
                       ("mount0.pix", "grafs.pal")] {
        let b = data.load(pix).unwrap();
        let p = Palette::load(&data, pal).unwrap();
        write_ppm(&format!("{out}/{pix}.ppm"), 320, b.len() / 320, b, &p);
    }

    // Sprite sheet of all IS2 sprites.
    let mut names: Vec<_> = data.names().into_iter().filter(|n| n.ends_with(".IS2")).collect();
    names.sort();
    let mut s = Screen::new(1024, 1400);
    s.clear(0);
    let (mut x, mut y, mut rowh) = (0, 0, 0);
    let ident: Vec<u8> = (0..=255).collect();
    for n in &names {
        let sp = Sprite::load(&data, n).unwrap();
        if x + sp.w > 1024 { x = 0; y += rowh + 2; rowh = 0; }
        if y + sp.h > 1400 { break; }
        for j in 0..sp.h { for i in 0..sp.w {
            let v = if sp.horizontal { sp.pix[(j * sp.w + i) as usize] } else { sp.pix[(i * sp.h + j) as usize] };
            s.put(x + i, y + j, if v == TRANSPARENT { 0 } else { ident[v as usize] });
        }}
        x += sp.w + 2; rowh = rowh.max(sp.h);
    }
    write_ppm(&format!("{out}/sprites.ppm"), s.w, s.h, &s.pix, &game_pal);

    // Fonts.
    let mut s = Screen::new(320, 200);
    s.clear(0);
    let mut y = 0;
    for f in ["fontbord.fnt", "mfontyel.fnt", "mfontred.fnt", "mfontwhi.fnt", "fontgrt.fnt", "fontcr_1.fnt"] {
        let font = Font::load(&data, f).unwrap();
        font.write(&mut s, 0, y, "Speed Haste 0123 abcXYZ!", 15);
        y += font.height as i32 + 2;
    }
    write_ppm(&format!("{out}/fonts.ppm"), 320, 200, &s.pix, &game_pal);
}


#[allow(dead_code)]
fn meshes(data: &Data) {
    for n in data.names().into_iter().filter(|n| n.ends_with(".I3D")) {
        match speedhaste_core::mesh::Mesh::load(data, &n) {
            Ok(m) => println!("{n}: {} verts, {} faces, {} mats ({} textured), flags {:#x}, sc {:?} dc {:?}",
                m.verts.len(), m.faces.len(), m.materials.len(),
                m.materials.iter().filter(|m| m.texture.is_some()).count(), m.flags, m.sc, m.dc),
            Err(e) => println!("{n}: ERROR {e}"),
        }
    }
}

/// Render the first seconds of each song to WAV for checking.
#[allow(dead_code)]
fn songs(data: &Data, out: &str) {
    for n in data.names().into_iter().filter(|n| n.ends_with(".S3M")) {
        let Some(mut p) = speedhaste_core::s3m::S3mPlayer::new(data.get(&n).unwrap(), 22050) else {
            println!("{n}: failed to load");
            continue;
        };
        let mut buf = vec![0f32; 22050 * 2 * 20];
        p.render(&mut buf, 1.0);
        let peak = buf.iter().fold(0f32, |m, v| m.max(v.abs()));
        let rms = (buf.iter().map(|v| v * v).sum::<f32>() / buf.len() as f32).sqrt();
        println!("{n}: peak {peak:.3} rms {rms:.3}");
        let mut w = Vec::new();
        let pcm: Vec<i16> = buf.iter().map(|v| (v.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + pcm.len() as u32 * 2).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 0x0002_0001, 22050, 22050 * 4, 0x0010_0004] { w.extend_from_slice(&v.to_le_bytes()); }
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(pcm.len() as u32 * 2).to_le_bytes());
        for v in pcm { w.extend_from_slice(&v.to_le_bytes()); }
        std::fs::write(format!("{out}/{n}.wav"), w).unwrap();
    }
}
