// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"fmt"
	"strconv"
)

// DecodePPM decodes a binary P6 (RGB) or P5 (grayscale) Netpbm pixmap into opaque RGBA.
func DecodePPM(data []byte) (*Image, error) {
	if len(data) < 7 {
		return nil, newUnexpectedEof()
	}
	magic := string(data[0:2])
	if magic != "P6" && magic != "P5" {
		return nil, newInvalidHeader("expected P6 or P5 header")
	}

	pos := 2
	width, err := readPPMToken(data, &pos)
	if err != nil {
		return nil, err
	}
	height, err := readPPMToken(data, &pos)
	if err != nil {
		return nil, err
	}
	maxval, err := readPPMToken(data, &pos)
	if err != nil {
		return nil, err
	}

	if width == 0 || height == 0 || maxval == 0 || maxval > 255 {
		return nil, newInvalidHeader("invalid PPM header values")
	}
	if err := ValidateDimensions(width, height); err != nil {
		return nil, err
	}

	if pos >= len(data) {
		return nil, newUnexpectedEof()
	}
	pos++ // single whitespace delimiter before binary raster

	out := New(width, height)
	if magic == "P6" {
		needed := int(width) * int(height) * 3
		if pos+needed > len(data) {
			return nil, newUnexpectedEof()
		}
		for y := uint32(0); y < height; y++ {
			for x := uint32(0); x < width; x++ {
				idx := pos + int(y*width+x)*3
				out.SetPixel(x, y, data[idx], data[idx+1], data[idx+2], 255)
			}
		}
	} else {
		needed := int(width) * int(height)
		if pos+needed > len(data) {
			return nil, newUnexpectedEof()
		}
		for y := uint32(0); y < height; y++ {
			for x := uint32(0); x < width; x++ {
				g := data[pos+int(y*width+x)]
				out.SetPixel(x, y, g, g, g, 255)
			}
		}
	}

	return out, nil
}

func readPPMToken(data []byte, pos *int) (uint32, error) {
	for *pos < len(data) {
		b := data[*pos]
		if b == '#' {
			for *pos < len(data) && data[*pos] != '\n' {
				*pos++
			}
		} else if isWhitespace(b) {
			*pos++
		} else {
			break
		}
	}
	start := *pos
	for *pos < len(data) && data[*pos] >= '0' && data[*pos] <= '9' {
		*pos++
	}
	if start == *pos {
		return 0, newInvalidHeader("expected numeric token in PPM header")
	}
	val, err := strconv.ParseUint(string(data[start:*pos]), 10, 32)
	if err != nil {
		return 0, newInvalidHeader("invalid number in PPM header")
	}
	return uint32(val), nil
}

func isWhitespace(b byte) bool {
	return b == ' ' || b == '\t' || b == '\r' || b == '\n'
}

// EncodePPM encodes image as a binary P6 PPM with maxval 255, dropping alpha.
func EncodePPM(image *Image) []byte {
	if image == nil {
		return nil
	}
	header := fmt.Sprintf("P6\n%d %d\n255\n", image.Width, image.Height)
	out := make([]byte, 0, len(header)+int(image.Width)*int(image.Height)*3)
	out = append(out, header...)
	for i := 0; i+3 < len(image.Pixels); i += 4 {
		out = append(out, image.Pixels[i], image.Pixels[i+1], image.Pixels[i+2])
	}
	return out
}
