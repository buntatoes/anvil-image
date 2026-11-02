// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"bytes"
)

// DecodeAuto detects a still image format from data by sniffing signatures and decodes it to RGBA.
// Signature detection order:
//  1. PNG (\x89PNG\r\n\x1a\n)
//  2. JPEG (FF D8)
//  3. GIF (GIF87a or GIF89a)
//  4. WebP (RIFF....WEBP)
//  5. ICO / CUR (00 00 01 00 or 00 00 02 00)
//  6. BMP (BM)
//  7. PPM / PGM (P6 or P5)
func DecodeAuto(data []byte) (*Image, error) {
	if len(data) >= 8 && bytes.Equal(data[:8], pngSignature) {
		return DecodePNG(data)
	}
	if len(data) >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
		return DecodeJPEG(data)
	}
	if len(data) >= 6 && (string(data[:6]) == "GIF87a" || string(data[:6]) == "GIF89a") {
		return DecodeGIF(data)
	}
	if len(data) >= 12 && string(data[0:4]) == "RIFF" && string(data[8:12]) == "WEBP" {
		return DecodeWebP(data)
	}
	if len(data) >= 4 && ((data[0] == 0 && data[1] == 0 && data[2] == 1 && data[3] == 0) ||
		(data[0] == 0 && data[1] == 0 && data[2] == 2 && data[3] == 0)) {
		return DecodeICO(data)
	}
	if len(data) >= 2 && string(data[:2]) == "BM" {
		return DecodeBMP(data)
	}
	if len(data) >= 2 && (string(data[:2]) == "P6" || string(data[:2]) == "P5") {
		return DecodePPM(data)
	}
	return nil, newUnsupportedFormat("unrecognized image signature")
}
