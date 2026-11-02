//! Standalone binary entrypoint for `skills/scroll-film/scripts/prep.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::prep::{run, PrepArgs};

fn main() -> Result<()> {
    run(PrepArgs::parse())
}
