//! wgpu renderer for the Speed Haste port: draws race [`Scene`]s at any
//! resolution (optionally with filtering) and presents plain software
//! frames (menus, or races drawn by the software renderer).
//!
//! Textures stay palettized on the GPU (R16Uint, [`TEXEL_CLEAR`] = hole);
//! shading goes through a "lit palette": the current palette seen through
//! each row of the game's colour table, rebuilt every frame so palette
//! fades work as on the VGA. Only textures and uniform buffers are used, so
//! the WebGL2 backend can run it as well.
//!
//! Texture replacements: true-colour images named after the game's data
//! (see [`SceneTexture::name`]; floor tiles `GRAFSnnn`, rotated as the map
//! needs; backgrounds `NUBESn` / `MOUNTn`), any size, alpha for holes.
//! They are shaded by an affine colour fit of each colour table row.

use std::collections::HashMap;
use std::sync::Arc;

use speedhaste_core::race::{OV_OPAQUE, Scene, SceneStatic, SceneTexture, SceneVertex, TEXEL_CLEAR};
use speedhaste_core::sys::Frame;

const _: () = assert!(std::mem::size_of::<SceneVertex>() == 40);
/// Uniform slots are this far apart (the minimum dynamic offset alignment).
const SLOT: u64 = 256;
const VIEW_SIZE: u64 = 7 * 16;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// Bilinear texture filtering and smooth shading/fog.
    pub filter: bool,
}

/// Run a wgpu future to completion (on native backends they are ready
/// at once or after a short wait; not for the web).
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// Where a 320x200 frame goes on a `w` x `h` target: 4:3 like a CRT,
/// letterboxed (x, y, w, h).
pub fn fit_4_3(w: u32, h: u32) -> [f32; 4] {
    let (w, h) = (w as f32, h as f32);
    let (dw, dh) = if w * 3.0 > h * 4.0 { (h * 4.0 / 3.0, h) } else { (w, w * 3.0 / 4.0) };
    [((w - dw) / 2.0).floor(), ((h - dh) / 2.0).floor(), dw.round(), dh.round()]
}

/// A true-colour image (RGBA8, row-major).
#[derive(Clone)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    /// Decode a PNG of any colour type into RGBA8.
    pub fn from_png(bytes: &[u8]) -> Option<Image> {
        let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
        dec.set_transformations(png::Transformations::normalize_to_color8() | png::Transformations::ALPHA);
        let mut r = dec.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()?];
        let info = r.next_frame(&mut buf).ok()?;
        let n = (info.width * info.height) as usize;
        let rgba = match info.color_type {
            png::ColorType::Rgba => buf[..n * 4].to_vec(),
            png::ColorType::GrayscaleAlpha => buf[..n * 2].chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            _ => return None,
        };
        Some(Image { w: info.width, h: info.height, rgba })
    }

    /// Every `NAME.png` in `dir`, keyed by NAME in uppercase (files that
    /// don't decode are reported and skipped).
    pub fn load_dir(dir: &std::path::Path) -> std::io::Result<HashMap<String, Image>> {
        let mut out = HashMap::new();
        for e in std::fs::read_dir(dir)? {
            let path = e?.path();
            if !path.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")) {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            match std::fs::read(&path).ok().and_then(|b| Image::from_png(&b)) {
                Some(img) => {
                    out.insert(name.to_ascii_uppercase(), img);
                }
                None => eprintln!("can't decode {}", path.display()),
            }
        }
        Ok(out)
    }

    /// Quarter turns like the floor tiles' (square images only).
    fn turned(&self, q: u16) -> Image {
        if q == 0 || self.w != self.h {
            return self.clone();
        }
        let n = self.w as usize;
        let mut rgba = vec![0; n * n * 4];
        for i in 0..n {
            for j in 0..n {
                let (si, sj) = match q {
                    1 => (n - 1 - j, i),
                    2 => (n - 1 - i, n - 1 - j),
                    _ => (j, n - 1 - i),
                };
                rgba[(i * n + j) * 4..][..4].copy_from_slice(&self.rgba[(si * n + sj) * 4..][..4]);
            }
        }
        Image { w: self.w, h: self.h, rgba }
    }
}

