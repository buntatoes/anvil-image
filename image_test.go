// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"testing"
)

func TestImageBasics(t *testing.T) {
	img := New(4, 4)
	if img.Width != 4 || img.Height != 4 {
		t.Fatalf("unexpected dimensions: %dx%d", img.Width, img.Height)
	}
	if len(img.Pixels) != 4*4*4 {
		t.Fatalf("unexpected pixel buffer size: %d", len(img.Pixels))
	}

	img.SetPixel(2, 3, 255, 128, 64, 200)
	r, g, b, a, ok := img.Pixel(2, 3)
	if !ok || r != 255 || g != 128 || b != 64 || a != 200 {
		t.Fatalf("Pixel readback mismatch: %v (%d,%d,%d,%d)", ok, r, g, b, a)
	}

	// Out of bounds check
	_, _, _, _, ok = img.Pixel(4, 4)
	if ok {
		t.Fatalf("Pixel out-of-bounds returned ok=true")
	}

	img.SetPixel(10, 10, 1, 2, 3, 4) // should not panic

	clone := img.Clone()
	if clone.Width != img.Width || clone.Height != img.Height || len(clone.Pixels) != len(img.Pixels) {
		t.Fatalf("Clone mismatch")
	}
	clone.SetPixel(2, 3, 0, 0, 0, 0)
	r2, _, _, _, _ := img.Pixel(2, 3)
	if r2 != 255 {
		t.Fatalf("Clone did not perform deep copy of pixel data")
	}

	// FromRGBA
	raw := make([]byte, 2*2*4)
	raw[0] = 42
	raw[3] = 255
	fromRaw, err := FromRGBA(2, 2, raw)
	if err != nil {
		t.Fatalf("FromRGBA failed: %v", err)
	}
	r, _, _, a, ok = fromRaw.Pixel(0, 0)
	if !ok || r != 42 || a != 255 {
		t.Fatalf("FromRGBA pixel mismatch: %d %d", r, a)
	}

	// Invalid size
	_, err = FromRGBA(2, 2, raw[:len(raw)-1])
	if err == nil {
		t.Fatalf("expected error on mismatched FromRGBA length")
	}

	// Zero dimensions validation
	if _, err := FromRGBA(0, 0, nil); err == nil {
		t.Fatalf("expected error on FromRGBA(0, 0, nil)")
	}
	if _, err := FromRGBA(0, 10, nil); err == nil {
		t.Fatalf("expected error on FromRGBA(0, 10, nil)")
	}
	if _, err := FromRGBA(10, 0, nil); err == nil {
		t.Fatalf("expected error on FromRGBA(10, 0, nil)")
	}
	if _, err := FromRGBA(8193, 1, make([]byte, 8193*4)); err == nil {
		t.Fatalf("expected error on FromRGBA with width > MaxImageDimension")
	}
}

func TestLimits(t *testing.T) {
	if err := ValidateDimensions(0, 10); err == nil {
		t.Fatalf("expected error on 0 width")
	}
	if err := ValidateDimensions(10, 0); err == nil {
		t.Fatalf("expected error on 0 height")
	}
	if err := ValidateDimensions(8193, 10); err == nil {
		t.Fatalf("expected error on width > MaxImageDimension")
	}
	if err := ValidateDimensions(10, 8193); err == nil {
		t.Fatalf("expected error on height > MaxImageDimension")
	}
	if err := ValidateDimensions(8192, 8192); err == nil {
		t.Fatalf("expected error on total pixels > MaxImagePixels")
	}
	if err := ValidateDimensions(4096, 4096); err != nil {
		t.Fatalf("unexpected error on 4096x4096: %v", err)
	}
}
