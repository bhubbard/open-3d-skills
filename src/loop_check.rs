//! Check and fix a generated clip that should loop seamlessly.

use crate::ffmpeg::{read_rgb_frames, spawn_ffmpeg_pipe};
use crate::imgproc::resize_rgb;
use anyhow::{bail, Context, Result};
use clap::Parser;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(about = "Check and fix a generated clip that should loop seamlessly.")]
pub struct LoopCheckArgs {
    /// Input video clip
    pub clip: PathBuf,
    /// Write a trimmed (and optionally blended) loop here
    #[arg(long)]
    pub fix: Option<PathBuf>,
    /// Cross-fade this many final frames into the opening
    #[arg(long, default_value_t = 0)]
    pub blend: usize,
    /// Seconds at the end to search for the loop point
    #[arg(long, default_value_t = 1.0)]
    pub search: f64,
    /// H.264 CRF quality factor
    #[arg(long, default_value_t = 18)]
    pub crf: u8,
}

pub fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 255.0;
    }
    let sum: u64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| (x as i32 - y as i32).unsigned_abs() as u64)
        .sum();
    (sum as f64) / (a.len() as f64)
}

pub fn run(args: LoopCheckArgs) -> Result<()> {
    let (w, h, fps, full) = read_rgb_frames(&args.clip)?;
    let n = full.len();
    if n < 4 {
        bail!("clip too short");
    }

    let sw = 320usize;
    let sh = (((h as f64) * (sw as f64) / (w as f64)).round() as usize).max(1);
    let small: Vec<Vec<u8>> = full
        .iter()
        .map(|f| resize_rgb(f, w, h, sw, sh))
        .collect();

    let as_is = mean_abs_diff(&small[n - 1], &small[0]);
    let search_frames = (args.search * fps).round() as usize;
    let lo = (n - 1).saturating_sub(search_frames).max(1);

    let mut best_d = f64::INFINITY;
    let mut best_i = n - 1;
    for (i, frame) in small.iter().enumerate().skip(lo) {
        let d = mean_abs_diff(frame, &small[0]);
        if d < best_d {
            best_d = d;
            best_i = i;
        }
    }

    let last = best_i - 1;
    let step_cnt = (n - 1).min(12);
    let step: f64 = (0..step_cnt)
        .map(|i| mean_abs_diff(&small[i], &small[i + 1]))
        .sum::<f64>()
        / (step_cnt.max(1) as f64);

    println!("{} frames at {} fps", n, fps);
    println!(
        "as is:       last -> first {:5.2}   (typical frame-to-frame change {:.2})",
        as_is, step
    );
    println!(
        "best cut:    frame {} matches frame 0 at {:5.2}; loop = frames 0..{} ({:.2}s)",
        best_i,
        best_d,
        last,
        ((last + 1) as f64) / fps
    );
    let verdict = if best_d < 3.0 {
        "seamless"
    } else if best_d < 8.0 {
        "visible on flat areas"
    } else {
        "jumps: regenerate, or --blend"
    };
    println!("verdict:     {}", verdict);

    let tail = if last > 6 {
        let t_start = last.saturating_sub(6);
        let cnt = (last - t_start).max(1);
        (t_start..last)
            .map(|i| mean_abs_diff(&small[i], &small[i + 1]))
            .sum::<f64>()
            / (cnt as f64)
    } else {
        step
    };

    if step > 0.5 && tail < step * 0.35 {
        println!(
            "note:        motion slows to {:.2} at the end (from {:.2}); the loop will hitch. Ask for constant motion.",
            tail, step
        );
    }

    if let Some(ref fix_path) = args.fix {
        let mut out_frames: Vec<Vec<u8>> = full[..=last].to_vec();
        let k = args.blend.min(out_frames.len() / 3);
        if k > 0 {
            let total_len = out_frames.len();
            let body: Vec<Vec<u8>> = out_frames[k..(total_len - k)].to_vec();
            let tail_slice = &out_frames[(total_len - k)..];
            let mut mixed = Vec::with_capacity(k);
            for (j, tail_frame) in tail_slice.iter().enumerate() {
                let alpha = (j + 1) as f32 / (k + 1) as f32;
                let open_frame = &full[j];
                let mut m = vec![0u8; tail_frame.len()];
                for idx in 0..m.len() {
                    let v = (tail_frame[idx] as f32) * (1.0 - alpha)
                        + (open_frame[idx] as f32) * alpha;
                    m[idx] = v.round().clamp(0.0, 255.0) as u8;
                }
                mixed.push(m);
            }
            out_frames = body;
            out_frames.extend(mixed);
        }

        let size_arg = format!("{}x{}", w, h);
        let fps_arg = format!("{}", fps);
        let crf_arg = format!("{}", args.crf);
        let fix_str = fix_path.to_string_lossy().to_string();

        let mut proc = spawn_ffmpeg_pipe(&[
            "-v",
            "error",
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            &size_arg,
            "-r",
            &fps_arg,
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            &crf_arg,
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-an",
            &fix_str,
        ])?;

        {
            let stdin = proc.stdin.as_mut().context("missing ffmpeg stdin")?;
            for f in &out_frames {
                stdin.write_all(f)?;
            }
        }
        let status = proc.wait()?;
        if !status.success() {
            bail!("ffmpeg failed writing {}", fix_path.display());
        }
        let suffix = if k > 0 {
            format!(", last {} blended", k)
        } else {
            String::new()
        };
        println!(
            "wrote {}: {} frames{}",
            fix_path.display(),
            out_frames.len(),
            suffix
        );
    }

    Ok(())
}
