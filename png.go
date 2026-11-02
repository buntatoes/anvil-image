// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"bytes"
	"encoding/binary"

	anvilcompress "github.com/buntatoes/anvil-compress"
)

var pngSignature = []byte{137, 80, 78, 71, 13, 10, 26, 10}

// DecodePNG decodes an 8-bit non-interlaced PNG image from bytes into RGBA.
func DecodePNG(data []byte) (*Image, error) {
	if len(data) < 8 || !bytes.Equal(data[:8], pngSignature) {
		return nil, newInvalidHeader("missing PNG signature")
	}

	pos := 8
	var width, height uint32
	var colorType uint8
	sawIHDR := false

	var palette [][3]uint8
	var trns []uint8
	var idat []byte

	for pos+8 <= len(data) {
		chunkLen := int(binary.BigEndian.Uint32(data[pos : pos+4]))
		chunkType := data[pos+4 : pos+8]
		pos += 8

		if pos+chunkLen+4 > len(data) {
			return nil, newUnexpectedEof()
		}

		chunkData := data[pos : pos+chunkLen]
		crcExpected := binary.BigEndian.Uint32(data[pos+chunkLen : pos+chunkLen+4])

		crcInput := make([]byte, 4+chunkLen)
		copy(crcInput[:4], chunkType)
		copy(crcInput[4:], chunkData)
		if anvilcompress.Crc32(crcInput) != crcExpected {
			return nil, newInvalidHeader("bad png chunk checksum")
		}
		pos += chunkLen + 4

		switch string(chunkType) {
		case "IHDR":
			if chunkLen < 13 {
				return nil, newInvalidHeader("short IHDR chunk")
			}
			width = binary.BigEndian.Uint32(chunkData[0:4])
			height = binary.BigEndian.Uint32(chunkData[4:8])
			bitDepth := chunkData[8]
			colorType = chunkData[9]
			compression := chunkData[10]
			filter := chunkData[11]
			interlace := chunkData[12]

			if width == 0 || height == 0 {
				return nil, newInvalidHeader("zero-sized PNG")
			}
			if err := ValidateDimensions(width, height); err != nil {
				return nil, err
			}
			if bitDepth != 8 {
				return nil, newUnsupportedFormat("only 8-bit PNG depth supported")
			}
			if compression != 0 || filter != 0 || interlace != 0 {
				return nil, newUnsupportedFormat("interlaced PNG not supported")
			}
			sawIHDR = true

		case "PLTE":
			palette = make([][3]uint8, 0, chunkLen/3)
			for i := 0; i+2 < chunkLen; i += 3 {
				palette = append(palette, [3]uint8{chunkData[i], chunkData[i+1], chunkData[i+2]})
			}

		case "tRNS":
			trns = append(trns, chunkData...)

		case "IDAT":
			idat = append(idat, chunkData...)

		case "IEND":
			pos = len(data) // stop walking
		}
	}

	if !sawIHDR {
		return nil, newInvalidHeader("missing IHDR chunk")
	}

	var bpp int
	switch colorType {
	case 0:
		bpp = 1 // Grayscale
	case 2:
		bpp = 3 // RGB
	case 3:
		bpp = 1 // Indexed palette
	case 4:
		bpp = 2 // Grayscale + Alpha
	case 6:
		bpp = 4 // RGBA
	default:
		return nil, newUnsupportedFormat("unsupported PNG color type")
	}

	stride := int(width) * bpp
	expectedRaw := int(height) * (1 + stride)

	raw, err := anvilcompress.ZlibDecompressBounded(idat, expectedRaw+4096)
	if err != nil {
		return nil, newCompressionError(err)
	}
	if len(raw) < expectedRaw {
		return nil, newUnexpectedEof()
	}

	prevRow := make([]byte, stride)
	curRow := make([]byte, stride)
	out := New(width, height)

	for y := 0; y < int(height); y++ {
		rowStart := y * (1 + stride)
		filter := raw[rowStart]
		encoded := raw[rowStart+1 : rowStart+1+stride]

		for i := 0; i < stride; i++ {
			x := encoded[i]
			var a byte
			if i >= bpp {
				a = curRow[i-bpp]
			}
			b := prevRow[i]
			var c byte
			if i >= bpp {
				c = prevRow[i-bpp]
			}

			switch filter {
			case 0:
				curRow[i] = x
			case 1:
				curRow[i] = x + a
			case 2:
				curRow[i] = x + b
			case 3:
				avg := byte((uint16(a) + uint16(b)) / 2)
				curRow[i] = x + avg
			case 4:
				curRow[i] = x + paethPredictor(a, b, c)
			default:
				return nil, newInvalidFilter(filter)
			}
		}

		for x := 0; x < int(width); x++ {
			var r, g, b, alpha uint8
			switch colorType {
			case 0:
				val := curRow[x]
				r, g, b, alpha = val, val, val, 255
			case 2:
				base := x * 3
				r, g, b, alpha = curRow[base], curRow[base+1], curRow[base+2], 255
			case 3:
				idx := int(curRow[x])
				if idx < len(palette) {
					rgb := palette[idx]
					r, g, b = rgb[0], rgb[1], rgb[2]
				} else {
					r, g, b = 0, 0, 0
				}
				if idx < len(trns) {
					alpha = trns[idx]
				} else {
					alpha = 255
				}
			case 4:
				base := x * 2
				val := curRow[base]
				r, g, b, alpha = val, val, val, curRow[base+1]
			case 6:
				base := x * 4
				r, g, b, alpha = curRow[base], curRow[base+1], curRow[base+2], curRow[base+3]
			default:
				r, g, b, alpha = 0, 0, 0, 255
			}
			out.SetPixel(uint32(x), uint32(y), r, g, b, alpha)
		}

		copy(prevRow, curRow)
	}

	return out, nil
}

