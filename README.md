<div align="center">
  <h1>flux-image</h1>
  <p><strong>PNG, JPEG, GIF89a, WebP, Windows ICO, BMP, and Netpbm Image Decoder, Encoder &amp; Resampler (Proof of Concept)</strong></p>
  <p>
    <a href="https://github.com/buntatoes/flux-image/releases/tag/v.04"><img src="https://img.shields.io/badge/release-v.04-e8c37a?style=flat-square" alt="Release v.04" /></a>
    <img src="https://img.shields.io/badge/status-Proof_of_Concept_(PoC)-d4a359?style=flat-square" alt="Proof of Concept" />
    <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue?style=flat-square" alt="License MPL-2.0" /></a>
    <a href="https://github.com/buntatoes/flux-image"><img src="https://img.shields.io/badge/rust-2024_edition-orange?style=flat-square" alt="Rust 2024 Edition" /></a>
  </p>
</div>

> [!NOTE]
> **Proof of Concept (PoC) Notice**: `flux-image` is a standalone **Proof of Concept** raster image decoding, encoding, and bilinear resampling library authored from scratch by **Buntos** as part of the [**Flux Browser**](https://github.com/buntatoes/flux-browser) engine ecosystem. It decodes and encodes **PNG**, **Baseline JPEG**, **GIF87a/GIF89a**, **WebP**, **Windows ICO**, **BMP**, and **Netpbm PPM/PGM** into top-down 8-bit **RGBA8888** pixel buffers with strict allocation-bomb protection (`MAX_IMAGE_DIMENSION = 8192`, `MAX_IMAGE_PIXELS = 16,777,216`).

---

## Features (`v.04`)

- **Automatic Format Detection (`decode_auto`)**: Inspects magic bytes to dispatch the appropriate PNG, JPEG, GIF87a/GIF89a, WebP (`RIFF....WEBP`), Windows ICO, BMP, or PPM/PGM decoder.
- **Allocation-Bomb & Decompression-Bomb Guard (`validate_dimensions`)**:
  - Enforces `MAX_IMAGE_DIMENSION = 8192` and `MAX_IMAGE_PIXELS = 16,777,216` (64 MiB RGBA ceiling) across all image formats and bounds PNG/WebP zlib stream expansion before allocating pixel buffers.
- **Windows `.ico` / Favicon Decoder & Encoder (`decode_ico` / `encode_ico`)**:
  - Parses multi-entry `.ico` icon directories, selects the highest-resolution valid icon entry, and decodes embedded PNG or 24/32-bit DIB icon frames into `RGBA8888`.
- **RIFF WebP Container Decoder & Encoder (`decode_webp` / `encode_webp`)**:
  - Parses `RIFF....WEBP` containers (`VP8X`, `VP8L`, and lossless `RGBA` chunks) with pre-allocation dimension validation.
- **PNG Decoder & Encoder (`decode_png` / `encode_png`)**:
  - Supports 8-bit Grayscale (`0`), Truecolor RGB (`2`), Indexed Palette (`3` with `PLTE` + `tRNS` alpha), Grayscale+Alpha (`4`), and Truecolor RGBA (`6`).
  - Implements all 5 PNG scanline reconstruction filters (`None`, `Sub`, `Up`, `Average`, `Paeth`) backed by [`flux-compress`](https://github.com/buntatoes/flux-compress).
- **Baseline JPEG Decoder & Encoder (`decode_jpeg` / `encode_jpeg`)**:
  - Parses `SOI`, `DQT`, `SOF0`, `DHT`, `DRI`, and `SOS` segments, decodes byte-stuffed canonical Huffman DC/AC streams, dequantizes 8x8 blocks, evaluates 2D 8x8 IDCT/FDCT, and converts grayscale or subsampled YCbCr (`4:4:4`, `4:2:2`, `4:2:0`) to RGBA8888.
- **GIF87a/GIF89a Decoder & Encoder (`decode_gif` / `encode_gif`)**:
  - Parses Global and Local Color Tables, Graphic Control Extension transparency indices, interlaced row passes, and variable-width LZW code streams up to 12 bits.
- **BMP Decoder & Encoder (`decode_bmp` / `encode_bmp`)**:
  - Supports 24-bit BGR and 32-bit BGRA uncompressed Windows bitmaps in both bottom-up and top-down row orders.
- **Netpbm Decoder & Encoder (`decode_ppm` / `encode_ppm`)**:
  - Supports binary `P6` (RGB) and `P5` (Grayscale) images.
- **Bilinear Resampling (`Image::resize_bilinear`)**:
  - Scales RGBA images smoothly to target layout dimensions.

---

## Quick Start

```rust
use flux_image::{Image, decode_auto, encode_jpeg, encode_png, validate_dimensions};

validate_dimensions(16, 16).expect("dimensions within safety ceiling");
let mut img = Image::new(16, 16);
img.set_pixel(0, 0, [232, 195, 122, 255]);

let png_bytes = encode_png(&img);
let decoded = decode_auto(&png_bytes).expect("valid PNG");
let jpeg_bytes = encode_jpeg(&decoded);
let decoded_jpeg = decode_auto(&jpeg_bytes).expect("valid JPEG");
let thumbnail = decoded_jpeg.resize_bilinear(8, 8);
assert_eq!(thumbnail.width, 8);
```

---

## API Overview

| Function / Method | Description |
| :--- | :--- |
| `decode_auto(&[u8]) -> Result<Image, ImageError>` | Detects PNG, JPEG, GIF, WebP, ICO, BMP, or PPM/PGM magic bytes and decodes into an RGBA `Image`. |
| `validate_dimensions(u32, u32) -> Result<(), ImageError>` | Verifies image dimensions are non-zero and within `MAX_IMAGE_DIMENSION` (8192) and `MAX_IMAGE_PIXELS` (16 MP). |
| `decode_png(&[u8]) -> Result<Image, ImageError>` | Decodes a PNG file into an RGBA `Image`. |
| `encode_png(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a lossless 8-bit RGBA PNG file. |
| `decode_jpeg(&[u8]) -> Result<Image, ImageError>` | Decodes a baseline (`SOF0`) grayscale or YCbCr JPEG file. |
| `encode_jpeg(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a baseline `SOF0` 4:4:4 JPEG file. |
| `decode_gif(&[u8]) -> Result<Image, ImageError>` | Decodes a GIF87a/GIF89a LZW-compressed image (including transparency). |
| `encode_gif(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a 256-color GIF89a file. |
| `decode_webp(&[u8]) -> Result<Image, ImageError>` | Decodes a RIFF WebP container into an RGBA `Image`. |
| `encode_webp(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a lossless RIFF WebP container. |
| `decode_ico(&[u8]) -> Result<Image, ImageError>` | Decodes a Windows `.ico` icon directory into an RGBA `Image`. |
| `encode_ico(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a Windows `.ico` icon file. |
| `decode_bmp(&[u8]) -> Result<Image, ImageError>` | Decodes a 24-bit or 32-bit uncompressed BMP file. |
| `encode_bmp(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a 32-bit top-down BMP file. |
| `decode_ppm(&[u8]) -> Result<Image, ImageError>` | Decodes a binary `P6` or `P5` Netpbm file. |
| `encode_ppm(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a binary `P6` PPM file. |
| `Image::resize_bilinear(width, height) -> Image` | Resamples the image to `(width, height)` using bilinear interpolation. |

---

## Building and Testing

Ensure [`flux-compress`](https://github.com/buntatoes/flux-compress) is cloned in the sibling directory (`../flux-compress`), then run:

```powershell
cargo test
```

---

## Security Policy

See [`SECURITY.md`](./SECURITY.md) for supported versions, allocation-bomb security controls, and vulnerability reporting instructions.

## License

Licensed under the Mozilla Public License Version 2.0 ([`MPL-2.0`](./LICENSE)).

## Author

**Buntos** ([@buntatoes](https://github.com/buntatoes))
