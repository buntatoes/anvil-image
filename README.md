# flux-image

[![Release](https://img.shields.io/badge/release-v.01-e8c37a)](https://github.com/buntatoes/flux-image/releases/tag/v.01)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](./LICENSE)
[![Edition](https://img.shields.io/badge/rust-2024_edition-orange.svg)](https://github.com/buntatoes/flux-image)

`flux-image` is a standalone Rust library for decoding, encoding, and resampling raster images (**PNG**, **BMP**, and **Netpbm PPM/PGM**) into top-down 8-bit **RGBA8888** pixel buffers.

---

## Features

- **Automatic Format Detection (`decode_auto`)**: Inspects magic bytes to dispatch the appropriate PNG, BMP, or PPM/PGM decoder.
- **PNG Decoder & Encoder (`decode_png` / `encode_png`)**:
  - Supports 8-bit Grayscale (`0`), Truecolor RGB (`2`), Indexed Palette (`3` with `PLTE` + `tRNS` alpha), Grayscale+Alpha (`4`), and Truecolor RGBA (`6`).
  - Implements all 5 PNG scanline reconstruction filters (`None`, `Sub`, `Up`, `Average`, `Paeth`) backed by [`flux-compress`](https://github.com/buntatoes/flux-compress).
- **BMP Decoder & Encoder (`decode_bmp` / `encode_bmp`)**:
  - Supports 24-bit BGR and 32-bit BGRA uncompressed Windows bitmaps in both bottom-up and top-down row orders.
- **Netpbm Decoder & Encoder (`decode_ppm` / `encode_ppm`)**:
  - Supports binary `P6` (RGB) and `P5` (Grayscale) images.
- **Bilinear Resampling (`Image::resize_bilinear`)**:
  - Scales RGBA images smoothly to target layout dimensions.

---

## Quick Start

```rust
use flux_image::{Image, decode_auto, encode_png};

let mut img = Image::new(16, 16);
img.set_pixel(0, 0, [232, 195, 122, 255]);

let png_bytes = encode_png(&img);
let decoded = decode_auto(&png_bytes).expect("valid PNG");
let thumbnail = decoded.resize_bilinear(8, 8);
assert_eq!(thumbnail.width, 8);
```

---

## API Overview

| Function / Method | Description |
| :--- | :--- |
| `decode_auto(&[u8]) -> Result<Image, ImageError>` | Detects PNG, BMP, or PPM/PGM magic bytes and decodes into an RGBA `Image`. |
| `decode_png(&[u8]) -> Result<Image, ImageError>` | Decodes a PNG file into an RGBA `Image`. |
| `encode_png(&Image) -> Vec<u8>` | Encodes an RGBA `Image` into a lossless 8-bit RGBA PNG file. |
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

## License

Licensed under the Mozilla Public License Version 2.0 ([`MPL-2.0`](./LICENSE)).

## Author

**Buntos**
