//! Main wgpu renderer for the visual novel engine.
//!
//! Renders, in order:
//!   1. Background image (full-screen quad)
//!   2. Character sprites (positioned quads)
//!   3. UI boxes (dialogue box, menu) as semi-transparent colored quads
//!   4. Text (via glyphon)
//!
//! Transitions are driven by the application layer, which sets opacity/offset
//! values on the [`Scene`] model each frame. The renderer simply samples
//! those values.

use crate::text::{text_area, TextState};
use crate::texture::TextureCache;
use bytemuck::{Pod, Zeroable};
use vnengine_core::assets::AssetSource;
use vnengine_core::model::{anchor_x, Scene};
use wgpu::util::DeviceExt;
use wgpu::{
    BlendComponent, BlendFactor, BlendOperation, BlendState, ColorTargetState, CompositeAlphaMode,
    Device, Queue, RenderPipeline, SurfaceColorSpace, SurfaceConfiguration, TextureFormat,
    TextureUsages,
};

/// Vertex for a textured quad (sprite or background).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct TexturedVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// Vertex for a flat-colored quad (UI boxes).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ColorVertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
}

/// Per-draw transition-FX parameters passed to the masked pipeline
/// through `set_immediates` (wgpu immediate data).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct FxParams {
    /// Mask kind: 0 = none, 1 = blinds-v, 2 = blinds-h, 3 = iris in,
    /// 4 = iris out, 5 = pixelate, 6 = squares.
    pub kind: u32,
    /// Reveal progress in 0..1.
    pub progress: f32,
    /// Mask parameter: strip count (blinds) / grid cells (squares).
    pub p0: f32,
    /// Reserved.
    pub p1: f32,
}

impl FxParams {
    /// No effect — every draw not carrying a transition mask uses this.
    pub const NONE: FxParams = FxParams {
        kind: 0,
        progress: 0.0,
        p0: 0.0,
        p1: 0.0,
    };

    /// Build the params for a transition at `progress`.
    pub fn of(t: &vnengine_core::transition::Transition, progress: f32) -> FxParams {
        FxParams {
            kind: t.mask_kind(),
            progress,
            p0: t.mask_param(),
            p1: 0.0,
        }
    }
}

/// A quad to draw this frame.
pub struct Quad {
    /// Center position in normalized coords (0..1), with y growing downward.
    pub x: f32,
    pub y: f32,
    /// Half-extents in normalized coords.
    pub hw: f32,
    pub hh: f32,
    /// Opacity / tint.
    pub color: [f32; 4],
    /// Pixel offset (for slide transitions), in normalized coords.
    pub offset: [f32; 2],
    /// Rotation in radians around the quad's center.
    pub rotation: f32,
}

pub struct Renderer {
    pub device: Device,
    pub queue: Queue,
    pub config: SurfaceConfiguration,
    pub surface: wgpu::Surface<'static>,
    pub textures: TextureCache,
    pub text: TextState,
    pub sampler: wgpu::Sampler,
    pub textured_pipeline: RenderPipeline,
    pub color_pipeline: RenderPipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    /// Whether the device supports `set_immediates` (masked
    /// transitions). False degrades masks to plain draws.
    pub immediates: bool,
    /// Default font family for all UI text (set from the script's
    /// `font default "..."`). `None` = system sans-serif.
    pub default_font: Option<String>,
}

impl Renderer {
    /// Create a renderer from pre-initialized wgpu resources.
    ///
    /// The caller (app layer) is responsible for creating the wgpu instance,
    /// adapter, device, queue and surface, since instance creation requires
    /// a display handle from the winit event loop.
    pub fn new(
        device: Device,
        queue: Queue,
        surface: wgpu::Surface<'static>,
        window_size: (u32, u32),
        format: TextureFormat,
    ) -> anyhow::Result<Self> {
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: window_size.0,
            height: window_size.1,
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &config);

        let textures = TextureCache::new(device.clone(), queue.clone(), format);
        let text = TextState::new(&device, &queue, format);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vnengine sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vnengine textured bind layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let immediates = device.features().contains(wgpu::Features::IMMEDIATES);
        let textured_pipeline =
            create_textured_pipeline(&device, format, &bind_group_layout, immediates);
        let color_pipeline = create_color_pipeline(&device, format);

