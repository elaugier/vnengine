//! Text rendering via glyphon (cosmic-text + wgpu).

use crate::richtext::RichSegment;
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
        let renderer =
            TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
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

    /// Load a font file (TTF/OTF bytes) into the font system.
    /// Call at startup for every file under the project's font directory.
    /// Returns `true` if the font was accepted.
    pub fn load_font_data(&mut self, bytes: &[u8]) -> bool {
        let db = self.font_system.db_mut();
        let before = db.faces().count();
        db.load_font_data(bytes.to_vec());
        db.faces().count() > before
    }

    /// Create a new text buffer with the given font size (in pixels) and
    /// wrapping width (in pixels).
    pub fn new_buffer(&mut self, font_size: f32, width: u32) -> Buffer {
        let mut buffer = Buffer::new(
            &mut self.font_system,
            Metrics::new(font_size, font_size * 1.2),
        );
        buffer.set_size(Some(width as f32), Some(font_size * 1.2 * 8.0));
        buffer
    }

    /// Resolve a font family: explicit name if given, else the engine default.
    fn family_of<'a>(family: Option<&'a str>, default: Option<&'a str>) -> Family<'a> {
        if let Some(name) = family.or(default) {
            Family::Name(name)
        } else {
            Family::SansSerif
        }
    }

    /// Set the text of a buffer, wrapping at its configured width.
    /// `family` overrides the default font family when set.
    pub fn set_text(
        &mut self,
        buffer: &mut Buffer,
        text: &str,
        color: [f32; 3],
        family: Option<&str>,
        default_family: Option<&str>,
    ) {
        let fam = Self::family_of(family, default_family);
        let attrs = Attrs::new()
            .color(Color::rgb(
                (color[0] * 255.0) as u8,
                (color[1] * 255.0) as u8,
                (color[2] * 255.0) as u8,
            ))
            .family(fam);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
    }

    /// Set rich text (with formatting tags already parsed into segments) on a
    /// buffer. Each segment gets its own attributes (bold, italic, color, etc.).
    pub fn set_rich_text(
        &mut self,
        buffer: &mut Buffer,
        segments: &[RichSegment],
        default_color: [f32; 3],
        family: Option<&str>,
        default_family: Option<&str>,
    ) {
        use glyphon::{Style, Weight};

        let fam = Self::family_of(family, default_family);
        // Build owned strings and attrs list. We need the strings to stay alive
        // while we pass references to set_rich_text.
        let default_attrs = Attrs::new()
            .color(Color::rgb(
                (default_color[0] * 255.0) as u8,
                (default_color[1] * 255.0) as u8,
                (default_color[2] * 255.0) as u8,
            ))
            .family(fam);

        // Collect (String, Attrs) pairs.
        let pairs: Vec<(String, Attrs)> = segments
            .iter()
            .map(|seg| {
                let mut attrs = default_attrs.clone();
                if seg.bold {
                    attrs = attrs.weight(Weight::BOLD);
                }
                if seg.italic {
                    attrs = attrs.style(Style::Italic);
                }
                if let Some(color) = seg.color {
                    let r = (color[0] * 255.0) as u8;
                    let g = (color[1] * 255.0) as u8;
                    let b = (color[2] * 255.0) as u8;
                    let a = (color[3] * 255.0) as u8;
                    attrs = attrs.color(Color::rgba(r, g, b, a));
                }
                (seg.text.clone(), attrs)
            })
            .collect();

        // Build references for set_rich_text.
        let refs: Vec<(&str, Attrs)> = pairs.iter().map(|(s, a)| (s.as_str(), a.clone())).collect();
        buffer.set_rich_text(refs, &default_attrs, Shaping::Advanced, None);
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
