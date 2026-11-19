use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "vnengine-pack",
    about = "Compile and package a vnengine project"
)]
struct Cli {
    #[arg(short, long)]
    project: PathBuf,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(long)]
    check_only: bool,
    /// With `--check-only`: emit the report as JSON on stdout.
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.check_only {
        if cli.json {
            let report = vnengine_pack::check(&cli.project)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report.ok {
                return Ok(());
            }
            std::process::exit(1);
        }
        return vnengine_pack::validate(&cli.project);
    }
    let output = cli.output.unwrap_or_else(|| cli.project.join("game.vnpkg"));
    vnengine_pack::build(&cli.project, &output)?;
    println!("created {}", output.display());
    Ok(())
}
