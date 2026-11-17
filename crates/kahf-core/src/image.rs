//! Small image transfer for the session File path.
//!
//! Pipeline (encode): box-downscale RGB -> quantise each channel to `bits` levels
//! -> horizontal delta (left neighbour, first column uses the pixel above) ->
//! zstd. Header is 8 bytes: magic "KI", version 1, bits, width u16 LE, height u16 LE.
//! Only royalty-free / patent-free building blocks are used (no JPEG/WebP/HEIC).

const MAGIC: [u8; 2] = *b"KI";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 8;
const MAX_DIM: u32 = 4096;
const ZSTD_LEVEL: i32 = 19;

/// Downscale factors tried by `encode_image`, highest first.
const SCALES: [f64; 5] = [1.0, 0.75, 0.5, 0.375, 0.25];
/// Channel bit depths tried by `encode_image`.
const BITS: [u8; 5] = [8, 6, 5, 4, 3];

/// Encode RGB (8 bits per channel, row-major, `width * height * 3` bytes) into at most
/// `max_bytes` bytes. Tries (scale, bit depth) pairs in order of decreasing pixel count
/// times bit depth and returns the first candidate that fits.
pub fn encode_image(rgb: &[u8], width: u32, height: u32, max_bytes: usize) -> Result<Vec<u8>, String> {
    check_dims(width, height)?;
    let (w, h) = (width as usize, height as usize);
    if rgb.len() != w * h * 3 {
        return Err(format!(
            "rgb length {} does not match {}x{}x3",
            rgb.len(),
            width,
            height
        ));
    }

    let mut candidates: Vec<(f64, f64, u8)> = Vec::new();
    for &s in &SCALES {
        for &b in &BITS {
            candidates.push((s, s * s * (w * h) as f64 * b as f64, b));
        }
    }
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    for (scale, _, bits) in candidates {
        let (tw, th) = target_dims(width, height, scale);
        let small = downscale(rgb, w, h, tw as usize, th as usize);
        let out = encode_fixed(&small, tw, th, bits)?;
        if out.len() <= max_bytes {
            return Ok(out);
        }
    }
    Err(format!("image does not fit in {} bytes", max_bytes))
}

/// Decode data produced by `encode_image`. Returns (width, height, RGB bytes).
pub fn decode_image(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if data.len() < HEADER_LEN {
        return Err("image data too short".into());
    }
    if data[0..2] != MAGIC {
        return Err("bad image magic".into());
    }
    if data[2] != VERSION {
        return Err(format!("unsupported image version {}", data[2]));
    }
    let bits = data[3];
    if !(1..=8).contains(&bits) {
        return Err(format!("bad bit depth {}", bits));
    }
    let width = u16::from_le_bytes([data[4], data[5]]) as u32;
    let height = u16::from_le_bytes([data[6], data[7]]) as u32;
    check_dims(width, height)?;
    let (w, h) = (width as usize, height as usize);
    let expected = w * h * 3;

    let residual = zstd::bulk::decompress(&data[HEADER_LEN..], expected)
        .map_err(|e| format!("zstd decode failed: {}", e))?;
    if residual.len() != expected {
        return Err("decoded length mismatch".into());
    }

    let levels = (1u32 << bits) - 1;
    let mut q = vec![0u8; expected];
    for i in 0..expected {
        let pred = predictor(&q, i, w);
        let v = residual[i].wrapping_add(pred);
        if v as u32 > levels {
            return Err("quantised value out of range".into());
        }
        q[i] = v;
    }
    let rgb = q.iter().map(|&v| reconstruct(v, levels)).collect();
    Ok((width, height, rgb))
}

/// Encode RGB as a PNG file (for display or saving the decoded image).
pub fn encode_png(rgb: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    check_dims(width, height)?;
    if rgb.len() != width as usize * height as usize * 3 {
        return Err("rgb length does not match dimensions".into());
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgb).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Decode a PNG (any colour type and bit depth) into (width, height, 8-bit RGB).
pub fn decode_png(data: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    check_dims(info.width, info.height)?;
    let px = info.width as usize * info.height as usize;
    let rgb = match info.color_type {
        png::ColorType::Rgb => buf[..px * 3].to_vec(),
        png::ColorType::Rgba => buf[..px * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect(),
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..px * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0]])
            .collect(),
        png::ColorType::Indexed => return Err("indexed PNG not supported after expansion".into()),
    };
    Ok((info.width, info.height, rgb))
}

fn check_dims(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("zero image dimension".into());
    }
    if width > MAX_DIM || height > MAX_DIM {
        return Err(format!("image dimension above {}", MAX_DIM));
    }
    Ok(())
}

fn target_dims(width: u32, height: u32, scale: f64) -> (u32, u32) {
    let tw = ((width as f64 * scale).round() as u32).max(1);
    let th = ((height as f64 * scale).round() as u32).max(1);
    (tw, th)
}