func paethPredictor(a, b, c byte) byte {
	ai := int32(a)
	bi := int32(b)
	ci := int32(c)
	p := ai + bi - ci
	pa := absInt32(p - ai)
	pb := absInt32(p - bi)
	pc := absInt32(p - ci)

	if pa <= pb && pa <= pc {
		return a
	} else if pb <= pc {
		return b
	}
	return c
}

func absInt32(v int32) int32 {
	if v < 0 {
		return -v
	}
	return v
}

// EncodePNG encodes image as a non-interlaced 8-bit RGBA PNG.
func EncodePNG(image *Image) []byte {
	if image == nil {
		return nil
	}
	var out []byte
	out = append(out, pngSignature...)

	ihdr := make([]byte, 13)
	binary.BigEndian.PutUint32(ihdr[0:4], image.Width)
	binary.BigEndian.PutUint32(ihdr[4:8], image.Height)
	ihdr[8] = 8  // bit depth 8
	ihdr[9] = 6  // color type 6 (RGBA)
	ihdr[10] = 0 // compression 0 (zlib)
	ihdr[11] = 0 // filter 0
	ihdr[12] = 0 // no interlace
	out = writePNGChunk(out, "IHDR", ihdr)

	stride := int(image.Width) * 4
	raw := make([]byte, 0, int(image.Height)*(1+stride))
	for y := 0; y < int(image.Height); y++ {
		raw = append(raw, 0) // Filter 0 (None)
		rowStart := y * stride
		raw = append(raw, image.Pixels[rowStart:rowStart+stride]...)
	}

	compressed := anvilcompress.ZlibCompress(raw)
	out = writePNGChunk(out, "IDAT", compressed)
	out = writePNGChunk(out, "IEND", nil)
	return out
}

func writePNGChunk(out []byte, chunkType string, data []byte) []byte {
	var lenBuf [4]byte
	binary.BigEndian.PutUint32(lenBuf[:], uint32(len(data)))
	out = append(out, lenBuf[:]...)

	crcStart := len(out)
	out = append(out, chunkType...)
	out = append(out, data...)

	crc := anvilcompress.Crc32(out[crcStart:])
	var crcBuf [4]byte
	binary.BigEndian.PutUint32(crcBuf[:], crc)
	out = append(out, crcBuf[:]...)
	return out
}
