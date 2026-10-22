//! Proprietary PNG, BMP, and Netpbm image decoder, encoder, and resampler.

use std::fmt;

use flux_compress::{CompressError, compress_zlib, crc32, decompress_zlib};

const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    UnexpectedEof,
    UnsupportedFormat(&'static str),
    InvalidHeader(&'static str),
    InvalidFilter(u8),
    Compression(CompressError),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of image file"),
            Self::UnsupportedFormat(msg) => write!(f, "unsupported image format: {msg}"),
            Self::InvalidHeader(msg) => write!(f, "invalid image header: {msg}"),
            Self::InvalidFilter(kind) => write!(f, "invalid PNG scanline filter {kind}"),
            Self::Compression(err) => write!(f, "image decompression failed: {err}"),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<CompressError> for ImageError {
    fn from(value: CompressError) -> Self {
        Self::Compression(value)
    }
}

/// Top-down 8-bit RGBA image buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        Self {
            width,
            height,
            pixels: vec![0; len],
        }
    }

    pub fn from_rgba(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, ImageError> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if pixels.len() != expected {
            return Err(ImageError::InvalidHeader("RGBA pixel buffer size mismatch"));
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = ((y * self.width + x) as usize) * 4;
        Some([
            self.pixels[idx],
            self.pixels[idx + 1],
            self.pixels[idx + 2],
            self.pixels[idx + 3],
        ])
    }

    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = ((y * self.width + x) as usize) * 4;
        self.pixels[idx..idx + 4].copy_from_slice(&rgba);
    }

    /// Resample this image to `(new_width, new_height)` using bilinear filtering.
    pub fn resize_bilinear(&self, new_width: u32, new_height: u32) -> Self {
        if new_width == 0 || new_height == 0 || self.width == 0 || self.height == 0 {
            return Self::new(new_width, new_height);
        }
        if new_width == self.width && new_height == self.height {
            return self.clone();
        }
        let mut out = Self::new(new_width, new_height);
        let x_ratio = (self.width as f32) / (new_width as f32);
        let y_ratio = (self.height as f32) / (new_height as f32);

        for ny in 0..new_height {
            let sy = ((ny as f32 + 0.5) * y_ratio - 0.5).clamp(0.0, (self.height - 1) as f32);
            let y0 = sy.floor() as u32;
            let y1 = (y0 + 1).min(self.height - 1);
            let wy = sy - (y0 as f32);

            for nx in 0..new_width {
                let sx = ((nx as f32 + 0.5) * x_ratio - 0.5).clamp(0.0, (self.width - 1) as f32);
                let x0 = sx.floor() as u32;
                let x1 = (x0 + 1).min(self.width - 1);
                let wx = sx - (x0 as f32);

                let c00 = self.pixel(x0, y0).unwrap_or([0; 4]);
                let c10 = self.pixel(x1, y0).unwrap_or([0; 4]);
                let c01 = self.pixel(x0, y1).unwrap_or([0; 4]);
                let c11 = self.pixel(x1, y1).unwrap_or([0; 4]);

                let mut rgba = [0u8; 4];
                for ch in 0..4 {
                    let top = (c00[ch] as f32) * (1.0 - wx) + (c10[ch] as f32) * wx;
                    let bot = (c01[ch] as f32) * (1.0 - wx) + (c11[ch] as f32) * wx;
                    let val = top * (1.0 - wy) + bot * wy;
                    rgba[ch] = val.round().clamp(0.0, 255.0) as u8;
                }
                out.set_pixel(nx, ny, rgba);
            }
        }
        out
    }
}

/// Automatically detect and decode a PNG, BMP, or PPM/PGM image.
pub fn decode_auto(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        decode_png(bytes)
    } else if bytes.starts_with(b"BM") {
        decode_bmp(bytes)
    } else if bytes.starts_with(b"P6") || bytes.starts_with(b"P5") {
        decode_ppm(bytes)
    } else {
        Err(ImageError::UnsupportedFormat(
            "unrecognized image signature",
        ))
    }
}

