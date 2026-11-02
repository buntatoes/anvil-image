<div align="center">
  <h1>Anvil Image</h1>
  <p>Clean-room PNG, JPEG, GIF, WebP, ICO, BMP, and PPM image codecs and resampling in 100% pure Go.</p>
  <p>
    <img src="https://img.shields.io/badge/release-v.09-e8c37a?style=flat-square" alt="v.09" />
    <img src="https://img.shields.io/badge/status-Proof_of_Concept-d4a359?style=flat-square" alt="Proof of Concept" />
    <img src="https://img.shields.io/badge/archive-Frozen_in_Time-8b949e?style=flat-square" alt="Archived and Frozen in Time" />
    <a href="./LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square" alt="Apache-2.0" /></a>
    <a href="./SECURITY.md"><img src="https://img.shields.io/badge/security-policy-2ea44f?style=flat-square" alt="Security" /></a>
    <img src="https://img.shields.io/badge/go-1.22%2B-00ADD8?style=flat-square" alt="Go 1.22+" />
  </p>
</div>

> [!IMPORTANT]
> **Archived & Frozen in Time**: This standalone repository is archived and frozen in time as a historical snapshot. Active development now lives in-tree inside [buntatoes/anvil-browser](https://github.com/buntatoes/anvil-browser) under `engines/go/anvil-image`.

Version 0.9.0 (`v.09`).

It imports companion pure-Go [anvil-compress](https://github.com/buntatoes/anvil-compress) in the sibling directory for DEFLATE and CRC-32 operations. Zero external third-party Go dependencies.

## What it does

- `DecodeAuto` inspects format signatures and automatically dispatches to the correct codec: PNG, baseline JPEG, GIF, WebP, ICO/CUR, BMP, or PPM/PGM.
- **PNG**: Clean-room RFC 2083 decoder and encoder. Reconstructs scanline filters 0..4 (None, Sub, Up, Average, Paeth) and unpacks 8-bit Gray, RGB, Indexed (PLTE/tRNS), Gray+Alpha, and RGBA into top-down 8-bit RGBA. Verifies 32-bit CRC chunk checksums against companion `anvil-compress`.
- **JPEG**: Sequential baseline 8-bit DCT (`SOF0`) decoder and encoder. Parses `DQT`, `DHT`, `DRI`, `SOS`, reorders 8x8 blocks from zigzag order, runs 2D separable floating-point IDCT-II, and performs full-range JFIF YCbCr to RGBA conversion.
- **GIF**: Decodes GIF87a and GIF89a streams with variable-bitlength LZW, local and global color tables, graphic control extension transparency, and 4-pass interlacing. Encodes 256-color palette GIF89a.
- **WebP**:
  - Lossless (`VP8L`): Canonical Huffman entropy decoder and bitwriter, 4 spatial transforms (predictor, cross-color, subtract green, color indexing), color cache, and LZ77 backward distance references.
  - Lossy (`VP8 `): Boolean arithmetic entropy decoder, uncompressed keyframe header parsing, macroblock reconstruction, intra prediction modes, 4x4 WHT/IDCT, loop filtering, and BT.601 YUV to RGBA conversion.
  - Container (`RIFF`/`WEBP`): Parses `VP8X`, `VP8L`, `VP8 `, `ALPH`, and `RGBA` chunks; rejects animated streams.
- **BMP**: Decodes 1, 4, 8, 24, and 32 bpp uncompressed `BI_RGB` and `BI_BITFIELDS` bitmaps, supporting top-down and bottom-up row orders. Encodes 32-bit top-down `BI_RGB`.
- **ICO**: Decodes Windows Icon (type 1) and Cursor (type 2) directories, extracting embedded PNG or DIB bitmaps. Encodes 1-entry ICO containers.
- **PPM/PGM**: Decodes Netpbm binary P6 (RGB) and P5 (grayscale) streams with comment stripping. Encodes binary P6 PPM.
- **Resampling**: `ResizeBilinear` scales images with multi-stage premultiplied-alpha 2x2 box downsampling (`MipmapHalve`) when downscaling by more than 2x, followed by premultiplied-alpha bilinear interpolation to eliminate dark fringe artifacts.
- **CLI & Worker**: `cmd/anvil-image` provides an IPC worker communicating over stdin/stdout with 1-byte opcodes (`OP_DECODE_AUTO`, `OP_RESIZE_BILINEAR`, `OP_ENCODE_PNG`, `OP_ENCODE_BMP`, `OP_SHUTDOWN`), `--version`, and `--self-test`.

## Limits

- Width and height must be non-zero and at most 8,192 pixels on each side (`MaxImageDimension`).
- Total pixel count is strictly capped at 16,777,216 pixels (64 MiB of 32-bit RGBA, `MaxImagePixels`).
- Lossless WebP prefix code tables are bounded at 8M table entries.

## Build & Test

```
go vet ./...
go test -v ./...
go run ./cmd/anvil-image --self-test
```

Go 1.22 or newer (`CGO_ENABLED=0`). Zero external Go module dependencies.

## The other repositories

The browser is [anvil-browser](https://github.com/buntatoes/anvil-browser) and the page engine is [anvil-forge](https://github.com/buntatoes/anvil-forge), built alongside the nine companion libraries below.

| Repository | What it is |
| --- | --- |
| [anvil-cipher](https://github.com/buntatoes/anvil-cipher) | AES-128, AES-256, CTR, AES-256-GCM, SHA-256, HMAC-SHA256, HKDF-SHA256 |
| [anvil-compress](https://github.com/buntatoes/anvil-compress) | DEFLATE, zlib, gzip (Go) |
| [anvil-hpack](https://github.com/buntatoes/anvil-hpack) | HPACK header compression for HTTP/2 (Go) |
| [anvil-image](https://github.com/buntatoes/anvil-image) | PNG, JPEG, GIF, WebP (`VP8L` + `VP8 `), ICO, BMP, PPM (Go) |
| [anvil-json](https://github.com/buntatoes/anvil-json) | JSON & RFC 6901 JSON Pointer (Go) |
| [anvil-pqc](https://github.com/buntatoes/anvil-pqc) | ML-KEM-768, X25519, X25519MLKEM768, SHA-3 |
| [anvil-ttf](https://github.com/buntatoes/anvil-ttf) | TrueType and OpenType (`glyf`, `CFF `, `kern`, `GPOS`) (Go) |
| [anvil-gpu](https://github.com/buntatoes/anvil-gpu) | Compositor passes for boxes, shadows, gradients, and NV12/I420, on the CPU or a GPU |
| [anvil-video](https://github.com/buntatoes/anvil-video) | CPU frames (AVID/ADCT) and PCM/ADPCM/FLAC/MDCT audio in MP4, fMP4, and WebM containers |

## License

Licensed under the **Apache License, Version 2.0** (`Apache-2.0`). Copyright (c) 2026 Buntato. Pursuant to Section 6 of the Apache License 2.0, the names **"Anvil"**, **"Anvil Browser"**, **"Anvil Forge"** (**"Forge"**), **"Cinder"**, **"Prism"**, and the Anvil brand logo/emblem are copyrighted and trademarked by **Buntato** (*All Rights Reserved*) and may not be used to brand derivative works without written permission. See [LICENSE](./LICENSE).
