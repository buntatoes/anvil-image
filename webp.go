// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"encoding/binary"

	anvilcompress "github.com/buntatoes/anvil-compress"
)

const flagAnimation uint8 = 0x02

type webpChunks struct {
	canvasW uint32
	canvasH uint32
	hasSize bool
	vp8l    []byte
	vp8     []byte
	alpha   []byte
	rgba    []byte
}

func readU24(bytes []byte) uint32 {
	return uint32(bytes[0]) | (uint32(bytes[1]) << 8) | (uint32(bytes[2]) << 16)
}

func walkWebP(data []byte) (*webpChunks, error) {
	if len(data) < 12 || string(data[0:4]) != "RIFF" || string(data[8:12]) != "WEBP" {
		return nil, newInvalidHeader("missing RIFF WEBP header")
	}

	riffSize := int(binary.LittleEndian.Uint32(data[4:8]))
	end := riffSize + 8
	if end < 12 {
		end = 12
	}
	if end > len(data) {
		end = len(data)
	}

	chunks := &webpChunks{}
	pos := 12

	for pos+8 <= end {
		fourcc := string(data[pos : pos+4])
		chunkLen := int(binary.LittleEndian.Uint32(data[pos+4 : pos+8]))
		pos += 8

		if chunkLen > end-pos {
			return nil, newUnexpectedEof()
		}
		payload := data[pos : pos+chunkLen]

		switch fourcc {
		case "VP8X":
			if len(payload) < 10 {
				return nil, newInvalidHeader("truncated VP8X chunk")
			}
			if payload[0]&flagAnimation != 0 {
				return nil, newUnsupportedFormat("animated WebP is not supported")
			}
			w := readU24(payload[4:7]) + 1
			h := readU24(payload[7:10]) + 1
			if err := ValidateDimensions(w, h); err != nil {
				return nil, err
			}
			if !chunks.hasSize {
				chunks.canvasW = w
				chunks.canvasH = h
				chunks.hasSize = true
			}

		case "ANIM", "ANMF":
			return nil, newUnsupportedFormat("animated WebP is not supported")

		case "VP8L":
			if chunks.vp8l == nil {
				chunks.vp8l = payload
			}

		case "VP8 ":
			if chunks.vp8 == nil {
				chunks.vp8 = payload
			}

		case "ALPH":
			if chunks.alpha == nil {
				chunks.alpha = payload
			}

		case "RGBA":
			if chunks.rgba == nil {
				chunks.rgba = payload
			}
		}

		pos += chunkLen + (chunkLen & 1)
	}

	return chunks, nil
}

func unfilterAlpha(alpha []byte, width, height int, filter uint8) {
	if filter == 0 || width == 0 {
		return
	}
	for x := 1; x < width; x++ {
		alpha[x] = alpha[x] + alpha[x-1]
	}
	for y := 1; y < height; y++ {
		row := y * width
		for x := 0; x < width; x++ {
			i := row + x
			above := alpha[i-width]
			var pred uint8
			if x == 0 {
				pred = above
			} else {
				switch filter {
				case 1:
					pred = alpha[i-1]
				case 2:
					pred = above
				default:
					left := int32(alpha[i-1])
					corner := int32(alpha[i-width-1])
					val := left + int32(above) - corner
					if val < 0 {
						val = 0
					} else if val > 255 {
						val = 255
					}
					pred = uint8(val)
				}
			}
			alpha[i] = alpha[i] + pred
		}
	}
}

