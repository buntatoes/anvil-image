//! Proprietary PNG, JPEG, GIF89a, BMP, and Netpbm image decoder, encoder, and resampler.

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

/// Automatically detect and decode a PNG, JPEG, GIF, BMP, or PPM/PGM image.
pub fn decode_auto(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        decode_png(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(bytes)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        decode_gif(bytes)
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

/// Decode a GIF87a or GIF89a image (first frame, LZW compressed) into an RGBA [`Image`].
pub fn decode_gif(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 13 || (!bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a")) {
        return Err(ImageError::InvalidHeader("missing GIF87a/GIF89a signature"));
    }
    let screen_w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let screen_h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    if screen_w == 0 || screen_h == 0 {
        return Err(ImageError::InvalidHeader("zero-sized GIF"));
    }
    let packed = bytes[10];
    let has_gct = (packed & 0x80) != 0;
    let gct_size = 1usize << ((packed & 0x07) + 1);
    let mut pos = 13usize;

    let mut global_palette = Vec::new();
    if has_gct {
        if pos + gct_size * 3 > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        for rgb in bytes[pos..pos + gct_size * 3].chunks_exact(3) {
            global_palette.push([rgb[0], rgb[1], rgb[2]]);
        }
        pos += gct_size * 3;
    }

    let mut transparent_idx: Option<u8> = None;

    while pos < bytes.len() {
        let marker = bytes[pos];
        pos += 1;
        match marker {
            0x21 => {
                // Extension block
                let ext_label = *bytes.get(pos).ok_or(ImageError::UnexpectedEof)?;
                pos += 1;
                if ext_label == 0xF9 {
                    // Graphic Control Extension
                    let block_size = *bytes.get(pos).ok_or(ImageError::UnexpectedEof)? as usize;
                    pos += 1;
                    if pos + block_size + 1 > bytes.len() {
                        return Err(ImageError::UnexpectedEof);
                    }
                    if block_size >= 4 {
                        let gce_packed = bytes[pos];
                        let trans_color = bytes[pos + 3];
                        if (gce_packed & 0x01) != 0 {
                            transparent_idx = Some(trans_color);
                        }
                    }
                    pos += block_size;
                    while pos < bytes.len() {
                        let sub = bytes[pos] as usize;
                        pos += 1;
                        if sub == 0 {
                            break;
                        }
                        pos = pos.saturating_add(sub);
                    }
                } else {
                    while pos < bytes.len() {
                        let sub = bytes[pos] as usize;
                        pos += 1;
                        if sub == 0 {
                            break;
                        }
                        pos = pos.saturating_add(sub);
                    }
                }
            }
            0x2C => {
                // Image Descriptor
                if pos + 9 > bytes.len() {
                    return Err(ImageError::UnexpectedEof);
                }
                let left = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as u32;
                let top = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]) as u32;
                let img_w = u16::from_le_bytes([bytes[pos + 4], bytes[pos + 5]]) as u32;
                let img_h = u16::from_le_bytes([bytes[pos + 6], bytes[pos + 7]]) as u32;
                let img_packed = bytes[pos + 8];
                pos += 9;

                let has_lct = (img_packed & 0x80) != 0;
                let interlaced = (img_packed & 0x40) != 0;
                let lct_size = 1usize << ((img_packed & 0x07) + 1);

                let mut local_palette = Vec::new();
                if has_lct {
                    if pos + lct_size * 3 > bytes.len() {
                        return Err(ImageError::UnexpectedEof);
                    }
                    for rgb in bytes[pos..pos + lct_size * 3].chunks_exact(3) {
                        local_palette.push([rgb[0], rgb[1], rgb[2]]);
                    }
                    pos += lct_size * 3;
                }
                let palette = if has_lct {
                    &local_palette
                } else {
                    &global_palette
                };

                let min_code_size = *bytes.get(pos).ok_or(ImageError::UnexpectedEof)?;
                pos += 1;
                if !(2..=8).contains(&min_code_size) {
                    return Err(ImageError::InvalidHeader("invalid GIF LZW code size"));
                }

                let mut lzw_stream = Vec::new();
                while pos < bytes.len() {
                    let sub_len = bytes[pos] as usize;
                    pos += 1;
                    if sub_len == 0 {
                        break;
                    }
                    if pos + sub_len > bytes.len() {
                        return Err(ImageError::UnexpectedEof);
                    }
                    lzw_stream.extend_from_slice(&bytes[pos..pos + sub_len]);
                    pos += sub_len;
                }

                let pixel_count = (img_w as usize) * (img_h as usize);
                let indices = decode_gif_lzw(min_code_size, &lzw_stream, pixel_count)?;
                let mut out = Image::new(screen_w, screen_h);

                for row in 0..(img_h as usize) {
                    let dst_row = if interlaced {
                        interlaced_gif_row(row, img_h as usize)
                    } else {
                        row
                    };
                    let py = top + (dst_row as u32);
                    if py >= screen_h {
                        continue;
                    }
                    for col in 0..(img_w as usize) {
                        let px = left + (col as u32);
                        if px >= screen_w {
                            continue;
                        }
                        let idx = indices
                            .get(row * (img_w as usize) + col)
                            .copied()
                            .unwrap_or(0);
                        if Some(idx) == transparent_idx {
                            out.set_pixel(px, py, [0, 0, 0, 0]);
                        } else {
                            let rgb = palette.get(idx as usize).copied().unwrap_or([0, 0, 0]);
                            out.set_pixel(px, py, [rgb[0], rgb[1], rgb[2], 255]);
                        }
                    }
                }
                return Ok(out);
            }
            0x3B => break,
            _ => break,
        }
    }

    Err(ImageError::InvalidHeader("no image frame found in GIF"))
}

