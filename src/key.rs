//! Key a generated clip shot on a flat colour plate into real alpha, frame by frame.

use crate::ffmpeg::{read_rgb_frames, spawn_ffmpeg_pipe};
use crate::imgproc::{
    dilate_binary_box, erode_3x3, gaussian_blur_f32, gaussian_blur_rgb_f32, median_f32,
    morph_close_3x3, morph_open_3x3, parse_hex_rgb, resize_gray_u8, resize_rgb, resize_rgba,
};
use crate::plate::KeyColor;
use anyhow::{bail, Context, Result};
use clap::Parser;
use image::RgbaImage;
use serde_json::json;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Parser, Debug, Clone)]
#[command(about = "Key a generated clip shot on a flat colour plate into real alpha, frame by frame.")]
pub struct KeyArgs {
    /// Input video clip or still image
    pub clip: PathBuf,
    /// H.264 mp4, colour on top, matte below
    #[arg(long)]
    pub stacked: Option<PathBuf>,
    /// VP9 webm with an alpha channel
    #[arg(long)]
    pub webm: Option<PathBuf>,
    /// Directory for an RGBA WebP sequence
    #[arg(long)]
    pub frames: Option<PathBuf>,
    /// Directory for full-size RGBA PNGs
    #[arg(long)]
    pub png: Option<PathBuf>,
    /// Key colour: magenta | green | blue
    #[arg(long, default_value = "magenta")]
    pub key: String,
    /// Comma-separated frame indices for --png
    #[arg(long, default_value = "0")]
    pub frame_list: String,
    /// Output width in pixels
    #[arg(long)]
    pub width: Option<usize>,
    /// Output frame rate override
    #[arg(long)]
    pub fps: Option<f64>,
    /// First source frame for --frames
    #[arg(long, default_value_t = 0)]
    pub start: usize,
    /// Last source frame for --frames
    #[arg(long)]
    pub end: Option<usize>,
    /// Frame step for --frames
    #[arg(long, default_value_t = 1)]
    pub step: usize,
    /// Comma list of source frames for --frames
    #[arg(long)]
    pub picks: Option<String>,
    /// WebP quality (0..100)
    #[arg(long, default_value_t = 82)]
    pub quality: u8,
    /// H.264 / VP9 CRF quality factor
    #[arg(long, default_value_t = 20)]
    pub crf: u8,
    /// Key excess at or below this is solid subject
    #[arg(long, default_value_t = 12.0)]
    pub solid: f32,
    /// Shave N px off the matte edge
    #[arg(long, default_value_t = 0)]
    pub erode: usize,
    /// Hex colour far from the subject under alpha 0
    #[arg(long, default_value = "808080")]
    pub fill: String,
}

fn parse_key_color(s: &str) -> Result<KeyColor> {
    match s {
        "magenta" => Ok(KeyColor::Magenta),
        "green" => Ok(KeyColor::Green),
        "blue" => Ok(KeyColor::Blue),
        other => bail!("invalid --key {other:?}; expected magenta|green|blue"),
    }
}

/// Measures the plate colour from the frame's top band and upper side columns.
pub fn plate_colour(rgb: &[u8], w: usize, h: usize) -> [f32; 3] {
    let b = (w / 100).max(4).min(w.min(h));
    let upper_h = ((h as f32) * 0.7) as usize;
    let mut rs = Vec::new();
    let mut gs = Vec::new();
    let mut bs = Vec::new();

    // Top band: y in 0..b
    for y in 0..b.min(h) {
        for x in 0..w {
            let idx = (y * w + x) * 3;
            rs.push(rgb[idx] as f32);
            gs.push(rgb[idx + 1] as f32);
            bs.push(rgb[idx + 2] as f32);
        }
    }
    // Left and right upper bands: y in 0..upper_h, x in 0..b and (w-b)..w
    for y in 0..upper_h.min(h) {
        for x in 0..b.min(w) {
            let idx = (y * w + x) * 3;
            rs.push(rgb[idx] as f32);
            gs.push(rgb[idx + 1] as f32);
            bs.push(rgb[idx + 2] as f32);
        }
        for x in w.saturating_sub(b)..w {
            let idx = (y * w + x) * 3;
            rs.push(rgb[idx] as f32);
            gs.push(rgb[idx + 1] as f32);
            bs.push(rgb[idx + 2] as f32);
        }
    }

    [
        median_f32(&mut rs),
        median_f32(&mut gs),
        median_f32(&mut bs),
    ]
}

