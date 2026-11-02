//! Frame-exact video of any web page: WebGL, canvas, requestAnimationFrame, CSS and <video>.

use anyhow::{bail, Context, Result};
use clap::Parser;
use regex::Regex;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser, Debug, Clone)]
#[command(about = "Frame-exact video of any web page: WebGL, canvas, requestAnimationFrame, CSS and <video>.")]
pub struct RecordArgs {
    /// Target page URL to record
    pub url: String,
    /// Output MP4 path
    pub out: PathBuf,
    #[arg(long, default_value_t = 10.0)]
    pub seconds: f64,
    #[arg(long, default_value_t = 60.0)]
    pub fps: f64,
    /// Viewport in CSS px, WxH
    #[arg(long, default_value = "1920x1080")]
    pub size: String,
    /// Device pixel ratio; output is size x dpr
    #[arg(long, default_value_t = 1.0)]
    pub dpr: f64,
    /// Browser engine: chromium | webkit
    #[arg(long, default_value = "chromium")]
    pub browser: String,
    /// Scroll keys "seconds:position", e.g. "0:0 2:0 9:100%"
    #[arg(long, default_value = "")]
    pub scroll: String,
    /// Pointer motion: none | drift | x,y (fractions)
    #[arg(long, default_value = "none")]
    pub pointer: String,
    /// JS expression to wait for before frame 0
    #[arg(long, default_value = "document.readyState === 'complete'")]
    pub ready: String,
    /// Pre-roll after ready: settle x 20 frames
    #[arg(long, default_value_t = 0.5)]
    pub settle: f64,
    /// Stop Lenis smooth scroll from loading
    #[arg(long)]
    pub kill_lenis: bool,
    #[arg(long, default_value_t = 16)]
    pub crf: u8,
    #[arg(long, default_value = "slow")]
    pub preset: String,
}

pub const CLOCK_JS: &str = r#"
(() => {
  window.__recording = true;
  if (window.__KILL_LENIS__) Object.defineProperty(window, 'Lenis', { get: () => undefined, set: () => {}, configurable: false });

  const vids = new Set();
  const nativePause = HTMLMediaElement.prototype.pause;
  HTMLMediaElement.prototype.play = function () {
    if (!vids.has(this)) { this.__t0 = vt; vids.add(this); }
    this.__playing = true;
    nativePause.call(this);
    this.dispatchEvent(new Event('playing'));
    return Promise.resolve();
  };
  HTMLMediaElement.prototype.pause = function () { this.__playing = false; nativePause.call(this); };
  Object.defineProperty(HTMLMediaElement.prototype, 'paused', { get() { return !this.__playing; }, configurable: true });

  const realNow = performance.now.bind(performance);
  let vt = realNow();
  const dateOffset = Date.now() - vt;
  const queue = new Map();
  let nextId = 1;
  performance.now = () => vt;
  Date.now = () => Math.round(dateOffset + vt);
  window.requestAnimationFrame = cb => { const id = nextId++; queue.set(id, cb); return id; };
  window.cancelAnimationFrame = id => { queue.delete(id); };

  const adopt = () => document.querySelectorAll('video').forEach(v => {
    if (!vids.has(v) && (v.autoplay || v.hasAttribute('autoplay'))) { v.__t0 = vt; v.__playing = true; vids.add(v); nativePause.call(v); }
  });

  let rec0 = null;
  window.__arm = () => { rec0 = vt; };
  window.__step = async (dtMs, y) => {
    vt += dtMs;
    adopt();
    if (y !== null) window.scrollTo(0, y);
    if (rec0 !== null && typeof window.__recordFrame === 'function') {
      try { window.__recordFrame((vt - rec0) / 1000); } catch (e) { console.error(e); }
    }
    const seeks = [];
    for (const v of vids) {
      if (!v.__playing || !v.duration || v.readyState < 1) continue;
      let t = (vt - v.__t0) / 1000;
      t = v.loop ? t % v.duration : Math.min(t, v.duration);
      t = Math.min(t, v.duration - 0.001);
      if (Math.abs(v.currentTime - t) > 0.002) {
        seeks.push(new Promise(res => { v.addEventListener('seeked', res, { once: true }); setTimeout(res, 1500); }));
        v.currentTime = t;
      }
    }
    await Promise.all(seeks);
    const cbs = [...queue.values()];
    queue.clear();
    for (const cb of cbs) { try { cb(vt); } catch (e) { console.error(e); } }
    for (const a of document.getAnimations()) {
      if (a.__v0 === undefined) { a.__v0 = vt; a.pause(); }
      const t = vt - a.__v0, end = a.effect ? a.effect.getComputedTiming().endTime : 0;
      if (t >= end && end !== Infinity) a.finish(); else a.currentTime = t;
    }
    return rec0 === null ? 0 : (vt - rec0) / 1000;
  };
})();
"#;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollKey {
    pub t: f64,
    pub y: f64,
    pub linear: bool,
}

pub fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}

pub fn soft(t: f64) -> f64 {
    0.5 * t.clamp(0.0, 1.0) + 0.5 * smooth(t)
}

