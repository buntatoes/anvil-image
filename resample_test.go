// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"testing"
)

func TestMipmapHalve(t *testing.T) {
	img := New(4, 4)
	for y := uint32(0); y < 4; y++ {
		for x := uint32(0); x < 4; x++ {
			img.SetPixel(x, y, 100, 150, 200, 255)
		}
	}

	half := img.MipmapHalve(true, true)
	if half.Width != 2 || half.Height != 2 {
		t.Fatalf("expected 2x2, got %dx%d", half.Width, half.Height)
	}
	r, g, b, a, ok := half.Pixel(0, 0)
	if !ok || r != 100 || g != 150 || b != 200 || a != 255 {
		t.Fatalf("unexpected pixel value: %d %d %d %d", r, g, b, a)
	}

	// Halve single axis
	halfX := img.MipmapHalve(true, false)
	if halfX.Width != 2 || halfX.Height != 4 {
		t.Fatalf("expected 2x4, got %dx%d", halfX.Width, halfX.Height)
	}

	// Odd dimensions
	odd := New(5, 7)
	oddHalf := odd.MipmapHalve(true, true)
	if oddHalf.Width != 3 || oddHalf.Height != 4 {
		t.Fatalf("expected 3x4, got %dx%d", oddHalf.Width, oddHalf.Height)
	}

	// 1x1 image
	single := New(1, 1)
	singleHalf := single.MipmapHalve(true, true)
	if singleHalf.Width != 1 || singleHalf.Height != 1 {
		t.Fatalf("expected 1x1, got %dx%d", singleHalf.Width, singleHalf.Height)
	}
}

func TestPremultipliedAlphaWeighting(t *testing.T) {
	// 2x2 box where pixel (0,0) is fully opaque red (255, 0, 0, 255)
	// and other 3 pixels are completely transparent black (0, 0, 0, 0).
	// In premultiplied alpha, the downsampled pixel should have red=255 and alpha=64.
	// Non-premultiplied naive averaging would produce red=64 and alpha=64 (dark fringe artifact).
	img := New(2, 2)
	img.SetPixel(0, 0, 255, 0, 0, 255)
	img.SetPixel(1, 0, 0, 0, 0, 0)
	img.SetPixel(0, 1, 0, 0, 0, 0)
	img.SetPixel(1, 1, 0, 0, 0, 0)

	half := img.MipmapHalve(true, true)
	if half.Width != 1 || half.Height != 1 {
		t.Fatalf("expected 1x1, got %dx%d", half.Width, half.Height)
	}

	r, g, b, a, _ := half.Pixel(0, 0)
	if r != 255 {
		t.Fatalf("premultiplied alpha failed: expected full red (255) without dark fringe, got %d", r)
	}
	if a < 63 || a > 65 {
		t.Fatalf("expected alpha ~64, got %d", a)
	}
	if g != 0 || b != 0 {
		t.Fatalf("expected green=0 and blue=0, got g=%d b=%d", g, b)
	}
}

func TestResizeBilinear(t *testing.T) {
	img := New(4, 4)
	for y := uint32(0); y < 4; y++ {
		for x := uint32(0); x < 4; x++ {
			img.SetPixel(x, y, 200, 100, 50, 255)
		}
	}

	// Identity resize
	same, err := ResizeBilinear(img, 4, 4)
	if err != nil {
		t.Fatalf("ResizeBilinear identity failed: %v", err)
	}
	if same.Width != 4 || same.Height != 4 {
		t.Fatalf("expected 4x4, got %dx%d", same.Width, same.Height)
	}
	r, g, b, a, _ := same.Pixel(2, 2)
	if r != 200 || g != 100 || b != 50 || a != 255 {
		t.Fatalf("identity pixel mismatch: %d %d %d %d", r, g, b, a)
	}

	// Upscale 4x4 -> 8x8
	up, err := ResizeBilinear(img, 8, 8)
	if err != nil {
		t.Fatalf("ResizeBilinear upscale failed: %v", err)
	}
	if up.Width != 8 || up.Height != 8 {
		t.Fatalf("expected 8x8, got %dx%d", up.Width, up.Height)
	}
	r, g, b, a, _ = up.Pixel(4, 4)
	if r != 200 || g != 100 || b != 50 || a != 255 {
		t.Fatalf("upscale uniform pixel mismatch: %d %d %d %d", r, g, b, a)
	}

	// Downscale with multiple mipmap stages: 64x64 -> 8x8
	big := New(64, 64)
	for y := uint32(0); y < 64; y++ {
		for x := uint32(0); x < 64; x++ {
			big.SetPixel(x, y, 120, 180, 240, 255)
		}
	}

	down, err := ResizeBilinear(big, 8, 8)
	if err != nil {
		t.Fatalf("ResizeBilinear downscale failed: %v", err)
	}
	if down.Width != 8 || down.Height != 8 {
		t.Fatalf("expected 8x8, got %dx%d", down.Width, down.Height)
	}
	r, g, b, a, _ = down.Pixel(4, 4)
	if r != 120 || g != 180 || b != 240 || a != 255 {
		t.Fatalf("downscale uniform pixel mismatch: %d %d %d %d", r, g, b, a)
	}
}