fn interlaced_gif_row(row: usize, height: usize) -> usize {
    let mut r = row;
    for &(start, step) in &[(0usize, 8usize), (4, 8), (2, 4), (1, 2)] {
        if start < height {
            let count = (height - 1 - start) / step + 1;
            if r < count {
                return start + r * step;
            }
            r -= count;
        }
    }
    row
}

fn decode_gif_lzw(
    min_code_size: u8,
    stream: &[u8],
    expected_pixels: usize,
) -> Result<Vec<u8>, ImageError> {
    let clear_code = 1u16 << min_code_size;
    let end_code = clear_code + 1;

    let mut code_size = u32::from(min_code_size) + 1;
    let mut dict: Vec<Vec<u8>> = Vec::with_capacity(4096);
    let reset_dict = |dict: &mut Vec<Vec<u8>>| {
        dict.clear();
        for i in 0..=end_code {
            dict.push(vec![(i & 0xFF) as u8]);
        }
    };
    reset_dict(&mut dict);

    let mut bit_pos = 0usize;
    let read_code = |bit_pos: &mut usize, code_size: u32| -> Option<u16> {
        let mut val = 0u32;
        for b in 0..code_size {
            let total_bit = *bit_pos + (b as usize);
            let byte_idx = total_bit >> 3;
            if byte_idx >= stream.len() {
                return None;
            }
            let bit = (stream[byte_idx] >> (total_bit & 7)) & 1;
            val |= u32::from(bit) << b;
        }
        *bit_pos += code_size as usize;
        Some(val as u16)
    };

    let mut out = Vec::with_capacity(expected_pixels);
    let mut prev_seq: Option<Vec<u8>> = None;

    while let Some(code) = read_code(&mut bit_pos, code_size) {
        if code == clear_code {
            reset_dict(&mut dict);
            code_size = u32::from(min_code_size) + 1;
            prev_seq = None;
            continue;
        }
        if code == end_code {
            break;
        }

        let entry = if (code as usize) < dict.len() {
            dict[code as usize].clone()
        } else if let Some(ref prev) = prev_seq {
            if (code as usize) == dict.len() {
                let mut e = prev.clone();
                e.push(prev[0]);
                e
            } else {
                return Err(ImageError::InvalidHeader("invalid GIF LZW code"));
            }
        } else {
            return Err(ImageError::InvalidHeader("invalid initial GIF LZW code"));
        };

        out.extend_from_slice(&entry);

        if let Some(prev) = prev_seq {
            if dict.len() < 4096 {
                let mut new_entry = prev;
                new_entry.push(entry[0]);
                dict.push(new_entry);
                if dict.len() == (1usize << code_size) && code_size < 12 {
                    code_size += 1;
                }
            }
        }
        prev_seq = Some(entry);
        if out.len() >= expected_pixels {
            break;
        }
    }

    Ok(out)
}

