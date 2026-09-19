use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use vnengine_core::EngineConfig;
use vnengine_script::Parser as ScriptParser;
use zip::write::SimpleFileOptions;

#[derive(Parser, Debug)]
#[command(
    name = "vnengine-pack",
    about = "Compile and package a vnengine project"
)]
struct Cli {
    #[arg(short, long)]
    project: PathBuf,
    #[arg(short, long)]
    output: PathBuf,
    #[arg(long)]
    check_only: bool,
}

#[derive(Serialize)]
struct Manifest<'a> {
    format: u32,
    engine: &'static str,
    engine_version: &'static str,
    title: &'a str,
    entrypoint: &'static str,
    resolution: [u32; 2],
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.project.join("project.toml");
    let config: EngineConfig = toml::from_str(
        &fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?,
    )
    .with_context(|| format!("failed to parse {}", config_path.display()))?;

    let script_path = cli.project.join(&config.entry_script);
    let source = fs::read_to_string(&script_path)
        .with_context(|| format!("failed to read {}", script_path.display()))?;
    let script = ScriptParser::parse(&source)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", script_path.display()))?;
    let script_bytes = serde_json::to_vec_pretty(&script)?;
    let manifest = Manifest {
        format: 1,
        engine: "vnengine",
        engine_version: env!("CARGO_PKG_VERSION"),
        title: &config.title,
        entrypoint: "script/main.vnb",
        resolution: [config.width, config.height],
    };

    if cli.check_only {
        println!("valid project: {}", config.title);
        println!("parsed script: {}", script_path.display());
        return Ok(());
    }

    if let Some(parent) = cli.output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(&cli.output)
        .with_context(|| format!("failed to create {}", cli.output.display()))?;
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    add_bytes(
        &mut archive,
        "manifest.json",
        &serde_json::to_vec_pretty(&manifest)?,
        options,
    )?;
    add_bytes(&mut archive, "script/main.vnb", &script_bytes, options)?;
    add_directory(
        &mut archive,
        &cli.project,
        &config.image_dir,
        "assets/images",
        options,
    )?;
    add_directory(
        &mut archive,
        &cli.project,
        &config.audio_dir,
        "assets/audio",
        options,
    )?;
    add_directory(
        &mut archive,
        &cli.project,
        &config.voice_dir,
        "assets/voice",
        options,
    )?;
    archive.finish()?.flush()?;
    println!("created {}", cli.output.display());
    Ok(())
}

fn add_bytes<W: Write + std::io::Seek>(
    archive: &mut zip::ZipWriter<W>,
    name: &str,
    bytes: &[u8],
    options: SimpleFileOptions,
) -> Result<()> {
    archive.start_file(name, options)?;
    archive.write_all(bytes)?;
    Ok(())
}

fn add_directory<W: Write + std::io::Seek>(
    archive: &mut zip::ZipWriter<W>,
    root: &Path,
    source_dir: &str,
    target_dir: &str,
    options: SimpleFileOptions,
) -> Result<()> {
    let dir = root.join(source_dir);
    if !dir.exists() {
        return Ok(());
    }
    for entry in walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(&dir)?;
        let name = format!("{target_dir}/{}", rel.to_string_lossy().replace('\\', "/"));
        let bytes = fs::read(entry.path())?;
        add_bytes(archive, &name, &bytes, options)?;
    }
    Ok(())
}