        Ok(Self {
            device,
            queue,
            config,
            surface,
            textures,
            text,
            sampler,
            textured_pipeline,
            color_pipeline,
            bind_group_layout,
            immediates,
            default_font: None,
        })
    }

    /// Resize the surface.
    pub fn resize(&mut self, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 {
            return;
        }
        self.config.width = size.0;
        self.config.height = size.1;
        self.surface.configure(&self.device, &self.config);
        self.text.resize(&self.queue, size.0, size.1);
    }

    /// Render a frame. Images are fetched from `src` under `image_dir`.
    pub fn render(
        &mut self,
        scene: &Scene,
        src: &dyn AssetSource,
        image_dir: &str,
        ui: &UiDraw,
    ) -> anyhow::Result<()> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Outdated
            | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(anyhow::anyhow!("surface lost"));
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(anyhow::anyhow!("surface validation error"));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vnengine"),
            });

        // --- Scene pass: backgrounds + sprites + UI boxes ---
        {
            // Pre-load all needed textures (mutable phase).
            if let Some(bg) = &scene.background {
                self.textures.ensure_loaded(src, image_dir, bg);
            }
            if let Some(bg) = &scene.prev_background {
                self.textures.ensure_loaded(src, image_dir, bg);
            }
            for s in &scene.sprites {
                self.textures.ensure_loaded(src, image_dir, &s.image);
                for l in &s.layers {
                    self.textures.ensure_loaded(src, image_dir, &l.image);
                }
            }
            for e in &scene.exiting {
                self.textures.ensure_loaded(src, image_dir, &e.sprite.image);
                for l in &e.sprite.layers {
                    self.textures.ensure_loaded(src, image_dir, &l.image);
                }
            }
            for l in scene.bg_layers.iter().chain(&scene.prev_bg_layers) {
                self.textures.ensure_loaded(src, image_dir, &l.image);
            }
            for lb in scene.layer_bgs.values() {
                self.textures.ensure_loaded(src, image_dir, &lb.image);
                for l in &lb.layers {
                    self.textures.ensure_loaded(src, image_dir, &l.image);
                }
            }
            for img in &ui.images {
                self.textures.ensure_loaded(src, image_dir, &img.path);
            }

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vnengine scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            let screen_aspect = self.config.width as f32 / self.config.height as f32;
            const BG_PARALLAX: f32 = 0.25;
            // Frame mask for masked scene transitions (`with blinds`
            // etc.): applied to the *new* frame — bg, its layers and
            // the active sprites.
            let frame_fx = scene
                .trans_fx
                .as_ref()
                .map(|(t, p)| FxParams::of(t, *p))
                .unwrap_or(FxParams::NONE);
            let immediates = self.immediates;
            let current = (
                scene.background.as_deref(),
                scene.bg_layers.as_slice(),
                scene.bg_opacity,
                scene.bg_zoom,
                scene.bg_offset,
                frame_fx,
            );
            let previous = (
                scene.prev_background.as_deref(),
                scene.prev_bg_layers.as_slice(),
                scene.prev_bg_opacity,
                scene.prev_bg_zoom,
                scene.prev_bg_offset,
                FxParams::NONE,
            );
            // Masked transitions reveal the incoming background over the outgoing one.
            let backgrounds = if scene.trans_fx.is_some() {
                [previous, current]
            } else {
                [current, previous]
            };
            for (image, layers, opacity, zoom, offset, fx) in backgrounds {
                let layer_quads = layers
                    .iter()
                    .map(|layer| (layer.image.as_str(), layer.alpha, layer.zoom, layer.offset));
                for (path, alpha, scale, shift) in image
                    .map(|path| (path, 1.0, 1.0, [0.0, 0.0]))
                    .into_iter()
                    .chain(layer_quads)
                {
                    if let Some(img) = self.textures.get(path) {
                        let mut quad =
                            full_screen_quad(img.width, img.height, screen_aspect, opacity * alpha);
                        quad.hw *= zoom * scale;
                        quad.hh *= zoom * scale;
                        quad.offset = [offset[0] + shift[0], offset[1] + shift[1]];
                        apply_camera(&mut quad, BG_PARALLAX, &scene.camera);
                        let verts = quad_vertices_lit(&quad, &scene.lights, scene.ambient);
                        draw_textured(
                            &mut pass,
                            &self.textured_pipeline,
                            &self.device,
                            &self.bind_group_layout,
                            &self.sampler,
                            img,
                            &verts,
                            fx,
                            immediates,
                        );
                    }
                }
            }

            // Shared sprite-quad builder: composite `layers` draw as
            // extra quads with the same transform; `anchor_frac`
            // shifts the quad so the anchor point lands on `position`.
            let make_sprite_quad = |s: &vnengine_core::model::ActiveSprite,
                                    opacity: f32,
                                    offset: [f32; 2],
                                    zoom: f32,
                                    alpha: f32,
                                    aspect: f32| {
                let hh = 0.45;
                let hw = hh * aspect;
                let x = s
                    .position
                    .map(|p| p.0)
                    .unwrap_or_else(|| anchor_x(s.anchor));
                let y = s.position.map(|p| p.1).unwrap_or(0.5);
                let mut q = Quad {
                    x,
                    y: y * 0.5 + 0.05,
                    hw: hw * s.scale[0] * zoom,
                    hh: hh * s.scale[1] * zoom,
                    color: [1.0, 1.0, 1.0, opacity * alpha],
                    offset,
                    rotation: s.rotation,
                };
                if let Some((ax, ay)) = s.anchor_frac {
                    q.x += (0.5 - ax) * 2.0 * q.hw;
                    q.y += (0.5 - ay) * 2.0 * q.hh;
                }
                q
            };

            // Per-layer content: a `scene x onlayer l` background sits
            // behind that layer's sprites.
            for layer in [
                vnengine_core::model::Layer::Background,
                vnengine_core::model::Layer::Midground,
                vnengine_core::model::Layer::Foreground,
                vnengine_core::model::Layer::Overlay,
            ] {
                if let Some(lb) = scene.layer_bgs.get(&(layer as u8)) {
                    if let Some(img) = self.textures.get(&lb.image) {
                        let mut quad =
                            full_screen_quad(img.width, img.height, screen_aspect, lb.opacity);
                        quad.hw *= lb.zoom;
                        quad.hh *= lb.zoom;
                        apply_camera(
                            &mut quad,
                            vnengine_core::model::parallax(layer),
                            &scene.camera,
                        );
                        let verts = quad_vertices_lit(&quad, &scene.lights, scene.ambient);
                        draw_textured(
                            &mut pass,
                            &self.textured_pipeline,
                            &self.device,
                            &self.bind_group_layout,
                            &self.sampler,
                            img,
                            &verts,
                            FxParams::NONE,
                            immediates,
                        );
                    }
                    for l in &lb.layers {
                        if let Some(img) = self.textures.get(&l.image) {
                            let mut quad = full_screen_quad(
                                img.width,
                                img.height,
                                screen_aspect,
                                lb.opacity * l.alpha,
                            );
                            quad.hw *= lb.zoom * l.zoom;
                            quad.hh *= lb.zoom * l.zoom;
                            quad.offset = l.offset;
                            apply_camera(
                                &mut quad,
                                vnengine_core::model::parallax(layer),
                                &scene.camera,
                            );
                            let verts = quad_vertices_lit(&quad, &scene.lights, scene.ambient);
                            draw_textured(
                                &mut pass,
                                &self.textured_pipeline,
                                &self.device,
                                &self.bind_group_layout,
                                &self.sampler,
                                img,
                                &verts,
                                FxParams::NONE,
                                immediates,
                            );
                        }
                    }
                }
                for s in scene.sprites.iter().filter(|s| s.layer == layer) {
                    // Entrance transition: fades ramp opacity, slides
                    // offset in, masks drive the shader reveal.
                    let mut fx = frame_fx;
                    let (opacity, offset, scale_mul) = match &s.entering {
                        Some(a) => {
                            let p = a.progress();
                            match &a.transition {
                                t if t.is_masked() => {
                                    fx = FxParams::of(t, p);
                                    (s.opacity, s.offset, 1.0)
                                }
                                vnengine_core::transition::Transition::Slide {
                                    direction, ..
                                }
                                | vnengine_core::transition::Transition::Push {
                                    direction, ..
                                } => {
                                    let d = 1.0 - p;
                                    let off = match direction {
                                        vnengine_core::transition::SlideDirection::Left => {
                                            [-d, 0.0]
                                        }
                                        vnengine_core::transition::SlideDirection::Right => {
                                            [d, 0.0]
                                        }
                                        vnengine_core::transition::SlideDirection::Up => [0.0, -d],
                                        vnengine_core::transition::SlideDirection::Down => [0.0, d],
                                    };
                                    (s.opacity, [s.offset[0] + off[0], s.offset[1] + off[1]], 1.0)
                                }
                                vnengine_core::transition::Transition::Zoom { .. } => {
                                    (s.opacity * p, s.offset, 0.5 + 0.5 * p)
                                }
                                _ => (s.opacity * p, s.offset, 1.0),
                            }
                        }
                        None => (s.opacity, s.offset, 1.0),
                    };
                    // Base image, or every composite layer.
                    let mut images: Vec<(&str, [f32; 2], f32, f32)> = Vec::new();
                    if s.layers.is_empty() {
                        images.push((s.image.as_str(), offset, scale_mul, 1.0));
                    } else {
                        for l in &s.layers {
                            images.push((
                                l.image.as_str(),
                                [offset[0] + l.offset[0], offset[1] + l.offset[1]],
                                scale_mul * l.zoom,
                                l.alpha,
                            ));
                        }
                    }
                    for (image, off, zoom, alpha) in images {
                        let Some(img) = self.textures.get(image) else {
                            continue;
                        };
                        let aspect = img.width as f32 / img.height as f32;
                        let mut quad = make_sprite_quad(s, opacity, off, zoom, alpha, aspect);
                        apply_camera(
                            &mut quad,
                            vnengine_core::model::parallax(s.layer),
                            &scene.camera,
                        );
                        let verts = quad_vertices_lit(&quad, &scene.lights, scene.ambient);
                        draw_textured(
                            &mut pass,
                            &self.textured_pipeline,
                            &self.device,
                            &self.bind_group_layout,
                            &self.sampler,
                            img,
                            &verts,
                            fx,
                            immediates,
                        );
                    }
                }
            }
            // Exiting sprites: transition applied from their progress.
            // They belong to the outgoing frame — no `trans_fx`.
            for e in &scene.exiting {
                let s = &e.sprite;
                let p = e.progress();
                let mut fx = FxParams::NONE;
                let (opacity, offset, scale_mul) = match &e.transition {
                    t if t.is_masked() => {
                        // The sprite disappears through the mask.
                        fx = FxParams::of(t, 1.0 - p);
                        (s.opacity, s.offset, 1.0)
                    }
                    vnengine_core::transition::Transition::Slide { direction, .. }
                    | vnengine_core::transition::Transition::Push { direction, .. } => {
                        let dir = match direction {
                            vnengine_core::transition::SlideDirection::Left => [-p, 0.0],
                            vnengine_core::transition::SlideDirection::Right => [p, 0.0],
                            vnengine_core::transition::SlideDirection::Up => [0.0, -p],
                            vnengine_core::transition::SlideDirection::Down => [0.0, p],
                        };
                        (s.opacity, [s.offset[0] + dir[0], s.offset[1] + dir[1]], 1.0)
                    }
                    vnengine_core::transition::Transition::Zoom { .. } => {
                        (s.opacity * (1.0 - p), s.offset, 0.5 + 0.5 * (1.0 - p))
                    }
                    _ => (s.opacity * (1.0 - p), s.offset, 1.0),
                };
                let mut images: Vec<(&str, [f32; 2], f32, f32)> = Vec::new();
                if s.layers.is_empty() {
                    images.push((s.image.as_str(), offset, scale_mul, 1.0));
                } else {
                    for l in &s.layers {
                        images.push((
                            l.image.as_str(),
                            [offset[0] + l.offset[0], offset[1] + l.offset[1]],
                            scale_mul * l.zoom,
                            l.alpha,
                        ));
                    }
                }
                for (image, off, zoom, alpha) in images {
                    if let Some(img) = self.textures.get(image) {
                        let aspect = img.width as f32 / img.height as f32;
                        let quad = make_sprite_quad(s, opacity, off, zoom, alpha, aspect);
                        let mut quad = quad;
                        apply_camera(
                            &mut quad,
                            vnengine_core::model::parallax(s.layer),
                            &scene.camera,
                        );
                        let verts = quad_vertices_lit(&quad, &scene.lights, scene.ambient);
                        draw_textured(
                            &mut pass,
                            &self.textured_pipeline,
                            &self.device,
                            &self.bind_group_layout,
                            &self.sampler,
                            img,
                            &verts,
                            fx,
                            immediates,
                        );
                    }
                }
            }

            // Fade overlay (solid color between the two fade phases).
            if scene.fade_overlay[3] > 0.0 {
                pass.set_pipeline(&self.color_pipeline);
                let overlay = UiBox {
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    color: scene.fade_overlay,
                };
                let verts = quad_vertices_color(&overlay);
                let buf = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("fade overlay"),
                        contents: bytemuck::cast_slice(&verts),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..6, 0..1);
            }

            // UI colored quads.
            if !ui.boxes.is_empty() {
                pass.set_pipeline(&self.color_pipeline);
                let verts: Vec<ColorVertex> =
                    ui.boxes.iter().flat_map(quad_vertices_color).collect();
                let buf = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("ui verts"),
                        contents: bytemuck::cast_slice(&verts),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..verts.len() as u32, 0..1);
            }
        }

        // --- UI Image pass ---
        for img in &ui.images {
            if let Some(texture) = self.textures.get(&img.path) {
                let (iw, ih) = (texture.width as f32, texture.height as f32);
                let (w, h) = img.size.unwrap_or((iw, ih));
                let scale = (w / iw).min(h / ih);
                let draw_w = iw * scale;
                let draw_h = ih * scale;
                let px = img.left + (w - draw_w) / 2.0;
                let py = img.top + (h - draw_h) / 2.0;

                let (screen_w, screen_h) = (self.config.width as f32, self.config.height as f32);
                let quad = Quad {
                    x: (px + draw_w / 2.0) / screen_w,
                    y: (py + draw_h / 2.0) / screen_h,
                    hw: draw_w / 2.0 / screen_w,
                    hh: draw_h / 2.0 / screen_h,
                    color: [1.0, 1.0, 1.0, img.opacity],
                    offset: [0.0, 0.0],
                    rotation: 0.0,
                };
                let verts = quad_vertices_textured(&quad);
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    layout: &self.bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&texture.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                    label: Some("ui image bind group"),
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("vnengine ui image"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.textured_pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                let buf = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("ui image verts"),
                        contents: bytemuck::cast_slice(&verts),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..verts.len() as u32, 0..1);
            }
        }

        // --- Text pass ---
        {
            let mut buffers: Vec<glyphon::Buffer> = Vec::new();
            let default_font = self.default_font.as_deref();
            for t in &ui.text {
                let mut buf = self.text.new_buffer(t.font_size, self.config.width);
                let fam = t.font_family.as_deref();
                if t.rich {
                    let segments = crate::richtext::parse_rich_text(&t.text);
                    self.text
                        .set_rich_text(&mut buf, &segments, t.color, fam, default_font);
                } else {
                    self.text
                        .set_text(&mut buf, &t.text, t.color, fam, default_font);
                }
                buffers.push(buf);
            }
            let mut areas = Vec::new();
            for (i, t) in ui.text.iter().enumerate() {
                areas.push(text_area(&buffers[i], t.left, t.top, t.color, t.bounds));
            }
            self.text.prepare(&self.device, &self.queue, areas)?;

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vnengine text"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.text.render(&mut pass)?;
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.present(frame);
        self.text.atlas.trim();
        Ok(())
    }
}