fn encode_fixed(rgb: &[u8], width: u32, height: u32, bits: u8) -> Result<Vec<u8>, String> {
    let w = width as usize;
    let levels = (1u32 << bits) - 1;
    let q: Vec<u8> = rgb
        .iter()
        .map(|&v| ((v as u32 * levels + 127) / 255) as u8)
        .collect();

    let mut residual = Vec::with_capacity(q.len());
    for i in 0..q.len() {
        let pred = predictor(&q, i, w);
        residual.push(q[i].wrapping_sub(pred));
    }

    let payload = zstd::bulk::compress(&residual, ZSTD_LEVEL).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(bits);
    out.extend_from_slice(&(width as u16).to_le_bytes());
    out.extend_from_slice(&(height as u16).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Predictor for byte index `i` (interleaved RGB, row width `w` pixels): the same
/// channel of the left pixel, or of the pixel above on the first column, or 0.
fn predictor(q: &[u8], i: usize, w: usize) -> u8 {
    let pixel = i / 3;
    let x = pixel % w;
    if x > 0 {
        q[i - 3]
    } else if pixel >= w {
        q[i - 3 * w]
    } else {
        0
    }
}

fn reconstruct(q: u8, levels: u32) -> u8 {
    ((q as u32 * 255 + levels / 2) / levels) as u8
}

/// Box-filter downscale of 8-bit RGB to `tw x th` (each output pixel is the mean of its block).
fn downscale(rgb: &[u8], w: usize, h: usize, tw: usize, th: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(tw * th * 3);
    for ty in 0..th {
        let y0 = ty * h / th;
        let y1 = ((ty + 1) * h / th).max(y0 + 1);
        for tx in 0..tw {
            let x0 = tx * w / tw;
            let x1 = ((tx + 1) * w / tw).max(x0 + 1);
            let mut sum = [0u64; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = (y * w + x) * 3;
                    sum[0] += rgb[p] as u64;
                    sum[1] += rgb[p + 1] as u64;
                    sum[2] += rgb[p + 2] as u64;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u64;
            for s in sum {
                out.push(((s + n / 2) / n) as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                v.push((x * 255 / (w - 1)) as u8);
                v.push((y * 255 / (h - 1)) as u8);
                v.push(((x + y) * 255 / (w + h - 2)) as u8);
            }
        }
        v
    }

    fn mae(a: &[u8], b: &[u8]) -> f64 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .map(|(&x, &y)| (x as f64 - y as f64).abs())
            .sum::<f64>()
            / a.len() as f64
    }

    #[test]
    fn round_trip_full_quality_is_exact() {
        let (w, h) = (320, 240);
        let img = gradient(w, h);
        let enc = encode_image(&img, w, h, 1 << 20).unwrap();
        let (dw, dh, dec) = decode_image(&enc).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert!(mae(&dec, &img) < 1.0, "MAE {}", mae(&dec, &img));
    }

    #[test]
    fn fits_budget_and_stays_close_to_reference() {
        let (w, h) = (320, 240);
        let img = gradient(w, h);
        for budget in [4096usize, 16384] {
            let enc = encode_image(&img, w, h, budget).unwrap();
            assert!(enc.len() <= budget, "{} > {}", enc.len(), budget);
            let (dw, dh, dec) = decode_image(&enc).unwrap();
            let (tw, th) = (dw as usize, dh as usize);
            let reference = downscale(&img, w as usize, h as usize, tw, th);
            let err = mae(&dec, &reference);
            assert!(err < 6.0, "budget {} MAE {} too high", budget, err);
        }
    }

    #[test]
    fn png_round_trip_is_exact() {
        let (w, h) = (17, 9);
        let img = gradient(w, h);
        let png = encode_png(&img, w, h).unwrap();
        let (dw, dh, dec) = decode_png(&png).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(dec, img);
    }

    #[test]
    fn bad_input_returns_err() {
        let img = gradient(8, 8);
        assert!(encode_image(&img[..10], 8, 8, 4096).is_err());
        assert!(encode_image(&img, 0, 8, 4096).is_err());
        assert!(encode_image(&img, 8, 8, 4).is_err());
        assert!(encode_image(&img, 8, 8, 0).is_err());
        assert!(decode_image(&[]).is_err());
        assert!(decode_image(b"XXXXXXXXXXXX").is_err());
        let enc = encode_image(&img, 8, 8, 4096).unwrap();
        assert!(decode_image(&enc[..enc.len() / 2]).is_err());
        let mut bad = enc.clone();
        bad[3] = 0;
        assert!(decode_image(&bad).is_err());
        let mut zero = enc.clone();
        zero[4] = 0;
        zero[5] = 0;
        assert!(decode_image(&zero).is_err());
        assert!(decode_png(b"not a png").is_err());
    }
}
