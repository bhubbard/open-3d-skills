//! Put the original pixels back into an outpainted or AI-extended image.

use crate::imgproc::{
    gaussian_blur_f32, match_template_ccoeff_normed, resize_rgb, rgb_to_gray_f32,
};
use anyhow::{Context, Result};
use clap::Parser;
use image::RgbImage;
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(about = "Put the original pixels back into an outpainted or AI-extended image.")]
pub struct PasteBackArgs {
    /// Original source image
    pub original: PathBuf,
    /// Outpainted / extended image
    pub extended: PathBuf,
    /// Output composite image path
    pub out: PathBuf,
    /// Pixels of soft edge inside the original's border
    #[arg(long, default_value_t = 24)]
    pub feather: usize,
    /// Skip colour matching
    #[arg(long)]
    pub no_match: bool,
}

/// Locates `orig` `(ow, oh)` inside `ext` `(ew, eh)` returning `(scale, x, y, score, resized_orig_rgb, tw, th)`.
pub fn locate(
    orig: &[u8],
    ow: usize,
    oh: usize,
    ext: &[u8],
    ew: usize,
    eh: usize,
) -> (f32, usize, usize, f32, Vec<u8>, usize, usize) {
    let small = (120.0f32 / (ew.max(eh) as f32)).min(1.0);
    let sew = ((ew as f32) * small).round().max(1.0) as usize;
    let seh = ((eh as f32) * small).round().max(1.0) as usize;
    let ext_small_rgb = resize_rgb(ext, ew, eh, sew, seh);
    let ext_small_g = rgb_to_gray_f32(&ext_small_rgb, sew, seh);

    let base = ((eh as f32) / (oh as f32)).min((ew as f32) / (ow as f32));
    let s_lo = (base * 0.5).min(0.85);
    let s_hi = base.max(1.0);
    let mut best = (-1.0f32, 1.0f32, 0.0f32, 0.0f32);

    let mut scales: Vec<f32> = (0..21)
        .map(|step| s_lo + (s_hi - s_lo) * (step as f32 / 20.0))
        .collect();
    scales.push(1.0);

    for s in scales {
        let w = ((ow as f32) * s * small).round() as usize;
        let h = ((oh as f32) * s * small).round() as usize;
        if w < 16 || h < 16 || w > sew || h > seh {
            continue;
        }
        let t_rgb = resize_rgb(orig, ow, oh, w, h);
        let t_g = rgb_to_gray_f32(&t_rgb, w, h);
        let (score, lx, ly) = match_template_ccoeff_normed(&ext_small_g, sew, seh, &t_g, w, h);
        if score > best.0 {
            best = (score, s, (lx as f32) / small, (ly as f32) / small);
        }
    }

    let (mut score, s, x_est, y_est) = best;
    let tw = ((ow as f32) * s).round().max(1.0) as usize;
    let th = ((oh as f32) * s).round().max(1.0) as usize;
    let t_rgb = resize_rgb(orig, ow, oh, tw, th);
    let t_g = rgb_to_gray_f32(&t_rgb, tw, th);

    let pad = 12usize;
    let x0 = (x_est as usize).saturating_sub(pad);
    let y0 = (y_est as usize).saturating_sub(pad);
    let win_w = (tw + 2 * pad).min(ew.saturating_sub(x0));
    let win_h = (th + 2 * pad).min(eh.saturating_sub(y0));

    let (mut fx, mut fy) = ((x_est as usize).min(ew.saturating_sub(tw)), (y_est as usize).min(eh.saturating_sub(th)));

    if win_w >= tw && win_h >= th {
        let mut win_rgb = Vec::with_capacity(win_w * win_h * 3);
        for wy in 0..win_h {
            let row_start = ((y0 + wy) * ew + x0) * 3;
            win_rgb.extend_from_slice(&ext[row_start..row_start + win_w * 3]);
        }
        let win_g = rgb_to_gray_f32(&win_rgb, win_w, win_h);
        let (r_score, lx, ly) = match_template_ccoeff_normed(&win_g, win_w, win_h, &t_g, tw, th);
        score = r_score;
        fx = x0 + lx;
        fy = y0 + ly;
    }

    (s, fx, fy, score, t_rgb, tw, th)
}

