//! Pure-Rust image processing and computer vision primitives replacing NumPy, OpenCV, and Pillow.

use image::{ImageBuffer, Rgb, RgbImage, Rgba, RgbaImage};
use std::collections::VecDeque;

/// Computes percentile (0.0 ..= 100.0) of a slice of f32 values.
pub fn percentile(values: &[f32], pct: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f32> = values.iter().copied().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((pct / 100.0) * ((sorted.len() - 1) as f32)).clamp(0.0, (sorted.len() - 1) as f32);
    let lo = idx.floor() as usize;
    let hi = idx.ceil() as usize;
    let frac = idx - (lo as f32);
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// Computes median of a slice of f32 values.
pub fn median_f32(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 1 {
        values[mid]
    } else {
        0.5 * (values[mid - 1] + values[mid])
    }
}

/// Converts an RGB image (`w x h`) into grayscale `f32` (`w x h`, row-major).
pub fn rgb_to_gray_f32(rgb: &[u8], w: usize, h: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(w * h);
    for px in rgb.chunks_exact(3) {
        let r = px[0] as f32;
        let g = px[1] as f32;
        let b = px[2] as f32;
        out.push(0.299 * r + 0.587 * g + 0.114 * b);
    }
    out
}

/// Converts an RGB image (`w x h`) into grayscale `u8` (`w x h`, row-major).
pub fn rgb_to_gray_u8(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for px in rgb.chunks_exact(3) {
        let y = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32)
            .round()
            .clamp(0.0, 255.0) as u8;
        out.push(y);
    }
    out
}

/// Resizes an RGB buffer `(w, h)` to `(nw, nh)` using area/bilinear interpolation.
pub fn resize_rgb(rgb: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u8> {
    if w == nw && h == nh {
        return rgb.to_vec();
    }
    let img = RgbImage::from_raw(w as u32, h as u32, rgb.to_vec()).expect("valid rgb buffer");
    let resized = image::imageops::resize(
        &img,
        nw as u32,
        nh as u32,
        image::imageops::FilterType::Triangle,
    );
    resized.into_raw()
}

/// Resizes an RGBA buffer `(w, h)` to `(nw, nh)`.
pub fn resize_rgba(rgba: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u8> {
    if w == nw && h == nh {
        return rgba.to_vec();
    }
    let img = RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec()).expect("valid rgba buffer");
    let resized = image::imageops::resize(
        &img,
        nw as u32,
        nh as u32,
        image::imageops::FilterType::Triangle,
    );
    resized.into_raw()
}

/// Resizes a single-channel `u8` buffer `(w, h)` to `(nw, nh)`.
pub fn resize_gray_u8(gray: &[u8], w: usize, h: usize, nw: usize, nh: usize) -> Vec<u8> {
    if w == nw && h == nh {
        return gray.to_vec();
    }
    let img: ImageBuffer<image::Luma<u8>, Vec<u8>> =
        ImageBuffer::from_raw(w as u32, h as u32, gray.to_vec()).expect("valid gray buffer");
    let resized = image::imageops::resize(
        &img,
        nw as u32,
        nh as u32,
        image::imageops::FilterType::Triangle,
    );
    resized.into_raw()
}

/// Separable 2D Gaussian blur on a single-channel `f32` plane `(w, h)`.
pub fn gaussian_blur_f32(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.05 || w == 0 || h == 0 {
        return src.to_vec();
    }
    let radius = ((sigma * 3.0).ceil() as usize).max(1);
    let mut kernel = Vec::with_capacity(2 * radius + 1);
    let mut sum = 0.0f32;
    let inv_2sig2 = 1.0 / (2.0 * sigma * sigma);
    for i in 0..=(2 * radius) {
        let d = (i as isize - radius as isize) as f32;
        let wgt = (-d * d * inv_2sig2).exp();
        kernel.push(wgt);
        sum += wgt;
    }
    for v in &mut kernel {
        *v /= sum;
    }

    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let mut acc = 0.0f32;
            for (ki, &kw) in kernel.iter().enumerate() {
                let sx = (x as isize + ki as isize - radius as isize).clamp(0, w as isize - 1) as usize;
                acc += src[row + sx] * kw;
            }
            tmp[row + x] = acc;
        }
    }

    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0f32;
            for (ki, &kw) in kernel.iter().enumerate() {
                let sy = (y as isize + ki as isize - radius as isize).clamp(0, h as isize - 1) as usize;
                acc += tmp[sy * w + x] * kw;
            }
            out[y * w + x] = acc;
        }
    }
    out
}

