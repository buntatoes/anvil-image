// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"bytes"
	"encoding/binary"
)

// DecodeICO decodes the largest icon entry from a Windows ICO (type 1) or CUR (type 2) stream.
func DecodeICO(data []byte) (*Image, error) {
	if len(data) < 6 {
		return nil, newUnexpectedEof()
	}

	reserved := binary.LittleEndian.Uint16(data[0:2])
	iconType := binary.LittleEndian.Uint16(data[2:4])
	count := int(binary.LittleEndian.Uint16(data[4:6]))

	if reserved != 0 || (iconType != 1 && iconType != 2) || count == 0 || count > 64 {
		return nil, newInvalidHeader("invalid ICO directory header")
	}
	if 6+count*16 > len(data) {
		return nil, newUnexpectedEof()
	}

	type entry struct {
		w      uint32
		h      uint32
		offset int
		size   int
	}
	var best *entry

	for i := 0; i < count; i++ {
		base := 6 + i*16
		w := uint32(data[base])
		if w == 0 {
			w = 256
		}
		h := uint32(data[base+1])
		if h == 0 {
			h = 256
		}
		size := int(binary.LittleEndian.Uint32(data[base+8 : base+12]))
		offset := int(binary.LittleEndian.Uint32(data[base+12 : base+16]))

		if size == 0 || offset+size > len(data) {
			continue
		}

		if best == nil || (w*h >= best.w*best.h) {
			best = &entry{w: w, h: h, offset: offset, size: size}
		}
	}

	if best == nil {
		return nil, newInvalidHeader("no valid ICO directory entry")
	}

	payload := data[best.offset : best.offset+best.size]
	if len(payload) >= 8 && bytes.Equal(payload[:8], pngSignature) {
		return DecodePNG(payload)
	}

	if len(payload) < 40 {
		return nil, newUnexpectedEof()
	}
	headerSize := int(binary.LittleEndian.Uint32(payload[0:4]))
	if headerSize < 40 || headerSize > len(payload) {
		return nil, newInvalidHeader("invalid ICO DIB header")
	}

	widthI32 := int32(binary.LittleEndian.Uint32(payload[4:8]))
	rawHeightI32 := int32(binary.LittleEndian.Uint32(payload[8:12]))

	width := uint32(absInt32(widthI32))
	rawHeight := uint32(absInt32(rawHeightI32))
	height := rawHeight / 2
	if height < 1 {
		height = 1
	}

	if err := ValidateDimensions(width, height); err != nil {
		return nil, err
	}

	bpp := binary.LittleEndian.Uint16(payload[14:16])
	if bpp != 32 && bpp != 24 {
		return nil, newUnsupportedFormat("ICO DIB must be 24-bit or 32-bit")
	}

	bytesPerPixel := int(bpp / 8)
	rowStride := ((int(width)*bytesPerPixel + 3) / 4) * 4
	xorBytes := rowStride * int(height)
	if headerSize+xorBytes > len(payload) {
		return nil, newUnexpectedEof()
	}

	img := New(width, height)
	for y := uint32(0); y < height; y++ {
		srcRow := int(height - 1 - y)
		rowStart := headerSize + srcRow*rowStride
		for x := uint32(0); x < width; x++ {
			px := rowStart + int(x)*bytesPerPixel
			b := payload[px]
			g := payload[px+1]
			r := payload[px+2]
			var a uint8 = 255
			if bpp == 32 {
				a = payload[px+3]
			}
			img.SetPixel(x, y, r, g, b, a)
		}
	}

	return img, nil
}

// EncodeICO encodes image as a single-entry ICO embedding a PNG payload.
func EncodeICO(image *Image) []byte {
	if image == nil {
		return nil
	}
	png := EncodePNG(image)
	out := make([]byte, 0, 22+len(png))

	var u16Buf [2]byte
	var u32Buf [4]byte

	binary.LittleEndian.PutUint16(u16Buf[:], 0) // reserved
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint16(u16Buf[:], 1) // type 1 (ICO)
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint16(u16Buf[:], 1) // 1 entry
	out = append(out, u16Buf[:]...)

	wByte := uint8(image.Width)
	if image.Width >= 256 {
		wByte = 0
	}
	hByte := uint8(image.Height)
	if image.Height >= 256 {
		hByte = 0
	}

	out = append(out, wByte, hByte, 0, 0)       // w, h, paletteCount=0, reserved=0
	binary.LittleEndian.PutUint16(u16Buf[:], 1) // planes=1
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint16(u16Buf[:], 32) // bpp=32
	out = append(out, u16Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], uint32(len(png)))
	out = append(out, u32Buf[:]...)
	binary.LittleEndian.PutUint32(u32Buf[:], 22) // payload starts at offset 22
	out = append(out, u32Buf[:]...)

	out = append(out, png...)
	return out
}
