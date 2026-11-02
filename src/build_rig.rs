//! Build a cursor-following head rig (a grid of head poses on one fixed body) from head-turn clips.

use crate::imgproc::{
    align_body_ecc, connected_components_with_stats, flow_morph_rgb, gaussian_blur_f32,
    match_template_ccoeff_normed, median_f32, percentile, point_in_poly, resize_rgb,
    rgb_to_gray_f32, rgb_to_gray_u8, warp_affine_inverse_rgb,
};
use anyhow::{bail, Context, Result};
use clap::Parser;
use image::RgbImage;
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Parser, Debug, Clone)]
#[command(about = "Build a cursor-following head rig (a grid of head poses on one fixed body) from head-turn clips.")]
pub struct BuildRigArgs {
    /// Directory containing head-turn clips (L.mp4, R.mp4, etc.)
    pub clips: PathBuf,
    /// Output directory for row_N.webp, plate.webp, and rig.json
    pub out: PathBuf,
    /// Head box x,y,w,h in plate px
    #[arg(long)]
    pub box_rect: String,
    #[arg(long, default_value = "")]
    pub prefix: String,
    /// Tracking mode: dots | template
    #[arg(long, default_value = "dots")]
    pub track: String,
    /// Template tracking: x,y of a facial point at the forward pose
    #[arg(long)]
    pub point: Option<String>,
    /// Dots: y0,y1 band where the eyes can be (default: middle of the box)
    #[arg(long)]
    pub eye_band: Option<String>,
    /// Dots: grey level below which a pixel can be an eye
    #[arg(long, default_value_t = 70)]
    pub dark: u8,
    /// Rows below this are body only, used for alignment (default: box bottom)
    #[arg(long)]
    pub body_y: Option<usize>,
    /// Yaw steps per row (odd, so the centre is forward)
    #[arg(long, default_value_t = 19)]
    pub cols: usize,
    #[arg(long, default_value_t = 0.6)]
    pub out_scale: f32,
    /// Rows per pitch unit; gaps are filled with morphs
    #[arg(long, default_value_t = 3)]
    pub between: usize,
    #[arg(long, default_value_t = 82)]
    pub quality: u8,
    #[arg(long, default_value_t = 28)]
    pub feather: usize,
    /// Head mask polygon x,y;x,y;... (default: an oval in the box)
    #[arg(long, default_value = "")]
    pub poly: String,
    /// Write full-size cells here for inspection
    #[arg(long)]
    pub debug: Option<PathBuf>,
}

const ROWS: [(&str, f32); 4] = [("U", -1.0), ("M", 0.0), ("H", 0.5), ("D", 1.0)];

fn clip_name(row: &str, side: &str) -> String {
    if row == "M" {
        side.to_string()
    } else {
        format!("{}{}", row, side)
    }
}