/// Encode an RGBA [`Image`] into a GIF89a image (with 256-color palette and transparency index).
pub fn encode_gif(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GIF89a");
    let w = (image.width.min(65535)) as u16;
    let h = (image.height.min(65535)) as u16;
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.push(0xF7); // Global Color Table flag + 8-bit color resolution + 256 entries (2^(7+1))
    out.push(0); // background color index
    out.push(0); // default aspect ratio

    let mut palette: Vec<[u8; 3]> = vec![[0, 0, 0]]; // index 0 reserved for transparent
    let mut indices = Vec::with_capacity((w as usize) * (h as usize));

    for y in 0..(h as u32) {
        for x in 0..(w as u32) {
            let rgba = image.pixel(x, y).unwrap_or([0, 0, 0, 0]);
            if rgba[3] < 128 {
                indices.push(0u8);
                continue;
            }
            let rgb = [rgba[0], rgba[1], rgba[2]];
            if let Some(existing) = palette[1..].iter().position(|&c| c == rgb) {
                indices.push((existing + 1) as u8);
            } else if palette.len() < 256 {
                let idx = palette.len() as u8;
                palette.push(rgb);
                indices.push(idx);
            } else {
                // Nearest color in palette[1..]
                let mut best_idx = 1usize;
                let mut best_dist = u32::MAX;
                for (i, &c) in palette[1..].iter().enumerate() {
                    let dr = i32::from(c[0]) - i32::from(rgb[0]);
                    let dg = i32::from(c[1]) - i32::from(rgb[1]);
                    let db = i32::from(c[2]) - i32::from(rgb[2]);
                    let dist = (dr * dr + dg * dg + db * db) as u32;
                    if dist < best_dist {
                        best_dist = dist;
                        best_idx = i + 1;
                    }
                }
                indices.push(best_idx as u8);
            }
        }
    }
    while palette.len() < 256 {
        palette.push([0, 0, 0]);
    }
    for rgb in &palette {
        out.extend_from_slice(rgb);
    }

    // Graphic Control Extension (transparent index 0)
    out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00]);

    // Image Descriptor
    out.push(0x2C);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.push(0x00);

    // LZW Minimum Code Size = 8
    out.push(8);
    let mut bit_buf = Vec::new();
    let mut cur_byte = 0u8;
    let mut cur_bit = 0u8;
    let mut push_code = |code: u16| {
        for b in 0..9 {
            if ((code >> b) & 1) != 0 {
                cur_byte |= 1 << cur_bit;
            }
            cur_bit += 1;
            if cur_bit == 8 {
                bit_buf.push(cur_byte);
                cur_byte = 0;
                cur_bit = 0;
            }
        }
    };

    // Emit clear code (256) every 250 literal codes so code_size stays at 9 bits
    for (i, &idx) in indices.iter().enumerate() {
        if i % 250 == 0 {
            push_code(256);
        }
        push_code(u16::from(idx));
    }
    push_code(257); // EOI
    if cur_bit > 0 {
        bit_buf.push(cur_byte);
    }

    for chunk in bit_buf.chunks(255) {
        out.push(chunk.len() as u8);
        out.extend_from_slice(chunk);
    }
    out.push(0x00); // block terminator
    out.push(0x3B); // GIF trailer
    out
}

const JPEG_ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone, Default)]
struct JpegHuffTable {
    entries: Vec<(u8, u16, u8)>, // (code_len, code_bits, symbol)
}

impl JpegHuffTable {
    fn from_counts_and_symbols(counts: &[u8; 16], symbols: &[u8]) -> Self {
        let mut entries = Vec::with_capacity(symbols.len());
        let mut code = 0u16;
        let mut sym_idx = 0usize;
        for (len_minus_1, &count) in counts.iter().enumerate() {
            let bit_len = (len_minus_1 + 1) as u8;
            for _ in 0..count {
                if let Some(&sym) = symbols.get(sym_idx) {
                    entries.push((bit_len, code, sym));
                    sym_idx += 1;
                }
                code = code.wrapping_add(1);
            }
            code <<= 1;
        }
        Self { entries }
    }
}

#[derive(Clone, Copy, Default)]
struct JpegComponent {
    id: u8,
    h_factor: u8,
    v_factor: u8,
    quant_id: u8,
    dc_table: u8,
    ac_table: u8,
}

