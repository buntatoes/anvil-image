// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

// Security caps on image decoding.
const (
	// MaxImageDimension is the largest width or height a decoder will accept, in pixels.
	MaxImageDimension uint32 = 8192

	// MaxImagePixels is the largest width * height a decoder will accept.
	// 16,777,216 pixels is 64 MiB of 8-bit RGBA.
	MaxImagePixels uint64 = 16777216
)

// ValidateDimensions rejects a zero size, an axis above MaxImageDimension, or too many pixels.
func ValidateDimensions(width, height uint32) error {
	if width == 0 || height == 0 {
		return newInvalidHeader("zero-sized image")
	}
	totalPixels := uint64(width) * uint64(height)
	if width > MaxImageDimension || height > MaxImageDimension || totalPixels > MaxImagePixels {
		return newDimensionsTooLarge(width, height)
	}
	return nil
}
