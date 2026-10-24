//! CPU image codecs that expand still pictures to 8-bit RGBA.
//!
//! Decoders accept these containers, and nothing else:
//!
//! * PNG: signature `137 80 78 71 13 10 26 10`, 8-bit samples only, color types
//!   0 (gray), 2 (RGB), 3 (indexed), 4 (gray plus alpha), and 6 (RGBA).
//!   Adam7 interlacing, bit depths other than 8, and non-zero compression or
//!   filter methods are rejected. Each chunk's CRC-32 is checked. `IDAT`
//!   payloads are concatenated and inflated with `flux-compress`.
//! * JPEG: baseline `SOF0` only, 8-bit, one component (gray) or three (YCbCr).
//!   Markers actually read are `DQT`, `DHT`, `DRI`, `SOF0`, and `SOS`, plus
//!   byte stuffing and restart markers inside the scan. Progressive, arithmetic,
//!   and other frame types are skipped or rejected.
//! * GIF: `GIF87a` and `GIF89a`. The first image descriptor is LZW-decoded,
//!   including a local or global palette, a graphic-control transparent index,
//!   and the four-pass interlace. Later frames are not composited.
//! * WebP: a RIFF `WEBP` container. Pixels are taken only from a zlib-compressed
//!   `RGBA` chunk written by [`encode_webp`]. `VP8X` and the `VP8L` header supply
//!   width and height. Lossy `VP8` and a real `VP8L` transform bitstream are not
//!   decoded.
//! * ICO and CUR: a directory with type 1 or 2. The largest entry is decoded,
//!   either as an embedded PNG or as a 24-bit or 32-bit DIB. The AND mask is not applied.
//! * BMP: uncompressed (`BI_RGB`) or `BI_BITFIELDS` files at 24 or 32 bits per
//!   pixel. Bitfield masks are not interpreted; samples are read as B, G, R, and
//!   optional A. Other compressions are rejected.
//! * Binary Netpbm: `P6` (RGB) and `P5` (gray) with `maxval` in `1..=255`.
//!   ASCII `P2` and `P3`, and 16-bit samples, are rejected.
//!
//! [`decode_auto`] matches those signatures in that order. `flux-browser`
//! calls it when a page image has been fetched. `flux-dom` only stores the
//! URL. `flux-layout` reads intrinsic width and height from an [`Image`] the
//! caller already decoded. `flux-raster` samples those pixels. Encoders write the same formats back out; GIF and JPEG clamp
//! each side to 65535 samples, and the WebP encoder does not emit a standard
//! `VP8` or `VP8L` bitstream.
//!
//! Width and height must each be in `1..=`[`MAX_IMAGE_DIMENSION`] and the product
//! must be at most [`MAX_IMAGE_PIXELS`] (8192 on a side, 16,777,216 pixels).
//! `flux-compress` supplies zlib inflate and deflate (`decompress_zlib_bounded`,
//! `compress_zlib`) and the PNG chunk CRC-32. There is no color-management path:
//! `gAMA`, `cHRM`, `iCCP`, and similar ancillary chunks are checksummed and ignored.
//! There is no animated multi-frame playback pipeline. This crate does not talk to a GPU.

use std::fmt;

use flux_compress::{CompressError, compress_zlib, crc32, decompress_zlib_bounded};

/// Eight-byte PNG signature. Compared in full before any chunk is read.
const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Largest width or height a decoder will accept, in pixels.
pub const MAX_IMAGE_DIMENSION: u32 = 8192;

/// Largest `width * height` a decoder will accept.
///
/// 16,777,216 pixels is 64 MiB of 8-bit RGBA. Either axis may still be rejected
/// by [`MAX_IMAGE_DIMENSION`] before the product is checked.
pub const MAX_IMAGE_PIXELS: u64 = 16_777_216;

/// Why a decode failed, or why [`validate_dimensions`] refused a size.
///
/// Encoders do not return this type. They always emit bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// `bytes` ended before a length field, chunk, scan, or pixel run was finished.
    UnexpectedEof,
    /// The container was recognized, but a required feature is not implemented.
    ///
    /// `msg` is a static description of the rejected case (bit depth, interlace,
    /// frame type, WebP chunk, and similar).
    UnsupportedFormat(&'static str),
    /// A signature, header field, checksum, or entropy code is not valid.
    ///
    /// `msg` names the field that failed. This is also used for a zero size and
    /// for an RGBA buffer whose length does not match `width * height * 4`.
    InvalidHeader(&'static str),
    /// `width` or `height` is above [`MAX_IMAGE_DIMENSION`], or the product is
    /// above [`MAX_IMAGE_PIXELS`].
    DimensionsTooLarge {
        /// Declared width in pixels.
        width: u32,
        /// Declared height in pixels.
        height: u32,
    },
    /// A PNG scanline filter byte was outside `0..=4`.
    ///
    /// The stored value is the rejected filter byte.
    InvalidFilter(u8),
    /// `flux-compress` failed while inflating a PNG `IDAT` or a WebP `RGBA` chunk.
    Compression(CompressError),
}

impl fmt::Display for ImageError {
    /// Write one sentence. Dimension errors include the declared size and the caps.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEof => write!(f, "unexpected end of image file"),
            Self::UnsupportedFormat(msg) => write!(f, "unsupported image format: {msg}"),
            Self::InvalidHeader(msg) => write!(f, "invalid image header: {msg}"),
            Self::DimensionsTooLarge { width, height } => write!(
                f,
                "image dimensions {width}x{height} exceed security limits ({MAX_IMAGE_DIMENSION}x{MAX_IMAGE_DIMENSION}, {MAX_IMAGE_PIXELS} pixels)"
            ),
            Self::InvalidFilter(kind) => write!(f, "invalid PNG scanline filter {kind}"),
            Self::Compression(err) => write!(f, "image decompression failed: {err}"),
        }
    }
}

/// Reject a zero size, an axis above [`MAX_IMAGE_DIMENSION`], or too many pixels.
///
/// `width` and `height` are the declared dimensions in pixels. The product is
/// computed in `u64` so the check itself cannot overflow. Returns
/// [`ImageError::InvalidHeader`] when either axis is 0, and
/// [`ImageError::DimensionsTooLarge`] when an axis or the product exceeds the caps.
/// [`Image::new`] does not call this; the decoders do.
pub fn validate_dimensions(width: u32, height: u32) -> Result<(), ImageError> {
    if width == 0 || height == 0 {
        return Err(ImageError::InvalidHeader("zero-sized image"));
    }
    let total_pixels = u64::from(width) * u64::from(height);
    // Both axes and the product are capped. A 8192 by 8192 image is over the pixel cap.
    if width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || total_pixels > MAX_IMAGE_PIXELS
    {
        return Err(ImageError::DimensionsTooLarge { width, height });
    }
    Ok(())
}

/// Allows `?` from this error into other crates' error types.
impl std::error::Error for ImageError {}