pub fn run(args: PasteBackArgs) -> Result<()> {
    let orig_im = image::open(&args.original)
        .with_context(|| format!("could not read {}", args.original.display()))?
        .to_rgb8();
    let ext_im = image::open(&args.extended)
        .with_context(|| format!("could not read {}", args.extended.display()))?
        .to_rgb8();

    let (ow, oh) = (orig_im.width() as usize, orig_im.height() as usize);
    let (ew, eh) = (ext_im.width() as usize, ext_im.height() as usize);
    let orig_raw = orig_im.into_raw();
    let mut ext_raw = ext_im.into_raw();

    let (s, x, y, score, t_rgb, tw, th) = locate(&orig_raw, ow, oh, &ext_raw, ew, eh);
    println!(
        "original found at ({},{}), {}x{} (scale {:.3}), match {:.3}",
        x, y, tw, th, s, score
    );
    if score < 0.9 {
        println!(
            "warning: weak match; the model changed the original area a lot. Check the result closely."
        );
    }

    let mut src_f: Vec<f32> = t_rgb.iter().map(|&v| v as f32).collect();
    if !args.no_match {
        let b = args.feather.max(4).min(tw.min(th) / 2);
        for c in 0..3 {
            let mut sv = Vec::new();
            let mut rv = Vec::new();
            for py in 0..th {
                for px in 0..tw {
                    if py < b || py >= th - b || px < b || px >= tw - b {
                        sv.push(src_f[(py * tw + px) * 3 + c]);
                        let ey = (y + py).min(eh - 1);
                        let ex = (x + px).min(ew - 1);
                        rv.push(ext_raw[(ey * ew + ex) * 3 + c] as f32);
                    }
                }
            }
            if !sv.is_empty() {
                let n = sv.len() as f32;
                let s_mean = sv.iter().sum::<f32>() / n;
                let r_mean = rv.iter().sum::<f32>() / n;
                let s_std = (sv.iter().map(|v| (v - s_mean).powi(2)).sum::<f32>() / n).sqrt();
                let r_std = (rv.iter().map(|v| (v - r_mean).powi(2)).sum::<f32>() / n).sqrt();
                let gain = (r_std / s_std.max(1e-3)).clamp(0.9, 1.1);
                for py in 0..th {
                    for px in 0..tw {
                        let idx = (py * tw + px) * 3 + c;
                        src_f[idx] = (src_f[idx] - s_mean) * gain + r_mean;
                    }
                }
            }
        }
    }

    let f = args.feather.min(tw.min(th) / 2);
    let mut mask = vec![0.0f32; tw * th];
    for py in f..th.saturating_sub(f) {
        for px in f..tw.saturating_sub(f) {
            mask[py * tw + px] = 1.0;
        }
    }
    let sigma = ((f as f32) / 2.5).max(1.0);
    let mask_blur = gaussian_blur_f32(&mask, tw, th, sigma);

    for py in 0..th {
        let ey = y + py;
        if ey >= eh {
            continue;
        }
        for px in 0..tw {
            let ex = x + px;
            if ex >= ew {
                continue;
            }
            let m = mask_blur[py * tw + px];
            let s_idx = (py * tw + px) * 3;
            let e_idx = (ey * ew + ex) * 3;
            for c in 0..3 {
                let blended = src_f[s_idx + c] * m + (ext_raw[e_idx + c] as f32) * (1.0 - m);
                ext_raw[e_idx + c] = blended.round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    let out_im = RgbImage::from_raw(ew as u32, eh as u32, ext_raw).expect("valid rgb");
    out_im
        .save(&args.out)
        .with_context(|| format!("failed to write {}", args.out.display()))?;
    println!("wrote {}", args.out.display());
    Ok(())
}