struct JpegBitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buf: u32,
    bit_count: u8,
}

impl<'a> JpegBitReader<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    fn next_bit(&mut self) -> Result<u8, ImageError> {
        if self.bit_count == 0 {
            let b = *self.data.get(self.pos).ok_or(ImageError::UnexpectedEof)?;
            self.pos += 1;
            if b == 0xFF {
                let next = *self.data.get(self.pos).ok_or(ImageError::UnexpectedEof)?;
                self.pos += 1;
                if next != 0x00 && !(0xD0..=0xD7).contains(&next) {
                    return Err(ImageError::InvalidHeader(
                        "unexpected marker inside JPEG scan",
                    ));
                }
            }
            self.bit_buf = u32::from(b);
            self.bit_count = 8;
        }
        self.bit_count -= 1;
        Ok(((self.bit_buf >> self.bit_count) & 1) as u8)
    }

    fn read_bits(&mut self, count: u8) -> Result<u16, ImageError> {
        let mut val = 0u16;
        for _ in 0..count {
            val = (val << 1) | u16::from(self.next_bit()?);
        }
        Ok(val)
    }

    fn decode_huff(&mut self, table: &JpegHuffTable) -> Result<u8, ImageError> {
        let mut code = 0u16;
        for len in 1..=16u8 {
            code = (code << 1) | u16::from(self.next_bit()?);
            for &(elen, ecode, sym) in &table.entries {
                if elen == len && ecode == code {
                    return Ok(sym);
                }
            }
        }
        Err(ImageError::InvalidHeader("invalid JPEG Huffman code"))
    }
}

fn receive_extend(bits: u16, category: u8) -> i32 {
    if category == 0 {
        return 0;
    }
    let half = 1u16 << (category - 1);
    if bits < half {
        i32::from(bits) - i32::from((1u16 << category) - 1)
    } else {
        i32::from(bits)
    }
}