/// Shelf-packed texture atlas, layers of `size` x `size`.
struct Atlas<T> {
    size: u32,
    clear: T,
    layers: Vec<Vec<T>>,
    /// Current shelf: x, y, height.
    shelf: (u32, u32, u32),
}

impl<T: Copy> Atlas<T> {
    fn new(size: u32, clear: T) -> Atlas<T> {
        Atlas { size, clear, layers: Vec::new(), shelf: (0, size, 0) }
    }

    /// Add a `w` x `h` image; returns x, y, layer, w | h << 16.
    fn add(&mut self, w: u32, h: u32, pix: &[T]) -> [u32; 4] {
        let clear = [self.clear];
        let (w, h, pix) = if w == 0 || h == 0 || w > self.size || h > self.size { (1, 1, &clear[..]) } else { (w, h, pix) };
        let (mut x, mut y, mut sh) = self.shelf;
        if x + w > self.size {
            (x, y, sh) = (0, y + sh, 0);
        }
        if y + h > self.size {
            (x, y, sh) = (0, 0, 0);
            self.layers.push(vec![self.clear; (self.size * self.size) as usize]);
        }
        let layer = self.layers.len() - 1;
        let dst = &mut self.layers[layer];
        for r in 0..h {
            let d = ((y + r) * self.size + x) as usize;
            dst[d..d + w as usize].copy_from_slice(&pix[(r * w) as usize..][..w as usize]);
        }
        self.shelf = (x + w, y, sh.max(h));
        [x, y, layer as u32, w | h << 16]
    }

    /// Upload as a texture array (2+ layers: GL backends make 1-layer
    /// textures plain 2D ones).
    fn upload(mut self, device: &wgpu::Device, queue: &wgpu::Queue, label: &str, format: wgpu::TextureFormat, bpp: u32) -> wgpu::Texture {
        let size = if self.layers.is_empty() { 1 } else { self.size };
        while self.layers.len() < 2 {
            self.layers.push(vec![self.clear; (size * size) as usize]);
        }
        let tex = texture(device, label, size, size, self.layers.len() as u32, format);
        for (i, l) in self.layers.iter().enumerate() {
            write(queue, &tex, i as u32, &bytes(l), bpp);
        }
        tex
    }
}

/// GPU copies of a race's [`SceneStatic`].
struct Static {
    src: Arc<SceneStatic>,
    atlas: wgpu::Texture,
    /// Texture replacements (RGBA).
    hiatlas: wgpu::Texture,
    texinfo: wgpu::Texture,
    floormap: wgpu::Texture,
    sky: u32,
    mount: u32,
}

fn texture(device: &wgpu::Device, label: &str, w: u32, h: u32, layers: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: layers },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write(queue: &wgpu::Queue, t: &wgpu::Texture, layer: u32, data: &[u8], bytes_per_texel: u32) {
    let size = t.size();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size.width * bytes_per_texel), rows_per_image: Some(size.height) },
        wgpu::Extent3d { width: size.width, height: size.height, depth_or_array_layers: 1 },
    );
}

fn bytes<T: Copy>(v: &[T]) -> Vec<u8> {
    // Only used for plain integer/float data without padding.
    let n = std::mem::size_of_val(v);
    // SAFETY: T is plain old data (u16, u32, [u32; 4], SceneVertex: repr(C),
    // 4-byte fields only), so every byte is initialized.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, n) }.to_vec()
}

