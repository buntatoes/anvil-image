# Security Policy

## Proof of Concept (PoC) Notice

`flux-image` is a standalone **Proof of Concept (PoC)** raster image decoder, encoder, and bilinear resampler (`PNG`, `JPEG`, `GIF89a`, `WebP`, `ICO`, `BMP`, `PPM/PGM`) authored from scratch by **Buntos** as part of the [**Flux Browser**](https://github.com/buntatoes/flux-browser) engine ecosystem.

---

## Supported Versions

| Version | Supported |
| :--- | :---: |
| `v.04` (Latest) | Yes |
| `< v.04` | No |

---

## Reporting a Vulnerability

Please **do not** report security vulnerabilities through public GitHub issues or pull requests.

- **GitHub Private Vulnerability Reporting (Recommended)**: [`https://github.com/buntatoes/flux-image/security/advisories/new`](https://github.com/buntatoes/flux-image/security/advisories/new)
- **Central Ecosystem Security Portal**: [`https://github.com/buntatoes/flux-browser/security/advisories/new`](https://github.com/buntatoes/flux-browser/security/advisories/new)
- **Author Attribution**: **Buntos** ([@buntatoes](https://github.com/buntatoes) · `Buntos <324143321+buntatoes@users.noreply.github.com>`)

*(Note: `324143321+buntatoes@users.noreply.github.com` is a GitHub privacy `noreply` commit-attribution address and does not accept direct inbound email; please use GitHub Private Vulnerability Reporting above to contact the maintainer privately.)*

---

## Security Controls in `flux-image`

- **Pre-Allocation Dimension & Pixel-Count Guard (`validate_dimensions`)**:
  - Enforces `MAX_IMAGE_DIMENSION = 8192` and `MAX_IMAGE_PIXELS = 16,777,216` (`64 MiB` RGBA8888 ceiling) before allocating any pixel buffer across PNG, JPEG, GIF, WebP, ICO, BMP, and Netpbm formats.
- **Bounded Zlib & LZW Expansion**:
  - Bounds compressed PNG `IDAT` and WebP chunks via `flux-compress::decompress_zlib_bounded` and enforces 12-bit LZW dictionary bounds in GIF decoding.