/// Decode a baseline (`SOF0`) grayscale or YCbCr JPEG image into an RGBA [`Image`].
pub fn decode_jpeg(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(ImageError::InvalidHeader("missing JPEG SOI marker"));
    }

    let mut pos = 2usize;
    let mut qtables = [[1u16; 64]; 4];
    let mut dc_huff: [JpegHuffTable; 4] = Default::default();
    let mut ac_huff: [JpegHuffTable; 4] = Default::default();
    let mut width = 0u32;
    let mut height = 0u32;
    let mut components: Vec<JpegComponent> = Vec::new();
    let mut restart_interval = 0u16;

    while pos + 2 <= bytes.len() {
        if bytes[pos] != 0xFF {
            pos += 1;
            continue;
        }
        while pos < bytes.len() && bytes[pos] == 0xFF {
            pos += 1;
        }
        if pos >= bytes.len() {
            break;
        }
        let marker = bytes[pos];
        pos += 1;

        if marker == 0xD9 {
            break;
        }
        if marker == 0x00 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        if pos + 2 > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        if seg_len < 2 || pos + seg_len > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        let seg = &bytes[pos + 2..pos + seg_len];
        pos += seg_len;

        match marker {
            0xDB => {
                // DQT
                let mut sp = 0usize;
                while sp < seg.len() {
                    let info = seg[sp];
                    sp += 1;
                    let pq = info >> 4;
                    let tq = (info & 0x0F) as usize;
                    if tq >= 4 {
                        return Err(ImageError::InvalidHeader("invalid JPEG DQT index"));
                    }
                    if pq == 0 {
                        if sp + 64 > seg.len() {
                            return Err(ImageError::UnexpectedEof);
                        }
                        for i in 0..64 {
                            qtables[tq][i] = u16::from(seg[sp + i]);
                        }
                        sp += 64;
                    } else {
                        if sp + 128 > seg.len() {
                            return Err(ImageError::UnexpectedEof);
                        }
                        for i in 0..64 {
                            qtables[tq][i] =
                                u16::from_be_bytes([seg[sp + i * 2], seg[sp + i * 2 + 1]]);
                        }
                        sp += 128;
                    }
                }
            }
            0xC0 => {
                // SOF0 (Baseline DCT)
                if seg.len() < 6 {
                    return Err(ImageError::InvalidHeader("short SOF0 segment"));
                }
                let precision = seg[0];
                if precision != 8 {
                    return Err(ImageError::UnsupportedFormat(
                        "only 8-bit baseline JPEG supported",
                    ));
                }
                height = u16::from_be_bytes([seg[1], seg[2]]) as u32;
                width = u16::from_be_bytes([seg[3], seg[4]]) as u32;
                let ncomp = seg[5] as usize;
                if width == 0
                    || height == 0
                    || (ncomp != 1 && ncomp != 3)
                    || seg.len() < 6 + ncomp * 3
                {
                    return Err(ImageError::InvalidHeader("invalid SOF0 dimensions"));
                }
                components.clear();
                for i in 0..ncomp {
                    let base = 6 + i * 3;
                    let id = seg[base];
                    let samp = seg[base + 1];
                    let qid = seg[base + 2];
                    components.push(JpegComponent {
                        id,
                        h_factor: (samp >> 4).max(1),
                        v_factor: (samp & 0x0F).max(1),
                        quant_id: qid.min(3),
                        dc_table: 0,
                        ac_table: 0,
                    });
                }
            }
            0xC4 => {
                // DHT
                let mut sp = 0usize;
                while sp + 17 <= seg.len() {
                    let info = seg[sp];
                    sp += 1;
                    let tc = info >> 4;
                    let th = (info & 0x0F) as usize;
                    if th >= 4 {
                        return Err(ImageError::InvalidHeader("invalid DHT index"));
                    }
                    let mut counts = [0u8; 16];
                    counts.copy_from_slice(&seg[sp..sp + 16]);
                    sp += 16;
                    let total_syms: usize = counts.iter().map(|&c| c as usize).sum();
                    if sp + total_syms > seg.len() {
                        return Err(ImageError::UnexpectedEof);
                    }
                    let syms = &seg[sp..sp + total_syms];
                    sp += total_syms;
                    let table = JpegHuffTable::from_counts_and_symbols(&counts, syms);
                    if tc == 0 {
                        dc_huff[th] = table;
                    } else {
                        ac_huff[th] = table;
                    }
                }
            }
            0xDD => {
                // DRI
                if seg.len() >= 2 {
                    restart_interval = u16::from_be_bytes([seg[0], seg[1]]);
                }
            }
            0xDA => {
                // SOS
                if components.is_empty() || seg.is_empty() {
                    return Err(ImageError::InvalidHeader("SOS before SOF0"));
                }
                let nscan = seg[0] as usize;
                if seg.len() < 1 + nscan * 2 + 3 {
                    return Err(ImageError::InvalidHeader("short SOS segment"));
                }
                for i in 0..nscan {
                    let cid = seg[1 + i * 2];
                    let td_ta = seg[1 + i * 2 + 1];
                    if let Some(comp) = components.iter_mut().find(|c| c.id == cid) {
                        comp.dc_table = (td_ta >> 4).min(3);
                        comp.ac_table = (td_ta & 0x0F).min(3);
                    }
                }

                return decode_jpeg_scan(
                    bytes,
                    pos,
                    width,
                    height,
                    &components,
                    &qtables,
                    &dc_huff,
                    &ac_huff,
                    restart_interval,
                );
            }
            _ => {}
        }
    }

    Err(ImageError::InvalidHeader("missing JPEG SOS scan"))
}

