// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"encoding/binary"
	"math"
)

// DecodeGIF decodes the first frame of a GIF87a or GIF89a file into RGBA.
func DecodeGIF(data []byte) (*Image, error) {
	if len(data) < 13 || (string(data[:6]) != "GIF87a" && string(data[:6]) != "GIF89a") {
		return nil, newInvalidHeader("missing GIF87a/GIF89a signature")
	}

	screenW := uint32(binary.LittleEndian.Uint16(data[6:8]))
	screenH := uint32(binary.LittleEndian.Uint16(data[8:10]))
	if screenW == 0 || screenH == 0 {
		return nil, newInvalidHeader("zero-sized GIF")
	}
	if err := ValidateDimensions(screenW, screenH); err != nil {
		return nil, err
	}

	packed := data[10]
	hasGCT := (packed & 0x80) != 0
	gctSize := 1 << ((packed & 0x07) + 1)
	pos := 13

	var globalPalette [][3]uint8
	if hasGCT {
		if pos+gctSize*3 > len(data) {
			return nil, newUnexpectedEof()
		}
		globalPalette = make([][3]uint8, gctSize)
		for i := 0; i < gctSize; i++ {
			base := pos + i*3
			globalPalette[i] = [3]uint8{data[base], data[base+1], data[base+2]}
		}
		pos += gctSize * 3
	}

	hasTransparent := false
	var transparentIdx uint8

	for pos < len(data) {
		marker := data[pos]
		pos++

		switch marker {
		case 0x21: // Extension introducer
			if pos >= len(data) {
				return nil, newUnexpectedEof()
			}
			extLabel := data[pos]
			pos++
			if extLabel == 0xF9 { // Graphic Control Extension
				if pos >= len(data) {
					return nil, newUnexpectedEof()
				}
				blockSize := int(data[pos])
				pos++
				if pos+blockSize > len(data) {
					return nil, newUnexpectedEof()
				}
				if blockSize >= 4 {
					gcePacked := data[pos]
					if (gcePacked & 0x01) != 0 {
						hasTransparent = true
						transparentIdx = data[pos+3]
					}
				}
				pos += blockSize
				// Skip sub-blocks
				for pos < len(data) {
					sub := int(data[pos])
					pos++
					if sub == 0 {
						break
					}
					pos += sub
				}
			} else {
				// Other extensions: skip sub-block chains
				for pos < len(data) {
					sub := int(data[pos])
					pos++
					if sub == 0 {
						break
					}
					pos += sub
				}
			}

		case 0x2C: // Image Descriptor
			if pos+9 > len(data) {
				return nil, newUnexpectedEof()
			}
			left := uint32(binary.LittleEndian.Uint16(data[pos : pos+2]))
			top := uint32(binary.LittleEndian.Uint16(data[pos+2 : pos+4]))
			imgW := uint32(binary.LittleEndian.Uint16(data[pos+4 : pos+6]))
			imgH := uint32(binary.LittleEndian.Uint16(data[pos+6 : pos+8]))
			if err := ValidateDimensions(imgW, imgH); err != nil {
				return nil, err
			}
			imgPacked := data[pos+8]
			pos += 9

			hasLCT := (imgPacked & 0x80) != 0
			interlaced := (imgPacked & 0x40) != 0
			lctSize := 1 << ((imgPacked & 0x07) + 1)

			var localPalette [][3]uint8
			if hasLCT {
				if pos+lctSize*3 > len(data) {
					return nil, newUnexpectedEof()
				}
				localPalette = make([][3]uint8, lctSize)
				for i := 0; i < lctSize; i++ {
					base := pos + i*3
					localPalette[i] = [3]uint8{data[base], data[base+1], data[base+2]}
				}
				pos += lctSize * 3
			}

			palette := globalPalette
			if hasLCT {
				palette = localPalette
			}

			if pos >= len(data) {
				return nil, newUnexpectedEof()
			}
			minCodeSize := data[pos]
			pos++
			if minCodeSize < 2 || minCodeSize > 8 {
				return nil, newInvalidHeader("invalid GIF LZW code size")
			}

			var lzwStream []byte
			for pos < len(data) {
				subLen := int(data[pos])
				pos++
				if subLen == 0 {
					break
				}
				if pos+subLen > len(data) {
					return nil, newUnexpectedEof()
				}
				lzwStream = append(lzwStream, data[pos:pos+subLen]...)
				pos += subLen
			}

			pixelCount := int(imgW) * int(imgH)
			indices, err := decodeGIFLZW(minCodeSize, lzwStream, pixelCount)
			if err != nil {
				return nil, err
			}

			out := New(screenW, screenH)
			for row := 0; row < int(imgH); row++ {
				dstRow := row
				if interlaced {
					dstRow = interlacedGIFRow(row, int(imgH))
				}
				py := top + uint32(dstRow)
				if py >= screenH {
					continue
				}
				for col := 0; col < int(imgW); col++ {
					px := left + uint32(col)
					if px >= screenW {
						continue
					}
					idxOffset := row*int(imgW) + col
					var idx uint8
					if idxOffset < len(indices) {
						idx = indices[idxOffset]
					}
					if hasTransparent && idx == transparentIdx {
						out.SetPixel(px, py, 0, 0, 0, 0)
					} else {
						var r, g, b uint8
						if int(idx) < len(palette) {
							c := palette[idx]
							r, g, b = c[0], c[1], c[2]
						}
						out.SetPixel(px, py, r, g, b, 255)
					}
				}
			}
			return out, nil

		case 0x3B: // Trailer
			pos = len(data)
		default:
			pos = len(data)
		}
	}

	return nil, newInvalidHeader("no image frame found in GIF")
}