// ---- UI drawing model ----

/// A flat-colored rectangle in normalized screen coords.
pub struct UiBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub color: [f32; 4],
}

/// A piece of text to render.
pub struct UiText {
    pub text: String,
    pub left: f32,
    pub top: f32,
    pub font_size: f32,
    pub color: [f32; 3],
    pub bounds: Option<glyphon::TextBounds>,
    /// If true, the text contains formatting tags ({b}, {i}, {color=}, etc.)
    /// and will be parsed as rich text.
    pub rich: bool,
    /// Font family override (e.g. "MyFont"). When `None`, the renderer's
    /// `default_font` is used, falling back to the system sans-serif.
    pub font_family: Option<String>,
}

/// An image to render in the UI layer.
pub struct UiImage {
    /// Image path relative to image_root.
    pub path: String,
    /// Position in pixels.
    pub left: f32,
    pub top: f32,
    /// Size in pixels (width, height). If None, use natural size.
    pub size: Option<(f32, f32)>,
    /// Opacity (0..1).
    pub opacity: f32,
}

/// Everything the app wants drawn as UI this frame.
pub struct UiDraw {
    pub boxes: Vec<UiBox>,
    pub text: Vec<UiText>,
    pub images: Vec<UiImage>,
}

impl UiDraw {
    pub fn empty() -> Self {
        Self {
            boxes: Vec::new(),
            text: Vec::new(),
            images: Vec::new(),
        }
    }
}

