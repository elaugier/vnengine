//! Build and read `.vnpkg` packages.
//!
//! A `.vnpkg` is a zip archive containing:
//!
//! ```text
//! manifest.json      format version, engine, title, asset index (sha256)
//! config.json        the project's EngineConfig
//! script/main.vnb    the compiled script (JSON-serialized AST)
//! assets/<dir>/...   images, audio, voice — mirroring the project dirs
//! licenses/...       license and notice files, if present in the project
//! ```
//!
//! Writing is deterministic: entries are sorted and timestamps fixed, so
//! building twice from the same tree produces byte-identical archives.
//! Reading validates paths (no traversal or duplicates) and size limits
//! before anything is decoded.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use vnengine_core::assets::{sanitize_rel, AssetSource};
use vnengine_core::EngineConfig;
use vnengine_script::ast::Script;
use vnengine_script::Parser as ScriptParser;
use zip::write::SimpleFileOptions;

/// Package format version written by [`build`].
pub const MANIFEST_FORMAT: u32 = 2;

/// Archive safety limits applied on [`Package::open`].
const MAX_ENTRIES: usize = 100_000;
const MAX_ENTRY_SIZE: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_SIZE: u64 = 4 * 1024 * 1024 * 1024;

/// Archive path of the compiled script.
const SCRIPT_ENTRY: &str = "script/main.vnb";

/// Package manifest: metadata plus the asset index with hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub engine: String,
    pub engine_version: String,
    pub title: String,
    pub entrypoint: String,
    pub resolution: [u32; 2],
    /// Asset index: archive path, uncompressed size and sha256.
    pub assets: Vec<AssetEntry>,
    /// License/notice files included in the package.
    pub licenses: Vec<String>,
    /// Plugin files included in the package (`plugins/*.wasm|.toml`).
    #[serde(default)]
    pub plugins: Vec<String>,
}

/// One file in the manifest's asset index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetEntry {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// A `.vnpkg` archive loaded for execution.
#[derive(Debug)]
pub struct Package {
    files: BTreeMap<String, Vec<u8>>,
    manifest: Manifest,
    config: EngineConfig,
    script: Script,
}

impl Package {
    /// Open and validate a `.vnpkg` file. Fails on traversal paths,
    /// duplicate entries, size-limit violations or missing members.
    pub fn open(path: &Path) -> Result<Self> {
        let file =
            File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("{} is not a valid archive", path.display()))?;
        if archive.len() > MAX_ENTRIES {
            bail!("package has {} entries (max {MAX_ENTRIES})", archive.len());
        }
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let Some(enclosed) = entry.enclosed_name() else {
                bail!("entry #{i} has an unsafe path (traversal or absolute)");
            };
            let name = enclosed.to_string_lossy().replace('\\', "/");
            if entry.is_dir() {
                continue;
            }
            if files.contains_key(&name) {
                bail!("duplicate archive entry `{name}`");
            }
            total = total.saturating_add(entry.size());
            if total > MAX_TOTAL_SIZE {
                bail!("package exceeds the {MAX_TOTAL_SIZE}-byte size limit");
            }
            // Cap the read at the per-entry limit regardless of the
            // declared size, to bound decompression.
            let mut buf = Vec::new();
            let read = entry
                .by_ref()
                .take(MAX_ENTRY_SIZE + 1)
                .read_to_end(&mut buf)?;
            if read as u64 > MAX_ENTRY_SIZE {
                bail!("entry `{name}` exceeds the {MAX_ENTRY_SIZE}-byte limit");
            }
            files.insert(name, buf);
        }

        let get = |name: &str| -> Result<&Vec<u8>> {
            files
                .get(name)
                .with_context(|| format!("package is missing `{name}`"))
        };
        let manifest: Manifest =
            serde_json::from_slice(get("manifest.json")?).context("invalid manifest.json")?;
        if manifest.format != MANIFEST_FORMAT {
            bail!(
                "unsupported package format {} (expected {MANIFEST_FORMAT})",
                manifest.format
            );
        }
        if manifest.engine != "vnengine" {
            bail!("package targets `{}`, not vnengine", manifest.engine);
        }
        let config: EngineConfig =
            serde_json::from_slice(get("config.json")?).context("invalid config.json")?;
        let script: Script =
            serde_json::from_slice(get(SCRIPT_ENTRY)?).context("invalid compiled script")?;
        Ok(Self {
            files,
            manifest,
            config,
            script,
        })
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }
    pub fn script(&self) -> &Script {
        &self.script
    }
}

/// Project-relative paths resolve to `assets/<rel>` inside the package.
impl AssetSource for Package {
    fn read(&self, rel: &str) -> io::Result<Vec<u8>> {
        let rel = sanitize_rel(rel)?;
        self.files
            .get(&format!("assets/{rel}"))
            .cloned()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("asset not found: `{rel}`"))
            })
    }

    fn exists(&self, rel: &str) -> bool {
        sanitize_rel(rel)
            .map(|r| self.files.contains_key(&format!("assets/{r}")))
            .unwrap_or(false)
    }

    fn list(&self, prefix: &str) -> Vec<String> {
        let Ok(prefix) = sanitize_rel(prefix) else {
            return Vec::new();
        };
        let full = format!("assets/{prefix}/");
        self.files
            .keys()
            .filter(|k| k.starts_with(&full))
            .map(|k| k["assets/".len()..].to_string())
            .collect()
    }
}

/// Asset directories mirrored into `assets/` inside the package.
fn asset_dirs(config: &EngineConfig) -> [&str; 4] {
    [
        config.image_dir.as_str(),
        config.audio_dir.as_str(),
        config.voice_dir.as_str(),
        "translations",
    ]
}