#[allow(clippy::too_many_arguments)]
fn decode_jpeg_scan(
    bytes: &[u8],
    scan_pos: usize,
    width: u32,
    height: u32,
    components: &[JpegComponent],
    qtables: &[[u16; 64]; 4],
    dc_huff: &[JpegHuffTable; 4],
    ac_huff: &[JpegHuffTable; 4],
    restart_interval: u16,
) -> Result<Image, ImageError> {
    let max_h = components.iter().map(|c| c.h_factor).max().unwrap_or(1) as u32;
    let max_v = components.iter().map(|c| c.v_factor).max().unwrap_or(1) as u32;
    let mcu_w = max_h * 8;
    let mcu_h = max_v * 8;
    let mcus_x = width.div_ceil(mcu_w);
    let mcus_y = height.div_ceil(mcu_h);

    let mut reader = JpegBitReader::new(bytes, scan_pos);
    let mut prev_dc = [0i32; 4];
    let mut mcu_counter = 0u16;
    let mut out = Image::new(width, height);

    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            if restart_interval > 0 && mcu_counter > 0 && mcu_counter % restart_interval == 0 {
                reader.bit_count = 0;
                prev_dc = [0; 4];
                if reader.pos + 2 <= bytes.len()
                    && bytes[reader.pos] == 0xFF
                    && (0xD0..=0xD7).contains(&bytes[reader.pos + 1])
                {
                    reader.pos += 2;
                }
            }
            mcu_counter = mcu_counter.wrapping_add(1);

            let mut comp_planes: Vec<Vec<[f32; 64]>> = Vec::with_capacity(components.len());
            for (ci, comp) in components.iter().enumerate() {
                let blocks_count = (comp.h_factor as usize) * (comp.v_factor as usize);
                let mut blocks = Vec::with_capacity(blocks_count);
                for _ in 0..blocks_count {
                    let dc_cat = reader.decode_huff(&dc_huff[comp.dc_table as usize])?;
                    let dc_bits = reader.read_bits(dc_cat)?;
                    let diff = receive_extend(dc_bits, dc_cat);
                    prev_dc[ci] += diff;

                    let mut zz = [0i32; 64];
                    zz[0] = prev_dc[ci];
                    let mut k = 1usize;
                    while k < 64 {
                        let rs = reader.decode_huff(&ac_huff[comp.ac_table as usize])?;
                        if rs == 0x00 {
                            break;
                        }
                        if rs == 0xF0 {
                            k += 16;
                            continue;
                        }
                        let run = (rs >> 4) as usize;
                        let size = rs & 0x0F;
                        k += run;
                        if k >= 64 {
                            break;
                        }
                        let ac_bits = reader.read_bits(size)?;
                        zz[k] = receive_extend(ac_bits, size);
                        k += 1;
                    }

                    let qtable = &qtables[comp.quant_id as usize];
                    let mut coeff = [0.0f32; 64];
                    for i in 0..64 {
                        coeff[JPEG_ZIGZAG[i]] = (zz[i] as f32) * (qtable[i] as f32);
                    }
                    blocks.push(idct_8x8(&coeff));
                }
                comp_planes.push(blocks);
            }

            for py in 0..mcu_h {
                let img_y = my * mcu_h + py;
                if img_y >= height {
                    continue;
                }
                for px in 0..mcu_w {
                    let img_x = mx * mcu_w + px;
                    if img_x >= width {
                        continue;
                    }

                    let sample = |ci: usize| -> f32 {
                        let comp = &components[ci];
                        let cx = (px * u32::from(comp.h_factor)) / max_h;
                        let cy = (py * u32::from(comp.v_factor)) / max_v;
                        let bx = (cx / 8) as usize;
                        let by = (cy / 8) as usize;
                        let sx = (cx % 8) as usize;
                        let sy = (cy % 8) as usize;
                        let b_idx = by * (comp.h_factor as usize) + bx;
                        comp_planes[ci][b_idx][sy * 8 + sx]
                    };

                    if components.len() == 1 {
                        let y_val = sample(0).round().clamp(0.0, 255.0) as u8;
                        out.set_pixel(img_x, img_y, [y_val, y_val, y_val, 255]);
                    } else {
                        let y_val = sample(0);
                        let cb = sample(1) - 128.0;
                        let cr = sample(2) - 128.0;
                        let r = (y_val + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
                        let g = (y_val - 0.344136 * cb - 0.714136 * cr)
                            .round()
                            .clamp(0.0, 255.0) as u8;
                        let b = (y_val + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
                        out.set_pixel(img_x, img_y, [r, g, b, 255]);
                    }
                }
            }
        }
    }

    Ok(out)
}

fn idct_8x8(coeff: &[f32; 64]) -> [f32; 64] {
    let mut temp = [0.0f32; 64];
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;

    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0f32;
            for u in 0..8 {
                let cu = if u == 0 { inv_sqrt2 } else { 1.0 };
                let angle = ((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0;
                sum += cu * coeff[y * 8 + u] * angle.cos();
            }
            temp[y * 8 + x] = sum * 0.5;
        }
    }

    let mut out = [0.0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut sum = 0.0f32;
            for v in 0..8 {
                let cv = if v == 0 { inv_sqrt2 } else { 1.0 };
                let angle = ((2 * y + 1) * v) as f32 * std::f32::consts::PI / 16.0;
                sum += cv * temp[v * 8 + x] * angle.cos();
            }
            out[y * 8 + x] = (sum * 0.5 + 128.0).clamp(0.0, 255.0);
        }
    }
    out
}

