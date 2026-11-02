//! Standalone binary entrypoint for `skills/frame-capture/scripts/compare.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::compare::{run, CompareArgs};

fn main() -> Result<()> {
    run(CompareArgs::parse())
}