/// Load and validate `project.toml` for a project directory.
pub fn load_config(project: &Path) -> Result<EngineConfig> {
    let config_path = project.join("project.toml");
    toml::from_str(
        &fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", config_path.display()))
}

/// Validate a project without writing a package: parses the config and
/// the entry script, and reports asset/license file counts.
pub fn validate(project: &Path) -> Result<()> {
    let config = load_config(project)?;
    let script_path = project.join(&config.entry_script);
    let source = fs::read_to_string(&script_path)
        .with_context(|| format!("failed to read {}", script_path.display()))?;
    let script = ScriptParser::parse(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", script_path.display()))?;

    let mut assets = Vec::new();
    for dir in asset_dirs(&config) {
        collect_files(&project.join(dir), &mut |fs_path| {
            assets.push(fs_path.to_path_buf());
            Ok(())
        })?;
    }
    let licenses = license_files(project);
    println!("valid project: {}", config.title);
    println!(
        "script: {} ({} labels)",
        script_path.display(),
        script.labels.len()
    );
    println!("assets: {} files", assets.len());
    if licenses.is_empty() {
        println!("licenses: none found (add LICENSE/NOTICE files or a licenses/ dir)");
    } else {
        println!("licenses: {} files", licenses.len());
    }
    Ok(())
}

/// Build a `.vnpkg` package for a project. The output is deterministic:
/// entries are sorted and timestamps fixed at the zip epoch.
pub fn build(project: &Path, output: &Path) -> Result<()> {
    let config = load_config(project)?;
    let script_path = project.join(&config.entry_script);
    let source = fs::read_to_string(&script_path)
        .with_context(|| format!("failed to read {}", script_path.display()))?;
    let script = ScriptParser::parse(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", script_path.display()))?;
    let script_bytes = serde_json::to_vec_pretty(&script)?;
    let config_bytes = serde_json::to_vec_pretty(&config)?;

    // Collect assets sorted by archive name for deterministic output.
    let mut members: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for dir in asset_dirs(&config) {
        let root = project.join(dir);
        collect_files(&root, &mut |fs_path| {
            let rel = fs_path
                .strip_prefix(&root)
                .expect("walked path is under root")
                .to_string_lossy()
                .replace('\\', "/");
            members.insert(format!("assets/{dir}/{rel}"), fs::read(fs_path)?);
            Ok(())
        })?;
    }
    // WASM plugins + their manifests ship under assets/plugins/.
    let plugins_root = project.join("plugins");
    let mut plugins = Vec::new();
    collect_files(&plugins_root, &mut |fs_path| {
        let rel = fs_path
            .strip_prefix(&plugins_root)
            .expect("walked path is under root")
            .to_string_lossy()
            .replace('\\', "/");
        if rel.ends_with(".wasm") || rel.ends_with(".wat") || rel.ends_with(".toml") {
            let key = format!("assets/plugins/{rel}");
            members.insert(key.clone(), fs::read(fs_path)?);
            plugins.push(key);
        }
        Ok(())
    })?;
    let mut licenses = Vec::new();
    for fs_path in license_files(project) {
        let name = fs_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let key = format!("licenses/{name}");
        members.insert(key.clone(), fs::read(&fs_path)?);
        licenses.push(key);
    }

    let mut index: Vec<AssetEntry> = members
        .iter()
        .map(|(path, bytes)| AssetEntry {
            path: path.clone(),
            size: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        })
        .collect();
    index.sort_by(|a, b| a.path.cmp(&b.path));

    let manifest = Manifest {
        format: MANIFEST_FORMAT,
        engine: "vnengine".into(),
        engine_version: env!("CARGO_PKG_VERSION").into(),
        title: config.title.clone(),
        entrypoint: SCRIPT_ENTRY.into(),
        resolution: [config.width, config.height],
        assets: index,
        licenses,
        plugins,
    };

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file =
        File::create(output).with_context(|| format!("failed to create {}", output.display()))?;
    let mut archive = zip::ZipWriter::new(file);
    // Fixed timestamp: reproducible output for identical inputs.
    let epoch = zip::DateTime::default();
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(epoch);
    write_member(
        &mut archive,
        "manifest.json",
        &serde_json::to_vec_pretty(&manifest)?,
        options,
    )?;
    write_member(&mut archive, "config.json", &config_bytes, options)?;
    write_member(&mut archive, SCRIPT_ENTRY, &script_bytes, options)?;
    for (name, bytes) in &members {
        write_member(&mut archive, name, bytes, options)?;
    }
    archive.finish()?.flush()?;
    Ok(())
}

fn write_member<W: Write + io::Seek>(
    archive: &mut zip::ZipWriter<W>,
    name: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<()> {
    archive.start_file(name, options)?;
    archive.write_all(bytes)?;
    Ok(())
}

/// Walk `dir`, calling `f` on each file in sorted order. Missing dirs are
/// skipped silently.
fn collect_files(dir: &Path, f: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut paths: Vec<_> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    paths.sort();
    for path in paths {
        f(&path)?;
    }
    Ok(())
}

/// License/notice files to embed: everything under `licenses/` plus
/// top-level LICENSE*/NOTICE*/COPYING* files.
fn license_files(project: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let _ = collect_files(&project.join("licenses"), &mut |p| {
        out.push(p.to_path_buf());
        Ok(())
    });
    if let Ok(entries) = fs::read_dir(project) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_uppercase();
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                && (name.starts_with("LICENSE")
                    || name.starts_with("NOTICE")
                    || name.starts_with("COPYING"))
            {
                out.push(entry.path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests;