/// Decode a PNG image into an RGBA [`Image`].
pub fn decode_png(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 8 || bytes[..8] != PNG_SIGNATURE {
        return Err(ImageError::InvalidHeader("missing PNG signature"));
    }

    let mut pos = 8usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut color_type = 0u8;
    let mut saw_ihdr = false;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut idat = Vec::new();

    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let kind = &bytes[pos + 4..pos + 8];
        pos += 8;
        if pos + len + 4 > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        let chunk_data = &bytes[pos..pos + len];
        pos += len + 4; // skip 4-byte chunk CRC

        match kind {
            b"IHDR" => {
                if chunk_data.len() < 13 {
                    return Err(ImageError::InvalidHeader("short IHDR chunk"));
                }
                width = u32::from_be_bytes([
                    chunk_data[0],
                    chunk_data[1],
                    chunk_data[2],
                    chunk_data[3],
                ]);
                height = u32::from_be_bytes([
                    chunk_data[4],
                    chunk_data[5],
                    chunk_data[6],
                    chunk_data[7],
                ]);
                let bit_depth = chunk_data[8];
                color_type = chunk_data[9];
                let compression = chunk_data[10];
                let filter = chunk_data[11];
                let interlace = chunk_data[12];
                if width == 0 || height == 0 {
                    return Err(ImageError::InvalidHeader("zero-sized PNG"));
                }
                if bit_depth != 8 {
                    return Err(ImageError::UnsupportedFormat(
                        "only 8-bit PNG depth supported",
                    ));
                }
                if compression != 0 || filter != 0 || interlace != 0 {
                    return Err(ImageError::UnsupportedFormat(
                        "interlaced PNG not supported",
                    ));
                }
                saw_ihdr = true;
            }
            b"PLTE" => {
                for rgb in chunk_data.chunks_exact(3) {
                    palette.push([rgb[0], rgb[1], rgb[2]]);
                }
            }
            b"tRNS" => {
                trns.extend_from_slice(chunk_data);
            }
            b"IDAT" => {
                idat.extend_from_slice(chunk_data);
            }
            b"IEND" => break,
            _ => {}
        }
    }

    if !saw_ihdr {
        return Err(ImageError::InvalidHeader("missing IHDR chunk"));
    }

    let bpp = match color_type {
        0 => 1usize, // Grayscale
        2 => 3usize, // RGB
        3 => 1usize, // Indexed palette
        4 => 2usize, // Grayscale + Alpha
        6 => 4usize, // RGBA
        _ => return Err(ImageError::UnsupportedFormat("unsupported PNG color type")),
    };

    let raw = decompress_zlib(&idat)?;
    let stride = (width as usize) * bpp;
    let expected_raw = (height as usize) * (1 + stride);
    if raw.len() < expected_raw {
        return Err(ImageError::UnexpectedEof);
    }

    let mut prev_row = vec![0u8; stride];
    let mut cur_row = vec![0u8; stride];
    let mut out = Image::new(width, height);

    for y in 0..(height as usize) {
        let row_start = y * (1 + stride);
        let filter = raw[row_start];
        let encoded = &raw[row_start + 1..row_start + 1 + stride];

        for i in 0..stride {
            let x = encoded[i];
            let a = if i >= bpp { cur_row[i - bpp] } else { 0 };
            let b = prev_row[i];
            let c = if i >= bpp { prev_row[i - bpp] } else { 0 };

            cur_row[i] = match filter {
                0 => x,
                1 => x.wrapping_add(a),
                2 => x.wrapping_add(b),
                3 => {
                    let avg = ((u16::from(a) + u16::from(b)) / 2) as u8;
                    x.wrapping_add(avg)
                }
                4 => x.wrapping_add(paeth_predictor(a, b, c)),
                other => return Err(ImageError::InvalidFilter(other)),
            };
        }

        for x in 0..(width as usize) {
            let rgba = match color_type {
                0 => {
                    let g = cur_row[x];
                    [g, g, g, 255]
                }
                2 => {
                    let base = x * 3;
                    [cur_row[base], cur_row[base + 1], cur_row[base + 2], 255]
                }
                3 => {
                    let idx = cur_row[x] as usize;
                    let rgb = palette.get(idx).copied().unwrap_or([0, 0, 0]);
                    let alpha = trns.get(idx).copied().unwrap_or(255);
                    [rgb[0], rgb[1], rgb[2], alpha]
                }
                4 => {
                    let base = x * 2;
                    let g = cur_row[base];
                    [g, g, g, cur_row[base + 1]]
                }
                6 => {
                    let base = x * 4;
                    [
                        cur_row[base],
                        cur_row[base + 1],
                        cur_row[base + 2],
                        cur_row[base + 3],
                    ]
                }
                _ => [0, 0, 0, 255],
            };
            out.set_pixel(x as u32, y as u32, rgba);
        }

        prev_row.copy_from_slice(&cur_row);
    }

    Ok(out)
}

