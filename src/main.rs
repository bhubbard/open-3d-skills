//! Unified CLI for Open 3D Skills.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "open-3d-skills",
    version,
    about = "Native Rust tools for Open 3D Skills (three.js scenes, scroll films, alpha cutouts, cursor rigs, frame capture, and site checks)."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Put a cut-out subject on a flat key plate (alpha-cutout)
    Plate(open_3d_skills::plate::PlateArgs),
    /// Key a generated clip shot on a flat colour plate into real alpha (alpha-cutout)
    Key(open_3d_skills::key::KeyArgs),
    /// Build a cursor-following head rig from head-turn clips (cursor-rig)
    BuildRig(open_3d_skills::build_rig::BuildRigArgs),
    /// Check and fix a generated clip that should loop seamlessly (motion-assets)
    LoopCheck(open_3d_skills::loop_check::LoopCheckArgs),
    /// Put original pixels back into an outpainted or AI-extended image (motion-assets)
    PasteBack(open_3d_skills::paste_back::PasteBackArgs),
    /// Turn rendered clips into scroll-film WebP sequences, scrub MP4, or check seams (scroll-film)
    Prep(open_3d_skills::prep::PrepArgs),
    /// Put two clips against each other, stacked or side-by-side, with labels (frame-capture)
    Compare(open_3d_skills::compare::CompareArgs),
    /// Record frame-exact video of any web page on a virtual clock (frame-capture)
    Record(open_3d_skills::record::RecordArgs),
    /// Static file server with HTTP Range support (frame-capture / site-check)
    Serve(open_3d_skills::serve::ServeArgs),
    /// Check a 3D or animated page in real headless browsers and build a contact sheet (site-check)
    Check(open_3d_skills::site_check::SiteCheckArgs),
    /// Verify all skills, strict YAML frontmatters, plugin manifests, and script references
    Verify {
        #[arg(default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Plate(args) => open_3d_skills::plate::run(args),
        Commands::Key(args) => open_3d_skills::key::run(args),
        Commands::BuildRig(args) => open_3d_skills::build_rig::run(args),
        Commands::LoopCheck(args) => open_3d_skills::loop_check::run(args),
        Commands::PasteBack(args) => open_3d_skills::paste_back::run(args),
        Commands::Prep(args) => open_3d_skills::prep::run(args),
        Commands::Compare(args) => open_3d_skills::compare::run(args),
        Commands::Record(args) => open_3d_skills::record::run(args),
        Commands::Serve(args) => open_3d_skills::serve::run(args),
        Commands::Check(args) => open_3d_skills::site_check::run(args),
        Commands::Verify { root } => {
            open_3d_skills::verify::verify_repo(&root)?;
            Ok(())
        }
    }
}