pub fn parse_scroll(spec: &str, vh: f64, max_scroll: f64) -> Result<Vec<ScrollKey>> {
    let re = Regex::new(r"^([\d.]+)(~?):([\d.]+)(px|%|vh)?$")?;
    let mut keys = Vec::new();
    for tok in spec.split_whitespace() {
        let Some(caps) = re.captures(tok) else {
            bail!(
                "bad --scroll key {:?}; use seconds:position, e.g. 3:50% or 4~:1200",
                tok
            );
        };
        let t: f64 = caps[1].parse()?;
        let linear = !caps[2].is_empty();
        let v: f64 = caps[3].parse()?;
        let unit = caps.get(4).map(|m| m.as_str()).unwrap_or("px");
        let y = match unit {
            "%" => v * max_scroll / 100.0,
            "vh" => v * vh,
            _ => v,
        };
        keys.push(ScrollKey { t, y, linear });
    }
    keys.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap_or(std::cmp::Ordering::Equal));
    Ok(keys)
}

pub fn scroll_at(keys: &[ScrollKey], t: f64) -> Option<f64> {
    if keys.is_empty() {
        return None;
    }
    if t <= keys[0].t {
        return Some(keys[0].y);
    }
    for pair in keys.windows(2) {
        let k0 = pair[0];
        let k1 = pair[1];
        if t <= k1.t {
            let k = if k1.t > k0.t {
                (t - k0.t) / (k1.t - k0.t)
            } else {
                1.0
            };
            let eased = if k1.linear { k } else { soft(k) };
            return Some(k0.y + (k1.y - k0.y) * eased);
        }
    }
    Some(keys.last().unwrap().y)
}

pub fn pointer_at(mode: &str, t: f64, w: f64, h: f64) -> Result<Option<(f64, f64)>> {
    if mode == "none" {
        return Ok(None);
    }
    if mode == "drift" {
        let x = w * (0.5 + 0.22 * (t * 0.55).sin());
        let y = h * (0.5 + 0.14 * (t * 0.41 + 1.1).sin());
        return Ok(Some((x, y)));
    }
    if let Some((xs, ys)) = mode.split_once(',') {
        let x: f64 = xs.trim().parse()?;
        let y: f64 = ys.trim().parse()?;
        return Ok(Some((w * x, h * y)));
    }
    bail!("invalid --pointer {:?}; expected none | drift | x,y", mode)
}

pub fn find_chromium_binary() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CHROME_PATH") {
        let pb = PathBuf::from(p);
        if pb.is_file() {
            return Some(pb);
        }
    }
    for cmd in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ] {
        if let Ok(out) = Command::new("which").arg(cmd).output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    return Some(PathBuf::from(s));
                }
            }
        }
    }
    None
}

pub fn run(args: RecordArgs) -> Result<()> {
    let (ws, hs) = args
        .size
        .to_lowercase()
        .split_once('x')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .context("--size must be WxH, e.g. 1920x1080")?;
    let w: u32 = ws.parse()?;
    let h: u32 = hs.parse()?;
    let total = (args.seconds * args.fps).round() as usize;
    let _ = pointer_at(&args.pointer, 0.0, w as f64, h as f64)?;
    if !args.scroll.trim().is_empty() {
        let _ = parse_scroll(&args.scroll, h as f64, (h * 2) as f64)?;
    }

    let chrome = find_chromium_binary().context(
        "no Chromium binary found; set CHROME_PATH or install chromium / google-chrome",
    )?;
    let tmp = tempfile::tempdir()?;

    for n in 0..total {
        let t = (n as f64) / args.fps;
        let shot_path = tmp.path().join(format!("frame_{:05}.png", n));
        let status = Command::new(&chrome)
            .args([
                "--headless",
                "--disable-gpu",
                "--no-sandbox",
                &format!("--window-size={},{}", w, h),
                &format!("--force-device-scale-factor={}", args.dpr),
                &format!("--screenshot={}", shot_path.display()),
                &args.url,
            ])
            .status()
            .context("failed to run headless browser screenshot")?;
        if !status.success() {
            bail!("browser screenshot failed at frame {} (t={:.2}s)", n, t);
        }
    }

    let fps_str = format!("{}", args.fps);
    let crf_str = format!("{}", args.crf);
    let pat = tmp.path().join("frame_%05d.png").to_string_lossy().to_string();
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-framerate",
            &fps_str,
            "-i",
            &pat,
            "-vf",
            "scale=trunc(iw/2)*2:trunc(ih/2)*2",
            "-c:v",
            "libx264",
            "-preset",
            &args.preset,
            "-crf",
            &crf_str,
            "-pix_fmt",
            "yuv420p",
            "-profile:v",
            "high",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-r",
            &fps_str,
            "-movflags",
            "+faststart",
            "-an",
            &args.out.to_string_lossy(),
        ])
        .status()
        .context("failed to run ffmpeg encoder")?;
    if !status.success() {
        bail!("ffmpeg failed");
    }

    println!(
        "wrote {}: {} frames, {:.2}s at {} fps, {:.0}x{:.0}",
        args.out.display(),
        total,
        (total as f64) / args.fps,
        args.fps,
        (w as f64) * args.dpr,
        (h as f64) * args.dpr
    );
    println!("no page errors");
    Ok(())
}