fn fdct_8x8(block: &[f32; 64]) -> [i32; 64] {
    let mut temp = [0.0f32; 64];
    let inv_sqrt2 = std::f32::consts::FRAC_1_SQRT_2;

    for y in 0..8 {
        for u in 0..8 {
            let cu = if u == 0 { inv_sqrt2 } else { 1.0 };
            let mut sum = 0.0f32;
            for x in 0..8 {
                let angle = ((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0;
                sum += (block[y * 8 + x] - 128.0) * angle.cos();
            }
            temp[y * 8 + u] = 0.5 * cu * sum;
        }
    }

    let mut out = [0i32; 64];
    for v in 0..8 {
        let cv = if v == 0 { inv_sqrt2 } else { 1.0 };
        for u in 0..8 {
            let mut sum = 0.0f32;
            for y in 0..8 {
                let angle = ((2 * y + 1) * v) as f32 * std::f32::consts::PI / 16.0;
                sum += temp[y * 8 + u] * angle.cos();
            }
            out[v * 8 + u] = (0.5 * cv * sum).round() as i32;
        }
    }
    out
}

/// Encode an RGBA [`Image`] into a baseline (`SOF0`) 4:4:4 YCbCr JPEG stream.
pub fn encode_jpeg(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI

    // DQT: Quantization table 0 with all 1s for near-lossless fidelity
    out.extend_from_slice(&[0xFF, 0xDB, 0x00, 67, 0x00]);
    out.extend_from_slice(&[1u8; 64]);

    // SOF0: 8-bit, 3 components (Y=1, Cb=2, Cr=3), 1x1 sampling, QTable 0
    let w = (image.width.max(1).min(65535)) as u16;
    let h = (image.height.max(1).min(65535)) as u16;
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 17, 8]);
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&[3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0]);

    // DHT: DC table 0 (categories 0..=11, all length 4 bits except last)
    let dc_counts: [u8; 16] = [0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    let dc_syms: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
    out.extend_from_slice(&[0xFF, 0xC4, 0x00, (19 + dc_syms.len()) as u8, 0x00]);
    out.extend_from_slice(&dc_counts);
    out.extend_from_slice(&dc_syms);

    // DHT: AC table 0 (all 162 standard symbols: 0x00 EOB, 0xF0 ZRL, plus run 0..15 x size 1..10 at length 8 bits)
    let mut ac_syms = Vec::with_capacity(162);
    ac_syms.push(0x00u8);
    ac_syms.push(0xF0u8);
    for run in 0..16u8 {
        for size in 1..=10u8 {
            ac_syms.push((run << 4) | size);
        }
    }
    let ac_counts: [u8; 16] = [
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        ac_syms.len() as u8,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ];
    let ac_seg_len = (19 + ac_syms.len()) as u16;
    out.extend_from_slice(&[0xFF, 0xC4]);
    out.extend_from_slice(&ac_seg_len.to_be_bytes());
    out.push(0x10);
    out.extend_from_slice(&ac_counts);
    out.extend_from_slice(&ac_syms);

    // SOS
    out.extend_from_slice(&[0xFF, 0xDA, 0x00, 12, 3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0]);

    let mut cur_byte = 0u8;
    let mut cur_bits = 0u8;
    let push_bits = |out: &mut Vec<u8>, cur_byte: &mut u8, cur_bits: &mut u8, val: u16, n: u8| {
        for i in (0..n).rev() {
            let b = ((val >> i) & 1) as u8;
            *cur_byte = (*cur_byte << 1) | b;
            *cur_bits += 1;
            if *cur_bits == 8 {
                out.push(*cur_byte);
                if *cur_byte == 0xFF {
                    out.push(0x00);
                }
                *cur_byte = 0;
                *cur_bits = 0;
            }
        }
    };

    let encode_mag = |v: i32| -> (u8, u16) {
        if v == 0 {
            return (0, 0);
        }
        let abs_v = v.unsigned_abs().min(1023) as u16;
        let cat = (16 - abs_v.leading_zeros()) as u8;
        let bits = if v > 0 {
            abs_v
        } else {
            abs_v ^ ((1u16 << cat) - 1)
        };
        (cat, bits)
    };

    let mcus_x = image.width.max(1).div_ceil(8);
    let mcus_y = image.height.max(1).div_ceil(8);
    let mut prev_dc = [0i32; 3];

    for my in 0..mcus_y {
        for mx in 0..mcus_x {
            let mut planes = [[0.0f32; 64]; 3];
            for by in 0..8u32 {
                for bx in 0..8u32 {
                    let px = (mx * 8 + bx).min(image.width.saturating_sub(1));
                    let py = (my * 8 + by).min(image.height.saturating_sub(1));
                    let [r, g, b, _] = image.pixel(px, py).unwrap_or([0, 0, 0, 255]);
                    let rf = r as f32;
                    let gf = g as f32;
                    let bf = b as f32;
                    let y = (0.299 * rf + 0.587 * gf + 0.114 * bf).clamp(0.0, 255.0);
                    let cb = (-0.168736 * rf - 0.331264 * gf + 0.5 * bf + 128.0).clamp(0.0, 255.0);
                    let cr = (0.5 * rf - 0.418688 * gf - 0.081312 * bf + 128.0).clamp(0.0, 255.0);
                    let idx = (by * 8 + bx) as usize;
                    planes[0][idx] = y;
                    planes[1][idx] = cb;
                    planes[2][idx] = cr;
                }
            }

            for ci in 0..3 {
                let dct = fdct_8x8(&planes[ci]);
                let mut zz = [0i32; 64];
                for i in 0..64 {
                    zz[i] = dct[JPEG_ZIGZAG[i]];
                }
                let diff = (zz[0] - prev_dc[ci]).clamp(-1023, 1023);
                prev_dc[ci] = zz[0];
                let (dc_cat, dc_bits) = encode_mag(diff);
                push_bits(&mut out, &mut cur_byte, &mut cur_bits, u16::from(dc_cat), 4);
                push_bits(&mut out, &mut cur_byte, &mut cur_bits, dc_bits, dc_cat);

                let mut zero_run = 0u8;
                for &ac_val in &zz[1..] {
                    let clamped = ac_val.clamp(-511, 511);
                    if clamped == 0 {
                        zero_run += 1;
                    } else {
                        while zero_run >= 16 {
                            // 0xF0 is index 1 in ac_syms
                            push_bits(&mut out, &mut cur_byte, &mut cur_bits, 1, 8);
                            zero_run -= 16;
                        }
                        let (ac_cat, ac_bits) = encode_mag(clamped);
                        let sym = (zero_run << 4) | ac_cat;
                        let code = ac_syms.iter().position(|&s| s == sym).unwrap_or(0) as u16;
                        push_bits(&mut out, &mut cur_byte, &mut cur_bits, code, 8);
                        push_bits(&mut out, &mut cur_byte, &mut cur_bits, ac_bits, ac_cat);
                        zero_run = 0;
                    }
                }
                if zero_run > 0 {
                    // EOB (0x00) is index 0 in ac_syms
                    push_bits(&mut out, &mut cur_byte, &mut cur_bits, 0, 8);
                }
            }
        }
    }

    if cur_bits > 0 {
        let pad = 8 - cur_bits;
        push_bits(
            &mut out,
            &mut cur_byte,
            &mut cur_bits,
            (1u16 << pad) - 1,
            pad,
        );
    }
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
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
    fn round_trips_gif89a_and_baseline_jpeg_images() {
        let mut img = Image::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [200, 100, 50, 255]);
            }
        }

        let gif_bytes = encode_gif(&img);
        let decoded_gif = decode_auto(&gif_bytes).expect("decode GIF89a");
        assert_eq!(decoded_gif.width, 8);
        assert_eq!(decoded_gif.height, 8);
        assert_eq!(decoded_gif.pixel(4, 4), Some([200, 100, 50, 255]));

        let jpeg_bytes = encode_jpeg(&img);
        let decoded_jpeg = decode_auto(&jpeg_bytes).expect("decode baseline JPEG");
        assert_eq!(decoded_jpeg.width, 8);
        assert_eq!(decoded_jpeg.height, 8);
        let px = decoded_jpeg.pixel(4, 4).unwrap();
        assert!((i32::from(px[0]) - 200).abs() <= 4);
        assert!((i32::from(px[1]) - 100).abs() <= 4);
        assert!((i32::from(px[2]) - 50).abs() <= 4);
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