// ---- pipeline creation ----

/// Build the textured shader source. When `immediates` is true the
/// transition-FX declaration and mask body are injected, keyed off
/// `var<immediate>` per-draw params.
fn textured_wgsl(immediates: bool) -> String {
    let src = include_str!("textured.wgsl");
    if !immediates {
        return src.replace("// @@FX_DECL", "").replace("// @@FX_BODY", "");
    }
    let decl = r#"
struct FxParams {
    kind: u32,
    progress: f32,
    p0: f32,
    p1: f32,
};
var<immediate> fx: FxParams;
"#;
    let body = r#"
    if (fx.kind == 5u) {
        // pixelate: uv snapped to a grid that refines with progress
        let cells = mix(6.0, 512.0, fx.progress);
        uv = (floor(uv * cells) + 0.5) / cells;
    } else if (fx.kind != 0u) {
        var m = 1.0;
        if (fx.kind == 1u || fx.kind == 2u) {
            // blinds: p0 strips along x (1 = vertical) or y (2)
            let coord = select(uv.y, uv.x, fx.kind == 1u);
            let strips = max(fx.p0, 1.0);
            m = step(fract(coord * strips), fx.progress);
        } else if (fx.kind == 3u) {
            // iris in: reveal from center outward
            let d = distance(uv, vec2<f32>(0.5));
            let r = fx.progress * 0.72;
            m = 1.0 - smoothstep(r - 0.03, r, d);
        } else if (fx.kind == 4u) {
            // iris out: reveal from edges inward
            let d = distance(uv, vec2<f32>(0.5));
            let r = (1.0 - fx.progress) * 0.72;
            m = smoothstep(r, r + 0.03, d);
        } else if (fx.kind == 6u) {
            // squares: grid cells revealed by a stable hash threshold
            let cells = max(fx.p0, 1.0);
            let cell = floor(uv * cells);
            let h = fract(sin(dot(cell, vec2<f32>(12.9898, 78.233))) * 43758.5453);
            m = step(h, fx.progress);
        }
        cm.a *= m;
    }
"#;
    src.replace("// @@FX_DECL", decl)
        .replace("// @@FX_BODY", body)
}

