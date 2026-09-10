//! Text rendering via glyphon (cosmic-text + wgpu).

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache,
    TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::{Device, Queue, TextureFormat};

/// Owns all glyphon state needed to render text each frame.
pub struct TextState {
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub atlas: TextAtlas,
    pub renderer: TextRenderer,
    pub viewport: Viewport,
}

impl TextState {
    pub fn new(device: &Device, queue: &Queue, format: TextureFormat) -> Self {
        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(device);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let renderer = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let viewport = Viewport::new(device, &cache);
        let _ = &mut font_system;
        Self {
            font_system,
            swash_cache,
            atlas,
            renderer,
            viewport,
        }
    }

    /// Update the viewport resolution. Call when the window resizes.
    pub fn resize(&mut self, queue: &Queue, width: u32, height: u32) {
        self.viewport.update(queue, Resolution { width, height });
    }

    /// Create a new text buffer with the given font size (in pixels) and
    /// wrapping width (in pixels).
    pub fn new_buffer(&mut self, font_size: f32, width: u32) -> Buffer {
        let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(font_size, font_size * 1.2));
        buffer.set_size(Some(width as f32), Some(font_size * 1.2 * 8.0));
        buffer
    }

    /// Set the text of a buffer, wrapping at its configured width.
    pub fn set_text(&mut self, buffer: &mut Buffer, text: &str, color: [f32; 3]) {
        let attrs = Attrs::new()
            .color(Color::rgb(
                (color[0] * 255.0) as u8,
                (color[1] * 255.0) as u8,
                (color[2] * 255.0) as u8,
            ))
            .family(Family::SansSerif);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Prepare a list of text areas for rendering. Call before `render`.
    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        areas: Vec<TextArea<'_>>,
    ) -> anyhow::Result<()> {
        self.renderer
            .prepare(
                device,
                queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                areas,
                &mut self.swash_cache,
            )
            .map_err(|e| anyhow::anyhow!("glyphon prepare: {e}"))?;
        Ok(())
    }

    /// Render all prepared text. Call within a render pass.
    pub fn render<'pass>(&'pass self, pass: &mut wgpu::RenderPass<'pass>) -> anyhow::Result<()> {
        self.renderer
            .render(&self.atlas, &self.viewport, pass)
            .map_err(|e| anyhow::anyhow!("glyphon render: {e}"))?;
        Ok(())
    }
}

/// Build a `TextArea` for a buffer positioned at `(left, top)` in pixels.
pub fn text_area<'a>(
    buffer: &'a Buffer,
    left: f32,
    top: f32,
    color: [f32; 3],
    bounds: Option<TextBounds>,
) -> TextArea<'a> {
    TextArea {
        buffer,
        left,
        top,
        scale: 1.0,
        bounds: bounds.unwrap_or_default(),
        default_color: Color::rgb(
            (color[0] * 255.0) as u8,
            (color[1] * 255.0) as u8,
            (color[2] * 255.0) as u8,
        ),
        custom_glyphs: &[],
    }
}
