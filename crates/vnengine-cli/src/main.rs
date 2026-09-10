use clap::Parser;
use std::path::PathBuf;
use vnengine_core::EngineConfig;

/// vnengine - a cross-platform visual novel engine.
#[derive(Parser, Debug)]
#[command(name = "vnengine", version, about)]
struct Cli {
    /// Path to the project directory (containing project.toml).
    #[arg(short, long, default_value = ".")]
    project: PathBuf,

    /// Run in fullscreen mode.
    #[arg(short = 'f', long)]
    fullscreen: bool,

    /// Enable verbose logging.
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.verbose {
        env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Debug)
            .init();
    } else {
        env_logger::Builder::from_default_env()
            .filter_level(log::LevelFilter::Warn)
            .init();
    }

    // Load project config.
    let config_path = cli.project.join("project.toml");
    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", config_path.display()))?;
    let mut config: EngineConfig = toml::from_str(&config_str)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", config_path.display()))?;

    if cli.fullscreen {
        config.fullscreen = true;
    }

    log::info!("Loading project from {}", cli.project.display());
    log::info!("Entry script: {}", config.entry_script);

    vnengine_app::App::run(config, cli.project)?;

    Ok(())
}
