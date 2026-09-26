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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.check_only {
        return vnengine_pack::validate(&cli.project);
    }
    let output = cli.output.unwrap_or_else(|| cli.project.join("game.vnpkg"));
    vnengine_pack::build(&cli.project, &output)?;
    println!("created {}", output.display());
    Ok(())
}
