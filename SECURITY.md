# Security

Report problems in private:

https://github.com/buntatoes/anvil-image/security/advisories/new

The browser repository takes the same kind of report:

https://github.com/buntatoes/anvil-browser/security/advisories/new

Do not put the details in a public issue.

## What is checked

- Width and height must be non-zero, at most 8192 on a side, and at most 16,777,216 pixels.
- PNG chunk checksums are checked. A bad checksum is rejected.
- PNG and WebP decompression goes through `anvil-compress`, which stops if the inflated stream is too large.

Version 0.7.1 is current.