/// Encode an RGBA [`Image`] as a lossless 8-bit RGBA PNG file.
pub fn encode_png(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&PNG_SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&image.width.to_be_bytes());
    ihdr.extend_from_slice(&image.height.to_be_bytes());
    ihdr.push(8); // bit depth = 8
    ihdr.push(6); // color type = 6 (RGBA)
    ihdr.push(0); // compression = 0
    ihdr.push(0); // filter = 0
    ihdr.push(0); // interlace = 0
    write_png_chunk(&mut out, b"IHDR", &ihdr);

    let stride = (image.width as usize) * 4;
    let mut raw = Vec::with_capacity((image.height as usize) * (1 + stride));
    for y in 0..(image.height as usize) {
        raw.push(0); // Filter type 0 (None)
        let row_start = y * stride;
        raw.extend_from_slice(&image.pixels[row_start..row_start + stride]);
    }
    let compressed = compress_zlib(&raw);
    write_png_chunk(&mut out, b"IDAT", &compressed);
    write_png_chunk(&mut out, b"IEND", &[]);
    out
}

fn write_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let crc_start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[crc_start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a_i = i32::from(a);
    let b_i = i32::from(b);
    let c_i = i32::from(c);
    let p = a_i + b_i - c_i;
    let pa = (p - a_i).abs();
    let pb = (p - b_i).abs();
    let pc = (p - c_i).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Decode a 24-bit or 32-bit uncompressed BMP file into an RGBA [`Image`].
pub fn decode_bmp(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err(ImageError::InvalidHeader("missing BMP header"));
    }
    let data_offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
    let width_i32 = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let height_i32 = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
    let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
    let compression = u32::from_le_bytes([bytes[30], bytes[31], bytes[32], bytes[33]]);

    if width_i32 <= 0 || height_i32 == 0 {
        return Err(ImageError::InvalidHeader("invalid BMP dimensions"));
    }
    if compression != 0 && compression != 3 {
        return Err(ImageError::UnsupportedFormat(
            "compressed BMP not supported",
        ));
    }
    if bpp != 24 && bpp != 32 {
        return Err(ImageError::UnsupportedFormat(
            "only 24-bit and 32-bit BMP supported",
        ));
    }

    let width = width_i32 as u32;
    let top_down = height_i32 < 0;
    let height = height_i32.unsigned_abs();
    let bytes_per_pixel = (bpp / 8) as usize;
    let row_stride = ((width as usize) * bytes_per_pixel + 3) & !3;

    if data_offset + row_stride * (height as usize) > bytes.len() {
        return Err(ImageError::UnexpectedEof);
    }

    let mut out = Image::new(width, height);
    for row in 0..(height as usize) {
        let dst_y = if top_down {
            row as u32
        } else {
            height - 1 - (row as u32)
        };
        let row_base = data_offset + row * row_stride;
        for x in 0..(width as usize) {
            let px_base = row_base + x * bytes_per_pixel;
            let b = bytes[px_base];
            let g = bytes[px_base + 1];
            let r = bytes[px_base + 2];
            let a = if bytes_per_pixel == 4 {
                bytes[px_base + 3]
            } else {
                255
            };
            out.set_pixel(x as u32, dst_y, [r, g, b, a]);
        }
    }
    Ok(out)
}

/// Encode an RGBA [`Image`] into a 32-bit BGRA BMP file.
pub fn encode_bmp(image: &Image) -> Vec<u8> {
    let pixel_bytes = (image.width as usize) * (image.height as usize) * 4;
    let file_size = 54 + pixel_bytes;
    let mut out = Vec::with_capacity(file_size);

    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());

    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(image.width as i32).to_le_bytes());
    out.extend_from_slice(&(-(image.height as i32)).to_le_bytes()); // top-down
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for px in image.pixels.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

