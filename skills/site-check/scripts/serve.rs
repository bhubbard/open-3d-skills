//! Standalone binary entrypoint for `skills/site-check/scripts/serve.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::serve::{run, ServeArgs};

fn main() -> Result<()> {
    run(ServeArgs::parse())
}
