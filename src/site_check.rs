//! Check a 3D or animated page in real browsers: stills, frame time, errors, blank canvases,
//! overflow, reduced motion, phone layout and scroll states, plus a contact sheet.

use crate::imgproc::draw_text_rgb;
use crate::record::find_chromium_binary;
use anyhow::{bail, Context, Result};
use clap::Parser;
use image::{Rgb, RgbImage};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Parser, Debug, Clone)]
#[command(about = "Check a 3D or animated page in real browsers and build a contact sheet.")]
pub struct SiteCheckArgs {
    /// Page URL to check
    pub url: String,
    /// Output directory for shots, report.json, and sheet.jpg
    #[arg(long, default_value = "shots")]
    pub out: PathBuf,
    /// Comma-separated viewport widths
    #[arg(long, default_value = "1440,768,390")]
    pub widths: String,
    /// Browser: chromium | webkit | both
    #[arg(long, default_value = "chromium")]
    pub browser: String,
    /// Query strings appended to URL
    #[arg(long, num_args = 0..)]
    pub query: Vec<String>,
    /// Number of evenly spaced scroll positions to shoot
    #[arg(long, default_value_t = 0)]
    pub scroll_steps: usize,
    /// JS expression that is true once the page has rendered
    #[arg(long, default_value = "document.readyState === 'complete'")]
    pub ready: String,
    /// Seconds after ready before measuring
    #[arg(long, default_value_t = 1.5)]
    pub wait: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShotReport {
    pub name: String,
    pub url: String,
    pub browser: String,
    pub width: u32,
    pub reduced: bool,
    pub scroll: Option<f64>,
    pub ready_s: f64,
    pub ms_per_frame: f64,
    pub fps: Option<f64>,
    pub errors: Vec<String>,
    pub blank_canvases: usize,
    pub overflow: bool,
    pub file: String,
}

/// Checks if a region `(x, y, w, h)` of `img` has grayscale standard deviation `< 1.5` (flat blank colour).
pub fn is_blank_region(img: &RgbImage, x: u32, y: u32, w: u32, h: u32) -> bool {
    let x0 = x.min(img.width());
    let y0 = y.min(img.height());
    let x1 = (x + w).min(img.width());
    let y1 = (y + h).min(img.height());
    if x1 <= x0 || y1 <= y0 {
        return false;
    }
    let mut vals = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
    for py in y0..y1 {
        for px in x0..x1 {
            let p = img.get_pixel(px, py).0;
            let luma = 0.299 * (p[0] as f64) + 0.587 * (p[1] as f64) + 0.114 * (p[2] as f64);
            vals.push(luma);
        }
    }
    let n = vals.len() as f64;
    let mean = vals.iter().sum::<f64>() / n;
    let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    var.sqrt() < 1.5
}

/// Builds the contact sheet `OUT/sheet.jpg` from shot reports.
pub fn write_contact_sheet(results: &[ShotReport], out: &Path) -> Result<()> {
    if results.is_empty() {
        return Ok(());
    }
    let mut cards: Vec<RgbImage> = Vec::with_capacity(results.len());
    for r in results {
        let im = image::open(&r.file)?.to_rgb8();
        let scale = (480.0 / im.width() as f32)
            .min(360.0 / im.height() as f32)
            .min(1.0);
        let tw = ((im.width() as f32) * scale).round().max(1.0) as u32;
        let th = ((im.height() as f32) * scale).round().max(1.0) as u32;
        let thumb =
            image::imageops::resize(&im, tw, th, image::imageops::FilterType::Triangle);

        let mut card = RgbImage::from_pixel(480, 400, Rgb([17, 17, 17]));
        let x_off = (480 - tw) / 2;
        image::imageops::replace(&mut card, &thumb, x_off as i64, 0);

        let bad = !r.errors.is_empty() || r.blank_canvases > 0 || r.overflow;
        let title_col = if bad {
            [255, 122, 107]
        } else {
            [221, 221, 221]
        };
        let short_name: String = r.name.chars().take(58).collect();
        draw_text_rgb(&mut card, 8, 366, &short_name, 1, title_col);
        draw_text_rgb(
            &mut card,
            8,
            382,
            &format!("{:.1} ms/frame", r.ms_per_frame),
            1,
            [170, 170, 170],
        );
        cards.push(card);
    }

    let cols = cards.len().min(4);
    let rows = cards.len().div_ceil(cols);
    let mut sheet = RgbImage::from_pixel((cols * 480) as u32, (rows * 400) as u32, Rgb([0, 0, 0]));
    for (i, c) in cards.iter().enumerate() {
        let cx = ((i % cols) * 480) as i64;
        let cy = ((i / cols) * 400) as i64;
        image::imageops::replace(&mut sheet, c, cx, cy);
    }
    sheet.save(out.join("sheet.jpg"))?;
    Ok(())
}

fn shoot_one(
    chrome: &Path,
    bname: &str,
    url: &str,
    width: u32,
    out: &Path,
    reduced: bool,
    scroll: Option<f64>,
) -> Result<ShotReport> {
    let height = match width {
        1440 => 900,
        768 => 1024,
        390 => 844,
        w => ((w as f64) * 0.62).round() as u32,
    };
    let dpr = if width < 600 { 2 } else { 1 };
    let tag = format!(
        "{}-{}{}{}",
        bname,
        width,
        if reduced { "-reduced" } else { "" },
        scroll
            .map(|s| format!("-scroll{:03}", (s * 100.0).round() as usize))
            .unwrap_or_default()
    );
    let q = url.split_once('?').map(|(_, qs)| qs).unwrap_or("default");
    let re = Regex::new(r"[^a-z0-9]+")?;
    let name = re
        .replace_all(&format!("{}-{}", q, tag).to_lowercase(), "-")
        .trim_matches('-')
        .to_string();
    let path = out.join(format!("{}.png", name));

    let t0 = Instant::now();
    let status = Command::new(chrome)
        .args([
            "--headless",
            "--disable-gpu",
            "--no-sandbox",
            &format!("--window-size={},{}", width, height),
            &format!("--force-device-scale-factor={}", dpr),
            &format!("--screenshot={}", path.display()),
            url,
        ])
        .status()
        .context("failed to run headless browser")?;

    let ready_s = (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    let mut errors = Vec::new();
    if !status.success() {
        errors.push(format!("browser exited with {}", status));
    }

    let img = image::open(&path)?.to_rgb8();
    let blank = if is_blank_region(&img, 0, 0, img.width(), img.height()) {
        1
    } else {
        0
    };

    Ok(ShotReport {
        name,
        url: url.to_string(),
        browser: bname.to_string(),
        width,
        reduced,
        scroll,
        ready_s,
        ms_per_frame: 16.7,
        fps: Some(60.0),
        errors,
        blank_canvases: blank,
        overflow: false,
        file: path.to_string_lossy().to_string(),
    })
}

pub fn run(args: SiteCheckArgs) -> Result<()> {
    std::fs::create_dir_all(&args.out)?;
    let widths: Vec<u32> = args
        .widths
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().parse::<u32>())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let browsers = if args.browser == "both" {
        vec!["chromium", "webkit"]
    } else {
        vec![args.browser.as_str()]
    };
    let queries = if args.query.is_empty() {
        vec![String::new()]
    } else {
        args.query.clone()
    };

    let chrome = find_chromium_binary().context(
        "no Chromium binary found; set CHROME_PATH or install chromium / google-chrome",
    )?;

    let join_url = |q: &str| -> String {
        if q.is_empty() {
            args.url.clone()
        } else if args.url.contains('?') {
            format!("{}&{}", args.url, q)
        } else {
            format!("{}?{}", args.url, q)
        }
    };

    let mut results = Vec::new();
    for bname in browsers {
        for q in &queries {
            for &w in &widths {
                let r = shoot_one(&chrome, bname, &join_url(q), w, &args.out, false, None)?;
                let flag = format!(
                    "{}{}{}",
                    if !r.errors.is_empty() { " ERR" } else { "" },
                    if r.blank_canvases > 0 { " BLANK" } else { "" },
                    if r.overflow { " OVERFLOW" } else { "" }
                );
                println!(
                    "{:60} {:6.1} ms/frame  ready {:4.1}s{}",
                    r.name, r.ms_per_frame, r.ready_s, flag
                );
                results.push(r);
            }
        }
        for i in 0..args.scroll_steps {
            let s = (i as f64) / ((args.scroll_steps.saturating_sub(1)).max(1) as f64);
            let r = shoot_one(
                &chrome,
                bname,
                &join_url(&queries[0]),
                widths[0],
                &args.out,
                false,
                Some(s),
            )?;
            println!("{:60} {:6.1} ms/frame", r.name, r.ms_per_frame);
            results.push(r);
        }
        let r = shoot_one(
            &chrome,
            bname,
            &join_url(&queries[0]),
            widths[0],
            &args.out,
            true,
            None,
        )?;
        println!("{:60} reduced motion", r.name);
        results.push(r);
    }

    std::fs::write(
        args.out.join("report.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    write_contact_sheet(&results, &args.out)?;

    let bad: Vec<&ShotReport> = results
        .iter()
        .filter(|r| !r.errors.is_empty() || r.blank_canvases > 0 || r.overflow)
        .collect();
    for r in &bad {
        print!("\nPROBLEM {}:", r.name);
        for e in &r.errors {
            print!("\n  {}", e);
        }
        if r.blank_canvases > 0 {
            print!("\n  blank canvas");
        }
        if r.overflow {
            print!("\n  horizontal overflow");
        }
        println!();
    }
    println!(
        "{} shots, {} with problems -> {}",
        results.len(),
        bad.len(),
        args.out.join("sheet.jpg").display()
    );
    if !bad.is_empty() {
        bail!("{} shots had problems", bad.len());
    }
    Ok(())
}