impl Static {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, src: Arc<SceneStatic>, repl: &HashMap<String, Image>) -> Static {
        let size = device.limits().max_texture_dimension_2d.min(2048);
        let as16 = |p: &[u8]| p.iter().map(|&c| c as u16).collect::<Vec<u16>>();
        // Every texture, in id order: the scene's, clouds, mountains, floor tiles.
        let mut all: Vec<SceneTexture> = Vec::new();
        let mut names: Vec<(String, u16)> = Vec::new();
        for t in &src.textures {
            names.push((t.name.clone(), 0));
        }
        let mut extra = |name: String, turns: u16, w: u32, h: u32, pix: Vec<u16>| {
            names.push((name.clone(), turns));
            all.push(SceneTexture { name, w, h, pix });
        };
        extra(format!("NUBES{}", src.sky_set), 0, 320, src.sky_h as u32, as16(&src.sky));
        extra(format!("MOUNT{}", src.sky_set), 0, 320, src.mount_h as u32, as16(&src.mount));
        for (t, &id) in src.floor_tiles.as_chunks::<4096>().0.iter().zip(&src.floor_ids) {
            extra(format!("GRAFS{:03}", id & 0x3FFF), id >> 14, 64, 64, as16(t));
        }
        let n = src.textures.len();
        let (sky, mount) = (n as u32, n as u32 + 1);
        let textures = src.textures.iter().chain(&all);

        let mut atlas = Atlas::new(size, TEXEL_CLEAR);
        let mut hi = Atlas::new(size, [0u8; 4]);
        let mut info = Vec::new();
        for (t, (name, turns)) in textures.zip(&names) {
            info.push(atlas.add(t.w, t.h, &t.pix));
            info.push(match repl.get(name.as_str()) {
                Some(img) => {
                    let img = img.turned(*turns);
                    let px: Vec<[u8; 4]> = img.rgba.as_chunks::<4>().0.to_vec();
                    hi.add(img.w, img.h, &px)
                }
                None => [0; 4],
            });
        }
        let ntex = info.len() / 2;
        let tiles: Vec<u32> = (n as u32 + 2..ntex as u32).collect();
        let tex = atlas.upload(device, queue, "atlas", wgpu::TextureFormat::R16Uint, 2);
        let hiatlas = hi.upload(device, queue, "replacements", wgpu::TextureFormat::Rgba8Unorm, 4);
        // Two texels per id: the indexed texture, the replacement (w = 0: none).
        let rows = (ntex as u32).div_ceil(256);
        info.resize((rows * 512) as usize, [0; 4]);
        let texinfo = texture(device, "texinfo", 512, rows, 1, wgpu::TextureFormat::Rgba32Uint);
        write(queue, &texinfo, 0, &bytes(&info), 16);
        let cells: Vec<u32> = src.floor_map.iter().map(|&t| tiles.get(t as usize).copied().unwrap_or(0)).collect();
        let floormap = texture(device, "floormap", 128, 128, 1, wgpu::TextureFormat::R32Uint);
        write(queue, &floormap, 0, &bytes(&cells), 4);
        Static { src, atlas: tex, hiatlas, texinfo, floormap, sky, mount }
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    srgb: bool,
    bgl: wgpu::BindGroupLayout,
    ground: wgpu::RenderPipeline,
    tris: wgpu::RenderPipeline,
    trans: wgpu::RenderPipeline,
    overlay_pipe: wgpu::RenderPipeline,
    stat: Option<Static>,
    /// Placeholder textures until a race comes.
    empty: Static,
    replacements: HashMap<String, Image>,
    clr: Vec<u8>,
    litpal: wgpu::Texture,
    /// Per shade level, the colour table row as an affine map of colours:
    /// row 0 = scale, row 1 = offset (for replacement textures).
    rowfit: wgpu::Texture,
    overlay: wgpu::Texture,
    uniforms: wgpu::Buffer,
    view_bg: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    depth: Option<wgpu::TextureView>,
    bind: Option<wgpu::BindGroup>,
}

