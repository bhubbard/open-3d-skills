//! Standalone binary entrypoint for `skills/cursor-rig/scripts/build_rig.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::build_rig::{run, BuildRigArgs};

fn main() -> Result<()> {
    run(BuildRigArgs::parse())
}
