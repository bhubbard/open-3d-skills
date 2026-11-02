//! Standalone binary entrypoint for `skills/motion-assets/scripts/paste_back.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::paste_back::{run, PasteBackArgs};

fn main() -> Result<()> {
    run(PasteBackArgs::parse())
}
