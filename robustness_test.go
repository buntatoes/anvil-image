// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"math/rand"
	"testing"
)

func TestTruncatedStreams(t *testing.T) {
	orig := makeTestImage(8, 8)

	payloads := map[string][]byte{
		"PNG":  EncodePNG(orig),
		"BMP":  EncodeBMP(orig),
		"PPM":  EncodePPM(orig),
		"GIF":  EncodeGIF(orig),
		"JPEG": EncodeJPEG(orig),
		"WebP": EncodeWebP(orig),
		"ICO":  EncodeICO(orig),
	}

	for name, data := range payloads {
		t.Run(name, func(t *testing.T) {
			// Test truncating at every single byte up to length
			step := len(data) / 10
			if step < 1 {
				step = 1
			}
			for cut := 0; cut < len(data); cut += step {
				truncated := data[:cut]
				// Must not panic!
				_, _ = DecodeAuto(truncated)

				switch name {
				case "PNG":
					_, _ = DecodePNG(truncated)
				case "BMP":
					_, _ = DecodeBMP(truncated)
				case "PPM":
					_, _ = DecodePPM(truncated)
				case "GIF":
					_, _ = DecodeGIF(truncated)
				case "JPEG":
					_, _ = DecodeJPEG(truncated)
				case "WebP":
					_, _ = DecodeWebP(truncated)
				case "ICO":
					_, _ = DecodeICO(truncated)
				}
			}
		})
	}
}

func TestCorruptAndFuzzedInputs(t *testing.T) {
	rng := rand.New(rand.NewSource(42))

	// 1. Completely random byte buffers
	for i := 0; i < 200; i++ {
		length := rng.Intn(1024)
		buf := make([]byte, length)
		rng.Read(buf)

		// Must not panic
		_, _ = DecodeAuto(buf)
		_, _ = DecodePNG(buf)
		_, _ = DecodeBMP(buf)
		_, _ = DecodePPM(buf)
		_, _ = DecodeGIF(buf)
		_, _ = DecodeJPEG(buf)
		_, _ = DecodeWebP(buf)
		_, _ = DecodeICO(buf)
	}

	// 2. Bit flipping / mutation fuzzing on valid images
	orig := makeTestImage(8, 8)
	base := EncodePNG(orig)

	for i := 0; i < 100; i++ {
		mutated := make([]byte, len(base))
		copy(mutated, base)

		// Flip random bytes
		numFlips := 1 + rng.Intn(10)
		for f := 0; f < numFlips; f++ {
			idx := rng.Intn(len(mutated))
			mutated[idx] ^= byte(rng.Intn(256))
		}

		// Must not panic
		_, _ = DecodeAuto(mutated)
		_, _ = DecodePNG(mutated)
	}
}

func TestSecurityDimensionLimits(t *testing.T) {
	// Zero dimensions
	if err := ValidateDimensions(0, 100); err == nil {
		t.Fatal("expected error on width 0")
	}
	if err := ValidateDimensions(100, 0); err == nil {
		t.Fatal("expected error on height 0")
	}

	// Above max dimension (8192)
	if err := ValidateDimensions(8193, 100); err == nil {
		t.Fatal("expected error on width > 8192")
	}
	if err := ValidateDimensions(100, 8193); err == nil {
		t.Fatal("expected error on height > 8192")
	}

	// Total pixels exceeding 16,777,216
	// 8192 * 8192 = 67,108,864 > 16,777,216
	if err := ValidateDimensions(8192, 8192); err == nil {
		t.Fatal("expected error on total pixels > 16,777,216")
	}

	// Exactly at limit: 4096 * 4096 = 16,777,216
	if err := ValidateDimensions(4096, 4096); err != nil {
		t.Fatalf("unexpected error on exact 4096x4096 limit: %v", err)
	}

	// 8192 * 2048 = 16,777,216
	if err := ValidateDimensions(8192, 2048); err != nil {
		t.Fatalf("unexpected error on 8192x2048 limit: %v", err)
	}

	// 8192 * 2049 = 16,785,408 > 16,777,216
	if err := ValidateDimensions(8192, 2049); err == nil {
		t.Fatal("expected error on 8192x2049 (> 16.7M pixels)")
	}
}
