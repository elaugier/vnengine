//! Build and read `.vnpkg` packages.
//!
//! A `.vnpkg` is a zip archive containing:
//!
//! ```text
//! manifest.json      format version, engine, title, asset index (sha256)
//! config.json        the project's EngineConfig
//! script/main.vnb    the compiled script (JSON-serialized AST)
//! <path>.vn          raw .vn sources actually parsed (entry + imports)
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
use vnengine_script::lint::{Diagnostic, LintOptions, Severity};
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
fn asset_dirs(config: &EngineConfig) -> [&str; 5] {
    [
        config.image_dir.as_str(),
        config.audio_dir.as_str(),
        config.voice_dir.as_str(),
        config.font_dir.as_str(),
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

/// A [`Diagnostic`] tied to the project-relative file it was found in.
#[derive(Debug, Clone, Serialize)]
pub struct FileDiagnostic {
    pub file: String,
    #[serde(flatten)]
    pub diagnostic: Diagnostic,
}

/// Structured result of [`check`]: the JSON shape emitted by
/// `vnengine-pack --check-only --json` and consumed by the editor
/// extension.
#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    /// `false` when any error-severity diagnostic was produced.
    pub ok: bool,
    /// Project title from `project.toml`, when it parsed.
    pub title: Option<String>,
    /// Number of labels in the entry script, when it parsed.
    pub labels: Option<usize>,
    /// Number of asset files found under the asset dirs.
    pub assets: usize,
    /// License/notice files found.
    pub licenses: usize,
    pub diagnostics: Vec<FileDiagnostic>,
}

/// Validate a project without writing a package: parses the config and
/// the entry script, lints the script against the on-disk asset set, and
/// collects everything into a [`CheckReport`]. Nothing here is fatal —
/// config/IO/parse failures come back as `error` diagnostics.
pub fn check(project: &Path) -> Result<CheckReport> {
    let mut diagnostics = Vec::new();
    let fail = |file: &str, e: &anyhow::Error| FileDiagnostic {
        file: file.to_string(),
        diagnostic: Diagnostic {
            severity: Severity::Error,
            message: e.to_string(),
            line: None,
            col: None,
            span: None,
        },
    };

    let config = match load_config(project) {
        Ok(c) => c,
        Err(e) => {
            diagnostics.push(fail("project.toml", &e));
            return Ok(CheckReport {
                ok: false,
                title: None,
                labels: None,
                assets: 0,
                licenses: 0,
                diagnostics,
            });
        }
    };

    let script_rel = config.entry_script.replace('\\', "/");
    let script_path = project.join(&config.entry_script);
    let source = match fs::read_to_string(&script_path) {
        Ok(s) => s,
        Err(e) => {
            diagnostics.push(fail(
                &script_rel,
                &anyhow::anyhow!("failed to read {}: {e}", script_path.display()),
            ));
            return Ok(CheckReport {
                ok: false,
                title: Some(config.title),
                labels: None,
                assets: 0,
                licenses: license_files(project).len(),
                diagnostics,
            });
        }
    };

    let mut assets = Vec::new();
    for dir in asset_dirs(&config) {
        collect_files(&project.join(dir), &mut |fs_path| {
            assets.push(fs_path.to_path_buf());
            Ok(())
        })?;
    }
    // Lint: normalized `dir/file` index, `/`-separated.
    let asset_set: std::collections::BTreeSet<String> = assets
        .iter()
        .filter_map(|p| {
            p.strip_prefix(project)
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
        })
        .collect();

    let mut labels = None;
    // Multi-file lint: resolve imports against the project root so labels
    // and assets referenced in imported files are checked too.
    let root = project.to_path_buf();
    let mut resolver = |resolved: &str, _cur: &str| -> Result<String, String> {
        let p = root.join(resolved);
        fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
    };
    let diags = vnengine_script::lint::check_source_multi(
        &source,
        &script_rel,
        &mut resolver,
        &config,
        Some(&asset_set),
        &LintOptions::default(),
    );
    if !diags.iter().any(|(_, d)| d.severity == Severity::Error) {
        let mut res2 = |resolved: &str, _cur: &str| -> Result<String, String> {
            let p = root.join(resolved);
            fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
        };
        labels = ScriptParser::parse_multi(&source, &script_rel, &mut res2)
            .ok()
            .map(|s| s.labels.len());
    }
    diagnostics.extend(diags.into_iter().map(|(file, d)| FileDiagnostic {
        file,
        diagnostic: d,
    }));

    let licenses = license_files(project);
    let ok = !diagnostics
        .iter()
        .any(|d| d.diagnostic.severity == Severity::Error);
    Ok(CheckReport {
        ok,
        title: Some(config.title),
        labels,
        assets: assets.len(),
        licenses: licenses.len(),
        diagnostics,
    })
}

/// Human-readable validation output built on [`check`]. Returns an error
/// when the report contains error-severity diagnostics.
pub fn validate(project: &Path) -> Result<()> {
    let report = check(project)?;
    for d in &report.diagnostics {
        let marker = match d.diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        println!("{marker}: {}", d.diagnostic.message);
        if let Some(line) = d.diagnostic.line {
            println!("  --> {}:{line}", d.file);
        }
    }
    if !report.ok {
        bail!("project validation failed");
    }
    let warnings = report
        .diagnostics
        .iter()
        .filter(|d| d.diagnostic.severity == Severity::Warning)
        .count();
    if warnings > 0 {
        println!("lint: {warnings} warning(s)");
    }
    println!("valid project: {}", report.title.as_deref().unwrap_or("?"));
    if let Some(labels) = report.labels {
        let config = load_config(project)?;
        println!(
            "script: {} ({labels} labels)",
            project.join(config.entry_script).display()
        );
    }
    println!("assets: {} files", report.assets);
    if report.licenses == 0 {
        println!("licenses: none found (add LICENSE/NOTICE files or a licenses/ dir)");
    } else {
        println!("licenses: {} files", report.licenses);
    }
    Ok(())
}

/// Build a `.vnpkg` package for a project. The output is deterministic:
/// entries are sorted and timestamps fixed at the zip epoch.
pub fn build(project: &Path, output: &Path) -> Result<()> {
    let config = load_config(project)?;
    let entry_rel =
        vnengine_script::parser::normalize_rel_path(&config.entry_script.replace('\\', "/"));
    let script_path = project.join(&config.entry_script);
    let source = fs::read_to_string(&script_path)
        .with_context(|| format!("failed to read {}", script_path.display()))?;
    // Multi-file parse: resolve `import` directives against the project
    // root. The serialized .vnb is the merged AST, so packages run without
    // re-parsing. The raw .vn sources the parse actually read are shipped
    // alongside under their project-relative path, for traceability.
    let root = project.to_path_buf();
    let mut imported: Vec<String> = Vec::new();
    let mut resolver = |resolved: &str, _cur: &str| -> Result<String, String> {
        if !imported.iter().any(|r| r == resolved) {
            imported.push(resolved.to_string());
        }
        let p = root.join(resolved);
        fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
    };
    let script = ScriptParser::parse_multi(&source, &entry_rel, &mut resolver)
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
    // Ship only the .vn sources the parse actually read (entry + resolved
    // imports), under their project-relative paths. `or_insert` keeps a
    // stray `.vn` import from clobbering a real member (e.g. assets/).
    members
        .entry(entry_rel.clone())
        .or_insert_with(|| source.into_bytes());
    for rel in &imported {
        let path = root.join(rel);
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        members.entry(rel.clone()).or_insert(bytes);
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
