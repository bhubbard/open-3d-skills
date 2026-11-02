//! Standalone binary entrypoint for `skills/alpha-cutout/scripts/plate.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::plate::{run, PlateArgs};

fn main() -> Result<()> {
    run(PlateArgs::parse())
}
