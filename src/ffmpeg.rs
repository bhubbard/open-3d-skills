//! FFmpeg and FFprobe helpers for decoding frames, probing video metadata, and encoding outputs.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Copy)]
pub struct VideoMeta {
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    pub duration: f64,
}

/// Probes video dimensions, frame rate, and duration via `ffprobe`.
pub fn probe_video(path: &Path) -> Result<VideoMeta> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,r_frame_rate,avg_frame_rate:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("failed to execute ffprobe on {}", path.display()))?;

    if !output.status.success() {
        anyhow::bail!(
            "ffprobe failed on {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let j: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let stream = j
        .get("streams")
        .and_then(|s| s.get(0))
        .context("ffprobe returned no video streams")?;

    let width = stream
        .get("width")
        .and_then(|v| v.as_u64())
        .context("missing width")? as usize;
    let height = stream
        .get("height")
        .and_then(|v| v.as_u64())
        .context("missing height")? as usize;

    let fps_str = stream
        .get("avg_frame_rate")
        .and_then(|v| v.as_str())
        .filter(|s| *s != "0/0")
        .or_else(|| stream.get("r_frame_rate").and_then(|v| v.as_str()))
        .unwrap_or("24/1");
    let fps = parse_rational(fps_str).unwrap_or(24.0);

    let duration = j
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| d.as_f64()))
        .unwrap_or(0.0);

    Ok(VideoMeta {
        width,
        height,
        fps,
        duration,
    })
}

fn parse_rational(s: &str) -> Option<f64> {
    if let Some((num, den)) = s.split_once('/') {
        let n: f64 = num.parse().ok()?;
        let d: f64 = den.parse().ok()?;
        if d > 0.0 {
            return Some(n / d);
        }
    }
    s.parse().ok()
}

/// Reads all frames from an image (`.png`, `.jpg`, `.jpeg`, `.webp`) or video file as `(width, height, fps, Vec<Vec<u8>>)`.
pub fn read_rgb_frames(path: &Path) -> Result<(usize, usize, f64, Vec<Vec<u8>>)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") {
        let img = image::open(path)
            .with_context(|| format!("failed to open image {}", path.display()))?
            .to_rgb8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        return Ok((w, h, 24.0, vec![img.into_raw()]));
    }

    let meta = probe_video(path)?;
    let (w, h) = (meta.width, meta.height);
    let frame_bytes = w * h * 3;

    let mut child = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .stdout(Stdio::piped())
        .spawn()
        .context("failed to spawn ffmpeg decoder")?;

    let mut stdout = child.stdout.take().context("missing ffmpeg stdout")?;
    let mut frames = Vec::new();
    loop {
        let mut buf = vec![0u8; frame_bytes];
        match stdout.read_exact(&mut buf) {
            Ok(()) => frames.push(buf),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
    }
    let _ = child.wait();
    Ok((w, h, meta.fps, frames))
}

/// Spawns an `ffmpeg` subprocess reading from stdin.
pub fn spawn_ffmpeg_pipe(args: &[&str]) -> Result<Child> {
    Command::new("ffmpeg")
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .context("failed to spawn ffmpeg encoder")
}