fn extract_clip_frames(
    clips: &Path,
    prefix: &str,
    name: &str,
    tmp: &Path,
) -> Result<Option<Vec<PathBuf>>> {
    let src = clips.join(format!("{}{}.mp4", prefix, name));
    if !src.exists() {
        return Ok(None);
    }
    let out = tmp.join(name);
    std::fs::create_dir_all(&out)?;
    let pat = out.join("%04d.png").to_string_lossy().to_string();
    let status = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i", &src.to_string_lossy(), &pat])
        .status()
        .context("failed to run ffmpeg frame extraction")?;
    if !status.success() {
        bail!("ffmpeg failed extracting {}", src.display());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&out)?
        .filter_map(|e| e.ok().map(|x| x.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    files.sort();
    Ok(if files.is_empty() { None } else { Some(files) })
}

#[derive(Debug, Clone)]
pub struct DotEyesTracker {
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    amin: f32,
    amax: f32,
    dx_min: f32,
    dx_max: f32,
    dy_max: f32,
    dark: u8,
}

impl DotEyesTracker {
    pub fn new(box_rect: (usize, usize, usize, usize), band: (usize, usize), dark: u8) -> Self {
        let (bx, _by, bw, bh) = box_rect;
        let a = (bw * bh) as f32;
        Self {
            x0: bx,
            x1: bx + bw,
            y0: band.0,
            y1: band.1,
            amin: 0.00025 * a,
            amax: 0.0018 * a,
            dx_min: 0.05 * (bw as f32),
            dx_max: 0.20 * (bw as f32),
            dy_max: 0.04 * (bh as f32),
            dark,
        }
    }

    pub fn track(
        &self,
        rgb: &[u8],
        w: usize,
        h: usize,
    ) -> Option<((f32, f32), Vec<(f32, f32, f32)>)> {
        let gray = rgb_to_gray_u8(rgb, w, h);
        let x0 = self.x0.min(w);
        let x1 = self.x1.min(w);
        let y0 = self.y0.min(h);
        let y1 = self.y1.min(h);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let rw = x1 - x0;
        let rh = y1 - y0;
        let mut mask = vec![false; rw * rh];
        for ry in 0..rh {
            for rx in 0..rw {
                mask[ry * rw + rx] = gray[(y0 + ry) * w + (x0 + rx)] < self.dark;
            }
        }
        let comps = connected_components_with_stats(&mask, rw, rh);
        let mut c = Vec::new();
        for st in comps {
            let a = st.area as f32;
            let rect_a = (st.w * st.h).max(1) as f32;
            let aspect = (st.w as f32) / (st.h.max(1) as f32);
            if a > self.amin && a < self.amax && (a / rect_a) > 0.6 && aspect > 0.5 && aspect < 1.4
            {
                let cx = st.cx + (x0 as f32);
                let cy = st.cy + (y0 as f32);
                let r = (a / std::f32::consts::PI).sqrt();
                c.push((cx, cy, r, a));
            }
        }

        let mut best: Option<[(f32, f32, f32, f32); 2]> = None;
        let mut best_score = 1e9f32;
        for i in 0..c.len() {
            for j in (i + 1)..c.len() {
                let p = c[i];
                let q = c[j];
                let dx = (p.0 - q.0).abs();
                let dy = (p.1 - q.1).abs();
                if dx > self.dx_min && dx < self.dx_max && dy < self.dy_max {
                    let sc = dy + (p.3 - q.3).abs() / 40.0;
                    if sc < best_score {
                        best_score = sc;
                        let mut pair = [p, q];
                        pair.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                        best = Some(pair);
                    }
                }
            }
        }

        let pair = best?;
        let eyes = vec![(pair[0].0, pair[0].1, pair[0].2), (pair[1].0, pair[1].1, pair[1].2)];
        let mid = ((eyes[0].0 + eyes[1].0) * 0.5, (eyes[0].1 + eyes[1].1) * 0.5);
        Some((mid, eyes))
    }
}

pub struct TemplateTracker {
    start: (f32, f32),
    pos: (f32, f32),
    half: usize,
    search: usize,
    tpl: Option<Vec<f32>>,
}

impl TemplateTracker {
    pub fn new(point: (f32, f32), box_w: usize) -> Self {
        let half = ((box_w as f32) * 0.06).astype_usize().max(8);
        let search = ((box_w as f32) * 0.12) as usize;
        Self {
            start: point,
            pos: point,
            half,
            search,
            tpl: None,
        }
    }

    pub fn reset(&mut self) {
        self.pos = self.start;
        self.tpl = None;
    }

    pub fn track(&mut self, rgb: &[u8], w: usize, h: usize) -> Option<(f32, f32)> {
        let g = rgb_to_gray_f32(rgb, w, h);
        let x = self.pos.0 as usize;
        let y = self.pos.1 as usize;
        let hf = self.half;
        let tw = 2 * hf;
        let th = 2 * hf;
        if x < hf || y < hf || x + hf > w || y + hf > h {
            return None;
        }
        if self.tpl.is_none() {
            let mut t = Vec::with_capacity(tw * th);
            for ty in (y - hf)..(y + hf) {
                t.extend_from_slice(&g[ty * w + (x - hf)..ty * w + (x + hf)]);
            }
            self.tpl = Some(t);
            return Some((x as f32, y as f32));
        }

        let s = self.search;
        let x0 = x.saturating_sub(hf + s);
        let y0 = y.saturating_sub(hf + s);
        let x1 = (x + hf + s).min(w);
        let y1 = (y + hf + s).min(h);
        let ww = x1 - x0;
        let wh = y1 - y0;
        if ww < tw || wh < th {
            return None;
        }
        let mut win = Vec::with_capacity(ww * wh);
        for wy in y0..y1 {
            win.extend_from_slice(&g[wy * w + x0..wy * w + x1]);
        }
        let tpl_ref = self.tpl.as_ref().unwrap();
        let (conf, lx, ly) = match_template_ccoeff_normed(&win, ww, wh, tpl_ref, tw, th);
        if conf < 0.4 {
            return None;
        }
        let nx = x0 + lx + hf;
        let ny = y0 + ly + hf;
        self.pos = (nx as f32, ny as f32);
        if nx >= hf && ny >= hf && nx + hf <= w && ny + hf <= h {
            let mut new_t = Vec::with_capacity(tw * th);
            for ty in (ny - hf)..(ny + hf) {
                new_t.extend_from_slice(&g[ty * w + (nx - hf)..ty * w + (nx + hf)]);
            }
            self.tpl = Some(new_t);
        }
        Some(self.pos)
    }
}

trait AsUsize {
    fn astype_usize(self) -> usize;
}
impl AsUsize for f32 {
    fn astype_usize(self) -> usize {
        self.max(0.0) as usize
    }
}

fn bg_gain(img: &[u8], plate: &[u8], w: usize, h: usize) -> [f32; 3] {
    let mut a_sum = [0.0f32; 3];
    let mut b_sum = [0.0f32; 3];
    let mut count = 0usize;

    let y_end = 60.min(h);
    for y in 10.min(y_end)..y_end {
        for x in 10.min(w)..110.min(w) {
            let idx = (y * w + x) * 3;
            for c in 0..3 {
                a_sum[c] += img[idx + c] as f32;
                b_sum[c] += plate[idx + c] as f32;
            }
            count += 1;
        }
        for x in w.saturating_sub(110)..w.saturating_sub(10) {
            let idx = (y * w + x) * 3;
            for c in 0..3 {
                a_sum[c] += img[idx + c] as f32;
                b_sum[c] += plate[idx + c] as f32;
            }
            count += 1;
        }
    }

    if count == 0 {
        return [1.0, 1.0, 1.0];
    }
    let inv = 1.0 / (count as f32);
    [
        (b_sum[0] * inv) / (a_sum[0] * inv).max(1.0),
        (b_sum[1] * inv) / (a_sum[1] * inv).max(1.0),
        (b_sum[2] * inv) / (a_sum[2] * inv).max(1.0),
    ]
}

fn head_mask(
    w: usize,
    h: usize,
    box_rect: (usize, usize, usize, usize),
    poly: Option<&[(i32, i32)]>,
    feather: usize,
) -> Vec<f32> {
    let mut m = vec![0.0f32; w * h];
    let (bx, by, bw, bh) = box_rect;
    let cx = (bx as f32) + (bw as f32) * 0.5;
    let cy = (by as f32) + (bh as f32) * 0.46;
    let rx = ((bw as f32) * 0.46).max(1.0);
    let ry = ((bh as f32) * 0.46).max(1.0);

    for y in 0..h {
        for x in 0..w {
            let inside = if let Some(pts) = poly {
                point_in_poly(x as f32, y as f32, pts)
            } else {
                let dx = ((x as f32) - cx) / rx;
                let dy = ((y as f32) - cy) / ry;
                dx * dx + dy * dy <= 1.0
            };
            if inside {
                m[y * w + x] = 1.0;
            }
        }
    }
    gaussian_blur_f32(&m, w, h, (feather as f32) / 2.5)
}

fn skin_tone(crop: &[u8], w: usize, h: usize, eyes: &[(f32, f32, f32)]) -> Vec<String> {
    let mut out = Vec::with_capacity(eyes.len());
    for &(x, y, r) in eyes {
        let x0 = ((x - 3.0 * r).max(0.0) as usize).min(w);
        let x1 = ((x + 3.0 * r).max(0.0) as usize).min(w);
        let y0 = ((y - 3.0 * r).max(0.0) as usize).min(h);
        let y1 = ((y + 3.0 * r).max(0.0) as usize).min(h);

        let mut ring_px: Vec<([f32; 3], f32)> = Vec::new();
        for py in y0..y1 {
            for px in x0..x1 {
                let d = ((px as f32 - x).powi(2) + (py as f32 - y).powi(2)).sqrt();
                if d > 1.6 * r && d < 2.5 * r {
                    let idx = (py * w + px) * 3;
                    let rr = crop[idx] as f32;
                    let gg = crop[idx + 1] as f32;
                    let bb = crop[idx + 2] as f32;
                    let lum = (rr + gg + bb) / 3.0;
                    ring_px.push(([rr, gg, bb], lum));
                }
            }
        }
        if ring_px.is_empty() {
            out.push("#c08070".to_string());
            continue;
        }
        let lums: Vec<f32> = ring_px.iter().map(|p| p.1).collect();
        let p40 = percentile(&lums, 40.0);
        let p95 = percentile(&lums, 95.0);
        let mut rs = Vec::new();
        let mut gs = Vec::new();
        let mut bs = Vec::new();
        for &(rgb, lum) in &ring_px {
            if lum >= p40 && lum <= p95 {
                rs.push(rgb[0]);
                gs.push(rgb[1]);
                bs.push(rgb[2]);
            }
        }
        if rs.is_empty() {
            for &(rgb, _) in &ring_px {
                rs.push(rgb[0]);
                gs.push(rgb[1]);
                bs.push(rgb[2]);
            }
        }
        let mr = median_f32(&mut rs).round().clamp(0.0, 255.0) as u8;
        let mg = median_f32(&mut gs).round().clamp(0.0, 255.0) as u8;
        let mb = median_f32(&mut bs).round().clamp(0.0, 255.0) as u8;
        out.push(format!("#{:02x}{:02x}{:02x}", mr, mg, mb));
    }
    out
}

pub fn run(args: BuildRigArgs) -> Result<()> {
    let box_parts: Vec<usize> = args
        .box_rect
        .split(',')
        .map(|s| s.trim().parse::<usize>())
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("--box must be x,y,w,h")?;
    if box_parts.len() != 4 {
        bail!("--box must have 4 integers: x,y,w,h");
    }
    let (bx, by, bw, bh) = (box_parts[0], box_parts[1], box_parts[2], box_parts[3]);
    let box_rect = (bx, by, bw, bh);
    let cols = if args.cols % 2 == 1 {
        args.cols
    } else {
        args.cols + 1
    };

    std::fs::create_dir_all(&args.out)?;
    let tmp = tempfile::tempdir()?;

    let level_l = extract_clip_frames(&args.clips, &args.prefix, "L", tmp.path())?;
    let Some(level_l) = level_l else {
        bail!(
            "need {}L.mp4 and {}R.mp4 in {}",
            args.prefix,
            args.prefix,
            args.clips.display()
        );
    };

    let plate_im = image::open(&level_l[0])?.to_rgb8();
    let (w, h) = (plate_im.width() as usize, plate_im.height() as usize);
    let plate_rgb = plate_im.into_raw();
    let plate_g = rgb_to_gray_f32(&plate_rgb, w, h);
    let body_y = args.body_y.unwrap_or_else(|| (by + bh).min(h.saturating_sub(20)));

    let dot_tracker = if args.track == "dots" {
        let band = if let Some(ref eb) = args.eye_band {
            let parts: Vec<usize> = eb
                .split(',')
                .map(|s| s.trim().parse::<usize>())
                .collect::<std::result::Result<Vec<_>, _>>()?;
            (parts[0], parts[1])
        } else {
            (
                by + ((bh as f32) * 0.2) as usize,
                by + ((bh as f32) * 0.65) as usize,
            )
        };
        Some(DotEyesTracker::new(box_rect, band, args.dark))
    } else {
        None
    };

    let mut tpl_tracker = if args.track == "template" {
        let pt_str = args
            .point
            .as_ref()
            .context("--track template needs --point x,y")?;
        let pts: Vec<f32> = pt_str
            .split(',')
            .map(|s| s.trim().parse::<f32>())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Some(TemplateTracker::new((pts[0], pts[1]), bw))
    } else {
        None
    };

    let poly: Option<Vec<(i32, i32)>> = if !args.poly.trim().is_empty() {
        let mut pts = Vec::new();
        for pair in args.poly.split(';') {
            if let Some((xs, ys)) = pair.split_once(',') {
                pts.push((xs.trim().parse::<i32>()?, ys.trim().parse::<i32>()?));
            }
        }
        Some(pts)
    } else {
        None
    };

    let mask = head_mask(w, h, box_rect, poly.as_deref(), args.feather);

    if let Some(ref dbg) = args.debug {
        std::fs::create_dir_all(dbg)?;
        let p_im = RgbImage::from_raw(w as u32, h as u32, plate_rgb.clone()).unwrap();
        p_im.save(dbg.join("plate.png"))?;
    }

    type CellData = (Vec<u8>, Option<Vec<(f32, f32, f32)>>);
    let mut src_rows: HashMap<&'static str, (f32, Vec<CellData>)> = HashMap::new();

    for &(row_name, pitch) in &ROWS {
        let fl = if row_name == "M" {
            Some(level_l.clone())
        } else {
            extract_clip_frames(
                &args.clips,
                &args.prefix,
                &clip_name(row_name, "L"),
                tmp.path(),
            )?
        };
        let fr = extract_clip_frames(
            &args.clips,
            &args.prefix,
            &clip_name(row_name, "R"),
            tmp.path(),
        )?;
        if fl.is_none() || fr.is_none() {
            if row_name == "M" {
                bail!("need {}R.mp4 too", args.prefix);
            }
            continue;
        }
        let fl = fl.unwrap();
        let fr = fr.unwrap();

        let mut seq: Vec<(f32, PathBuf, [[f32; 3]; 2], Option<Vec<(f32, f32, f32)>>)> = Vec::new();
        let mut centre: Option<f32> = None;

        for (frames, is_left) in [(&fl, true), (&fr, false)] {
            let f0_rgb = image::open(&frames[0])?.to_rgb8().into_raw();
            let f_last_rgb = image::open(frames.last().unwrap())?.to_rgb8().into_raw();
            let warp = align_body_ecc(&plate_g, &rgb_to_gray_f32(&f0_rgb, w, h), w, h, body_y);
            let warp_last =
                align_body_ecc(&plate_g, &rgb_to_gray_f32(&f_last_rgb, w, h), w, h, body_y);
            let drift = (warp[0][2] - warp_last[0][2])
                .abs()
                .max((warp[1][2] - warp_last[1][2]).abs());
            let parent_name = frames[0]
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or(row_name);
            let warn = if drift > 6.0 {
                "  (high: was the camera locked off?)"
            } else {
                ""
            };
            println!("{}: body drift first->last {:.1}px{}", parent_name, drift, warn);

            if let Some(ref mut t) = tpl_tracker {
                t.reset();
            }

            for (fi, p) in frames.iter().enumerate() {
                let raw = image::open(p)?.to_rgb8().into_raw();
                let aligned = warp_affine_inverse_rgb(&raw, w, h, warp);
                let tracked = if let Some(ref dt) = dot_tracker {
                    dt.track(&aligned, w, h).map(|(m, e)| (m, Some(e)))
                } else if let Some(ref mut tt) = tpl_tracker {
                    tt.track(&aligned, w, h).map(|m| (m, None))
                } else {
                    None
                };
                let Some((m, eyes)) = tracked else {
                    continue;
                };
                if fi == 0 && !is_left {
                    centre = Some(m.0);
                }
                if !(is_left && fi == 0) {
                    seq.push((m.0, p.clone(), warp, eyes));
                }
            }
        }

        if seq.len() < cols {
            println!("row {}: only {} tracked frames, skipped", row_name, seq.len());
            continue;
        }

        let mut xs: Vec<f32> = seq.iter().map(|s| s.0).collect();
        let x_min = xs.iter().copied().fold(f32::INFINITY, f32::min);
        let x_max = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let cx = centre.unwrap_or_else(|| median_f32(&mut xs));
        let k = cols / 2;

        let mut targets = Vec::with_capacity(cols);
        for i in 0..k {
            targets.push(x_min + (cx - x_min) * (i as f32) / (k as f32));
        }
        targets.push(cx);
        for i in 1..=k {
            targets.push(cx + (x_max - cx) * (i as f32) / (k as f32));
        }

        let mut cells = Vec::with_capacity(cols);
        for &t in &targets {
            let best_s = seq
                .iter()
                .min_by(|a, b| (a.0 - t).abs().partial_cmp(&(b.0 - t).abs()).unwrap())
                .unwrap();
            let raw = image::open(&best_s.1)?.to_rgb8().into_raw();
            let aligned = warp_affine_inverse_rgb(&raw, w, h, best_s.2);
            let gain = bg_gain(&aligned, &plate_rgb, w, h);

            let mut crop = vec![0u8; bw * bh * 3];
            for cy in 0..bh {
                let py = (by + cy).min(h - 1);
                for cx_i in 0..bw {
                    let px = (bx + cx_i).min(w - 1);
                    let p_idx = (py * w + px) * 3;
                    let m_val = mask[py * w + px];
                    let c_idx = (cy * bw + cx_i) * 3;
                    for c in 0..3 {
                        let fg = (aligned[p_idx + c] as f32) * gain[c];
                        let bg = plate_rgb[p_idx + c] as f32;
                        let v = fg * m_val + bg * (1.0 - m_val);
                        crop[c_idx + c] = v.round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
            let rel_eyes = best_s.3.as_ref().map(|es| {
                es.iter()
                    .map(|&(ex, ey, er)| (ex - bx as f32, ey - by as f32, er))
                    .collect::<Vec<_>>()
            });
            cells.push((crop, rel_eyes));
        }
        println!(
            "row {}: {} tracked frames, swing {:.0} .. {:.0} .. {:.0}",
            row_name,
            seq.len(),
            x_min,
            cx,
            x_max
        );
        src_rows.insert(row_name, (pitch, cells));
    }

    let order: Vec<&'static str> = ROWS
        .iter()
        .map(|&(r, _)| r)
        .filter(|r| src_rows.contains_key(r))
        .collect();
    let mut grid: Vec<Vec<CellData>> = Vec::new();
    let mut pitches: Vec<f32> = Vec::new();

    for (i, &r) in order.iter().enumerate() {
        let (p0, cells0) = &src_rows[r];
        grid.push(cells0.clone());
        pitches.push(*p0);
        if i + 1 < order.len() {
            let (p1, cells1) = &src_rows[order[i + 1]];
            let n = (((p1 - p0).abs() * (args.between as f32)).round() as isize - 1).max(0) as usize;
            for j in 1..=n {
                let t = (j as f32) / ((n + 1) as f32);
                let mut morph_row = Vec::with_capacity(cols);
                for ((ca, ea), (cb, eb)) in cells0.iter().zip(cells1.iter()) {
                    let morphed = flow_morph_rgb(ca, cb, bw, bh, t);
                    let eyes = match (ea, eb) {
                        (Some(va), Some(vb)) => Some(
                            va.iter()
                                .zip(vb.iter())
                                .map(|(u, v)| {
                                    (
                                        (1.0 - t) * u.0 + t * v.0,
                                        (1.0 - t) * u.1 + t * v.1,
                                        (1.0 - t) * u.2 + t * v.2,
                                    )
                                })
                                .collect(),
                        ),
                        _ => None,
                    };
                    morph_row.push((morphed, eyes));
                }
                grid.push(morph_row);
                pitches.push(p0 + (p1 - p0) * t);
            }
        }
    }

    let sc = args.out_scale;
    let ow = ((bw as f32) * sc).max(1.0) as usize;
    let oh = ((bh as f32) * sc).max(1.0) as usize;
    let mut rows_json = Vec::new();

    for (ri, row) in grid.iter().enumerate() {
        let strip_w = ow * row.len();
        let mut strip = vec![0u8; strip_w * oh * 3];
        let mut cells_json = Vec::new();
        for (ci, (crop, eyes)) in row.iter().enumerate() {
            let resized = resize_rgb(crop, bw, bh, ow, oh);
            for y in 0..oh {
                let dst_start = (y * strip_w + ci * ow) * 3;
                let src_start = y * ow * 3;
                strip[dst_start..dst_start + ow * 3]
                    .copy_from_slice(&resized[src_start..src_start + ow * 3]);
            }
            if let Some(ref es) = eyes {
                let eye_coords: Vec<[f32; 3]> = es
                    .iter()
                    .map(|&(x, y, r)| {
                        [
                            ((x * sc) * 10.0).round() / 10.0,
                            ((y * sc) * 10.0).round() / 10.0,
                            ((r * sc) * 10.0).round() / 10.0,
                        ]
                    })
                    .collect();
                let skin = skin_tone(crop, bw, bh, es);
                cells_json.push(json!({ "eyes": eye_coords, "skin": skin }));
            } else {
                cells_json.push(json!({}));
            }
        }
        let name = format!("row_{}.webp", ri);
        let strip_im = RgbImage::from_raw(strip_w as u32, oh as u32, strip).unwrap();
        strip_im.save(args.out.join(&name))?;
        rows_json.push(json!({ "src": name, "cells": cells_json }));
        println!(
            "row {} (pitch {:+.2}): {} cells -> {}",
            ri,
            pitches[ri],
            cells_json.len(),
            name
        );
    }

    let plate_out = RgbImage::from_raw(w as u32, h as u32, plate_rgb).unwrap();
    plate_out.save(args.out.join("plate.webp"))?;

    let rig = json!({
        "version": 1,
        "plate": [w, h],
        "box": [bx, by, bw, bh],
        "frame": [ow, oh],
        "cols": cols,
        "center": cols / 2,
        "pitch": pitches.iter().map(|p| ((*p as f64) * 10000.0).round() / 10000.0).collect::<Vec<_>>(),
        "rows": rows_json,
    });
    std::fs::write(args.out.join("rig.json"), rig.to_string())?;
    println!("done: {} rows x {} poses -> {}", grid.len(), cols, args.out.display());
    Ok(())
}