/// Separable 2D Gaussian blur on a 3-channel `f32` plane `(w, h, 3)`.
pub fn gaussian_blur_rgb_f32(src: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    if sigma <= 0.05 || w == 0 || h == 0 {
        return src.to_vec();
    }
    let radius = ((sigma * 3.0).ceil() as usize).max(1);
    let mut kernel = Vec::with_capacity(2 * radius + 1);
    let mut sum = 0.0f32;
    let inv_2sig2 = 1.0 / (2.0 * sigma * sigma);
    for i in 0..=(2 * radius) {
        let d = (i as isize - radius as isize) as f32;
        let wgt = (-d * d * inv_2sig2).exp();
        kernel.push(wgt);
        sum += wgt;
    }
    for v in &mut kernel {
        *v /= sum;
    }

    let mut tmp = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sx = (x as isize + ki as isize - radius as isize).clamp(0, w as isize - 1) as usize;
                let idx = (y * w + sx) * 3;
                acc[0] += src[idx] * kw;
                acc[1] += src[idx + 1] * kw;
                acc[2] += src[idx + 2] * kw;
            }
            let dst = (y * w + x) * 3;
            tmp[dst] = acc[0];
            tmp[dst + 1] = acc[1];
            tmp[dst + 2] = acc[2];
        }
    }

    let mut out = vec![0.0f32; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 3];
            for (ki, &kw) in kernel.iter().enumerate() {
                let sy = (y as isize + ki as isize - radius as isize).clamp(0, h as isize - 1) as usize;
                let idx = (sy * w + x) * 3;
                acc[0] += tmp[idx] * kw;
                acc[1] += tmp[idx + 1] * kw;
                acc[2] += tmp[idx + 2] * kw;
            }
            let dst = (y * w + x) * 3;
            out[dst] = acc[0];
            out[dst + 1] = acc[1];
            out[dst + 2] = acc[2];
        }
    }
    out
}

/// 3x3 morphological erosion on `u8` plane (`cross_only = true` for 3x3 ellipse/cross).
pub fn erode_3x3(src: &[u8], w: usize, h: usize, cross_only: bool) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut mn = 255u8;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    if cross_only && dx != 0 && dy != 0 {
                        continue;
                    }
                    let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    mn = mn.min(src[sy * w + sx]);
                }
            }
            out[y * w + x] = mn;
        }
    }
    out
}

/// 3x3 morphological dilation on `u8` plane.
pub fn dilate_3x3(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut mx = 0u8;
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    let sy = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    let sx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    mx = mx.max(src[sy * w + sx]);
                }
            }
            out[y * w + x] = mx;
        }
    }
    out
}

/// Morphological opening (`erode` then `dilate`) with 3x3 box kernel.
pub fn morph_open_3x3(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let e = erode_3x3(src, w, h, false);
    dilate_3x3(&e, w, h)
}

/// Morphological closing (`dilate` then `erode`) with 3x3 box kernel.
pub fn morph_close_3x3(src: &[u8], w: usize, h: usize) -> Vec<u8> {
    let d = dilate_3x3(src, w, h);
    erode_3x3(&d, w, h, false)
}

/// Separable binary box dilation with radius `r` (window size `(2r+1) x (2r+1)`).
pub fn dilate_binary_box(src: &[bool], w: usize, h: usize, r: usize) -> Vec<bool> {
    let mut tmp = vec![false; w * h];
    for y in 0..h {
        let mut count = 0usize;
        let x_end = r.min(w.saturating_sub(1));
        for sx in 0..=x_end {
            if src[y * w + sx] {
                count += 1;
            }
        }
        for x in 0..w {
            tmp[y * w + x] = count > 0;
            if x + r + 1 < w && src[y * w + x + r + 1] {
                count += 1;
            }
            if x >= r && src[y * w + x - r] {
                count -= 1;
            }
        }
    }

    let mut out = vec![false; w * h];
    for x in 0..w {
        let mut count = 0usize;
        let y_end = r.min(h.saturating_sub(1));
        for sy in 0..=y_end {
            if tmp[sy * w + x] {
                count += 1;
            }
        }
        for y in 0..h {
            out[y * w + x] = count > 0;
            if y + r + 1 < h && tmp[(y + r + 1) * w + x] {
                count += 1;
            }
            if y >= r && tmp[(y - r) * w + x] {
                count -= 1;
            }
        }
    }
    out
}

