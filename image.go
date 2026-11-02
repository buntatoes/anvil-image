// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

// Image is a top-down 8-bit RGBA image buffer.
// One pixel is four bytes: R, G, B, then A, stored row-major. Row 0 is the top row.
type Image struct {
	Width  uint32
	Height uint32
	Pixels []byte
}

// New allocates a transparent black RGBA buffer of width by height.
func New(width, height uint32) *Image {
	total := uint64(width) * uint64(height) * 4
	if total > 0xFFFFFFFF {
		total = 0xFFFFFFFF
	}
	return &Image{
		Width:  width,
		Height: height,
		Pixels: make([]byte, total),
	}
}

// FromRGBA builds an Image from an existing tightly packed RGBA slice.
// pixels must be exactly width * height * 4 bytes.
func FromRGBA(width, height uint32, pixels []byte) (*Image, error) {
	if err := ValidateDimensions(width, height); err != nil {
		return nil, err
	}
	expected := uint64(width) * uint64(height) * 4
	if uint64(len(pixels)) != expected {
		return nil, newInvalidHeader("RGBA pixel buffer size mismatch")
	}
	return &Image{
		Width:  width,
		Height: height,
		Pixels: pixels,
	}, nil
}

// Pixel returns the RGBA sample at column x, row y, or ok=false if outside bounds.
func (img *Image) Pixel(x, y uint32) (r, g, b, a uint8, ok bool) {
	if img == nil || x >= img.Width || y >= img.Height {
		return 0, 0, 0, 0, false
	}
	idx := (uint64(y)*uint64(img.Width) + uint64(x)) * 4
	if idx+3 >= uint64(len(img.Pixels)) {
		return 0, 0, 0, 0, false
	}
	return img.Pixels[idx], img.Pixels[idx+1], img.Pixels[idx+2], img.Pixels[idx+3], true
}

// SetPixel writes r, g, b, a at column x, row y. Out-of-bounds coordinates are ignored.
func (img *Image) SetPixel(x, y uint32, r, g, b, a uint8) {
	if img == nil || x >= img.Width || y >= img.Height {
		return
	}
	idx := (uint64(y)*uint64(img.Width) + uint64(x)) * 4
	if idx+3 < uint64(len(img.Pixels)) {
		img.Pixels[idx] = r
		img.Pixels[idx+1] = g
		img.Pixels[idx+2] = b
		img.Pixels[idx+3] = a
	}
}

// Clone returns a deep copy of the image.
func (img *Image) Clone() *Image {
	if img == nil {
		return nil
	}
	p := make([]byte, len(img.Pixels))
	copy(p, img.Pixels)
	return &Image{
		Width:  img.Width,
		Height: img.Height,
		Pixels: p,
	}
}
