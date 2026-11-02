//! Standalone binary entrypoint for `skills/frame-capture/scripts/record.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::record::{run, RecordArgs};

fn main() -> Result<()> {
    run(RecordArgs::parse())
}