/// Connected component statistics for 8-connected binary foreground regions.
#[derive(Debug, Clone, Copy)]
pub struct ComponentStat {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub area: usize,
    pub cx: f32,
    pub cy: f32,
}

/// Finds 8-connected components in a binary mask (`w x h`).
pub fn connected_components_with_stats(mask: &[bool], w: usize, h: usize) -> Vec<ComponentStat> {
    let mut visited = vec![false; w * h];
    let mut stats = Vec::new();
    let mut queue = VecDeque::new();

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if !mask[idx] || visited[idx] {
                continue;
            }
            visited[idx] = true;
            queue.push_back((x, y));

            let (mut min_x, mut max_x) = (x, x);
            let (mut min_y, mut max_y) = (y, y);
            let mut sum_x = 0usize;
            let mut sum_y = 0usize;
            let mut area = 0usize;

            while let Some((cx, cy)) = queue.pop_front() {
                min_x = min_x.min(cx);
                max_x = max_x.max(cx);
                min_y = min_y.min(cy);
                max_y = max_y.max(cy);
                sum_x += cx;
                sum_y += cy;
                area += 1;

                for dy in -1isize..=1 {
                    for dx in -1isize..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = cx as isize + dx;
                        let ny = cy as isize + dy;
                        if nx >= 0 && nx < w as isize && ny >= 0 && ny < h as isize {
                            let nidx = (ny as usize) * w + (nx as usize);
                            if mask[nidx] && !visited[nidx] {
                                visited[nidx] = true;
                                queue.push_back((nx as usize, ny as usize));
                            }
                        }
                    }
                }
            }

            stats.push(ComponentStat {
                x: min_x,
                y: min_y,
                w: max_x - min_x + 1,
                h: max_y - min_y + 1,
                area,
                cx: sum_x as f32 / area as f32,
                cy: sum_y as f32 / area as f32,
            });
        }
    }
    stats
}

/// Normalized Cross-Correlation (`TM_CCOEFF_NORMED`) template matching.
/// Returns `(best_score, best_x, best_y)`.
pub fn match_template_ccoeff_normed(
    img: &[f32],
    iw: usize,
    ih: usize,
    tpl: &[f32],
    tw: usize,
    th: usize,
) -> (f32, usize, usize) {
    if tw == 0 || th == 0 || iw < tw || ih < th {
        return (0.0, 0, 0);
    }
    let n = (tw * th) as f32;
    let tpl_mean: f32 = tpl.iter().sum::<f32>() / n;
    let mut tpl_zero = Vec::with_capacity(tw * th);
    let mut tpl_norm2 = 0.0f32;
    for &v in tpl {
        let d = v - tpl_mean;
        tpl_zero.push(d);
        tpl_norm2 += d * d;
    }
    if tpl_norm2 < 1e-6 {
        return (0.0, 0, 0);
    }

    // Integral images of `img` and `img^2` for O(1) window mean and variance
    let stride = iw + 1;
    let mut sat1 = vec![0.0f64; (iw + 1) * (ih + 1)];
    let mut sat2 = vec![0.0f64; (iw + 1) * (ih + 1)];
    for y in 0..ih {
        let mut r1 = 0.0f64;
        let mut r2 = 0.0f64;
        for x in 0..iw {
            let v = img[y * iw + x] as f64;
            r1 += v;
            r2 += v * v;
            sat1[(y + 1) * stride + (x + 1)] = sat1[y * stride + (x + 1)] + r1;
            sat2[(y + 1) * stride + (x + 1)] = sat2[y * stride + (x + 1)] + r2;
        }
    }

    let mut best_score = -1.0f32;
    let mut best_pos = (0usize, 0usize);

    for y in 0..=(ih - th) {
        for x in 0..=(iw - tw) {
            let x1 = x + tw;
            let y1 = y + th;
            let sum1 = sat1[y1 * stride + x1] - sat1[y * stride + x1] - sat1[y1 * stride + x]
                + sat1[y * stride + x];
            let sum2 = sat2[y1 * stride + x1] - sat2[y * stride + x1] - sat2[y1 * stride + x]
                + sat2[y * stride + x];
            let win_var = (sum2 - (sum1 * sum1) / (n as f64)).max(0.0) as f32;
            if win_var < 1e-6 {
                continue;
            }

            let mut num = 0.0f32;
            for ty in 0..th {
                let i_row = (y + ty) * iw + x;
                let t_row = ty * tw;
                for tx in 0..tw {
                    num += img[i_row + tx] * tpl_zero[t_row + tx];
                }
            }
            let score = num / (win_var * tpl_norm2).sqrt();
            if score > best_score {
                best_score = score;
                best_pos = (x, y);
            }
        }
    }

    (best_score, best_pos.0, best_pos.1)
}

