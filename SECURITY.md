# Security

Report problems in private:

https://github.com/buntatoes/anvil-image/security/advisories/new

Do not put the details in a public issue.

## What is checked

- Width and height must be non-zero, at most 8,192 pixels on each side, and at most 16,777,216 total pixels.
- PNG chunk checksums are verified via 32-bit CRC. Bad chunk checksums are rejected.
- PNG and WebP compressed streams are bounded and decompressed via companion `anvil-compress`, stopping immediately if an inflated payload exceeds bounds.
- Lossless WebP prefix code table entries are capped at 8M entries to prevent excessive memory allocation.
- Corrupted Huffman codes, out-of-range DC/AC categories, and truncated streams return structured errors and never panic.

Version 0.9.0 is current.