func interlacedGIFRow(row, height int) int {
	r := row
	passes := []struct {
		start, step int
	}{
		{0, 8},
		{4, 8},
		{2, 4},
		{1, 2},
	}
	for _, p := range passes {
		if p.start < height {
			count := (height-1-p.start)/p.step + 1
			if r < count {
				return p.start + r*p.step
			}
			r -= count
		}
	}
	return row
}

func decodeGIFLZW(minCodeSize uint8, stream []byte, expectedPixels int) ([]byte, error) {
	clearCode := uint16(1 << minCodeSize)
	endCode := clearCode + 1

	codeSize := uint32(minCodeSize) + 1
	dict := make([][]byte, 0, 4096)
	resetDict := func() {
		dict = dict[:0]
		for i := uint16(0); i <= endCode; i++ {
			dict = append(dict, []byte{byte(i & 0xFF)})
		}
	}
	resetDict()

	bitPos := 0
	readCode := func(size uint32) (uint16, bool) {
		var val uint32
		for b := uint32(0); b < size; b++ {
			totalBit := bitPos + int(b)
			byteIdx := totalBit >> 3
			if byteIdx >= len(stream) {
				return 0, false
			}
			bit := (stream[byteIdx] >> (totalBit & 7)) & 1
			val |= uint32(bit) << b
		}
		bitPos += int(size)
		return uint16(val), true
	}

	out := make([]byte, 0, expectedPixels)
	var prevSeq []byte

	for {
		code, ok := readCode(codeSize)
		if !ok {
			break
		}
		if code == clearCode {
			resetDict()
			codeSize = uint32(minCodeSize) + 1
			prevSeq = nil
			continue
		}
		if code == endCode {
			break
		}

		var entry []byte
		if int(code) < len(dict) {
			entry = append([]byte(nil), dict[code]...)
		} else if len(prevSeq) > 0 {
			if int(code) == len(dict) {
				entry = append([]byte(nil), prevSeq...)
				entry = append(entry, prevSeq[0])
			} else {
				return nil, newInvalidHeader("invalid GIF LZW code")
			}
		} else {
			return nil, newInvalidHeader("invalid initial GIF LZW code")
		}

		out = append(out, entry...)

		if len(prevSeq) > 0 && len(dict) < 4096 {
			newEntry := append([]byte(nil), prevSeq...)
			newEntry = append(newEntry, entry[0])
			dict = append(dict, newEntry)
			if len(dict) == (1<<codeSize) && codeSize < 12 {
				codeSize++
			}
		}
		prevSeq = entry
		if len(out) >= expectedPixels {
			break
		}
	}

	return out, nil
}

