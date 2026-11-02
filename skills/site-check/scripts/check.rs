//! Standalone binary entrypoint for `skills/site-check/scripts/check.rs`.

use anyhow::Result;
use clap::Parser;
use open_3d_skills::site_check::{run, SiteCheckArgs};

fn main() -> Result<()> {
    run(SiteCheckArgs::parse())
}
