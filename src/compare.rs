//! Put two clips against each other, stacked or side by side, each with a label.

use crate::ffmpeg::probe_video;
use crate::imgproc::{draw_text_rgb, parse_hex_rgb, rounded_rect_mask};
use anyhow::{bail, Context, Result};
use clap::Parser;
use image::{Rgb, RgbImage};
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser, Debug, Clone)]
#[command(about = "Put two clips against each other, stacked or side by side, each with a label.")]
pub struct CompareArgs {
    /// First video clip
    pub a: PathBuf,
    /// Second video clip
    pub b: PathBuf,
    /// Output comparison MP4 path
    pub out: PathBuf,
    /// Labels for clip A and clip B
    #[arg(long, num_args = 2, default_values_t = vec!["A".to_string(), "B".to_string()])]
    pub labels: Vec<String>,
    /// Layout: stack | side
    #[arg(long, default_value = "stack")]
    pub layout: String,
    /// Panel width in px
    #[arg(long, default_value_t = 1080)]
    pub width: u32,
    /// Output frame rate
    #[arg(long, default_value_t = 30.0)]
    pub fps: f64,
    /// Background hex colour
    #[arg(long, default_value = "f1ece4")]
    pub bg: String,
    /// Label ink hex colour
    #[arg(long, default_value = "1c1a17")]
    pub ink: String,
    /// Panel corner radius in px
    #[arg(long, default_value_t = 24)]
    pub radius: u32,
    /// Gap between panels in px
    #[arg(long, default_value_t = 40)]
    pub gap: u32,
    /// Outer margin in px
    #[arg(long, default_value_t = 56)]
    pub margin: u32,
    /// Optional font path (ignored when built-in vector/bitmap font is used)
    #[arg(long, default_value = "")]
    pub font: String,
    /// H.264 CRF quality factor
    #[arg(long, default_value_t = 16)]
    pub crf: u8,
}

pub fn run(args: CompareArgs) -> Result<()> {
    let meta_a = probe_video(&args.a)?;
    let meta_b = probe_video(&args.b)?;

    let pw = (args.width / 2) * 2;
    let ph0 = (((pw as f64) * (meta_a.height as f64) / (meta_a.width as f64) / 2.0).round() as u32) * 2;
    let ph1 = (((pw as f64) * (meta_b.height as f64) / (meta_b.width as f64) / 2.0).round() as u32) * 2;
    let ph = [ph0.max(2), ph1.max(2)];

    let label_h = ((pw as f64) * 0.075).round() as u32;
    let duration = meta_a.duration.max(meta_b.duration).max(0.1);
    let m = args.margin;
    let g = args.gap;

    let (mut w, mut h, slots) = if args.layout == "stack" {
        let w = pw + 2 * m;
        let h = m + label_h + ph[0] + g + label_h + ph[1] + m;
        let slots = [(m, m + label_h), (m, m + label_h + ph[0] + g + label_h)];
        (w, h, slots)
    } else if args.layout == "side" {
        let w = 2 * pw + 2 * m + g;
        let h = m + label_h + ph[0].max(ph[1]) + m;
        let slots = [(m, m + label_h), (m + pw + g, m + label_h)];
        (w, h, slots)
    } else {
        bail!("invalid --layout {:?}; expected stack|side", args.layout);
    };
    w = (w / 2) * 2;
    h = (h / 2) * 2;

    let bg_rgb = parse_hex_rgb(&args.bg)?;
    let ink_rgb = parse_hex_rgb(&args.ink)?;

    let tmp = tempfile::tempdir()?;
    let base_path = tmp.path().join("base.png");
    let mut base = RgbImage::from_pixel(w, h, Rgb(bg_rgb));
    let scale = (label_h / 14).max(2);
    for ((sx, sy), text) in slots.iter().zip(args.labels.iter()) {
        let text_y = (*sy as i32) - ((label_h as f32 * 0.65) as i32);
        draw_text_rgb(&mut base, (*sx + 2) as i32, text_y, text, scale, ink_rgb);
    }
    base.save(&base_path)?;

    let mut mask_paths = Vec::with_capacity(2);
    for (i, &hgt) in ph.iter().enumerate() {
        let mk = rounded_rect_mask(pw, hgt, args.radius);
        let mp = tmp.path().join(format!("mask{}.png", i));
        mk.save(&mp)?;
        mask_paths.push(mp);
    }

    let mut fc = Vec::new();
    for i in 0..2 {
        fc.push(format!(
            "[{}:v]fps={},scale={}:{}:flags=lanczos,format=rgba,tpad=stop_mode=clone:stop_duration={:.3}[v{}]",
            i, args.fps, pw, ph[i], duration, i
        ));
        fc.push(format!("[{}:v]format=gray,loop=-1:1:0[m{}]", i + 3, i));
        fc.push(format!("[v{}][m{}]alphamerge[p{}]", i, i, i));
    }
    fc.push("[2:v]loop=-1:1:0[bg]".to_string());
    fc.push(format!(
        "[bg][p0]overlay={}:{}:shortest=0[t0]",
        slots[0].0, slots[0].1
    ));
    fc.push(format!(
        "[t0][p1]overlay={}:{}:shortest=0,format=yuv420p[out]",
        slots[1].0, slots[1].1
    ));

    let filter_complex = fc.join(";");
    let dur_str = format!("{:.3}", duration);
    let fps_str = format!("{}", args.fps);
    let crf_str = format!("{}", args.crf);

    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-i",
            &args.a.to_string_lossy(),
            "-i",
            &args.b.to_string_lossy(),
            "-i",
            &base_path.to_string_lossy(),
            "-i",
            &mask_paths[0].to_string_lossy(),
            "-i",
            &mask_paths[1].to_string_lossy(),
            "-filter_complex",
            &filter_complex,
            "-map",
            "[out]",
            "-t",
            &dur_str,
            "-r",
            &fps_str,
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            &crf_str,
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-an",
            &args.out.to_string_lossy(),
        ])
        .status()
        .context("failed to run ffmpeg for compare")?;

    if !status.success() {
        bail!("ffmpeg compare failed");
    }

    println!(
        "wrote {}: {}x{}, {:.2}s, {}",
        args.out.display(),
        w,
        h,
        duration,
        args.layout
    );
    Ok(())
}
