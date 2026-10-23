//! Write every texture the GPU renderer can replace as a PNG named the way
//! replacements are looked up: sprites and walls `NAME.png` (their IS2),
//! mesh materials `I3DNAME_n.png`, floor tiles `GRAFSnnn.png` (unrotated),
//! backgrounds `NUBESn.png` / `MOUNTn.png`. Clear pixels get alpha 0.
//! Edit or upscale them, then point `-textures DIR` at the results.
//!
//! usage: dump_textures SPEEDH.JCL OUTDIR

use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::gfx::Palette;
use speedhaste_core::is2::{Sprite, TRANSPARENT};
use speedhaste_core::mesh::{MF_HOLES, Mesh};

/// Write `pix` (None = clear) as an RGBA PNG.
fn save(path: &str, w: u32, h: u32, pix: impl Iterator<Item = Option<u8>>, pal: &[[u8; 4]; 256]) {
    let rgba: Vec<u8> = pix.flat_map(|c| c.map_or([0; 4], |c| pal[c as usize])).collect();
    let f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut enc = png::Encoder::new(f, w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = Data::new(Jcl::parse(std::fs::read(&args[1]).unwrap()).unwrap());
    let out = &args[2];
    std::fs::create_dir_all(out).unwrap();
    // GamePal: grafs.pal with the VGA BIOS colours where it has placeholders.
    let mut pal = Palette::load(&data, "grafs.pal").unwrap();
    pal.0[..48].copy_from_slice(speedhaste_core::game::VGA_DEFAULT_LO.as_flattened());
    pal.0[720..].copy_from_slice(speedhaste_core::game::VGA_DEFAULT_HI.as_flattened());
    let pal = pal.to_rgba_table();
    let mut n = 0;
    let mut names = data.names();
    names.sort();
    for name in &names {
        let stem = name.rsplit_once('.').map_or(name.as_str(), |s| s.0);
        if name.ends_with(".IS2") {
            let Ok(sp) = Sprite::load(&data, name) else { continue };
            let (w, h) = (sp.w.max(0) as usize, sp.h.max(0) as usize);
            if w == 0 || h == 0 {
                continue;
            }
            let at = |x: usize, y: usize| if sp.horizontal { sp.pix[y * w + x] } else { sp.pix[x * h + y] };
            let pix = (0..w * h).map(|i| at(i % w, i / w)).map(|v| (v != TRANSPARENT).then_some(v as u8));
            save(&format!("{out}/{stem}.png"), w as u32, h as u32, pix, &pal);
            n += 1;
        } else if name.ends_with(".I3D") {
            let Ok(m) = Mesh::load(&data, name) else { continue };
            for (i, mat) in m.materials.iter().enumerate() {
                if let Some(t) = &mat.texture {
                    let holes = mat.flags & MF_HOLES != 0;
                    save(&format!("{out}/{stem}_{i}.png"), 64, 64, t.iter().map(|&c| (!holes || c != 0).then_some(c)), &pal);
                    n += 1;
                }
            }
        } else if name.starts_with("NUBES") || name.starts_with("MOUNT") {
            let b = data.load(name).unwrap();
            let mount = name.starts_with("MOUNT");
            save(&format!("{out}/{stem}.png"), 320, (b.len() / 320) as u32, b.iter().map(|&c| (!mount || c != 0).then_some(c)), &pal);
            n += 1;
        }
    }
    let grafs = data.load("grafs.dat").unwrap();
    for (i, t) in grafs.as_chunks::<4096>().0.iter().enumerate() {
        save(&format!("{out}/GRAFS{i:03}.png"), 64, 64, t.iter().map(|&c| Some(c)), &pal);
        n += 1;
    }
    eprintln!("{n} textures written to {out}");
}
