//! Turn rendered clips into scroll-film assets: WebP frame sequences and scrub-ready MP4.

use crate::imgproc::resize_rgb;
use crate::loop_check::mean_abs_diff;
use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser, Debug, Clone)]
#[command(about = "Turn rendered clips into scroll-film assets: WebP frame sequences and scrub-ready MP4.")]
pub struct PrepArgs {
    #[command(subcommand)]
    pub cmd: PrepCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum PrepCommand {
    /// Extracts every leg in order into one numbered WebP sequence (000.webp ...) plus manifest.json
    Frames {
        /// Input leg clips followed by output directory as the final positional argument
        #[arg(required = true, num_args = 2..)]
        legs_and_out: Vec<PathBuf>,
        #[arg(long, default_value_t = 1600)]
        width: usize,
        #[arg(long, default_value_t = 24.0)]
        fps: f64,
        #[arg(long, default_value_t = 78)]
        quality: u8,
        /// Keep each later leg's first frame
        #[arg(long)]
        keep_joins: bool,
    },
    /// Joins the legs into one MP4 with a short GOP and no B-frames
    Video {
        /// Input leg clips followed by output MP4 path as the final positional argument
        #[arg(required = true, num_args = 2..)]
        legs_and_out: Vec<PathBuf>,
        #[arg(long, default_value_t = 1920)]
        width: usize,
        #[arg(long, default_value_t = 24.0)]
        fps: f64,
        #[arg(long, default_value_t = 4)]
        gop: usize,
        #[arg(long, default_value_t = 20)]
        crf: u8,
    },
    /// Measures how well each leg's last frame matches the next leg's first frame
    Seams {
        #[arg(required = true, num_args = 2..)]
        legs: Vec<PathBuf>,
    },
}

fn run_ffmpeg(args: &[&str]) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .context("failed to execute ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg exited with {}", status);
    }
    Ok(())
}

fn extract_last_frame(path: &Path, out: &Path) -> Result<()> {
    run_ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-sseof",
        "-0.25",
        "-i",
        &path.to_string_lossy(),
        "-update",
        "1",
        "-q:v",
        "1",
        &out.to_string_lossy(),
    ])
}

fn extract_first_frame(path: &Path, out: &Path) -> Result<()> {
    run_ffmpeg(&[
        "-v",
        "error",
        "-y",
        "-i",
        &path.to_string_lossy(),
        "-frames:v",
        "1",
        "-q:v",
        "1",
        &out.to_string_lossy(),
    ])
}

pub fn run(args: PrepArgs) -> Result<()> {
    match args.cmd {
        PrepCommand::Frames {
            mut legs_and_out,
            width,
            fps,
            quality: _quality,
            keep_joins,
        } => {
            let out = legs_and_out.pop().context("missing OUT_DIR")?;
            let legs = legs_and_out;
            std::fs::create_dir_all(&out)?;
            let tmp = tempfile::tempdir()?;
            let mut n = 0usize;
            for (li, leg) in legs.iter().enumerate() {
                let d = tmp.path().join(format!("leg{}", li));
                std::fs::create_dir_all(&d)?;
                let vf = format!("fps={},scale={}:-2:flags=lanczos", fps, width);
                let pat = d.join("%05d.png").to_string_lossy().to_string();
                run_ffmpeg(&[
                    "-v",
                    "error",
                    "-y",
                    "-i",
                    &leg.to_string_lossy(),
                    "-vf",
                    &vf,
                    "-start_number",
                    "0",
                    &pat,
                ])?;
                let mut files: Vec<PathBuf> = std::fs::read_dir(&d)?
                    .filter_map(|e| e.ok().map(|x| x.path()))
                    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
                    .collect();
                files.sort();
                let start_idx = if li > 0 && !keep_joins && files.len() > 1 {
                    1
                } else {
                    0
                };
                for f in &files[start_idx..] {
                    let im = image::open(f)?;
                    im.save(out.join(format!("{:03}.webp", n)))?;
                    n += 1;
                }
            }
            if n == 0 {
                bail!("no frames extracted");
            }
            let first_webp = image::open(out.join("000.webp"))?;
            let (w, h) = (first_webp.width(), first_webp.height());
            let size: u64 = std::fs::read_dir(&out)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("webp"))
                .filter_map(|e| e.metadata().ok().map(|m| m.len()))
                .sum();
            let manifest = json!({
                "count": n,
                "width": w,
                "height": h,
                "fps": fps,
                "pattern": "{i:03d}.webp",
                "bytes": size
            });
            std::fs::write(out.join("manifest.json"), manifest.to_string())?;
            println!(
                "{}: {} frames {}x{}, {:.1} MB ({:.0} kB a frame)",
                out.display(),
                n,
                w,
                h,
                (size as f64) / 1e6,
                (size as f64) / (n as f64) / 1e3
            );
            Ok(())
        }
        PrepCommand::Video {
            mut legs_and_out,
            width,
            fps,
            gop,
            crf,
        } => {
            let out = legs_and_out.pop().context("missing OUT.mp4")?;
            let legs = legs_and_out;
            let tmp = tempfile::tempdir()?;
            let lst = tmp.path().join("list.txt");
            let mut content = String::new();
            for p in &legs {
                let abs = std::fs::canonicalize(p)?;
                content.push_str(&format!("file '{}'\n", abs.display()));
            }
            std::fs::write(&lst, content)?;
            let vf = format!("scale={}:-2:flags=lanczos,fps={}", width, fps);
            let crf_s = crf.to_string();
            let gop_s = gop.to_string();
            run_ffmpeg(&[
                "-v",
                "error",
                "-y",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
                &lst.to_string_lossy(),
                "-vf",
                &vf,
                "-c:v",
                "libx264",
                "-preset",
                "slow",
                "-crf",
                &crf_s,
                "-g",
                &gop_s,
                "-keyint_min",
                &gop_s,
                "-bf",
                "0",
                "-sc_threshold",
                "0",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
                "-an",
                &out.to_string_lossy(),
            ])?;
            let size = std::fs::metadata(&out)?.len();
            println!(
                "wrote {} (GOP {}, no B-frames, {:.1} MB)",
                out.display(),
                gop,
                (size as f64) / 1e6
            );
            Ok(())
        }
        PrepCommand::Seams { legs } => {
            let tmp = tempfile::tempdir()?;
            let mut worst = 0.0f64;
            for i in 0..(legs.len() - 1) {
                let l_path = tmp.path().join("l.png");
                let f_path = tmp.path().join("f.png");
                extract_last_frame(&legs[i], &l_path)?;
                extract_first_frame(&legs[i + 1], &f_path)?;
                let img_a = image::open(&l_path)?.to_rgb8();
                let img_b = image::open(&f_path)?.to_rgb8();
                let (aw, ah) = (img_a.width() as usize, img_a.height() as usize);
                let (bw, bh) = (img_b.width() as usize, img_b.height() as usize);
                let a_raw = img_a.into_raw();
                let b_raw = if (bw, bh) != (aw, ah) {
                    resize_rgb(&img_b.into_raw(), bw, bh, aw, ah)
                } else {
                    img_b.into_raw()
                };
                let d = mean_abs_diff(&a_raw, &b_raw);
                worst = worst.max(d);
                let verdict = if d < 4.0 {
                    "clean"
                } else if d > 10.0 {
                    "visible"
                } else {
                    "check by eye"
                };
                let n0 = legs[i]
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("leg0");
                let n1 = legs[i + 1]
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("leg1");
                println!("{} -> {}: {:.2}  {}", n0, n1, d, verdict);
            }
            if worst > 10.0 {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}