/// Map a zlib failure from `flux-compress` onto [`ImageError::Compression`].
impl From<CompressError> for ImageError {
    /// Store `value` in [`ImageError::Compression`] without inspecting it.
    fn from(value: CompressError) -> Self {
        Self::Compression(value)
    }
}

/// Top-down 8-bit image. One pixel is four bytes, `R`, `G`, `B`, then `A`, row-major.
///
/// Row 0 is the top row. There is no stride padding. [`pixel`] and [`set_pixel`]
/// index with `(y * width + x) * 4`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes when constructed by a decoder or [`Image::from_rgba`].
    ///
    /// [`Image::new`] sizes the buffer with saturating multiplication, so an
    /// overflowing dimension pair yields a shorter `Vec` instead of panicking.
    pub pixels: Vec<u8>,
}

impl Image {
    /// Allocate a transparent black buffer of `width` by `height`.
    ///
    /// Does not apply [`validate_dimensions`]. The length is
    /// `width * height * 4` computed with saturating casts, filled with zeros
    /// (`R = G = B = A = 0`).
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

    /// Build an image from an existing tightly packed RGBA buffer.
    ///
    /// `pixels` must be exactly `width * height * 4` bytes in the same layout as
    /// [`Image::pixels`]. The expected length uses saturating multiplication, matching
    /// [`Image::new`]. Returns [`ImageError::InvalidHeader`] on a length mismatch.
    /// Dimensions are not checked against the security caps.
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

    /// Return the RGBA sample at column `x`, row `y`, or `None` if that pixel is outside the buffer.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        // Out of range is absence, not a panic, so callers can probe edges.
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

    /// Write `rgba` at column `x`, row `y`. Coordinates outside the image are ignored.
    pub fn set_pixel(&mut self, x: u32, y: u32, rgba: [u8; 4]) {
        // Same bounds test as [`Image::pixel`]. A short `pixels` buffer is not rechecked here.
        if x >= self.width || y >= self.height {
            return;
        }
        let idx = ((y * self.width + x) as usize) * 4;
        self.pixels[idx..idx + 4].copy_from_slice(&rgba);
    }

