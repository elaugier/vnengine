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

        let textured_pipeline = create_textured_pipeline(&device, format, &bind_group_layout);
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

            // Background.
            if let Some(bg) = &scene.background {
                if let Some(img) = self.textures.get(bg) {
                    let quad = full_screen_quad(img.width, img.height, scene.bg_opacity);
                    draw_textured(
                        &mut pass,
                        &self.textured_pipeline,
                        &self.device,
                        &self.bind_group_layout,
                        &self.sampler,
                        img,
                        &[quad],
                    );
                }
            }
            // Previous background (for cross-fades).
            if let Some(bg) = &scene.prev_background {
                if let Some(img) = self.textures.get(bg) {
                    let quad = full_screen_quad(img.width, img.height, scene.prev_bg_opacity);
                    draw_textured(
                        &mut pass,
                        &self.textured_pipeline,
                        &self.device,
                        &self.bind_group_layout,
                        &self.sampler,
                        img,
                        &[quad],
                    );
                }
            }

            // Sprites.
            for s in &scene.sprites {
                if let Some(img) = self.textures.get(&s.image) {
                    let aspect = img.width as f32 / img.height as f32;
                    let hh = 0.45;
                    let hw = hh * aspect;
                    let x = s
                        .position
                        .map(|p| p.0)
                        .unwrap_or_else(|| anchor_x(s.anchor));
                    let y = s.position.map(|p| p.1).unwrap_or(0.5);
                    let quad = Quad {
                        x,
                        y: y * 0.5 + 0.05,
                        hw: hw * s.scale,
                        hh: hh * s.scale,
                        color: [1.0, 1.0, 1.0, s.opacity],
                        offset: s.offset,
                    };
                    draw_textured(
                        &mut pass,
                        &self.textured_pipeline,
                        &self.device,
                        &self.bind_group_layout,
                        &self.sampler,
                        img,
                        std::slice::from_ref(&quad),
                    );
                }
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
            for t in &ui.text {
                let mut buf = self.text.new_buffer(t.font_size, self.config.width);
                if t.rich {
                    let segments = crate::richtext::parse_rich_text(&t.text);
                    self.text.set_rich_text(&mut buf, &segments, t.color);
                } else {
                    self.text.set_text(&mut buf, &t.text, t.color);
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

fn create_textured_pipeline(
    device: &Device,
    format: TextureFormat,
    bgl: &wgpu::BindGroupLayout,
) -> RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("textured shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("textured.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("textured layout"),
        bind_group_layouts: &[Some(bgl)],
        immediate_size: 0,
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

pub fn full_screen_quad(tex_w: u32, tex_h: u32, opacity: f32) -> Quad {
    let aspect = tex_w as f32 / tex_h as f32;
    let screen_aspect = 16.0 / 9.0;
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
    }
}

pub fn quad_vertices_textured(q: &Quad) -> [TexturedVertex; 6] {
    let x = q.x + q.offset[0];
    let y = q.y + q.offset[1];
    let (hw, hh) = (q.hw, q.hh);
    let c = q.color;
    let nx = |v: f32| v * 2.0 - 1.0;
    let ny = |v: f32| 1.0 - v * 2.0;
    [
        TexturedVertex {
            pos: [nx(x - hw), ny(y - hh)],
            uv: [0.0, 0.0],
            color: c,
        },
        TexturedVertex {
            pos: [nx(x + hw), ny(y - hh)],
            uv: [1.0, 0.0],
            color: c,
        },
        TexturedVertex {
            pos: [nx(x - hw), ny(y + hh)],
            uv: [0.0, 1.0],
            color: c,
        },
        TexturedVertex {
            pos: [nx(x + hw), ny(y - hh)],
            uv: [1.0, 0.0],
            color: c,
        },
        TexturedVertex {
            pos: [nx(x + hw), ny(y + hh)],
            uv: [1.0, 1.0],
            color: c,
        },
        TexturedVertex {
            pos: [nx(x - hw), ny(y + hh)],
            uv: [0.0, 1.0],
            color: c,
        },
    ]
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

fn draw_textured<'pass>(
    pass: &mut wgpu::RenderPass<'pass>,
    pipeline: &'pass RenderPipeline,
    device: &'pass Device,
    bgl: &'pass wgpu::BindGroupLayout,
    sampler: &'pass wgpu::Sampler,
    img: &'pass crate::texture::GpuImage,
    quads: &[Quad],
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
    let verts: Vec<TexturedVertex> = quads.iter().flat_map(quad_vertices_textured).collect();
    let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("quad verts"),
        contents: bytemuck::cast_slice(&verts),
        usage: wgpu::BufferUsages::VERTEX,
    });
    pass.set_vertex_buffer(0, buf.slice(..));
    pass.draw(0..verts.len() as u32, 0..1);
}