/// Estimates a 2x3 translation/affine warp `[1, 0, dx; 0, 1, dy]` aligning `plate_gray` to `img_gray`
/// on the body region (`y >= body_y`).
pub fn align_body_ecc(
    plate_gray: &[f32],
    img_gray: &[f32],
    w: usize,
    h: usize,
    body_y: usize,
) -> [[f32; 3]; 2] {
    let y0 = body_y.min(h.saturating_sub(16));
    let bh = h - y0;
    if bh < 16 || w < 32 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    }
    let margin_x = (w / 10).max(8).min(32);
    let margin_y = (bh / 10).max(8).min(32);
    let tw = w.saturating_sub(2 * margin_x);
    let th = bh.saturating_sub(2 * margin_y);
    if tw < 16 || th < 16 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    }

    let mut tpl = Vec::with_capacity(tw * th);
    for y in (y0 + margin_y)..(y0 + margin_y + th) {
        tpl.extend_from_slice(&plate_gray[y * w + margin_x..y * w + margin_x + tw]);
    }
    let body_slice = &img_gray[y0 * w..];
    let (score, bx, by) = match_template_ccoeff_normed(body_slice, w, bh, &tpl, tw, th);
    if score < 0.3 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    }
    let dx = (bx as f32) - (margin_x as f32);
    let dy = (by as f32) - (margin_y as f32);
    [[1.0, 0.0, dx], [0.0, 1.0, dy]]
}