impl Renderer {
    /// A renderer drawing to targets of `format`.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Renderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("speedhaste"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let tex_entry = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
            count: None,
        };
        let uint = wgpu::TextureSampleType::Uint;
        let d2 = wgpu::TextureViewDimension::D2;
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textures"),
            entries: &[
                tex_entry(0, uint, wgpu::TextureViewDimension::D2Array),
                tex_entry(1, uint, d2),
                tex_entry(2, wgpu::TextureSampleType::Float { filterable: false }, d2),
                tex_entry(3, uint, d2),
                tex_entry(4, uint, d2),
                tex_entry(5, wgpu::TextureSampleType::Float { filterable: false }, wgpu::TextureViewDimension::D2Array),
                tex_entry(6, wgpu::TextureSampleType::Float { filterable: false }, d2),
            ],
        });
        let view_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("view"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(VIEW_SIZE),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&bgl), Some(&view_bgl)],
            immediate_size: 0,
        });
        let attrs = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x2, 2 => Float32, 3 => Float32, 4 => Uint32, 5 => Uint32, 6 => Uint32
        ];
        let vlayout = [Some(wgpu::VertexBufferLayout { array_stride: 40, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs })];
        let pipe = |vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], blend: Option<wgpu::BlendState>, depth_write: bool, cmp: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some(vs), compilation_options: Default::default(), buffers },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(cmp),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::{BlendComponent, BlendFactor, BlendOperation, BlendState, CompareFunction};
        let multiply = BlendState {
            color: BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::Src, operation: BlendOperation::Add },
            alpha: BlendComponent::OVER,
        };
        let ground = pipe("vs_full", "fs_ground", &[], None, false, CompareFunction::Always);
        let tris = pipe("vs_tris", "fs_tris", &vlayout, None, true, CompareFunction::Greater);
        let trans = pipe("vs_tris", "fs_trans", &vlayout, Some(multiply), false, CompareFunction::GreaterEqual);
        let overlay_pipe = pipe("vs_full", "fs_overlay", &[], Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING), false, CompareFunction::Always);

        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("views"),
            size: SLOT * 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &view_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &uniforms, offset: 0, size: wgpu::BufferSize::new(VIEW_SIZE) }),
            }],
        });
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vertices"),
            size: 40 << 12,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let empty = Static::new(
            device,
            queue,
            Arc::new(SceneStatic {
                clr: Vec::new(),
                floor_tiles: vec![0; 4096],
                floor_map: vec![0; 128 * 128],
                floor_ids: vec![0],
                sky_set: 0,
                sky: vec![0; 320],
                sky_h: 1,
                mount: vec![0; 320],
                mount_h: 1,
                textures: Vec::new(),
            }),
            &HashMap::new(),
        );
        Renderer {
            device: device.clone(),
            queue: queue.clone(),
            srgb: format.is_srgb(),
            bgl,
            ground,
            tris,
            trans,
            overlay_pipe,
            stat: None,
            empty,
            replacements: HashMap::new(),
            clr: (0..32 * 256).map(|i| i as u8).collect(),
            litpal: texture(device, "litpal", 256, 34, 1, wgpu::TextureFormat::Rgba8Unorm),
            rowfit: texture(device, "rowfit", 32, 2, 1, wgpu::TextureFormat::Rgba32Float),
            overlay: texture(device, "overlay", 1, 1, 1, wgpu::TextureFormat::Rg8Uint),
            uniforms,
            view_bg,
            vertices,
            depth: None,
            bind: None,
        }
    }

    /// The current palette through each colour table row, plus the
    /// translucency factors (row 33).
    fn update_litpal(&self, frame: &Frame) {
        let pal = frame.pal.to_rgba_table();
        let lum = |c: [u8; 4]| 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
        let base: f32 = pal.iter().map(|&c| lum(c)).sum::<f32>().max(1.0);
        let mut px = vec![[0u8; 4]; 256 * 34];
        for r in 0..32 {
            let row = &self.clr[r * 256..][..256];
            let mut sum = 0.0;
            for c in 0..256 {
                px[r * 256 + c] = pal[row[c] as usize];
                sum += lum(pal[row[c] as usize]);
            }
            let k = (sum / base * 255.0).round().clamp(0.0, 255.0) as u8;
            px[33 * 256 + r] = [k, k, k, 255];
        }
        px[32 * 256..33 * 256].copy_from_slice(&pal);
        write(&self.queue, &self.litpal, 0, &bytes(&px), 4);

        // Least-squares fit of each row, per channel: out = k * in + o.
        let mut fit = vec![[0f32; 4]; 64];
        for r in 0..32 {
            let row = &self.clr[r * 256..][..256];
            for ch in 0..3 {
                let x = |c: usize| pal[c][ch] as f32 / 255.0;
                let y = |c: usize| pal[row[c] as usize][ch] as f32 / 255.0;
                let (mx, my) = ((0..256).map(x).sum::<f32>() / 256.0, (0..256).map(y).sum::<f32>() / 256.0);
                let var: f32 = (0..256).map(|c| (x(c) - mx).powi(2)).sum();
                let cov: f32 = (0..256).map(|c| (x(c) - mx) * (y(c) - my)).sum();
                let k = if var > 1e-6 { cov / var } else { 1.0 };
                fit[r][ch] = k;
                fit[32 + r][ch] = my - k * mx;
            }
        }
        write(&self.queue, &self.rowfit, 0, &bytes(&fit), 16);
    }

    /// Use these texture replacements (by name, see the crate doc) from the
    /// next race frame on.
    pub fn set_replacements(&mut self, replacements: HashMap<String, Image>) {
        self.replacements = replacements;
        self.stat = None;
        self.bind = None;
    }

    fn set_overlay(&mut self, w: u32, h: u32, data: &[u8]) {
        let s = self.overlay.size();
        if (s.width, s.height) != (w, h) {
            self.overlay = texture(&self.device, "overlay", w, h, 1, wgpu::TextureFormat::Rg8Uint);
            self.bind = None;
        }
        write(&self.queue, &self.overlay, 0, data, 2);
    }

    fn ensure_static(&mut self, src: &Arc<SceneStatic>) {
        if self.stat.as_ref().is_some_and(|s| Arc::ptr_eq(&s.src, src)) {
            return;
        }
        self.stat = Some(Static::new(&self.device, &self.queue, src.clone(), &self.replacements));
        self.clr = src.clr.clone();
        self.clr.resize(32 * 256, 0);
        self.bind = None;
    }

    fn bind_group(&mut self) -> wgpu::BindGroup {
        if let Some(b) = &self.bind {
            return b.clone();
        }
        let st = self.stat.as_ref().unwrap_or(&self.empty);
        let array = st.atlas.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let hiarray = st.hiatlas.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let views = [array, view(&st.texinfo), view(&self.litpal), view(&st.floormap), view(&self.overlay), hiarray, view(&self.rowfit)];
        let entries: Vec<wgpu::BindGroupEntry> =
            views.iter().enumerate().map(|(i, v)| wgpu::BindGroupEntry { binding: i as u32, resource: wgpu::BindingResource::TextureView(v) }).collect();
        let b = self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("textures"), layout: &self.bgl, entries: &entries });
        self.bind = Some(b.clone());
        b
    }

    /// Draw `frame` onto `target` (`size` pixels), the 320x200 frame
    /// covering `display` (x, y, w, h; see [`fit_4_3`]); the rest is black.
    pub fn render(&mut self, frame: &Frame, target: &wgpu::TextureView, size: (u32, u32), display: [f32; 4], opts: &Options) {
        let scene: Option<&Scene> = frame.scene.as_deref();
        if let Some(s) = scene {
            self.ensure_static(&s.stat);
            let ov: Vec<u8> = s.overlay.iter().zip(&s.overlay_kind).flat_map(|(&c, &k)| [c, k]).collect();
            self.set_overlay(320, 200, &ov);
        } else {
            let ov: Vec<u8> = frame.pix.iter().flat_map(|&c| [c, OV_OPAQUE]).collect();
            self.set_overlay(frame.w as u32, frame.h as u32, &ov);
        }
        self.update_litpal(frame);
        if self.depth.as_ref().is_none_or(|d| {
            let s = d.texture().size();
            (s.width, s.height) != size
        }) {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some(t.create_view(&Default::default()));
        }
        let bind = self.bind_group();

        // Uniform slot 0 is the frame (overlay), then one per view.
        let views = scene.map_or(&[][..], |s| &s.views[..]);
        let nslots = views.len() + 1;
        if self.uniforms.size() < SLOT * nslots as u64 {
            self.uniforms = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("views"),
                size: SLOT * nslots as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.view_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.ground.get_bind_group_layout(1),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.uniforms, offset: 0, size: wgpu::BufferSize::new(VIEW_SIZE) }),
                }],
            });
        }
        let st = self.stat.as_ref().unwrap_or(&self.empty);
        let (sky, mount, sky_h, mount_h) = (st.sky, st.mount, st.src.sky_h as u32, st.src.mount_h as u32);
        let ov = self.overlay.size();
        let map = [display[0], display[1], display[2] / 320.0, display[3] / 200.0];
        let misc = [opts.filter as u32, ov.width, ov.height, self.srgb as u32];
        let mut ubuf = vec![0u8; SLOT as usize * nslots];
        let put = |ubuf: &mut [u8], slot: usize, f: [[f32; 4]; 4], u: [[u32; 4]; 3]| {
            let words: Vec<u32> = f.iter().flatten().map(|x| x.to_bits()).chain(u.iter().flatten().copied()).collect();
            ubuf[slot * SLOT as usize..][..VIEW_SIZE as usize].copy_from_slice(&bytes(&words));
        };
        put(&mut ubuf, 0, [[0.0, 0.0, 320.0, 200.0], map, [0.0; 4], [0.0; 4]], [[0; 4], [sky, mount, sky_h, mount_h], misc]);
        let mut verts: Vec<SceneVertex> = Vec::new();
        let mut ranges = Vec::new();
        for (i, v) in views.iter().enumerate() {
            let r = v.rect.map(|x| x as f32);
            put(
                &mut ubuf,
                i + 1,
                [r, map, [v.cx, v.cy, v.focus, v.height], [v.cos, v.sin, v.sky_rows as f32, v.tilt as f32]],
                [[v.cam_x, v.cam_y, v.sky_off as u32, v.mount_off as u32], [sky, mount, sky_h, mount_h], misc],
            );
            let a = verts.len() as u32;
            verts.extend_from_slice(&v.tris);
            let b = verts.len() as u32;
            verts.extend_from_slice(&v.trans);
            ranges.push((a, b, verts.len() as u32));
        }
        self.queue.write_buffer(&self.uniforms, 0, &ubuf);
        let vbytes = bytes(&verts);
        if self.vertices.size() < vbytes.len() as u64 {
            self.vertices = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vertices"),
                size: (vbytes.len() as u64).next_power_of_two(),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !vbytes.is_empty() {
            self.queue.write_buffer(&self.vertices, 0, &vbytes);
        }

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.depth.as_ref().unwrap(),
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let [dx, dy, dw, dh] = display;
            if dw < 1.0 || dh < 1.0 {
                return;
            }
            pass.set_viewport(dx, dy, dw, dh, 0.0, 1.0);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_vertex_buffer(0, self.vertices.slice(..));
            // Scissor to a rectangle of the 320x200 frame.
            let scissor = |pass: &mut wgpu::RenderPass, r: [f32; 4]| {
                let x0 = (dx + r[0] * map[2]).round().clamp(0.0, size.0 as f32) as u32;
                let y0 = (dy + r[1] * map[3]).round().clamp(0.0, size.1 as f32) as u32;
                let x1 = (dx + (r[0] + r[2]) * map[2]).round().clamp(0.0, size.0 as f32) as u32;
                let y1 = (dy + (r[1] + r[3]) * map[3]).round().clamp(0.0, size.1 as f32) as u32;
                pass.set_scissor_rect(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0));
            };
            for (i, (v, &(a, b, c))) in views.iter().zip(&ranges).enumerate() {
                scissor(&mut pass, v.rect.map(|x| x as f32));
                pass.set_bind_group(1, &self.view_bg, &[(SLOT as usize * (i + 1)) as u32]);
                pass.set_pipeline(&self.ground);
                pass.draw(0..3, 0..1);
                if b > a {
                    pass.set_pipeline(&self.tris);
                    pass.draw(a..b, 0..1);
                }
                if c > b {
                    pass.set_pipeline(&self.trans);
                    pass.draw(b..c, 0..1);
                }
            }
            scissor(&mut pass, [0.0, 0.0, 320.0, 200.0]);
            pass.set_bind_group(1, &self.view_bg, &[0]);
            pass.set_pipeline(&self.overlay_pipe);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }
}
