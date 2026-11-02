//! Standalone binary entrypoint for `skills/motion-assets/scripts/loop_check.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::loop_check::{run, LoopCheckArgs};

fn main() -> Result<()> {
    run(LoopCheckArgs::parse())
}