/// Applies inverse affine warp (`WARP_INVERSE_MAP` with bilinear interpolation and replicate border).
pub fn warp_affine_inverse_rgb(
    img: &[u8],
    w: usize,
    h: usize,
    warp: [[f32; 3]; 2],
) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let sx = warp[0][0] * (x as f32) + warp[0][1] * (y as f32) + warp[0][2];
            let sy = warp[1][0] * (x as f32) + warp[1][1] * (y as f32) + warp[1][2];
            let x0 = sx.floor() as isize;
            let y0 = sy.floor() as isize;
            let fx = sx - (x0 as f32);
            let fy = sy - (y0 as f32);

            let cx0 = x0.clamp(0, w as isize - 1) as usize;
            let cx1 = (x0 + 1).clamp(0, w as isize - 1) as usize;
            let cy0 = y0.clamp(0, h as isize - 1) as usize;
            let cy1 = (y0 + 1).clamp(0, h as isize - 1) as usize;

            let p00 = (cy0 * w + cx0) * 3;
            let p10 = (cy0 * w + cx1) * 3;
            let p01 = (cy1 * w + cx0) * 3;
            let p11 = (cy1 * w + cx1) * 3;
            let dst = (y * w + x) * 3;

            for c in 0..3 {
                let v0 = img[p00 + c] as f32 * (1.0 - fx) + img[p10 + c] as f32 * fx;
                let v1 = img[p01 + c] as f32 * (1.0 - fx) + img[p11 + c] as f32 * fx;
                out[dst + c] = (v0 * (1.0 - fy) + v1 * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Point-in-polygon check using ray-casting.
pub fn point_in_poly(x: f32, y: f32, poly: &[(i32, i32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i].0 as f32, poly[i].1 as f32);
        let (xj, yj) = (poly[j].0 as f32, poly[j].1 as f32);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi + 1e-6) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Morphs between two RGB images `a` and `b` `(w, h)` at fraction `t` (`0.0 ..= 1.0`).
pub fn flow_morph_rgb(a: &[u8], b: &[u8], w: usize, h: usize, t: f32) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 3];
    for i in 0..out.len() {
        let v = (a[i] as f32) * (1.0 - t) + (b[i] as f32) * t;
        out[i] = v.round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Generates a rounded-rectangle grayscale mask `(w, h)` with corner radius `radius`.
pub fn rounded_rect_mask(w: u32, h: u32, radius: u32) -> ImageBuffer<image::Luma<u8>, Vec<u8>> {
    let mut img = ImageBuffer::from_pixel(w, h, image::Luma([0u8]));
    let r = radius.min(w / 2).min(h / 2) as f32;
    for y in 0..h {
        for x in 0..w {
            let xf = x as f32;
            let yf = y as f32;
            let cx = xf.clamp(r, (w.saturating_sub(1) as f32) - r);
            let cy = yf.clamp(r, (h.saturating_sub(1) as f32) - r);
            let dist = ((xf - cx).powi(2) + (yf - cy).powi(2)).sqrt();
            if dist <= r + 0.5 {
                img.put_pixel(x, y, image::Luma([255u8]));
            }
        }
    }
    img
}

/// Crisp built-in 5x7 scalable pixel glyph renderer so text labels work with zero external font files.
pub fn draw_text_rgb(
    img: &mut RgbImage,
    x: i32,
    y: i32,
    text: &str,
    scale: u32,
    color: [u8; 3],
) {
    let scale = scale.max(1) as i32;
    let mut cursor_x = x;
    for ch in text.chars() {
        let glyph = glyph_5x7(ch);
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..5 {
                if (bits >> (4 - col)) & 1 == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = cursor_x + (col as i32) * scale + sx;
                            let py = y + (row as i32) * scale + sy;
                            if px >= 0
                                && py >= 0
                                && (px as u32) < img.width()
                                && (py as u32) < img.height()
                            {
                                img.put_pixel(px as u32, py as u32, Rgb(color));
                            }
                        }
                    }
                }
            }
        }
        cursor_x += 6 * scale;
    }
}

fn glyph_5x7(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        ':' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        '/' => [0b00001, 0b00010, 0b00100, 0b00100, 0b01000, 0b10000, 0b00000],
        _ => [0b00000; 7],
    }
}

/// Parses a 6-digit hex color (with or without `#`) into `[u8; 3]`.
pub fn parse_hex_rgb(hex: &str) -> anyhow::Result<[u8; 3]> {
    let s = hex.trim().trim_start_matches('#');
    if s.len() != 6 {
        anyhow::bail!("expected 6-char hex color, got {hex:?}");
    }
    let r = u8::from_str_radix(&s[0..2], 16)?;
    let g = u8::from_str_radix(&s[2..4], 16)?;
    let b = u8::from_str_radix(&s[4..6], 16)?;
    Ok([r, g, b])
}

/// Helper to composite an RGBA image onto another RGBA image at `(x, y)`.
pub fn alpha_composite(dst: &mut RgbaImage, src: &RgbaImage, x: i32, y: i32) {
    for sy in 0..src.height() {
        for sx in 0..src.width() {
            let dx = x + sx as i32;
            let dy = y + sy as i32;
            if dx < 0 || dy < 0 || dx >= dst.width() as i32 || dy >= dst.height() as i32 {
                continue;
            }
            let sp = src.get_pixel(sx, sy).0;
            let dp = dst.get_pixel(dx as u32, dy as u32).0;
            let sa = sp[3] as f32 / 255.0;
            let da = dp[3] as f32 / 255.0;
            let out_a = sa + da * (1.0 - sa);
            if out_a <= 1e-5 {
                dst.put_pixel(dx as u32, dy as u32, Rgba([0, 0, 0, 0]));
            } else {
                let r = ((sp[0] as f32 * sa + dp[0] as f32 * da * (1.0 - sa)) / out_a)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let g = ((sp[1] as f32 * sa + dp[1] as f32 * da * (1.0 - sa)) / out_a)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let b = ((sp[2] as f32 * sa + dp[2] as f32 * da * (1.0 - sa)) / out_a)
                    .round()
                    .clamp(0.0, 255.0) as u8;
                let a = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
                dst.put_pixel(dx as u32, dy as u32, Rgba([r, g, b, a]));
            }
        }
    }
}
