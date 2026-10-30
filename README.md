<div align="center">
  <h1>Anvil Image</h1>
  <p>PNG, JPEG, GIF, WebP, ICO, BMP, and PPM.</p>
  <p>
    <img src="https://img.shields.io/badge/release-v.07-e8c37a?style=flat-square" alt="v.07" />
    <img src="https://img.shields.io/badge/status-Proof_of_Concept-d4a359?style=flat-square" alt="Proof of Concept" />
    <a href="./LICENSE"><img src="https://img.shields.io/badge/license-PolyForm_Noncommercial_1.0.0_%7C_Paid_Commercial_%28Non--Compete%29-blue?style=flat-square" alt="PolyForm Noncommercial 1.0.0 | Paid Commercial (Non-Compete)" /></a>
    <a href="./SECURITY.md"><img src="https://img.shields.io/badge/security-policy-2ea44f?style=flat-square" alt="Security" /></a>
    <img src="https://img.shields.io/badge/rust-2024-orange?style=flat-square" alt="Rust 2024" />
  </p>
</div>

Version 0.7.1 (`v.07`).

It needs [anvil-compress](https://github.com/buntatoes/anvil-compress) in the directory next to this one.

## What it does

- `decode_auto` picks a decoder from the file header: PNG, baseline JPEG, GIF, WebP, ICO, BMP, or PPM/PGM.
- PNG is 8-bit, not interlaced. Color types are gray, RGB, palette, gray+alpha, and RGBA. Each chunk checksum is checked before the chunk is used.
- JPEG is baseline (`SOF0`), gray or YCbCr. A coefficient category outside the 8-bit baseline range is an error, not a guess.
- GIF reads a color table, transparency, interlacing, and LZW.
- WebP decodes still images, written from the WebP spec:
  - Lossless (`VP8L`): all four transforms (predictor, color, subtract green, color indexing), the color cache, back references, and meta prefix codes.
  - Lossy (`VP8 `): key frames with segments, both loop filters, and all intra prediction modes. Chroma is upsampled bilinearly and converted with BT.601, so RGB values can be one step off from other decoders.
  - Alpha on lossy images from an `ALPH` chunk, raw or lossless, with any of its three filters.
  - The simple layout and the extended one with a `VP8X` header. ICC, EXIF, and XMP chunks are skipped, so no color profile is applied.
  - Animated files (`ANIM`, `ANMF`) are rejected with an error. So are VP8 interframes, which a still image never has.
  - Legacy files with a `VP8X` header and a zlib `RGBA` chunk still load.
- ICO picks one image from the directory. BMP is uncompressed 24-bit or 32-bit. PPM/PGM is the binary Netpbm form.
- `encode_png`, `encode_jpeg`, `encode_gif`, `encode_webp`, `encode_ico`, `encode_bmp`, and `encode_ppm` write those formats back. `encode_webp` writes a standard simple lossless `RIFF`/`WEBP`/`VP8L` container (subtract-green transform, 6-bit color cache, and LZ77 backward references).
- `resize_bilinear` scales an image.

## Limits

- A side longer than 8192 pixels is rejected.
- More than 16,777,216 pixels is rejected.
- A lossless WebP file whose prefix code tables would pass 32 MiB is rejected.

## Build

```
cargo test
```

Rust 2024 edition, Rust 1.99.1 or newer. The only dependency is `anvil-compress`, by path.

## The other repositories

The browser is [anvil-browser](https://github.com/buntatoes/anvil-browser) and the page engine is [anvil-engine](https://github.com/buntatoes/anvil-engine), built alongside the nine companion libraries below. None of them come from crates.io.

| Repository | What it is |
| --- | --- |
| [anvil-cipher](https://github.com/buntatoes/anvil-cipher) | AES-128, AES-256, CTR, AES-256-GCM, SHA-256, HMAC-SHA256, HKDF-SHA256 |
| [anvil-compress](https://github.com/buntatoes/anvil-compress) | DEFLATE, zlib, gzip |
| [anvil-hpack](https://github.com/buntatoes/anvil-hpack) | HPACK header compression for HTTP/2 |
| [anvil-image](https://github.com/buntatoes/anvil-image) | PNG, JPEG, GIF, WebP (`VP8L` + `VP8 `), ICO, BMP, PPM |
| [anvil-json](https://github.com/buntatoes/anvil-json) | JSON |
| [anvil-pqc](https://github.com/buntatoes/anvil-pqc) | ML-KEM-768, X25519, X25519MLKEM768, SHA-3 |
| [anvil-ttf](https://github.com/buntatoes/anvil-ttf) | TrueType and OpenType (`glyf`, `CFF `, `kern`, `GPOS`) |
| [anvil-gpu](https://github.com/buntatoes/anvil-gpu) | Compositor passes for boxes, shadows, gradients, and NV12/I420, on the CPU or a GPU |
| [anvil-video](https://github.com/buntatoes/anvil-video) | CPU frames (AVID/ADCT) and PCM/ADPCM/FLAC/MDCT audio in MP4, fMP4, and WebM containers |

## License

Dual-licensed under the **PolyForm Noncommercial License 1.0.0** and a **Paid Commercial License with a Non-Compete Clause**. Copyright (c) 2026 Buntato. See [LICENSE](./LICENSE).