// EncodeGIF encodes image as a single-frame GIF89a with a 256-color palette.
func EncodeGIF(image *Image) []byte {
	if image == nil {
		return nil
	}
	w := uint16(image.Width)
	h := uint16(image.Height)
	if image.Width > 65535 {
		w = 65535
	}
	if image.Height > 65535 {
		h = 65535
	}

	var out []byte
	out = append(out, "GIF89a"...)
	var u16Buf [2]byte
	binary.LittleEndian.PutUint16(u16Buf[:], w)
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint16(u16Buf[:], h)
	out = append(out, u16Buf[:]...)
	out = append(out, 0xF7, 0, 0) // GCT flag (256 entries), bg index 0, aspect 0

	palette := [][3]uint8{{0, 0, 0}} // index 0 reserved for transparent
	indices := make([]byte, 0, int(w)*int(h))

	for y := uint32(0); y < uint32(h); y++ {
		for x := uint32(0); x < uint32(w); x++ {
			r, g, b, a, _ := image.Pixel(x, y)
			if a < 128 {
				indices = append(indices, 0)
				continue
			}
			rgb := [3]uint8{r, g, b}
			found := -1
			for i := 1; i < len(palette); i++ {
				if palette[i] == rgb {
					found = i
					break
				}
			}
			if found != -1 {
				indices = append(indices, uint8(found))
			} else if len(palette) < 256 {
				idx := uint8(len(palette))
				palette = append(palette, rgb)
				indices = append(indices, idx)
			} else {
				// Nearest Euclidean RGB distance
				bestIdx := uint8(1)
				bestDist := uint32(math.MaxUint32)
				for i := 1; i < len(palette); i++ {
					dr := int32(palette[i][0]) - int32(rgb[0])
					dg := int32(palette[i][1]) - int32(rgb[1])
					db := int32(palette[i][2]) - int32(rgb[2])
					dist := uint32(dr*dr + dg*dg + db*db)
					if dist < bestDist {
						bestDist = dist
						bestIdx = uint8(i)
					}
				}
				indices = append(indices, bestIdx)
			}
		}
	}

	for len(palette) < 256 {
		palette = append(palette, [3]uint8{0, 0, 0})
	}
	for _, rgb := range palette {
		out = append(out, rgb[0], rgb[1], rgb[2])
	}

	// Graphic Control Extension: 0xF9, transparent enabled on index 0
	out = append(out, 0x21, 0xF9, 0x04, 0x01, 0x00, 0x00, 0x00, 0x00)

	// Image Descriptor at (0, 0)
	out = append(out, 0x2C)
	out = append(out, 0, 0, 0, 0) // left=0, top=0
	binary.LittleEndian.PutUint16(u16Buf[:], w)
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint16(u16Buf[:], h)
	out = append(out, u16Buf[:]...)
	out = append(out, 0x00) // no local table, not interlaced

	// LZW min code size 8
	out = append(out, 8)
	var bitBuf []byte
	var curByte uint8
	var curBit uint8

	pushCode := func(code uint16) {
		for b := uint32(0); b < 9; b++ {
			if ((code >> b) & 1) != 0 {
				curByte |= 1 << curBit
			}
			curBit++
			if curBit == 8 {
				bitBuf = append(bitBuf, curByte)
				curByte = 0
				curBit = 0
			}
		}
	}

	for i, idx := range indices {
		if i%250 == 0 {
			pushCode(256) // clear code
		}
		pushCode(uint16(idx))
	}
	pushCode(257) // end code
	if curBit > 0 {
		bitBuf = append(bitBuf, curByte)
	}

	// Emit sub-blocks
	for i := 0; i < len(bitBuf); i += 255 {
		end := i + 255
		if end > len(bitBuf) {
			end = len(bitBuf)
		}
		chunk := bitBuf[i:end]
		out = append(out, uint8(len(chunk)))
		out = append(out, chunk...)
	}
	out = append(out, 0x00) // terminator
	out = append(out, 0x3B) // trailer
	return out
}