    /// Resample to `new_width` by `new_height` with a bilinear filter on each channel, including alpha.
    ///
    /// Output pixel centers are mapped with `(n + 0.5) * (src / dst) - 0.5` and clamped
    /// to `[0, src_side - 1]`. The four surrounding source pixels are blended
    /// independently per byte and rounded to `0..=255`. A zero destination or source
    /// side returns [`Image::new`] without sampling. An unchanged size returns a clone.
    /// The result is not checked against [`MAX_IMAGE_DIMENSION`].
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
                // Separable bilinear blend. Alpha is filtered the same way as color, not premultiplied.
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

/// Detect a still image in `bytes` and decode it to RGBA.
///
/// Signature order is PNG, JPEG (`FF D8`), `GIF87a` or `GIF89a`, RIFF `WEBP`,
/// ICO (`00 00 01 00`) or CUR (`00 00 02 00`), BMP (`BM`), then binary `P6` or `P5`.
/// The first match wins. Anything else returns [`ImageError::UnsupportedFormat`].
/// `flux-dom` and `flux-browser` use this for page images. Dimension and format
/// errors are those of the chosen decoder.
pub fn decode_auto(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.starts_with(&PNG_SIGNATURE) {
        decode_png(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        decode_jpeg(bytes)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        decode_gif(bytes)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        decode_webp(bytes)
    } else if bytes.starts_with(&[0, 0, 1, 0]) || bytes.starts_with(&[0, 0, 2, 0]) {
        decode_ico(bytes)
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

/// Decode an 8-bit, non-interlaced PNG from `bytes` into RGBA.
///
/// Chunk layout, repeated until `IEND` or the buffer ends: 4-byte big-endian length,
/// 4-byte type, `length` data bytes, 4-byte big-endian CRC-32. The CRC covers the
/// type and the data, not the length field or the CRC itself, and is computed by
/// [`crc32`](flux_compress::crc32). A bad CRC is [`ImageError::InvalidHeader`].
/// A chunk that would run past the buffer is [`ImageError::UnexpectedEof`].
///
/// `IHDR` must be 13 bytes: width, height, bit depth, color type, compression
/// method, filter method, interlace method. Only bit depth 8 and methods all zero
/// are accepted, so Adam7 is rejected as [`ImageError::UnsupportedFormat`].
/// `PLTE` is read as RGB triples. `tRNS` is kept as raw bytes (one alpha per
/// palette index for color type 3). Every `IDAT` is appended in order and inflated
/// with [`decompress_zlib_bounded`](flux_compress::decompress_zlib_bounded).
/// Unknown chunks are skipped after the CRC check, including color-profile chunks.
///
/// Each inflated scanline is one filter byte plus `width * bytes_per_pixel` samples.
/// Filters 0 through 4 (None, Sub, Up, Average, Paeth) are reconstructed. Color
/// types expand to 8-bit RGBA. Missing `IHDR`, a short inflate, or an unknown color
/// type is an error. Returns the top-down image on success.
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
        // Length, type, data, and the trailing CRC must all lie inside `bytes`.
        if pos + len + 4 > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        let chunk_data = &bytes[pos..pos + len];
        let crc_got = u32::from_be_bytes([
            bytes[pos + len],
            bytes[pos + len + 1],
            bytes[pos + len + 2],
            bytes[pos + len + 3],
        ]);
        // CRC scope is the 4-byte type followed by the data. The length prefix is excluded.
        let mut crc_input = Vec::with_capacity(4 + len);
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(chunk_data);
        if crc32(&crc_input) != crc_got {
            return Err(ImageError::InvalidHeader("bad png chunk checksum"));
        }
        pos += len + 4;

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
                validate_dimensions(width, height)?;
                if bit_depth != 8 {
                    return Err(ImageError::UnsupportedFormat(
                        "only 8-bit PNG depth supported",
                    ));
                }
                // Adam7 (interlace 1) is rejected, and so are non-zero compression or filter methods.
                // All three failures share one error string.
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

    // Bytes per pixel in the filtered stream, before expansion to RGBA.
    let bpp = match color_type {
        0 => 1usize, // Grayscale
        2 => 3usize, // RGB
        3 => 1usize, // Indexed palette
        4 => 2usize, // Grayscale + Alpha
        6 => 4usize, // RGBA
        _ => return Err(ImageError::UnsupportedFormat("unsupported PNG color type")),
    };

    let stride = (width as usize) * bpp;
    // One filter byte plus `stride` samples on every row.
    let expected_raw = (height as usize) * (1 + stride);
    // Inflate cap is the reconstructed size plus a small slack, not the whole file limit.
    let raw = decompress_zlib_bounded(&idat, expected_raw.saturating_add(4096))?;
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

            // PNG filters reconstruct with wrapping 8-bit addition. `a` is left, `b` is up, `c` is up-left.
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
            // Expand the scanline sample to straight 8-bit RGBA. Missing palette entries become opaque black.
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
                    // `tRNS` is one alpha byte per palette index. A short chunk leaves the rest opaque.
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

/// Encode `image` as a non-interlaced 8-bit RGBA PNG.
///
/// Writes the PNG signature, one `IHDR` (bit depth 8, color type 6, methods 0),
/// one `IDAT`, and `IEND`. Scanlines use filter 0 (None): a zero byte followed by
/// the raw RGBA row. The filtered bytes are zlib-wrapped by
/// [`compress_zlib`](flux_compress::compress_zlib). Does not return an error.
/// Dimensions are written as stored and are not clamped.
pub fn encode_png(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&PNG_SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&image.width.to_be_bytes());
    ihdr.extend_from_slice(&image.height.to_be_bytes());
    ihdr.push(8); // bit depth = 8
    ihdr.push(6); // color type = 6 (RGBA)
    ihdr.push(0); // compression method 0 (zlib)
    ihdr.push(0); // filter method 0
    ihdr.push(0); // no Adam7 interlace
    write_png_chunk(&mut out, b"IHDR", &ihdr);

    let stride = (image.width as usize) * 4;
    let mut raw = Vec::with_capacity((image.height as usize) * (1 + stride));
    for y in 0..(image.height as usize) {
        raw.push(0); // Filter 0 (None): the row is stored unchanged.
        let row_start = y * stride;
        raw.extend_from_slice(&image.pixels[row_start..row_start + stride]);
    }
    let compressed = compress_zlib(&raw);
    write_png_chunk(&mut out, b"IDAT", &compressed);
    write_png_chunk(&mut out, b"IEND", &[]);
    out
}

/// Append one PNG chunk: big-endian length, `kind`, `data`, then CRC-32 of type and data.
///
/// The CRC is [`crc32`](flux_compress::crc32) over the bytes written after the length
/// field. `kind` is the four-byte chunk type, such as `b"IHDR"`.
fn write_png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let crc_start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    // CRC scope starts at the type byte and stops before this checksum.
    let crc = crc32(&out[crc_start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// PNG Paeth predictor. `a` is the left sample, `b` is above, `c` is above-left.
///
/// Returns whichever of `a`, `b`, or `c` is closest to `a + b - c`, with ties
/// broken toward `a` and then `b`.
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

/// Decode a 24-bit or 32-bit BMP from `bytes` into top-down RGBA.
///
/// Requires a `BM` file header and a BITMAPINFOHEADER starting at offset 14.
/// Fields used, all little-endian: pixel offset at byte 10 (`u32`), width at 18
/// (`i32`), height at 22 (`i32`), bit count at 28 (`u16`), compression at 30 (`u32`).
/// Compression 0 (`BI_RGB`) and 3 (`BI_BITFIELDS`) are accepted. Masks are not read;
/// 24-bit pixels are `B, G, R` and 32-bit pixels are `B, G, R, A`. Any other
/// compression or bit count is [`ImageError::UnsupportedFormat`].
///
/// A negative height is a top-down bitmap and is stored in that order. A positive
/// height is bottom-up and is flipped. Rows are padded to a multiple of 4 bytes.
/// The pixel array must fit at `data_offset`. Zero or negative width, and a zero
/// height, are [`ImageError::InvalidHeader`]. Oversized frames are
/// [`ImageError::DimensionsTooLarge`].
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
    // Negative biHeight means the first stored row is the top of the image.
    let top_down = height_i32 < 0;
    let height = height_i32.unsigned_abs();
    validate_dimensions(width, height)?;
    let bytes_per_pixel = (bpp / 8) as usize;
    // BMP rows are padded up to the next multiple of 4 bytes.
    let row_stride = ((width as usize) * bytes_per_pixel + 3) & !3;

    // The pixel array, including row padding, must fit after the declared offset.
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
            // Stored order is B, G, R, and A only when the pixel is 32 bits. 24-bit is opaque.
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

/// Encode `image` as a 32-bit top-down BMP (`BI_RGB`, no row padding).
///
/// The file is 14 bytes of `BITMAPFILEHEADER` plus 40 bytes of `BITMAPINFOHEADER`
/// (size 40, planes 1, bit count 32, compression 0, pixel offset 54) followed by
/// pixels in `B, G, R, A` order. Height is stored negative so readers treat the
/// first row as the top. Horizontal and vertical resolution are 2835 pixels per
/// meter. Alpha is copied unchanged. Does not return an error.
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
    out.extend_from_slice(&(-(image.height as i32)).to_le_bytes()); // negative height: top-down rows
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for px in image.pixels.chunks_exact(4) {
        // RGBA in memory becomes B, G, R, A on disk.
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    out
}

/// Decode a binary `P6` (RGB) or `P5` (gray) pixmap from `bytes` into opaque RGBA.
///
/// The header is ASCII: magic, width, height, `maxval`, each a decimal token.
/// `#` comments run to the next newline and are skipped, as is surrounding
/// whitespace. `maxval` must be in `1..=255`. Samples are one byte and are not
/// rescaled when `maxval` is below 255. Exactly one byte after the `maxval` token
/// is skipped as the required whitespace separator. `P6` pixels are `R, G, B`
/// expanded with alpha 255. `P5` pixels are one gray byte copied into R, G, and B.
/// A short header or a short raster is [`ImageError::UnexpectedEof`]. A bad token,
/// a zero size, or `maxval` outside that range is [`ImageError::InvalidHeader`].
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
    validate_dimensions(width, height)?;
    if pos >= bytes.len() {
        return Err(ImageError::UnexpectedEof);
    }
    pos += 1; // single whitespace delimiter before the binary raster

    let mut out = Image::new(width, height);
    if magic == b"P6" {
        let needed = (width as usize) * (height as usize) * 3;
        // Tight RGB raster: three bytes per pixel, no row padding.
        if pos + needed > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        for y in 0..height {
            for x in 0..width {
                let idx = pos + ((y * width + x) as usize) * 3;
                // P6 is RGB. Alpha is forced opaque. Values are not scaled by `maxval`.
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
                // P5 is one gray sample, replicated into R, G, and B.
                out.set_pixel(x, y, [g, g, g, 255]);
            }
        }
    }
    Ok(out)
}

/// Encode `image` as a binary `P6` PPM with `maxval` 255, dropping alpha.
///
/// The header is `P6\n{width} {height}\n255\n`. Each pixel contributes `R, G, B`
/// and the alpha byte is discarded. Does not return an error.
pub fn encode_ppm(image: &Image) -> Vec<u8> {
    let header = format!("P6\n{} {}\n255\n", image.width, image.height);
    let mut out = Vec::with_capacity(header.len() + (image.width * image.height * 3) as usize);
    out.extend_from_slice(header.as_bytes());
    for px in image.pixels.chunks_exact(4) {
        // Alpha is not representable in P6.
        out.extend_from_slice(&[px[0], px[1], px[2]]);
    }
    out
}

/// Read one unsigned decimal token from a Netpbm header, advancing `pos`.
///
/// Skips ASCII whitespace. A `#` starts a comment that runs through the next
/// newline, or to the end of `bytes` if there is none. Returns
/// [`ImageError::InvalidHeader`] when the next token is missing or is not a
/// `u32`. Does not require a trailing delimiter.
fn read_ppm_token(bytes: &[u8], pos: &mut usize) -> Result<u32, ImageError> {
    while *pos < bytes.len() {
        let b = bytes[*pos];
        if b == b'#' {
            // Comment runs to the newline. The newline itself is left for the whitespace skip.
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

/// Decode the first frame of a `GIF87a` or `GIF89a` file in `bytes` to RGBA.
///
/// The logical screen is a little-endian width and height at bytes 6 and 8.
/// Byte 10 is the packed field: bit 7 selects a global color table, and the low
/// three bits are `size - 1` where the table has `2^(n+1)` RGB triples. The
/// background index and aspect byte are skipped. A zero screen, a bad signature,
/// or a truncated table is an error. The screen must pass [`validate_dimensions`].
///
/// Blocks that follow: `0x21` extensions (sub-block chains, skipped except a
/// graphic-control extension `0xF9`, whose transparent-color flag and index are
/// kept), `0x2C` image descriptor (the first one is decoded and returned), and
/// `0x3B` trailer. Any other marker stops the walk. If no image descriptor is
/// found, the result is [`ImageError::InvalidHeader`].
///
/// The descriptor is left, top, width, height (all `u16` little-endian) and a
/// packed byte: bit 7 local color table, bit 6 interlace, low three bits local
/// table size. LZW minimum code size must be `2..=8`. Sub-blocks are concatenated
/// and passed to [`decode_gif_lzw`]. The frame is painted onto a transparent
/// canvas of the logical screen size. Pixels outside the screen are skipped.
/// A transparent index becomes alpha 0. There is no multi-frame compositor.
pub fn decode_gif(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 13 || (!bytes.starts_with(b"GIF87a") && !bytes.starts_with(b"GIF89a")) {
        return Err(ImageError::InvalidHeader("missing GIF87a/GIF89a signature"));
    }
    let screen_w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let screen_h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    if screen_w == 0 || screen_h == 0 {
        return Err(ImageError::InvalidHeader("zero-sized GIF"));
    }
    validate_dimensions(screen_w, screen_h)?;
    let packed = bytes[10];
    let has_gct = (packed & 0x80) != 0;
    // Packed size field `n` means `2^(n+1)` palette entries, three bytes each.
    let gct_size = 1usize << ((packed & 0x07) + 1);
    let mut pos = 13usize;

    let mut global_palette = Vec::new();
    if has_gct {
        // Global color table is RGB triples and must fit before the first block.
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
                // Extension introducer. The next byte is the label, then a sub-block chain.
                let ext_label = *bytes.get(pos).ok_or(ImageError::UnexpectedEof)?;
                pos += 1;
                if ext_label == 0xF9 {
                    // Graphic control: block size, packed flags, delay, transparent index.
                    let block_size = *bytes.get(pos).ok_or(ImageError::UnexpectedEof)? as usize;
                    pos += 1;
                    if pos + block_size + 1 > bytes.len() {
                        return Err(ImageError::UnexpectedEof);
                    }
                    if block_size >= 4 {
                        let gce_packed = bytes[pos];
                        let trans_color = bytes[pos + 3];
                        // Bit 0 of the packed field enables the transparent color index.
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
                // Image descriptor: 8 bytes of geometry plus one packed fields byte.
                if pos + 9 > bytes.len() {
                    return Err(ImageError::UnexpectedEof);
                }
                let left = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as u32;
                let top = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]) as u32;
                let img_w = u16::from_le_bytes([bytes[pos + 4], bytes[pos + 5]]) as u32;
                let img_h = u16::from_le_bytes([bytes[pos + 6], bytes[pos + 7]]) as u32;
                validate_dimensions(img_w, img_h)?;
                let img_packed = bytes[pos + 8];
                pos += 9;

                let has_lct = (img_packed & 0x80) != 0;
                // Bit 6 selects the four-pass interlace. The stored order is not top-to-bottom.
                let interlaced = (img_packed & 0x40) != 0;
                let lct_size = 1usize << ((img_packed & 0x07) + 1);

                let mut local_palette = Vec::new();
                if has_lct {
                    // Local table replaces the global one for this frame only.
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
                // Image data is a chain of sub-blocks: one length byte, then that many bytes, ended by 0.
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
                    // Interlaced frames store passes, not sequential rows. Map the stored index back.
                    let dst_row = if interlaced {
                        interlaced_gif_row(row, img_h as usize)
                    } else {
                        row
                    };
                    let py = top + (dst_row as u32);
                    // Frame origin can place pixels outside the logical screen. Those samples are dropped.
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
                            // Palette RGB becomes opaque RGBA. An index past the table is black.
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

/// Map a stored interlaced row index to its destination row.
///
/// GIF passes are `(start, step)` pairs `(0, 8)`, `(4, 8)`, `(2, 4)`, then `(1, 2)`.
/// `row` is the index in stored order. `height` is the frame height. Rows that the
/// pass does not cover are skipped. If `row` falls past every pass, `row` itself
/// is returned.
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

/// Decompress one GIF LZW image into `expected_pixels` palette indices.
///
/// `min_code_size` is the byte stored before the sub-blocks (`2..=8`, checked by
/// the caller). The clear code is `1 << min_code_size` and the end code is the
/// next value. Codes are little-endian bit strings, starting at `min_code_size + 1`
/// bits and growing by one bit each time the dictionary length hits a power of two,
/// up to 12 bits. The dictionary holds at most 4096 entries. Output stops at the
/// end code, at a short bit stream, or once `expected_pixels` indices have been
/// written. An out-of-range code is [`ImageError::InvalidHeader`]. A short stream
/// still returns the indices decoded so far.
fn decode_gif_lzw(
    min_code_size: u8,
    stream: &[u8],
    expected_pixels: usize,
) -> Result<Vec<u8>, ImageError> {
    let clear_code = 1u16 << min_code_size;
    let end_code = clear_code + 1;

    let mut code_size = u32::from(min_code_size) + 1;
    let mut dict: Vec<Vec<u8>> = Vec::with_capacity(4096);
    // Clear code resets the dictionary to one single-byte entry per code through the end code.
    let reset_dict = |dict: &mut Vec<Vec<u8>>| {
        dict.clear();
        for i in 0..=end_code {
            dict.push(vec![(i & 0xFF) as u8]);
        }
    };
    reset_dict(&mut dict);

    let mut bit_pos = 0usize;
    // Next LZW code, low bit first. `None` when the code runs off the end of `stream`.
    let read_code = |bit_pos: &mut usize, code_size: u32| -> Option<u16> {
        let mut val = 0u32;
        for b in 0..code_size {
            let total_bit = *bit_pos + (b as usize);
            let byte_idx = total_bit >> 3;
            // A code that runs past the sub-block bytes ends the decode.
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
            // The code one past the dictionary is previous sequence plus its first byte.
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
                // Widen the code when the dictionary fills the current bit width. 12 bits is the cap.
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

/// Encode `image` as a single-frame `GIF89a` with a 256-color global palette.
///
/// Width and height are clamped to 65535. The logical screen packed byte is `0xF7`
/// (global table present, 256 entries). Palette index 0 is reserved and is the
/// transparent color. A source pixel with alpha below 128 is stored as index 0.
/// Other colors are appended until the table is full, then matched by squared
/// RGB distance against entries `1..`. The table is padded to 256 with black.
///
/// A graphic-control extension marks index 0 transparent. The image descriptor
/// has no local table and is not interlaced. LZW minimum code size is 8. The
/// encoder does not grow a dictionary: it writes 9-bit codes and emits clear
/// code 256 before every 250th literal so the decoder's code size stays at 9.
/// End code 257 follows the literals. Sub-blocks are at most 255 bytes, then a
/// zero terminator and the `0x3B` trailer. Does not return an error.
pub fn encode_gif(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"GIF89a");
    let w = (image.width.min(65535)) as u16;
    let h = (image.height.min(65535)) as u16;
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.push(0xF7); // Global color table flag, 8-bit resolution, 256 entries (2^(7+1)).
    out.push(0); // background color index
    out.push(0); // pixel aspect ratio, 0 means none was given

    let mut palette: Vec<[u8; 3]> = vec![[0, 0, 0]]; // index 0 reserved for transparent
    let mut indices = Vec::with_capacity((w as usize) * (h as usize));

    for y in 0..(h as u32) {
        for x in 0..(w as u32) {
            let rgba = image.pixel(x, y).unwrap_or([0, 0, 0, 0]);
            // Alpha below 128 becomes the reserved transparent index. Color is not quantized.
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
                // Table is full. Pick the nearest opaque entry by squared RGB distance.
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

    // Graphic control: label 0xF9, size 4, transparent flag, delay 0, index 0, terminator.
    out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00]);

    // Image descriptor at (0, 0), no local table, not interlaced.
    out.push(0x2C);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.push(0x00);

    // LZW minimum code size 8. Clear is 256 and the end code is 257, both 9-bit values.
    out.push(8);
    let mut bit_buf = Vec::new();
    let mut cur_byte = 0u8;
    let mut cur_bit = 0u8;
    // Write one 9-bit LZW code, least significant bit first, into the sub-block buffer.
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

    // Clear code 256 every 250 literals so a decoder never has to widen past 9 bits.
    for (i, &idx) in indices.iter().enumerate() {
        if i % 250 == 0 {
            push_code(256);
        }
        push_code(u16::from(idx));
    }
    push_code(257); // end code
    if cur_bit > 0 {
        bit_buf.push(cur_byte);
    }

    for chunk in bit_buf.chunks(255) {
        out.push(chunk.len() as u8);
        out.extend_from_slice(chunk);
    }
    out.push(0x00); // sub-block terminator
    out.push(0x3B); // trailer
    out
}

/// JPEG zigzag positions. Index `i` in zigzag order lives at natural frequency `JPEG_ZIGZAG[i]`.
const JPEG_ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// One JPEG Huffman table, expanded to `(bit length, code, symbol)` triples.
///
/// Codes are the canonical JPEG assignment: symbols are taken in the order given
/// by the `DHT` segment, lengths 1 through 16, and the code value increments then
/// shifts left at each new length.
#[derive(Clone, Default)]
struct JpegHuffTable {
    /// `(code_len, code_bits, symbol)` in the order the symbols were defined.
    entries: Vec<(u8, u16, u8)>,
}

impl JpegHuffTable {
    /// Build a table from the 16 `DHT` length counts and the following symbol list.
    ///
    /// `counts[i]` is how many symbols have bit length `i + 1`. Extra counts past
    /// `symbols` still advance the code, but no entry is stored for a missing symbol.
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

/// One component described by `SOF0` and updated by `SOS`.
#[derive(Clone, Copy, Default)]
struct JpegComponent {
    /// Component identifier from `SOF0` (commonly 1 = Y, 2 = Cb, 3 = Cr).
    id: u8,
    /// Horizontal sampling factor. A zero factor in the file is stored as 1.
    h_factor: u8,
    /// Vertical sampling factor. A zero factor in the file is stored as 1.
    v_factor: u8,
    /// Quantization table selector, clamped to `0..=3`.
    quant_id: u8,
    /// DC Huffman table selector from `SOS`, clamped to `0..=3`.
    dc_table: u8,
    /// AC Huffman table selector from `SOS`, clamped to `0..=3`.
    ac_table: u8,
}

/// Bit cursor over a JPEG entropy scan, starting at `pos` in `data`.
struct JpegBitReader<'a> {
    /// Scan bytes, including stuffed `FF 00` sequences.
    data: &'a [u8],
    /// Index of the next byte to pull into [`bit_buf`](Self::bit_buf).
    pos: usize,
    /// Current byte, consumed from the high bit downward.
    bit_buf: u32,
    /// How many bits in [`bit_buf`](Self::bit_buf) have not been returned yet.
    bit_count: u8,
}

impl<'a> JpegBitReader<'a> {
    /// Start reading at `pos`. No bits are buffered yet.
    fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos,
            bit_buf: 0,
            bit_count: 0,
        }
    }

    /// Return the next entropy bit, MSB first within each byte.
    ///
    /// A `0xFF` byte must be followed by `0x00` (stuffed data `0xFF`) or by a
    /// restart marker `0xD0..=0xD7`. In both of those cases the data byte inserted
    /// into the bit buffer is the `0xFF` itself. Any other following byte is
    /// [`ImageError::InvalidHeader`]. Running off the end of the scan is
    /// [`ImageError::UnexpectedEof`].
    fn next_bit(&mut self) -> Result<u8, ImageError> {
        if self.bit_count == 0 {
            let b = *self.data.get(self.pos).ok_or(ImageError::UnexpectedEof)?;
            self.pos += 1;
            if b == 0xFF {
                let next = *self.data.get(self.pos).ok_or(ImageError::UnexpectedEof)?;
                self.pos += 1;
                // `FF 00` is a stuffed 0xFF. `FF D0`-`FF D7` is also accepted here and still yields 0xFF.
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

    /// Read `count` bits, MSB first. A `count` of 0 returns 0.
    fn read_bits(&mut self, count: u8) -> Result<u16, ImageError> {
        let mut val = 0u16;
        for _ in 0..count {
            val = (val << 1) | u16::from(self.next_bit()?);
        }
        Ok(val)
    }

    /// Decode one symbol from `table` by growing the code from 1 bit to 16.
    ///
    /// Returns [`ImageError::InvalidHeader`] when no entry matches.
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

/// Sign-extend a JPEG magnitude category.
///
/// `category` is the bit count. `bits` is the unsigned value just read. Category
/// 0 returns 0. Values below `1 << (category - 1)` are negative: `bits - (2^category - 1)`.
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

/// Decode a baseline `SOF0` JPEG from `bytes` into opaque RGBA.
///
/// The file must start with `FF D8` (`SOI`). Marker bytes are `FF` followed by a
/// type. Fill bytes of extra `FF` are skipped. `EOI` (`D9`) ends the walk.
/// Standalone bytes `00` and `D0` through `D8` have no length field and are skipped.
/// Every other marker has a big-endian `u16` length that includes those two bytes.
///
/// Segments that are interpreted:
/// * `DB` (`DQT`): 8-bit (`Pq = 0`) or 16-bit (`Pq = 1`) tables, destination `0..=3`.
/// * `C0` (`SOF0`): precision must be 8, and the component count must be 1 or 3.
///   Height and width are big-endian `u16` values. Sampling factors of 0 are stored as 1.
/// * `C4` (`DHT`): DC tables when the class nibble is 0, AC tables otherwise, index `0..=3`.
/// * `DD` (`DRI`): restart interval as a big-endian `u16`, when the payload is at least 2 bytes.
/// * `DA` (`SOS`): binds DC and AC table ids, then decodes the single scan that follows.
///
/// Any other marker with a length is ignored. Missing `SOS`, a short segment, a bad
/// table index, or a non-8-bit frame is an error. The frame must pass
/// [`validate_dimensions`]. Gray frames expand to `R = G = B`. YCbCr uses the
/// conversion in [`decode_jpeg_scan`]. There is no progressive or multi-scan decoder.
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
            // EOI. A scan should already have returned.
            break;
        }
        // Stuffed `00` and markers without a length (RST0-RST7, and SOI) are not segments.
        if marker == 0x00 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        if pos + 2 > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        let seg_len = u16::from_be_bytes([bytes[pos], bytes[pos + 1]]) as usize;
        // `seg_len` counts itself. The payload is the following `seg_len - 2` bytes.
        if seg_len < 2 || pos + seg_len > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        let seg = &bytes[pos + 2..pos + seg_len];
        pos += seg_len;

        match marker {
            0xDB => {
                // DQT. Several tables may share one segment.
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
                        // 8-bit quantization values, zigzag order, 64 bytes.
                        if sp + 64 > seg.len() {
                            return Err(ImageError::UnexpectedEof);
                        }
                        for i in 0..64 {
                            qtables[tq][i] = u16::from(seg[sp + i]);
                        }
                        sp += 64;
                    } else {
                        // 16-bit quantization values, big-endian, 128 bytes.
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
                // SOF0 baseline DCT. Precision, height, width, then 3 bytes per component.
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
                validate_dimensions(width, height)?;
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
                // DHT. Class nibble 0 is DC, 1 is AC. The low nibble selects the table slot.
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
                    // The 16 counts are followed by exactly that many symbol bytes.
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
                // DRI. Restart interval in MCUs. A short payload leaves the interval at 0.
                if seg.len() >= 2 {
                    restart_interval = u16::from_be_bytes([seg[0], seg[1]]);
                }
            }
            0xDA => {
                // SOS. The entropy-coded scan begins at the first byte after this segment.
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

/// Decode the single entropy scan that starts at `scan_pos` and expand it to RGBA.
///
/// `components` is in `SOF0` order. `qtables`, `dc_huff`, and `ac_huff` are the
/// four slots filled by earlier markers. `restart_interval` is in MCUs; zero
/// disables restarts. The MCU size is `8 * max(horizontal factors)` by
/// `8 * max(vertical factors)`. Each block is Huffman-decoded, dequantized in
/// zigzag order, and inverse-transformed by [`idct_8x8`]. Samples that fall
/// outside `width` by `height` are skipped. One component is gray. Three
/// components are Y, Cb, Cr converted to RGB with alpha 255. The scan is not
/// required to end on `EOI`.
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
                // Restart: drop the partial byte, reset DC predictors, and skip an RST marker if present.
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
                    // Undo zigzag, then scale each coefficient by its quantization value.
                    for i in 0..64 {
                        coeff[JPEG_ZIGZAG[i]] = (zz[i] as f32) * (qtable[i] as f32);
                    }
                    blocks.push(idct_8x8(&coeff));
                }
                comp_planes.push(blocks);
            }

            for py in 0..mcu_h {
                let img_y = my * mcu_h + py;
                // MCUs are padded to a multiple of 8 samples. Pixels past the frame are discarded.
                if img_y >= height {
                    continue;
                }
                for px in 0..mcu_w {
                    let img_x = mx * mcu_w + px;
                    if img_x >= width {
                        continue;
                    }

                    // Pick one sample from component `ci`, honoring its sampling factor inside the MCU.
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
                        // One component is luminance, replicated into R, G, and B.
                        let y_val = sample(0).round().clamp(0.0, 255.0) as u8;
                        out.set_pixel(img_x, img_y, [y_val, y_val, y_val, 255]);
                    } else {
                        // JFIF full-range YCbCr. Cb and Cr are centered at 0 before the matrix.
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

/// Inverse 8x8 DCT-II, separable, then level-shift by +128 and clamp to `0..=255`.
///
/// `coeff` is in natural frequency order, not zigzag order. The DC basis uses
/// `1/sqrt(2)`. The result is one byte-scaled sample per spatial position, row-major.
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

/// Forward 8x8 DCT-II used by [`encode_jpeg`].
///
/// `block` holds levels in `0..=255`, row-major. Each sample is shifted by -128
/// before the transform. Coefficients are rounded to `i32` in natural frequency
/// order. No quantization is applied here.
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

/// Encode `image` as a baseline `SOF0` JPEG, 4:4:4 YCbCr, one scan.
///
/// Width and height are clamped into `1..=65535`. Alpha is ignored. The stream is
/// `SOI`, one `DQT` (table 0, every step equal to 1), `SOF0` with components 1, 2,
/// and 3 each using sampling `0x11` and table 0, a DC `DHT` (12 symbols of length
/// 4, categories 0 through 11), an AC `DHT` (162 symbols of length 8: end-of-block
/// `0x00`, sixteen-zero `0xF0`, then run `0..=15` with size `1..=10`), `SOS`, the
/// entropy scan, and `EOI`.
///
/// DC differences and AC values are magnitude-coded. The DC category is written as
/// itself in 4 bits, which matches that canonical table. Each AC symbol is written
/// as its index in the symbol list, in 8 bits. A run of 16 zeros emits symbol index
/// 1. A trailing zero run emits end-of-block (index 0). Bytes equal to `0xFF` in
/// the scan are stuffed with a following `0x00`. Unused bits in the last byte are
/// filled with ones. Does not return an error. The result is not bit-identical to
/// a typical encoder because the quantizer is all ones and the Huffman tables are
/// flat.
pub fn encode_jpeg(image: &Image) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI

    // DQT: table 0, 8-bit, length 67, every step 1 so coefficients are not quantized down.
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
    // Emit `n` bits of `val`, most significant bit first, stuffing `FF 00` when a byte is `FF`.
    let push_bits = |out: &mut Vec<u8>, cur_byte: &mut u8, cur_bits: &mut u8, val: u16, n: u8| {
        for i in (0..n).rev() {
            let b = ((val >> i) & 1) as u8;
            *cur_byte = (*cur_byte << 1) | b;
            *cur_bits += 1;
            if *cur_bits == 8 {
                out.push(*cur_byte);
                // A data byte of 0xFF must be followed by 0x00 so it is not read as a marker.
                if *cur_byte == 0xFF {
                    out.push(0x00);
                }
                *cur_byte = 0;
                *cur_bits = 0;
            }
        }
    };

    // JPEG magnitude: category is the bit length, and a negative value is ones' complement.
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
                    // RGB to full-range YCbCr. Alpha is not stored. Edge pixels repeat to fill the 8x8 block.
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

/// Decode the largest entry in an ICO (type 1) or CUR (type 2) directory.
///
/// The header is three little-endian `u16` values: reserved (must be 0), type
/// (1 or 2), and a count in `1..=64`. Each directory entry is 16 bytes: width,
/// height (a 0 byte means 256), two ignored bytes, two ignored `u16` fields, then
/// the payload size and the file offset as little-endian `u32`. Entries that do
/// not fit are skipped. Among the rest, the greatest `width * height` wins, and
/// a tie keeps the later entry.
///
/// A payload that starts with the PNG signature is passed to [`decode_png`].
/// Otherwise it must be a DIB of at least 40 bytes. The DIB height is twice the
/// visual height (XOR bitmap plus AND mask). Only 24-bit and 32-bit XOR bitmaps
/// are read, bottom-up, padded to a multiple of 4 bytes, as `B, G, R` and optional
/// `A`. The AND mask is not applied. 24-bit alpha is forced to 255. A bad header
/// is [`ImageError::InvalidHeader`]. A short payload is [`ImageError::UnexpectedEof`].
/// An unsupported DIB depth is [`ImageError::UnsupportedFormat`].
pub fn decode_ico(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 6 {
        return Err(ImageError::UnexpectedEof);
    }
    let reserved = u16::from_le_bytes([bytes[0], bytes[1]]);
    let icon_type = u16::from_le_bytes([bytes[2], bytes[3]]);
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if reserved != 0 || (icon_type != 1 && icon_type != 2) || count == 0 || count > 64 {
        return Err(ImageError::InvalidHeader("invalid ICO directory header"));
    }
    // The directory is 6 bytes plus 16 bytes for every entry.
    if 6 + count * 16 > bytes.len() {
        return Err(ImageError::UnexpectedEof);
    }

    let mut best_entry: Option<(u32, u32, usize, usize)> = None;
    for i in 0..count {
        let base = 6 + i * 16;
        // A zero dimension byte means 256, which does not fit in one byte otherwise.
        let w = if bytes[base] == 0 {
            256
        } else {
            u32::from(bytes[base])
        };
        let h = if bytes[base + 1] == 0 {
            256
        } else {
            u32::from(bytes[base + 1])
        };
        let size = u32::from_le_bytes([
            bytes[base + 8],
            bytes[base + 9],
            bytes[base + 10],
            bytes[base + 11],
        ]) as usize;
        let offset = u32::from_le_bytes([
            bytes[base + 12],
            bytes[base + 13],
            bytes[base + 14],
            bytes[base + 15],
        ]) as usize;
        // Skip an entry whose payload is empty or runs past the file.
        if size == 0 || offset.saturating_add(size) > bytes.len() {
            continue;
        }
        if best_entry.is_none_or(|(bw, bh, _, _)| w * h >= bw * bh) {
            best_entry = Some((w, h, offset, size));
        }
    }

    let (_, _, offset, size) =
        best_entry.ok_or(ImageError::InvalidHeader("no valid ICO directory entry"))?;
    let payload = &bytes[offset..offset + size];
    // PNG-compressed icons are a whole PNG file, including the signature and CRC checks.
    if payload.starts_with(&PNG_SIGNATURE) {
        return decode_png(payload);
    }
    if payload.len() < 40 {
        return Err(ImageError::UnexpectedEof);
    }
    let header_size = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    if header_size < 40 || header_size > payload.len() {
        return Err(ImageError::InvalidHeader("invalid ICO DIB header"));
    }
    let width = i32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]).unsigned_abs();
    let raw_height =
        i32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]).unsigned_abs();
    // ICO DIB height counts the XOR bitmap and the AND mask, so the visible height is half.
    let height = (raw_height / 2).max(1);
    validate_dimensions(width, height)?;
    let bpp = u16::from_le_bytes([payload[14], payload[15]]);
    if bpp != 32 && bpp != 24 {
        return Err(ImageError::UnsupportedFormat(
            "ICO DIB must be 24-bit or 32-bit",
        ));
    }
    let bytes_per_pixel = (bpp / 8) as usize;
    // XOR rows are padded to a multiple of 4. The AND mask that follows is not read.
    let row_stride = (width as usize * bytes_per_pixel).div_ceil(4) * 4;
    let xor_bytes = row_stride
        .checked_mul(height as usize)
        .ok_or(ImageError::UnexpectedEof)?;
    if header_size + xor_bytes > payload.len() {
        return Err(ImageError::UnexpectedEof);
    }
    let mut img = Image::new(width, height);
    for y in 0..height {
        // The XOR bitmap is bottom-up. The last stored row is the top of the icon.
        let src_row = (height - 1 - y) as usize;
        let row_start = header_size + src_row * row_stride;
        for x in 0..width {
            let px = row_start + (x as usize) * bytes_per_pixel;
            // DIB order is B, G, R. 24-bit icons have no alpha byte.
            let b = payload[px];
            let g = payload[px + 1];
            let r = payload[px + 2];
            let a = if bpp == 32 { payload[px + 3] } else { 255 };
            img.set_pixel(x, y, [r, g, b, a]);
        }
    }
    Ok(img)
}

/// Encode `image` as a one-entry ICO whose payload is a PNG from [`encode_png`].
///
/// The directory is reserved 0, type 1, count 1, then one 16-byte entry. Width and
/// height bytes are 0 when that side is 256 or larger, otherwise the low 8 bits.
/// Planes is 1, bit count is 32, and the data offset is 22 (6-byte header plus the
/// 16-byte entry). Does not return an error. Cursor files are not written.
pub fn encode_ico(image: &Image) -> Vec<u8> {
    let png = encode_png(image);
    let mut out = Vec::with_capacity(22 + png.len());
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved, must be 0
    out.extend_from_slice(&1u16.to_le_bytes()); // type 1 (icon, not cursor)
    out.extend_from_slice(&1u16.to_le_bytes()); // one directory entry
    let w_byte = if image.width >= 256 {
        0u8
    } else {
        image.width as u8
    };
    let h_byte = if image.height >= 256 {
        0u8
    } else {
        image.height as u8
    };
    out.push(w_byte);
    out.push(h_byte);
    out.push(0); // palette count, 0 because the payload is a PNG
    out.push(0); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // color planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel claimed for the entry
    out.extend_from_slice(&(png.len() as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes()); // payload starts immediately after this entry
    out.extend_from_slice(&png);
    out
}

/// Encode `image` as a RIFF `WEBP` container with a zlib `RGBA` chunk.
///
/// This is not a `VP8` or `VP8L` bitstream. The file is `RIFF`, a little-endian
/// size of everything after that field, then `WEBP`. The first chunk is `VP8X`,
/// payload length 10: flags `0x10` (alpha), three reserved zero bytes, then
/// width-minus-one and height-minus-one as 24-bit little-endian values. A zero
/// side is treated as 1 before the subtraction. The second chunk is `RGBA`: a
/// little-endian length and the raw [`Image::pixels`] bytes zlib-wrapped by
/// [`compress_zlib`](flux_compress::compress_zlib). An odd-sized payload is padded
/// with one zero byte so the next chunk would stay even-aligned. Does not return
/// an error. [`decode_webp`] is the matching reader.
pub fn encode_webp(image: &Image) -> Vec<u8> {
    let w_minus_1 = image.width.max(1) - 1;
    let h_minus_1 = image.height.max(1) - 1;
    let compressed_rgba = compress_zlib(&image.pixels);

    let mut chunks = Vec::new();
    // VP8X: 10-byte payload, alpha flag 0x10, then 24-bit width-1 and height-1.
    chunks.extend_from_slice(b"VP8X");
    chunks.extend_from_slice(&10u32.to_le_bytes());
    chunks.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]);
    chunks.extend_from_slice(&[
        (w_minus_1 & 0xFF) as u8,
        ((w_minus_1 >> 8) & 0xFF) as u8,
        ((w_minus_1 >> 16) & 0xFF) as u8,
    ]);
    chunks.extend_from_slice(&[
        (h_minus_1 & 0xFF) as u8,
        ((h_minus_1 >> 8) & 0xFF) as u8,
        ((h_minus_1 >> 16) & 0xFF) as u8,
    ]);

    // Custom chunk: zlib of tightly packed RGBA, not a VP8L transform stream.
    chunks.extend_from_slice(b"RGBA");
    chunks.extend_from_slice(&(compressed_rgba.len() as u32).to_le_bytes());
    chunks.extend_from_slice(&compressed_rgba);
    // RIFF chunks are padded to an even size. The pad is not part of the chunk length.
    if compressed_rgba.len() % 2 == 1 {
        chunks.push(0);
    }

    let riff_size = (4 + chunks.len()) as u32;
    let mut out = Vec::with_capacity(8 + riff_size as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff_size.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&chunks);
    out
}

/// Decode a RIFF `WEBP` file produced by [`encode_webp`], or one with the same `RGBA` chunk.
///
/// Requires `RIFF` at byte 0 and `WEBP` at byte 8. Each chunk is a 4-byte FourCC,
/// a little-endian length, that many payload bytes, and one pad byte when the
/// length is odd. `VP8X` must hold at least 10 bytes; width and height are the
/// 24-bit little-endian fields at payload offsets 4 and 7, plus one. A `VP8L`
/// chunk is used only when it has at least 5 bytes and starts with signature
/// `0x2F`: the next 14 bits are width-minus-one and the following 14 are
/// height-minus-one. The `VP8L` transform data after those bits is not decoded.
///
/// Pixels come only from an `RGBA` chunk, inflated with
/// [`decompress_zlib_bounded`](flux_compress::decompress_zlib_bounded) to exactly
/// `width * height * 4` bytes and checked by [`Image::from_rgba`]. Missing that
/// chunk returns [`ImageError::UnsupportedFormat`], including ordinary lossy `VP8`
/// files. A short chunk is [`ImageError::UnexpectedEof`]. A bad `VP8L` signature
/// or a zero size after the walk is [`ImageError::InvalidHeader`]. Dimensions are
/// checked with [`validate_dimensions`] both when a header is seen and again
/// before inflate.
pub fn decode_webp(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(ImageError::InvalidHeader("missing RIFF WEBP header"));
    }
    let mut pos = 12usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut rgba_chunk: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let fourcc = &bytes[pos..pos + 4];
        let chunk_len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;
        // The payload must fit. The even pad is applied after the payload is copied.
        if pos + chunk_len > bytes.len() {
            return Err(ImageError::UnexpectedEof);
        }
        let payload = &bytes[pos..pos + chunk_len];
        if fourcc == b"VP8X" {
            if payload.len() < 10 {
                return Err(ImageError::InvalidHeader("truncated VP8X chunk"));
            }
            let w_minus_1 = u32::from(payload[4])
                | (u32::from(payload[5]) << 8)
                | (u32::from(payload[6]) << 16);
            let h_minus_1 = u32::from(payload[7])
                | (u32::from(payload[8]) << 8)
                | (u32::from(payload[9]) << 16);
            width = w_minus_1 + 1;
            height = h_minus_1 + 1;
            validate_dimensions(width, height)?;
        } else if fourcc == b"VP8L" && payload.len() >= 5 {
            // Only the 14-bit width and height fields are read. The bitstream is not.
            if payload[0] != 0x2F {
                return Err(ImageError::InvalidHeader("invalid VP8L signature"));
            }
            let bits = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
            width = (bits & 0x3FFF) + 1;
            height = ((bits >> 14) & 0x3FFF) + 1;
            validate_dimensions(width, height)?;
        } else if fourcc == b"RGBA" {
            rgba_chunk = Some(payload);
        }
        // Odd payloads are followed by one pad byte that is not included in `chunk_len`.
        pos += chunk_len + (chunk_len % 2);
    }

    validate_dimensions(width, height)?;
    let expected = (width as usize) * (height as usize) * 4;
    if let Some(compressed) = rgba_chunk {
        let pixels = decompress_zlib_bounded(compressed, expected)?;
        return Image::from_rgba(width, height, pixels);
    }
    Err(ImageError::UnsupportedFormat(
        "WebP stream requires lossless RGBA chunk",
    ))
}

/// In-memory round trips for each encoder and decoder, plus the dimension cap.
/// Individual tests are not commented.
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

        let mut bad = png_bytes.clone();
        bad[16] ^= 0xff;
        assert!(decode_png(&bad).is_err());

        let ico_bytes = encode_ico(&img);
        let decoded_ico = decode_auto(&ico_bytes).unwrap();
        assert_eq!(decoded_ico, img);

        let webp_bytes = encode_webp(&img);
        let decoded_webp = decode_auto(&webp_bytes).unwrap();
        assert_eq!(decoded_webp, img);

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

    #[test]
    fn rejects_adversarial_image_dimensions_exceeding_security_caps() {
        assert!(matches!(
            validate_dimensions(16384, 16384),
            Err(ImageError::DimensionsTooLarge {
                width: 16384,
                height: 16384
            })
        ));
        let mut fake_png = Vec::new();
        fake_png.extend_from_slice(&PNG_SIGNATURE);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&20000u32.to_be_bytes());
        ihdr.extend_from_slice(&20000u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        write_png_chunk(&mut fake_png, b"IHDR", &ihdr);
        write_png_chunk(&mut fake_png, b"IEND", &[]);
        assert!(matches!(
            decode_png(&fake_png),
            Err(ImageError::DimensionsTooLarge {
                width: 20000,
                height: 20000
            })
        ));
    }
}
