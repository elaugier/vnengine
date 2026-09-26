//! GPU texture cache for images.
//!
//! Loads image files from disk, decodes them with the `image` crate and
//! uploads them to wgpu textures. Textures are cached by path so repeated
//! requests are cheap.

use ahash::AHashMap;
use vnengine_core::assets::{AssetSource, IMAGE_EXTS};
use wgpu::TextureFormat;

/// A cached GPU texture and its dimensions.
pub struct GpuImage {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

pub struct TextureCache {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: TextureFormat,
    cache: AHashMap<String, GpuImage>,
}

impl TextureCache {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, format: TextureFormat) -> Self {
        Self {
            device,
            queue,
            format,
            cache: AHashMap::new(),
        }
    }

    /// Ensure a texture is loaded into the cache. Call before `get`.
    /// `rel` is an image reference relative to `dir` (the image dir);
    /// common extensions are probed when `rel` has none.
    pub fn ensure_loaded(&mut self, src: &dyn AssetSource, dir: &str, rel: &str) {
        if self.cache.contains_key(rel) {
            return;
        }
        let full = format!("{dir}/{rel}");
        let bytes = match src.read_any(&full, IMAGE_EXTS) {
            Ok((_, bytes)) => bytes,
            Err(e) => {
                log::warn!("failed to load image `{full}`: {e}");
                return;
            }
        };
        match image::load_from_memory(&bytes) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                let size = wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                };
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(rel),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                });
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * w),
                        rows_per_image: Some(h),
                    },
                    size,
                );
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                self.cache.insert(
                    rel.to_string(),
                    GpuImage {
                        texture,
                        view,
                        width: w,
                        height: h,
                    },
                );
            }
            Err(e) => {
                log::warn!("failed to decode image `{full}`: {e}");
            }
        }
    }

    /// Get a loaded texture by relative path. Returns `None` if not loaded.
    pub fn get(&self, rel: &str) -> Option<&GpuImage> {
        self.cache.get(rel)
    }

    /// Access the underlying device/queue (e.g. for creating samplers).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    pub fn format(&self) -> TextureFormat {
        self.format
    }
}
