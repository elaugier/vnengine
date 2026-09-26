//! Read-only access to game assets.
//!
//! An [`AssetSource`] abstracts where assets live: a loose project
//! directory during development, or a packaged `.vnpkg` archive for
//! distribution. All reads take *project-relative* paths like
//! `images/bg.png`; packaged sources transparently map those to their
//! `assets/` prefix inside the archive.

use std::io;
use std::path::{Component, Path, PathBuf};

/// Image extensions probed when an image reference lacks a file extension.
pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];

/// Audio extensions probed for music/sfx/voice references.
pub const AUDIO_EXTS: &[&str] = &["ogg", "wav", "mp3", "flac"];

fn not_found(rel: &str) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("asset not found: `{rel}`"))
}

/// Normalize a project-relative asset path: forward slashes, and reject
/// absolute paths, prefixes and `..` components so a script or archive
/// cannot escape the asset root.
pub fn sanitize_rel(rel: &str) -> io::Result<String> {
    let rel = rel.replace('\\', "/");
    for component in Path::new(&rel).components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid asset path `{rel}`"),
                ));
            }
        }
    }
    Ok(rel)
}

/// A read-only source of project assets.
pub trait AssetSource: Send + Sync {
    /// Read the bytes at a project-relative path (`images/bg.png`).
    fn read(&self, rel: &str) -> io::Result<Vec<u8>>;

    /// Whether `rel` exists in this source.
    fn exists(&self, rel: &str) -> bool;

    /// Read `rel`, falling back to `rel.<ext>` for each candidate
    /// extension. Returns the resolved path and its bytes.
    fn read_any(&self, rel: &str, exts: &[&str]) -> io::Result<(String, Vec<u8>)> {
        if self.exists(rel) {
            return self.read(rel).map(|b| (rel.to_string(), b));
        }
        for ext in exts {
            let candidate = format!("{rel}.{ext}");
            if self.exists(&candidate) {
                return self.read(&candidate).map(|b| (candidate, b));
            }
        }
        Err(not_found(rel))
    }
}

/// Loose assets in a project directory (development mode).
pub struct DirAssets(pub PathBuf);

impl AssetSource for DirAssets {
    fn read(&self, rel: &str) -> io::Result<Vec<u8>> {
        std::fs::read(self.0.join(sanitize_rel(rel)?))
    }

    fn exists(&self, rel: &str) -> bool {
        sanitize_rel(rel)
            .map(|r| self.0.join(r).is_file())
            .unwrap_or(false)
    }
}