/// Keys a single RGB frame `(w, h)` into `(unmixed_rgb, alpha_f32, plate_rgb)`.
pub fn key_frame(
    rgb: &[u8],
    w: usize,
    h: usize,
    key_col: KeyColor,
    solid: f32,
    erode_iters: usize,
) -> Result<(Vec<u8>, Vec<f32>, [f32; 3])> {
    let k = plate_colour(rgb, w, h);
    let k_ex = key_col.excess(k[0], k[1], k[2]);
    if k_ex < solid + 8.0 {
        bail!(
            "plate colour [{}, {}, {}] is not a {} plate (excess {:.0}); check --key, or the subject touches the top edge",
            k[0].round() as i32,
            k[1].round() as i32,
            k[2].round() as i32,
            key_col.as_str(),
            k_ex
        );
    }

    let high = 0.97 * k_ex;
    let denom = (high - solid).max(1.0);
    let n = w * h;
    let mut f = vec![0.0f32; n * 3];
    let mut a8 = vec![0u8; n];

    for i in 0..n {
        let r = rgb[i * 3] as f32;
        let g = rgb[i * 3 + 1] as f32;
        let b = rgb[i * 3 + 2] as f32;
        let ex = key_col.excess(r, g, b);
        let a = ((high - ex) / denom).clamp(0.0, 1.0);
        let ka = a.max(0.25);
        if a < 0.999 {
            f[i * 3] = ((r - (1.0 - ka) * k[0]) / ka).clamp(0.0, 255.0);
            f[i * 3 + 1] = ((g - (1.0 - ka) * k[1]) / ka).clamp(0.0, 255.0);
            f[i * 3 + 2] = ((b - (1.0 - ka) * k[2]) / ka).clamp(0.0, 255.0);
        } else {
            f[i * 3] = r;
            f[i * 3 + 1] = g;
            f[i * 3 + 2] = b;
        }
        a8[i] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    a8 = morph_open_3x3(&a8, w, h);
    a8 = morph_close_3x3(&a8, w, h);
    for _ in 0..erode_iters {
        a8 = erode_3x3(&a8, w, h, true);
    }

    let af: Vec<f32> = a8.iter().map(|&v| v as f32 / 255.0).collect();
    let a_blur = gaussian_blur_f32(&af, w, h, 0.6);

    let (spill_ch, (o0, o1), mult) = match key_col {
        KeyColor::Magenta => (2usize, (1usize, 1usize), 2.0f32),
        KeyColor::Green => (1usize, (0usize, 2usize), 1.0f32),
        KeyColor::Blue => (2usize, (0usize, 1usize), 1.0f32),
    };

    let mut out_rgb = vec![0u8; n * 3];
    for i in 0..n {
        let a = a_blur[i];
        if a < 0.999 {
            let cap = f[i * 3 + o0].max(f[i * 3 + o1]) + solid * mult;
            f[i * 3 + spill_ch] = f[i * 3 + spill_ch].min(cap);
        }
        out_rgb[i * 3] = f[i * 3].round().clamp(0.0, 255.0) as u8;
        out_rgb[i * 3 + 1] = f[i * 3 + 1].round().clamp(0.0, 255.0) as u8;
        out_rgb[i * 3 + 2] = f[i * 3 + 2].round().clamp(0.0, 255.0) as u8;
    }

    Ok((out_rgb, a_blur, k))
}

/// Pushes the subject's edge colour outward under transparent pixels to prevent chroma subsampling bleed.
pub fn under_colour(col: &[u8], a: &[f32], w: usize, h: usize, fill: [u8; 3]) -> Vec<u8> {
    let n = w * h;
    let mut fw = vec![0.0f32; n * 3];
    for i in 0..n {
        let wi = a[i];
        fw[i * 3] = (col[i * 3] as f32) * wi;
        fw[i * 3 + 1] = (col[i * 3 + 1] as f32) * wi;
        fw[i * 3 + 2] = (col[i * 3 + 2] as f32) * wi;
    }

    let mut out = vec![0.0f32; n * 3];
    for i in 0..n {
        out[i * 3] = fill[0] as f32;
        out[i * 3 + 1] = fill[1] as f32;
        out[i * 3 + 2] = fill[2] as f32;
    }
    let mut have = vec![false; n];

    for sigma in [3.0f32, 10.0f32] {
        let den = gaussian_blur_f32(a, w, h, sigma);
        let num = gaussian_blur_rgb_f32(&fw, w, h, sigma);
        for i in 0..n {
            if den[i] > 1e-3 && !have[i] {
                out[i * 3] = num[i * 3] / den[i];
                out[i * 3 + 1] = num[i * 3 + 1] / den[i];
                out[i * 3 + 2] = num[i * 3 + 2] / den[i];
                have[i] = true;
            }
        }
    }

    let fg_mask: Vec<bool> = a.iter().map(|&v| v > 0.02).collect();
    let near = dilate_binary_box(&fg_mask, w, h, 16);

    let mut res = vec![0u8; n * 3];
    for i in 0..n {
        if a[i] > 0.02 {
            res[i * 3] = col[i * 3];
            res[i * 3 + 1] = col[i * 3 + 1];
            res[i * 3 + 2] = col[i * 3 + 2];
        } else if near[i] {
            res[i * 3] = out[i * 3].round().clamp(0.0, 255.0) as u8;
            res[i * 3 + 1] = out[i * 3 + 1].round().clamp(0.0, 255.0) as u8;
            res[i * 3 + 2] = out[i * 3 + 2].round().clamp(0.0, 255.0) as u8;
        } else {
            res[i * 3] = fill[0];
            res[i * 3 + 1] = fill[1];
            res[i * 3 + 2] = fill[2];
        }
    }
    res
}

fn target_size(orig_w: usize, orig_h: usize, req_w: Option<usize>) -> (usize, usize) {
    let w = (req_w.unwrap_or(orig_w) / 2) * 2;
    let h = (((orig_h as f64) * (w as f64) / (orig_w as f64) / 2.0).round() as usize) * 2;
    (w.max(2), h.max(2))
}

pub fn run(args: KeyArgs) -> Result<()> {
    if args.stacked.is_none()
        && args.webm.is_none()
        && args.frames.is_none()
        && args.png.is_none()
    {
        bail!("choose at least one of --stacked, --webm, --frames, --png");
    }

    let key_col = parse_key_color(&args.key)?;
    let fill_rgb = parse_hex_rgb(&args.fill)?;
    let (w0, h0, clip_fps, frames) = read_rgb_frames(&args.clip)?;
    if frames.is_empty() {
        bail!("no frames decoded from {}", args.clip.display());
    }
    let fps = args.fps.unwrap_or(clip_fps);

    if let Some(ref out_path) = args.stacked {
        write_stacked(
            &frames,
            w0,
            h0,
            fps,
            out_path,
            key_col,
            fill_rgb,
            &args,
        )?;
    }
    if let Some(ref out_path) = args.webm {
        write_webm(
            &frames,
            w0,
            h0,
            fps,
            out_path,
            key_col,
            fill_rgb,
            &args,
        )?;
    }
    if let Some(ref out_dir) = args.frames {
        write_frames(&frames, w0, h0, out_dir, key_col, fill_rgb, &args)?;
    }
    if let Some(ref out_dir) = args.png {
        write_png(&frames, w0, h0, out_dir, key_col, &args)?;
    }

    Ok(())
}

fn write_stacked(
    frames: &[Vec<u8>],
    w0: usize,
    h0: usize,
    fps: f64,
    out: &Path,
    key_col: KeyColor,
    fill_rgb: [u8; 3],
    args: &KeyArgs,
) -> Result<()> {
    let (w, h) = target_size(w0, h0, args.width);
    let size_arg = format!("{}x{}", w, h * 2);
    let fps_arg = format!("{}", fps);
    let crf_arg = format!("{}", args.crf);
    let out_str = out.to_string_lossy().to_string();

    let mut proc = spawn_ffmpeg_pipe(&[
        "-y",
        "-loglevel",
        "error",
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
        "-profile:v",
        "high",
        "-pix_fmt",
        "yuv420p",
        "-crf",
        &crf_arg,
        "-preset",
        "slow",
        "-movflags",
        "+faststart",
        "-an",
        &out_str,
    ])?;

    {
        let stdin = proc.stdin.as_mut().context("missing ffmpeg stdin")?;
        for rgb in frames {
            let (col, a, _) = key_frame(rgb, w0, h0, key_col, args.solid, args.erode)?;
            let under = under_colour(&col, &a, w0, h0, fill_rgb);
            let col_resized = resize_rgb(&under, w0, h0, w, h);
            let a_u8: Vec<u8> = a
                .iter()
                .map(|&v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
                .collect();
            let m_resized = resize_gray_u8(&a_u8, w0, h0, w, h);
            stdin.write_all(&col_resized)?;
            let mut matte_rgb = Vec::with_capacity(w * h * 3);
            for &m in &m_resized {
                matte_rgb.push(m);
                matte_rgb.push(m);
                matte_rgb.push(m);
            }
            stdin.write_all(&matte_rgb)?;
        }
    }
    let status = proc.wait()?;
    if !status.success() {
        bail!("ffmpeg failed writing stacked mp4");
    }
    println!(
        "{}",
        json!({
            "out": out_str,
            "kind": "stacked",
            "frames": frames.len(),
            "width": w,
            "height": h,
            "fps": fps
        })
    );
    Ok(())
}

fn write_webm(
    frames: &[Vec<u8>],
    w0: usize,
    h0: usize,
    fps: f64,
    out: &Path,
    key_col: KeyColor,
    fill_rgb: [u8; 3],
    args: &KeyArgs,
) -> Result<()> {
    let (w, h) = target_size(w0, h0, args.width);
    let size_arg = format!("{}x{}", w, h);
    let fps_arg = format!("{}", fps);
    let crf_arg = format!("{}", args.crf + 10);
    let out_str = out.to_string_lossy().to_string();

    let mut proc = spawn_ffmpeg_pipe(&[
        "-y",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-s",
        &size_arg,
        "-r",
        &fps_arg,
        "-i",
        "-",
        "-c:v",
        "libvpx-vp9",
        "-pix_fmt",
        "yuva420p",
        "-b:v",
        "0",
        "-crf",
        &crf_arg,
        "-auto-alt-ref",
        "0",
        "-row-mt",
        "1",
        "-an",
        &out_str,
    ])?;

    {
        let stdin = proc.stdin.as_mut().context("missing ffmpeg stdin")?;
        for rgb in frames {
            let (col, a, _) = key_frame(rgb, w0, h0, key_col, args.solid, args.erode)?;
            let under = under_colour(&col, &a, w0, h0, fill_rgb);
            let mut rgba = Vec::with_capacity(w0 * h0 * 4);
            for i in 0..(w0 * h0) {
                rgba.push(under[i * 3]);
                rgba.push(under[i * 3 + 1]);
                rgba.push(under[i * 3 + 2]);
                rgba.push((a[i] * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            let resized = resize_rgba(&rgba, w0, h0, w, h);
            stdin.write_all(&resized)?;
        }
    }
    let status = proc.wait()?;
    if !status.success() {
        bail!("ffmpeg failed writing vp9 webm");
    }
    println!(
        "{}",
        json!({
            "out": out_str,
            "kind": "webm",
            "frames": frames.len(),
            "width": w,
            "height": h,
            "fps": fps
        })
    );
    Ok(())
}

fn write_frames(
    frames: &[Vec<u8>],
    w0: usize,
    h0: usize,
    out_dir: &Path,
    key_col: KeyColor,
    fill_rgb: [u8; 3],
    args: &KeyArgs,
) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let picks_set: Option<HashSet<usize>> = if let Some(ref p) = args.picks {
        Some(
            p.split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().parse::<usize>())
                .collect::<std::result::Result<HashSet<_>, _>>()?,
        )
    } else {
        None
    };

    let (w, h) = if let Some(req_w) = args.width {
        target_size(w0, h0, Some(req_w))
    } else {
        (w0, h0)
    };

    let mut picked = Vec::new();
    for (i, rgb) in frames.iter().enumerate() {
        if let Some(ref set) = picks_set {
            if !set.contains(&i) {
                continue;
            }
        } else if i < args.start
            || args.end.is_some_and(|e| i > e)
            || !(i - args.start).is_multiple_of(args.step.max(1))
        {
            continue;
        }
        let (col, a, _) = key_frame(rgb, w0, h0, key_col, args.solid, args.erode)?;
        let under = under_colour(&col, &a, w0, h0, fill_rgb);
        let mut rgba = Vec::with_capacity(w0 * h0 * 4);
        for j in 0..(w0 * h0) {
            rgba.push(under[j * 3]);
            rgba.push(under[j * 3 + 1]);
            rgba.push(under[j * 3 + 2]);
            rgba.push((a[j] * 255.0).round().clamp(0.0, 255.0) as u8);
        }
        let resized = resize_rgba(&rgba, w0, h0, w, h);
        let img = RgbaImage::from_raw(w as u32, h as u32, resized).expect("valid rgba");
        let frame_path = out_dir.join(format!("{:03}.webp", picked.len()));
        img.save(&frame_path)?;
        picked.push(i);
    }

    let manifest = json!({
        "count": picked.len(),
        "width": w,
        "height": h,
        "pattern": "{i:03d}.webp",
        "source_frames": picked
    });
    std::fs::write(out_dir.join("manifest.json"), manifest.to_string())?;
    println!(
        "{}",
        json!({
            "out": out_dir.to_string_lossy(),
            "kind": "frames",
            "count": picked.len(),
            "width": w,
            "height": h
        })
    );
    Ok(())
}

fn write_png(
    frames: &[Vec<u8>],
    w0: usize,
    h0: usize,
    out_dir: &Path,
    key_col: KeyColor,
    args: &KeyArgs,
) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let want: HashSet<usize> = args
        .frame_list
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse::<usize>())
        .collect::<std::result::Result<HashSet<_>, _>>()?;

    for (i, rgb) in frames.iter().enumerate() {
        if want.contains(&i) {
            let (col, a, k) = key_frame(rgb, w0, h0, key_col, args.solid, args.erode)?;
            let mut rgba = Vec::with_capacity(w0 * h0 * 4);
            let mut subj_count = 0usize;
            for j in 0..(w0 * h0) {
                if a[j] > 0.5 {
                    subj_count += 1;
                }
                rgba.push(col[j * 3]);
                rgba.push(col[j * 3 + 1]);
                rgba.push(col[j * 3 + 2]);
                rgba.push((a[j] * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            let img = RgbaImage::from_raw(w0 as u32, h0 as u32, rgba).expect("valid rgba");
            img.save(out_dir.join(format!("f{:03}.png", i)))?;
            let pct = 100.0 * (subj_count as f64) / ((w0 * h0).max(1) as f64);
            println!(
                "frame {}: plate [{}, {}, {}], {:.1}% subject",
                i,
                k[0].round() as i32,
                k[1].round() as i32,
                k[2].round() as i32,
                pct
            );
        }
    }
    Ok(())
}
