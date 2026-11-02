//! Static file server with HTTP Range support, for previewing and recording pages.

use anyhow::{Context, Result};
use clap::Parser;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};

#[derive(Parser, Debug, Clone)]
#[command(about = "Static file server with HTTP Range support, for previewing and recording pages.")]
pub struct ServeArgs {
    /// Directory to serve
    #[arg(default_value = ".")]
    pub dir: PathBuf,
    /// Port to listen on
    #[arg(long, default_value_t = 8000)]
    pub port: u16,
    /// Host address to bind
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
}

pub fn mime_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "mjs" | "js" => "text/javascript",
        "wasm" => "application/wasm",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "webm" => "video/webm",
        "mp4" => "video/mp4",
        "json" => "application/json",
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Parses an HTTP `Range: bytes=start-end` header against a file of `size` bytes.
/// Returns `Some(Ok((start, end)))` for a valid range, `Some(Err(()))` for 416 out-of-bounds,
/// or `None` if the header is malformed and should fall back to a full 200 response.
pub fn parse_byte_range(header: &str, size: u64) -> Option<std::result::Result<(u64, u64), ()>> {
    let s = header.trim().strip_prefix("bytes=")?;
    let (left, right) = s.split_once('-')?;
    if left.is_empty() && right.is_empty() {
        return None;
    }
    if size == 0 {
        return Some(Err(()));
    }
    if !left.is_empty() {
        let start: u64 = left.parse().ok()?;
        let end: u64 = if !right.is_empty() {
            right.parse().ok()?
        } else {
            size - 1
        };
        if start >= size || start > end {
            return Some(Err(()));
        }
        Some(Ok((start, end.min(size - 1))))
    } else {
        let suffix: u64 = right.parse().ok()?;
        if suffix == 0 {
            return Some(Err(()));
        }
        let start = size.saturating_sub(suffix);
        Some(Ok((start, size - 1)))
    }
}

fn resolve_safe_path(root: &Path, req_path: &str) -> PathBuf {
    let without_query = req_path.split('?').next().unwrap_or("/");
    let trimmed = without_query.trim_start_matches('/');
    let mut out = root.to_path_buf();
    for comp in Path::new(trimmed).components() {
        if let Component::Normal(part) = comp {
            out.push(part);
        }
    }
    if out.is_dir() {
        out.push("index.html");
    }
    out
}

pub fn handle_client(mut stream: TcpStream, root: &Path) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];
    let req_path = parts[1];

    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("range") {
                range_header = Some(v.trim().to_string());
            }
        }
    }

    let file_path = resolve_safe_path(root, req_path);
    if !file_path.is_file() {
        let resp = "HTTP/1.1 404 Not Found\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\nContent-Length: 9\r\n\r\nNot Found";
        stream.write_all(resp.as_bytes())?;
        return Ok(());
    }

    let size = std::fs::metadata(&file_path)?.len();
    let content_type = mime_type_for(&file_path);

    if let Some(ref rng) = range_header {
        match parse_byte_range(rng, size) {
            Some(Ok((start, end))) => {
                let len = end - start + 1;
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: {}\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\n\r\n",
                    content_type, start, end, size, len
                );
                stream.write_all(header.as_bytes())?;
                if method != "HEAD" {
                    let mut f = File::open(&file_path)?;
                    f.seek(SeekFrom::Start(start))?;
                    let mut remaining = len;
                    let mut buf = [0u8; 65536];
                    while remaining > 0 {
                        let to_read = (buf.len() as u64).min(remaining) as usize;
                        let n = f.read(&mut buf[..to_read])?;
                        if n == 0 {
                            break;
                        }
                        stream.write_all(&buf[..n])?;
                        remaining -= n as u64;
                    }
                }
                return Ok(());
            }
            Some(Err(())) => {
                let header = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\nContent-Length: 0\r\n\r\n",
                    size
                );
                stream.write_all(header.as_bytes())?;
                return Ok(());
            }
            None => {}
        }
    }

    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccept-Ranges: bytes\r\n\r\n",
        content_type, size
    );
    stream.write_all(header.as_bytes())?;
    if method != "HEAD" {
        let mut f = File::open(&file_path)?;
        std::io::copy(&mut f, &mut stream)?;
    }
    Ok(())
}

pub fn run(args: ServeArgs) -> Result<()> {
    let abs_dir = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("failed to resolve directory {}", args.dir.display()))?;
    let bind_addr = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&bind_addr)
        .with_context(|| format!("failed to bind http://{}", bind_addr))?;
    println!(
        "serving {} at http://{}:{}/",
        abs_dir.display(),
        args.host,
        args.port
    );

    for incoming in listener.incoming() {
        if let Ok(stream) = incoming {
            let root = abs_dir.clone();
            std::thread::spawn(move || {
                let _ = handle_client(stream, &root);
            });
        }
    }
    Ok(())
}
