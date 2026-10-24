<div align="center">
  <h1>anvil-image</h1>
  <p>PNG, JPEG, GIF, WebP, ICO, BMP, and PPM.</p>
  <p>
    <img src="https://img.shields.io/badge/release-v.05-e8c37a?style=flat-square" alt="v.05" />
    <img src="https://img.shields.io/badge/status-Proof_of_Concept-d4a359?style=flat-square" alt="Proof of Concept" />
    <a href="./LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue?style=flat-square" alt="MPL-2.0" /></a>
    <a href="./SECURITY.md"><img src="https://img.shields.io/badge/security-policy-2ea44f?style=flat-square" alt="Security" /></a>
    <img src="https://img.shields.io/badge/rust-2024-orange?style=flat-square" alt="Rust 2024" />
  </p>
</div>

Version 0.5.0.

It needs [anvil-compress](https://github.com/buntatoes/anvil-compress) in the directory next to this one.

## What it does

- `decode_auto` picks a decoder from the file header: PNG, baseline JPEG, GIF, WebP, ICO, BMP, or PPM/PGM.
- PNG is 8-bit, not interlaced. Color types are gray, RGB, palette, gray+alpha, and RGBA. Each chunk checksum is checked before the chunk is used.
- JPEG is baseline (`SOF0`), gray or YCbCr.
- GIF reads a color table, transparency, interlacing, and LZW.
- WebP reads a RIFF container. ICO picks one image from the directory. BMP is uncompressed 24-bit or 32-bit. PPM/PGM is the binary Netpbm form.
- `encode_png`, `encode_jpeg`, `encode_gif`, `encode_webp`, `encode_ico`, `encode_bmp`, and `encode_ppm` write those formats back.
- `resize_bilinear` scales an image.

## Limits

- A side longer than 8192 pixels is rejected.
- More than 16,777,216 pixels is rejected.

## Build

```
cargo test
```

Rust 2024. The only dependency is `anvil-compress`, by path.

## The other repositories

The browser is [anvil-browser](https://github.com/buntatoes/anvil-browser). These libraries sit next to it. None of them come from crates.io.

| Repository | What it is |
| --- | --- |
| [anvil-cipher](https://github.com/buntatoes/anvil-cipher) | AES-128, AES-256, CTR, AES-256-GCM |
| [anvil-compress](https://github.com/buntatoes/anvil-compress) | DEFLATE, zlib, gzip |
| [anvil-image](https://github.com/buntatoes/anvil-image) | PNG, JPEG, GIF, WebP, ICO, BMP, PPM |
| [anvil-json](https://github.com/buntatoes/anvil-json) | JSON |
| [anvil-ttf](https://github.com/buntatoes/anvil-ttf) | TrueType and OpenType |
| [anvil-gpu](https://github.com/buntatoes/anvil-gpu) | CPU drawing of boxes, shadows, and NV12 |
| [anvil-video](https://github.com/buntatoes/anvil-video) | CPU frames in a small MP4 container |

## License

MPL-2.0. Copyright (c) 2026 Buntos.