/// Decode a binary PPM (`P6`) or PGM (`P5`) image into an RGBA [`Image`].
pub fn decode_ppm(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 7 {
        return Err(ImageError::UnexpectedEof);
    }
    let magic = &bytes[0..2];
    if magic != b"P6" && magic != b"P5" {
        return Err(ImageError::InvalidHeader("expected P6 or P5 header"));
    }
    let mut pos = 2usize;
    let width = read_ppm_token(bytes, &mut pos)?;
    let height = read_ppm_token(bytes, &mut pos)?;
    let maxval = read_ppm_token(bytes, &mut pos)?;
    if width == 0 || height == 0 || maxval == 0 || maxval > 255 {
        return Err(ImageError::InvalidHeader("invalid PPM header values"));
    }
    if pos >= bytes.len() {
        return Err(ImageError::UnexpectedEof);
    }
    pos += 1; // single whitespace delimiter before binary data

    let mut out = Image::new(width, height);
    if magic == b"P6" {
        let needed = (width as usize) * (height as usize) * 3;
        if pos + needed > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        for y in 0..height {
            for x in 0..width {
                let idx = pos + ((y * width + x) as usize) * 3;
                out.set_pixel(x, y, [bytes[idx], bytes[idx + 1], bytes[idx + 2], 255]);
            }
        }
    } else {
        let needed = (width as usize) * (height as usize);
        if pos + needed > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        for y in 0..height {
            for x in 0..width {
                let g = bytes[pos + (y * width + x) as usize];
                out.set_pixel(x, y, [g, g, g, 255]);
            }
        }
    }
    Ok(out)
}

/// Encode an RGBA [`Image`] into a binary `P6` PPM image.
pub fn encode_ppm(image: &Image) -> Vec<u8> {
    let header = format!("P6\n{} {}\n255\n", image.width, image.height);
    let mut out = Vec::with_capacity(header.len() + (image.width * image.height * 3) as usize);
    out.extend_from_slice(header.as_bytes());
    for px in image.pixels.chunks_exact(4) {
        out.extend_from_slice(&[px[0], px[1], px[2]]);
    }
    out
}

fn read_ppm_token(bytes: &[u8], pos: &mut usize) -> Result<u32, ImageError> {
    while *pos < bytes.len() {
        let b = bytes[*pos];
        if b == b'#' {
            while *pos < bytes.len() && bytes[*pos] != b'\n' {
                *pos += 1;
            }
        } else if b.is_ascii_whitespace() {
            *pos += 1;
        } else {
            break;
        }
    }
    let start = *pos;
    while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }
    if start == *pos {
        return Err(ImageError::InvalidHeader(
            "expected numeric token in PPM header",
        ));
    }
    std::str::from_utf8(&bytes[start..*pos])
        .ok()
        .and_then(|s| s.parse().ok())
        .ok_or(ImageError::InvalidHeader("invalid number in PPM header"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_png_bmp_and_ppm_images() {
        let mut img = Image::new(4, 3);
        img.set_pixel(0, 0, [220, 40, 60, 255]);
        img.set_pixel(3, 2, [30, 140, 250, 255]);

        let png_bytes = encode_png(&img);
        let decoded_png = decode_auto(&png_bytes).unwrap();
        assert_eq!(decoded_png, img);

        let bmp_bytes = encode_bmp(&img);
        let decoded_bmp = decode_auto(&bmp_bytes).unwrap();
        assert_eq!(decoded_bmp, img);

        let ppm_bytes = encode_ppm(&img);
        let decoded_ppm = decode_auto(&ppm_bytes).unwrap();
        assert_eq!(decoded_ppm.pixel(0, 0), Some([220, 40, 60, 255]));
        assert_eq!(decoded_ppm.pixel(3, 2), Some([30, 140, 250, 255]));
    }

    #[test]
    fn resamples_image_with_bilinear_filtering() {
        let mut img = Image::new(2, 2);
        img.set_pixel(0, 0, [255, 0, 0, 255]);
        img.set_pixel(1, 0, [0, 255, 0, 255]);
        img.set_pixel(0, 1, [0, 0, 255, 255]);
        img.set_pixel(1, 1, [255, 255, 255, 255]);

        let scaled = img.resize_bilinear(8, 6);
        assert_eq!(scaled.width, 8);
        assert_eq!(scaled.height, 6);
        assert_eq!(scaled.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(scaled.pixel(7, 5), Some([255, 255, 255, 255]));
    }
}
