//! Standalone binary entrypoint for `skills/alpha-cutout/scripts/key.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::key::{run, KeyArgs};

fn main() -> Result<()> {
    run(KeyArgs::parse())
}
