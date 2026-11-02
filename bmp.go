// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"encoding/binary"
)

// DecodeBMP decodes a Windows Bitmap (1, 4, 8, 24, or 32 bpp, uncompressed BI_RGB or BI_BITFIELDS)
// into top-down RGBA.
func DecodeBMP(data []byte) (*Image, error) {
	if len(data) < 54 || string(data[0:2]) != "BM" {
		return nil, newInvalidHeader("missing BMP header")
	}

	dataOffset := int(binary.LittleEndian.Uint32(data[10:14]))
	headerSize := binary.LittleEndian.Uint32(data[14:18])
	if headerSize < 40 {
		return nil, newUnsupportedFormat("unsupported BMP header type")
	}

	widthI32 := int32(binary.LittleEndian.Uint32(data[18:22]))
	heightI32 := int32(binary.LittleEndian.Uint32(data[22:26]))
	planes := binary.LittleEndian.Uint16(data[26:28])
	bpp := binary.LittleEndian.Uint16(data[28:30])
	compression := binary.LittleEndian.Uint32(data[30:34])

	if planes != 1 {
		return nil, newInvalidHeader("BMP planes must be 1")
	}
	if widthI32 <= 0 || heightI32 == 0 {
		return nil, newInvalidHeader("invalid BMP dimensions")
	}
	if compression != 0 && compression != 3 {
		return nil, newUnsupportedFormat("compressed BMP not supported")
	}
	if bpp != 1 && bpp != 4 && bpp != 8 && bpp != 24 && bpp != 32 {
		return nil, newUnsupportedFormat("unsupported BMP bit depth")
	}

	width := uint32(widthI32)
	topDown := heightI32 < 0
	var height uint32
	if heightI32 < 0 {
		height = uint32(-heightI32)
	} else {
		height = uint32(heightI32)
	}

	if err := ValidateDimensions(width, height); err != nil {
		return nil, err
	}

	// Read palette for indexed images (1, 4, 8 bpp)
	var palette [][4]uint8
	if bpp <= 8 {
		paletteOffset := 14 + int(headerSize)
		numColors := 1 << bpp
		if paletteOffset+numColors*4 <= len(data) {
			palette = make([][4]uint8, numColors)
			for i := 0; i < numColors; i++ {
				base := paletteOffset + i*4
				palette[i] = [4]uint8{data[base+2], data[base+1], data[base], 255} // R, G, B, A
			}
		}
	}

	rowStride := ((int(width)*int(bpp) + 31) / 32) * 4
	if dataOffset+rowStride*int(height) > len(data) {
		return nil, newUnexpectedEof()
	}

	out := New(width, height)
	for row := 0; row < int(height); row++ {
		var dstY uint32
		if topDown {
			dstY = uint32(row)
		} else {
			dstY = height - 1 - uint32(row)
		}

		rowBase := dataOffset + row*rowStride
		switch bpp {
		case 1:
			for x := uint32(0); x < width; x++ {
				byteIdx := rowBase + int(x/8)
				bitIdx := 7 - (x % 8)
				colorIdx := (data[byteIdx] >> bitIdx) & 1
				if int(colorIdx) < len(palette) {
					c := palette[colorIdx]
					out.SetPixel(x, dstY, c[0], c[1], c[2], c[3])
				}
			}
		case 4:
			for x := uint32(0); x < width; x++ {
				byteIdx := rowBase + int(x/2)
				var colorIdx uint8
				if x%2 == 0 {
					colorIdx = (data[byteIdx] >> 4) & 0x0F
				} else {
					colorIdx = data[byteIdx] & 0x0F
				}
				if int(colorIdx) < len(palette) {
					c := palette[colorIdx]
					out.SetPixel(x, dstY, c[0], c[1], c[2], c[3])
				}
			}
		case 8:
			for x := uint32(0); x < width; x++ {
				colorIdx := data[rowBase+int(x)]
				if int(colorIdx) < len(palette) {
					c := palette[colorIdx]
					out.SetPixel(x, dstY, c[0], c[1], c[2], c[3])
				}
			}
		case 24:
			for x := uint32(0); x < width; x++ {
				pxBase := rowBase + int(x)*3
				b := data[pxBase]
				g := data[pxBase+1]
				r := data[pxBase+2]
				out.SetPixel(x, dstY, r, g, b, 255)
			}
		case 32:
			for x := uint32(0); x < width; x++ {
				pxBase := rowBase + int(x)*4
				b := data[pxBase]
				g := data[pxBase+1]
				r := data[pxBase+2]
				a := data[pxBase+3]
				out.SetPixel(x, dstY, r, g, b, a)
			}
		}
	}

	return out, nil
}

// EncodeBMP encodes image as a 32-bit top-down BMP (BI_RGB, no row padding).
func EncodeBMP(image *Image) []byte {
	if image == nil {
		return nil
	}
	pixelBytes := int(image.Width) * int(image.Height) * 4
	fileSize := 54 + pixelBytes
	out := make([]byte, 0, fileSize)

	// BITMAPFILEHEADER (14 bytes)
	out = append(out, "BM"...)
	var u32Buf [4]byte
	binary.LittleEndian.PutUint32(u32Buf[:], uint32(fileSize))
	out = append(out, u32Buf[:]...)
	out = append(out, 0, 0, 0, 0) // reserved
	binary.LittleEndian.PutUint32(u32Buf[:], 54)
	out = append(out, u32Buf[:]...)

	// BITMAPINFOHEADER (40 bytes)
	binary.LittleEndian.PutUint32(u32Buf[:], 40)
	out = append(out, u32Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], uint32(image.Width))
	out = append(out, u32Buf[:]...)
	// Negative height for top-down rows
	binary.LittleEndian.PutUint32(u32Buf[:], uint32(-int32(image.Height)))
	out = append(out, u32Buf[:]...)
	var u16Buf [2]byte
	binary.LittleEndian.PutUint16(u16Buf[:], 1) // planes
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint16(u16Buf[:], 32) // bit count
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], 0) // compression BI_RGB
	out = append(out, u32Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], uint32(pixelBytes))
	out = append(out, u32Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], 2835) // X pixels per meter
	out = append(out, u32Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], 2835) // Y pixels per meter
	out = append(out, u32Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], 0) // colors used
	out = append(out, u32Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], 0) // important colors
	out = append(out, u32Buf[:]...)

	// Pixels: RGBA in memory -> BGRA on disk
	for i := 0; i+3 < len(image.Pixels); i += 4 {
		r := image.Pixels[i]
		g := image.Pixels[i+1]
		b := image.Pixels[i+2]
		a := image.Pixels[i+3]
		out = append(out, b, g, r, a)
	}
	return out
}