fn create_textured_pipeline(
    device: &Device,
    format: TextureFormat,
    bgl: &wgpu::BindGroupLayout,
    immediates: bool,
) -> RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("textured shader"),
        source: wgpu::ShaderSource::Wgsl(textured_wgsl(immediates).into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("textured layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: if immediates {
            std::mem::size_of::<FxParams>() as u32
        } else {
            0
        },
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("textured pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<TexturedVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x2,
                    1 => Float32x2,
                    2 => Float32x4,
                ],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn create_color_pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("color shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("color.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("color layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("color pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<ColorVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x2,
                    1 => Float32x4,
                ],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::SrcAlpha,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

// ---- quad geometry ----

/// Full-screen background quad, cover-fitted to `screen_aspect`
/// (`width / height` of the render surface — not hardcoded).
pub fn full_screen_quad(tex_w: u32, tex_h: u32, screen_aspect: f32, opacity: f32) -> Quad {
    let aspect = tex_w as f32 / tex_h as f32;
    let (hw, hh) = if aspect > screen_aspect {
        (1.0, 1.0 / aspect * screen_aspect)
    } else {
        (aspect / screen_aspect, 1.0)
    };
    Quad {
        x: 0.5,
        y: 0.5,
        hw,
        hh,
        color: [1.0, 1.0, 1.0, opacity],
        offset: [0.0, 0.0],
        rotation: 0.0,
    }
}

/// Corner vertex of `q` at local offset `(dx, dy)`, tinted by the
/// scene's point lights at its rotated position.
fn textured_corner(
    q: &Quad,
    dx: f32,
    dy: f32,
    uv: [f32; 2],
    lights: &[vnengine_core::model::PointLight],
    ambient: [f32; 3],
) -> TexturedVertex {
    let x = q.x + q.offset[0];
    let y = q.y + q.offset[1];
    let (sin, cos) = q.rotation.sin_cos();
    // Local corner offset rotated around the quad center.
    let cx = x + dx * cos - dy * sin;
    let cy = y + dx * sin + dy * cos;
    let light = vnengine_core::model::light_at(cx, cy, lights, ambient);
    let c = q.color;
    TexturedVertex {
        pos: [cx * 2.0 - 1.0, 1.0 - cy * 2.0],
        uv,
        color: [c[0] * light[0], c[1] * light[1], c[2] * light[2], c[3]],
    }
}

/// Lit variant: each vertex is tinted by `light_at` at its position.
pub fn quad_vertices_lit(
    q: &Quad,
    lights: &[vnengine_core::model::PointLight],
    ambient: [f32; 3],
) -> [TexturedVertex; 6] {
    let (hw, hh) = (q.hw, q.hh);
    [
        textured_corner(q, -hw, -hh, [0.0, 0.0], lights, ambient),
        textured_corner(q, hw, -hh, [1.0, 0.0], lights, ambient),
        textured_corner(q, -hw, hh, [0.0, 1.0], lights, ambient),
        textured_corner(q, hw, -hh, [1.0, 0.0], lights, ambient),
        textured_corner(q, hw, hh, [1.0, 1.0], lights, ambient),
        textured_corner(q, -hw, hh, [0.0, 1.0], lights, ambient),
    ]
}

pub fn quad_vertices_textured(q: &Quad) -> [TexturedVertex; 6] {
    quad_vertices_lit(q, &[], [1.0, 1.0, 1.0])
}

pub fn quad_vertices_color(b: &UiBox) -> [ColorVertex; 6] {
    let (x, y, w, h) = (b.x, b.y, b.w, b.h);
    let c = b.color;
    let nx = |v: f32| v * 2.0 - 1.0;
    let ny = |v: f32| 1.0 - v * 2.0;
    [
        ColorVertex {
            pos: [nx(x), ny(y)],
            color: c,
        },
        ColorVertex {
            pos: [nx(x + w), ny(y)],
            color: c,
        },
        ColorVertex {
            pos: [nx(x), ny(y + h)],
            color: c,
        },
        ColorVertex {
            pos: [nx(x + w), ny(y)],
            color: c,
        },
        ColorVertex {
            pos: [nx(x + w), ny(y + h)],
            color: c,
        },
        ColorVertex {
            pos: [nx(x), ny(y + h)],
            color: c,
        },
    ]
}

/// Apply camera parallax/zoom to a quad in normalized space.
/// `depth` is the layer's parallax factor (1.0 = foreground).
pub fn apply_camera(q: &mut Quad, depth: f32, cam: &vnengine_core::model::Camera) {
    let zz = 1.0 + (cam.zoom - 1.0) * depth;
    q.x = (q.x - 0.5 - cam.x * depth) * zz + 0.5;
    q.y = (q.y - 0.5 - cam.y * depth) * zz + 0.5;
    q.hw *= zz;
    q.hh *= zz;
}

fn draw_textured<'pass>(
    pass: &mut wgpu::RenderPass<'pass>,
    pipeline: &'pass RenderPipeline,
    device: &'pass Device,
    bgl: &'pass wgpu::BindGroupLayout,
    sampler: &'pass wgpu::Sampler,
    img: &'pass crate::texture::GpuImage,
    verts: &[TexturedVertex],
    fx: FxParams,
    immediates: bool,
) {
    pass.set_pipeline(pipeline);
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&img.view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    });
    pass.set_bind_group(0, &bind, &[]);
    if immediates {
        // Immediate data persists across draws in a pass — every draw
        // writes its params (masked or NONE) explicitly.
        pass.set_immediates(0, bytemuck::bytes_of(&fx));
    }
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("quad verts"),
        contents: bytemuck::cast_slice(verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    pass.set_vertex_buffer(0, buf.slice(..));
    pass.draw(0..verts.len() as u32, 0..1);
}
