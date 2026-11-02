// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"bytes"
	"math"
	"testing"
)

func makeTestImage(w, h uint32) *Image {
	img := New(w, h)
	for y := uint32(0); y < h; y++ {
		for x := uint32(0); x < w; x++ {
			r := uint8((x * 255) / (w | 1))
			g := uint8((y * 255) / (h | 1))
			b := uint8(((x + y) * 128) / ((w + h) | 1))
			a := uint8(255 - ((y * 128) / (h | 1)))
			img.SetPixel(x, y, r, g, b, a)
		}
	}
	return img
}

func TestPNGCodec(t *testing.T) {
	orig := makeTestImage(16, 16)
	encoded := EncodePNG(orig)
	if len(encoded) == 0 {
		t.Fatal("EncodePNG produced empty output")
	}

	decoded, err := DecodePNG(encoded)
	if err != nil {
		t.Fatalf("DecodePNG failed: %v", err)
	}

	if decoded.Width != orig.Width || decoded.Height != orig.Height {
		t.Fatalf("Dimension mismatch: got %dx%d, want %dx%d", decoded.Width, decoded.Height, orig.Width, orig.Height)
	}

	if !bytes.Equal(decoded.Pixels, orig.Pixels) {
		t.Fatal("Decoded PNG pixels do not match original")
	}
}

func TestBMPCodec(t *testing.T) {
	orig := makeTestImage(12, 10)
	encoded := EncodeBMP(orig)
	if len(encoded) == 0 {
		t.Fatal("EncodeBMP produced empty output")
	}

	decoded, err := DecodeBMP(encoded)
	if err != nil {
		t.Fatalf("DecodeBMP failed: %v", err)
	}

	if decoded.Width != orig.Width || decoded.Height != orig.Height {
		t.Fatalf("Dimension mismatch: got %dx%d, want %dx%d", decoded.Width, decoded.Height, orig.Width, orig.Height)
	}

	if !bytes.Equal(decoded.Pixels, orig.Pixels) {
		t.Fatal("Decoded BMP pixels do not match original")
	}
}

func TestPPMCodec(t *testing.T) {
	orig := makeTestImage(8, 8)
	encoded := EncodePPM(orig)
	if len(encoded) == 0 {
		t.Fatal("EncodePPM produced empty output")
	}

	decoded, err := DecodePPM(encoded)
	if err != nil {
		t.Fatalf("DecodePPM failed: %v", err)
	}

	if decoded.Width != orig.Width || decoded.Height != orig.Height {
		t.Fatalf("Dimension mismatch: got %dx%d, want %dx%d", decoded.Width, decoded.Height, orig.Width, orig.Height)
	}

	// PPM P6 encodes RGB only, alpha defaults to 255
	for i := 0; i < len(orig.Pixels); i += 4 {
		if decoded.Pixels[i] != orig.Pixels[i] ||
			decoded.Pixels[i+1] != orig.Pixels[i+1] ||
			decoded.Pixels[i+2] != orig.Pixels[i+2] ||
			decoded.Pixels[i+3] != 255 {
			t.Fatalf("Pixel mismatch at %d: got (%d,%d,%d,%d), want RGB=(%d,%d,%d)",
				i/4, decoded.Pixels[i], decoded.Pixels[i+1], decoded.Pixels[i+2], decoded.Pixels[i+3],
				orig.Pixels[i], orig.Pixels[i+1], orig.Pixels[i+2])
		}
	}
}

func TestGIFCodec(t *testing.T) {
	orig := makeTestImage(16, 16)
	encoded := EncodeGIF(orig)
	if len(encoded) == 0 {
		t.Fatal("EncodeGIF produced empty output")
	}

	decoded, err := DecodeGIF(encoded)
	if err != nil {
		t.Fatalf("DecodeGIF failed: %v", err)
	}

	if decoded.Width != orig.Width || decoded.Height != orig.Height {
		t.Fatalf("Dimension mismatch: got %dx%d, want %dx%d", decoded.Width, decoded.Height, orig.Width, orig.Height)
	}
}

func TestJPEGCodec(t *testing.T) {
	orig := makeTestImage(16, 16)
	encoded := EncodeJPEG(orig)
	if len(encoded) == 0 {
		t.Fatal("EncodeJPEG produced empty output")
	}

	decoded, err := DecodeJPEG(encoded)
	if err != nil {
		t.Fatalf("DecodeJPEG failed: %v", err)
	}

	if decoded.Width != orig.Width || decoded.Height != orig.Height {
		t.Fatalf("Dimension mismatch: got %dx%d, want %dx%d", decoded.Width, decoded.Height, orig.Width, orig.Height)
	}

	// Verify reasonable peak error bounds on lossy DCT reconstruction
	var maxDiff float64
	for i := 0; i < len(orig.Pixels); i += 4 {
		for c := 0; c < 3; c++ {
			diff := math.Abs(float64(decoded.Pixels[i+c]) - float64(orig.Pixels[i+c]))
			if diff > maxDiff {
				maxDiff = diff
			}
		}
	}
	if maxDiff > 40.0 {
		t.Fatalf("JPEG lossy error too large: maxDiff = %f", maxDiff)
	}
}

func TestWebPCodec(t *testing.T) {
	orig := makeTestImage(16, 16)
	encoded := EncodeWebP(orig)
	if len(encoded) == 0 {
		t.Fatal("EncodeWebP produced empty output")
	}

	decoded, err := DecodeWebP(encoded)
	if err != nil {
		t.Fatalf("DecodeWebP failed: %v", err)
	}

	if decoded.Width != orig.Width || decoded.Height != orig.Height {
		t.Fatalf("Dimension mismatch: got %dx%d, want %dx%d", decoded.Width, decoded.Height, orig.Width, orig.Height)
	}
}

func TestICOCodec(t *testing.T) {
	orig := makeTestImage(16, 16)
	encoded := EncodeICO(orig)
	if len(encoded) == 0 {
		t.Fatal("EncodeICO produced empty output")
	}

	decoded, err := DecodeICO(encoded)
	if err != nil {
		t.Fatalf("DecodeICO failed: %v", err)
	}

	if decoded.Width != orig.Width || decoded.Height != orig.Height {
		t.Fatalf("Dimension mismatch: got %dx%d, want %dx%d", decoded.Width, decoded.Height, orig.Width, orig.Height)
	}

	if !bytes.Equal(decoded.Pixels, orig.Pixels) {
		t.Fatal("Decoded ICO pixels do not match original")
	}
}

func TestDecodeAutoAllFormats(t *testing.T) {
	orig := makeTestImage(8, 8)

	formats := []struct {
		name string
		data []byte
	}{
		{"PNG", EncodePNG(orig)},
		{"BMP", EncodeBMP(orig)},
		{"PPM", EncodePPM(orig)},
		{"GIF", EncodeGIF(orig)},
		{"JPEG", EncodeJPEG(orig)},
		{"WebP", EncodeWebP(orig)},
		{"ICO", EncodeICO(orig)},
	}

	for _, f := range formats {
		t.Run(f.name, func(t *testing.T) {
			dec, err := DecodeAuto(f.data)
			if err != nil {
				t.Fatalf("DecodeAuto failed on %s: %v", f.name, err)
			}
			if dec.Width != 8 || dec.Height != 8 {
				t.Fatalf("%s: expected 8x8, got %dx%d", f.name, dec.Width, dec.Height)
			}
		})
	}
}