func decodeAlpha(payload []byte, width, height uint32) ([]uint8, error) {
	if len(payload) == 0 {
		return nil, newUnexpectedEof()
	}
	info := payload[0]
	data := payload[1:]
	count := int(width) * int(height)
	method := info & 3

	var alpha []uint8
	switch method {
	case 0:
		if len(data) < count {
			return nil, newUnexpectedEof()
		}
		alpha = make([]uint8, count)
		copy(alpha, data[:count])

	case 1:
		stream, err := decodeVP8LHeaderless(data, width, height)
		if err != nil {
			return nil, err
		}
		alpha = make([]uint8, len(stream))
		for i, p := range stream {
			alpha[i] = uint8((p >> 8) & 0xFF) // green channel carries alpha
		}

	default:
		return nil, newInvalidHeader("unknown ALPH compression method")
	}

	filter := (info >> 2) & 3
	unfilterAlpha(alpha, int(width), int(height), filter)
	return alpha, nil
}

func argbToImage(width, height uint32, argb []uint32) (*Image, error) {
	pixels := make([]byte, len(argb)*4)
	for i, p := range argb {
		o := i * 4
		pixels[o] = uint8((p >> 16) & 0xFF)   // R
		pixels[o+1] = uint8((p >> 8) & 0xFF)  // G
		pixels[o+2] = uint8(p & 0xFF)         // B
		pixels[o+3] = uint8((p >> 24) & 0xFF) // A
	}
	return FromRGBA(width, height, pixels)
}

// DecodeWebP decodes a RIFF WEBP image (lossless VP8L, lossy VP8 with optional ALPH, or RGBA) into RGBA.
func DecodeWebP(data []byte) (*Image, error) {
	chunks, err := walkWebP(data)
	if err != nil {
		return nil, err
	}

	if chunks.vp8l != nil {
		w, h, argb, err := decodeVP8L(chunks.vp8l)
		if err != nil {
			return nil, err
		}
		if chunks.hasSize && (chunks.canvasW != w || chunks.canvasH != h) {
			return nil, newInvalidHeader("WebP bitstream size differs from VP8X canvas")
		}
		return argbToImage(w, h, argb)
	}

	if chunks.vp8 != nil {
		frame, err := decodeVP8(chunks.vp8)
		if err != nil {
			return nil, err
		}
		if chunks.hasSize && (chunks.canvasW != frame.width || chunks.canvasH != frame.height) {
			return nil, newInvalidHeader("WebP bitstream size differs from VP8X canvas")
		}
		rgba := frame.toRGBA()
		if chunks.alpha != nil {
			alpha, err := decodeAlpha(chunks.alpha, frame.width, frame.height)
			if err != nil {
				return nil, err
			}
			for i := 0; i < len(alpha) && i*4+3 < len(rgba); i++ {
				rgba[i*4+3] = alpha[i]
			}
		}
		return FromRGBA(frame.width, frame.height, rgba)
	}

	if chunks.rgba != nil && chunks.hasSize {
		expected := int(chunks.canvasW) * int(chunks.canvasH) * 4
		pixels, err := anvilcompress.ZlibDecompressBounded(chunks.rgba, expected)
		if err != nil {
			return nil, newCompressionError(err)
		}
		return FromRGBA(chunks.canvasW, chunks.canvasH, pixels)
	}

	return nil, newUnsupportedFormat("WebP file has no recognized image chunk")
}

// EncodeWebP encodes image as a simple lossless RIFF WEBP container with a VP8L chunk.
func EncodeWebP(image *Image) []byte {
	if image == nil {
		return nil
	}
	payload := encodeVP8L(image.Width, image.Height, image.Pixels)
	pad := len(payload) & 1
	riffSize := uint32(4 + 8 + len(payload) + pad)

	out := make([]byte, 0, 8+int(riffSize))
	out = append(out, "RIFF"...)
	var u32Buf [4]byte
	binary.LittleEndian.PutUint32(u32Buf[:], riffSize)
	out = append(out, u32Buf[:]...)
	out = append(out, "WEBP"...)
	out = append(out, "VP8L"...)
	binary.LittleEndian.PutUint32(u32Buf[:], uint32(len(payload)))
	out = append(out, u32Buf[:]...)
	out = append(out, payload...)
	if pad == 1 {
		out = append(out, 0)
	}
	return out
}
