//! Dev tool: run the game twice without a window (software and GPU race
//! renderer, same options), saving both frames for comparison.
//!
//! usage: gpu_shot SPEEDH.JCL OUTDIR FRAME[,FRAME...] WxH [game options...]
//! e.g. `gpu_shot SPEEDH.JCL /tmp/g 300,600 1280x800 -track 3 -car 2 -seed 5`
//! writes OUTDIR/soft_N.ppm (the software frame) and OUTDIR/gpu_N.ppm (the
//! scene at WxH) for each frame number N (at 60 fps). Env GPU_FILTER=1
//! turns filtering on, TEXTURES=DIR loads texture replacements; KEYS=N:NAME,... taps key NAME at frame N (names as
//! in keys.rs, e.g. `KEYS=60:ENTER,120:ENTER`).

use std::sync::{Arc, Mutex};

use speedhaste_core::audio::Audio;
use speedhaste_core::data::{Data, Jcl};
use speedhaste_core::keys;
use speedhaste_core::sys::{Game, NoStorage};
use speedhaste_gpu::{Options, Renderer, block_on};

fn save_ppm(path: &str, w: usize, h: usize, rgb: impl Iterator<Item = [u8; 3]>) {
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    out.extend(rgb.flatten());
    std::fs::write(path, out).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).unwrap();
    let out = &args[2];
    let frames: Vec<u32> = args[3].split(',').map(|f| f.parse().unwrap()).collect();
    let (w, h) = args[4].split_once('x').map(|(w, h)| (w.parse::<u32>().unwrap(), h.parse::<u32>().unwrap())).unwrap();
    let opts = Options { filter: std::env::var("GPU_FILTER").is_ok_and(|v| v == "1") };
    std::fs::create_dir_all(out).unwrap();

    let new_game = |gpu: bool| {
        let data = Data::new(Jcl::parse(bytes.clone()).unwrap());
        let audio = Arc::new(Mutex::new(Audio::new(44100)));
        let mut g = Game::new(data, audio.clone(), Box::new(NoStorage), args[5..].to_vec(), speedhaste_core::game::run);
        g.set_gpu(gpu);
        (g, audio)
    };
    // Music fades only advance as audio gets mixed.
    let (mut soft, soft_audio) = new_game(false);
    let (mut gpu, gpu_audio) = new_game(true);
    let mut abuf = vec![0f32; 2 * 735];

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).expect("no GPU adapter");
    eprintln!("adapter: {:?}", adapter.get_info().name);
    let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .unwrap();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut r = Renderer::new(&device, &queue, format);
    if let Ok(dir) = std::env::var("TEXTURES") {
        let repl = speedhaste_gpu::Image::load_dir(std::path::Path::new(&dir)).unwrap();
        eprintln!("{} texture replacements", repl.len());
        r.set_replacements(repl);
    }
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let stride = (w * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (stride * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let script: Vec<(u32, u8)> = std::env::var("KEYS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (n, name) = s.split_once(':').unwrap();
            let k = (0..=255u8).find(|&k| keys::name(k) == Some(name)).unwrap_or_else(|| panic!("unknown key {name}"));
            (n.parse().unwrap(), k)
        })
        .collect();
    let last = *frames.iter().max().unwrap();
    for n in 1..=last {
        let t = n as f64 * 1000.0 / 60.0;
        for g in [&mut soft, &mut gpu] {
            for &(at, k) in &script {
                if at == n {
                    g.key(k, true);
                } else if at + 1 == n {
                    g.key(k, false);
                }
            }
        }
        for a in [&soft_audio, &gpu_audio] {
            a.lock().unwrap().render(&mut abuf);
        }
        soft.frame(t);
        gpu.frame(t);
        if !frames.contains(&n) {
            continue;
        }
        let f = soft.front().unwrap();
        let lut = f.pal.to_rgba_table();
        save_ppm(&format!("{out}/soft_{n}.ppm"), f.w, f.h, f.pix.iter().map(|&c| {
            let [r, g, b, _] = lut[c as usize];
            [r, g, b]
        }));
        drop(f);
        let f = gpu.front().unwrap();
        if f.scene.is_none() {
            eprintln!("frame {n}: no scene (not racing?)");
        }
        let t0 = std::time::Instant::now();
        r.render(&f, &view, (w, h), [0.0, 0.0, w as f32, h as f32], &opts);
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride), rows_per_image: Some(h) },
            },
            target.size(),
        );
        queue.submit([enc.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        eprintln!("frame {n}: rendered in {:?}", t0.elapsed());
        {
            let data = readback.slice(..).get_mapped_range().unwrap();
            let rows = (0..h as usize).flat_map(|y| data[y * stride as usize..][..w as usize * 4].chunks(4).map(|p| [p[0], p[1], p[2]]).collect::<Vec<_>>());
            save_ppm(&format!("{out}/gpu_{n}.ppm"), w as usize, h as usize, rows);
        }
        readback.unmap();
    }
}
