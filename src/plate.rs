//! Put a cut-out subject on a flat key plate, ready to animate with an image-to-video model.

use crate::imgproc::{alpha_composite, percentile};
use anyhow::{bail, Context, Result};
use clap::Parser;
use image::{DynamicImage, Rgba, RgbaImage};
use std::path::PathBuf;

#[derive(Parser, Debug, Clone)]
#[command(about = "Put a cut-out subject on a flat key plate, ready to animate with an image-to-video model.")]
pub struct PlateArgs {
    /// Input cut-out subject image with alpha channel
    pub subject: PathBuf,
    /// Output plate image path
    pub out: PathBuf,
    /// Key colour: auto | magenta | green | blue
    #[arg(long, default_value = "auto")]
    pub key: String,
    /// Plate size WxH; match the video model's aspect
    #[arg(long, default_value = "1350x1800")]
    pub size: String,
    /// Subject height as a share of the plate
    #[arg(long, default_value_t = 0.82)]
    pub fit: f32,
    /// Anchor position: bottom | center
    #[arg(long, default_value = "bottom")]
    pub anchor: String,
    /// Gap below the subject, share of height
    #[arg(long, default_value_t = 0.04)]
    pub margin: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyColor {
    Magenta,
    Green,
    Blue,
}

impl KeyColor {
    pub fn as_str(self) -> &'static str {
        match self {
            KeyColor::Magenta => "magenta",
            KeyColor::Green => "green",
            KeyColor::Blue => "blue",
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            KeyColor::Magenta => [255, 0, 255],
            KeyColor::Green => [0, 255, 0],
            KeyColor::Blue => [0, 0, 255],
        }
    }

    pub fn excess(self, r: f32, g: f32, b: f32) -> f32 {
        match self {
            KeyColor::Magenta => r.min(b) - g,
            KeyColor::Green => g - r.max(b),
            KeyColor::Blue => b - r.max(g),
        }
    }
}

pub fn collisions(rgba: &RgbaImage) -> Vec<(KeyColor, f32, f32)> {
    let keys = [KeyColor::Magenta, KeyColor::Green, KeyColor::Blue];
    let mut out = Vec::with_capacity(3);
    for &k in &keys {
        let mut excess_vals = Vec::new();
        let mut over_60 = 0usize;
        for px in rgba.pixels() {
            if px[3] > 200 {
                let e = k.excess(px[0] as f32, px[1] as f32, px[2] as f32);
                if e > 60.0 {
                    over_60 += 1;
                }
                excess_vals.push(e);
            }
        }
        let share = if excess_vals.is_empty() {
            0.0
        } else {
            (over_60 as f32 / excess_vals.len() as f32) * 100.0
        };
        let p995 = percentile(&excess_vals, 99.5);
        out.push((k, share, p995));
    }
    out
}

fn alpha_bbox(im: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let mut min_x = im.width();
    let mut min_y = im.height();
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut found = false;
    for (x, y, px) in im.enumerate_pixels() {
        if px[3] > 0 {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            found = true;
        }
    }
    if found {
        Some((min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
    } else {
        None
    }
}

pub fn run(args: PlateArgs) -> Result<()> {
    let dyn_im = image::open(&args.subject)
        .with_context(|| format!("failed to open {}", args.subject.display()))?;
    let has_alpha = matches!(
        dyn_im,
        DynamicImage::ImageRgba8(_)
            | DynamicImage::ImageRgba16(_)
            | DynamicImage::ImageRgba32F(_)
            | DynamicImage::ImageLumaA8(_)
            | DynamicImage::ImageLumaA16(_)
    );
    let rgba = dyn_im.to_rgba8();
    let min_alpha = rgba.pixels().map(|p| p[3]).min().unwrap_or(255);
    if !has_alpha || min_alpha == 255 {
        bail!("subject has no transparency; cut it out first");
    }

    let (bx, by, bw, bh) =
        alpha_bbox(&rgba).context("subject is completely transparent")?;
    let cropped = image::imageops::crop_imm(&rgba, bx, by, bw, bh).to_image();

    let report = collisions(&cropped);
    for &(k, share, p995) in &report {
        println!(
            "{:8} {:5.2}% of the subject would key out (99.5th pct excess {:.0})",
            k.as_str(),
            share,
            p995
        );
    }

    let chosen_key = match args.key.as_str() {
        "auto" => {
            report
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap()
                .0
        }
        "magenta" => KeyColor::Magenta,
        "green" => KeyColor::Green,
        "blue" => KeyColor::Blue,
        other => bail!("invalid --key {other:?}; expected auto|magenta|green|blue"),
    };

    let chosen_share = report
        .iter()
        .find(|(k, _, _)| *k == chosen_key)
        .map(|x| x.1)
        .unwrap_or(0.0);
    if chosen_share > 0.5 {
        println!(
            "warning: {:.2}% of the subject is close to {}; the key will bite into it",
            chosen_share,
            chosen_key.as_str()
        );
    }

    let (w_str, h_str) = args
        .size
        .to_lowercase()
        .split_once('x')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .context("--size must be WxH, e.g. 1350x1800")?;
    let w: u32 = w_str.parse()?;
    let h: u32 = h_str.parse()?;

    let mut s = args.fit * (h as f32) / (cropped.height() as f32);
    if (cropped.width() as f32) * s > (w as f32) * 0.94 {
        s = (w as f32) * 0.94 / (cropped.width() as f32);
    }
    let nw = ((cropped.width() as f32) * s).round().max(1.0) as u32;
    let nh = ((cropped.height() as f32) * s).round().max(1.0) as u32;

    let mut resized = image::imageops::resize(
        &cropped,
        nw,
        nh,
        image::imageops::FilterType::Lanczos3,
    );

    // Hard-edged alpha: soft edges pre-blended with the plate colour teach the model a fringe
    for px in resized.pixels_mut() {
        px[3] = if px[3] > 127 { 255 } else { 0 };
    }

    let x = ((w as i32) - (nw as i32)) / 2;
    let y = if args.anchor == "bottom" {
        (h as i32) - (nh as i32) - (args.margin * (h as f32)).round() as i32
    } else {
        ((h as i32) - (nh as i32)) / 2
    };

    let k_rgb = chosen_key.rgb();
    let mut plate = RgbaImage::from_pixel(w, h, Rgba([k_rgb[0], k_rgb[1], k_rgb[2], 255]));
    alpha_composite(&mut plate, &resized, x, y);

    let out_rgb = DynamicImage::ImageRgba8(plate).to_rgb8();
    out_rgb
        .save(&args.out)
        .with_context(|| format!("failed to write {}", args.out.display()))?;

    println!(
        "key {}: wrote {} {}x{}, subject {}x{} at ({},{})",
        chosen_key.as_str(),
        args.out.display(),
        w,
        h,
        nw,
        nh,
        x,
        y
    );
    Ok(())
}
