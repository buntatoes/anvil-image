// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"encoding/binary"
	"math"
	"math/bits"
)

var jpegZigzag = [64]int{
	0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
	13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
	52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
}

const (
	jpegMaxDCCategory uint8 = 11
	jpegMaxACCategory uint8 = 10
)

type jpegHuffEntry struct {
	codeLen  uint8
	codeBits uint16
	symbol   uint8
}

type jpegHuffTable struct {
	entries []jpegHuffEntry
}

func newJpegHuffTable(counts *[16]uint8, symbols []uint8) jpegHuffTable {
	entries := make([]jpegHuffEntry, 0, len(symbols))
	var code uint16
	symIdx := 0
	for lenMinus1, count := range counts {
		bitLen := uint8(lenMinus1 + 1)
		for c := uint8(0); c < count; c++ {
			if symIdx < len(symbols) {
				entries = append(entries, jpegHuffEntry{
					codeLen:  bitLen,
					codeBits: code,
					symbol:   symbols[symIdx],
				})
				symIdx++
			}
			code++
		}
		code <<= 1
	}
	return jpegHuffTable{entries: entries}
}

type jpegComponent struct {
	id      uint8
	hFactor uint8
	vFactor uint8
	quantID uint8
	dcTable uint8
	acTable uint8
}

type jpegBitReader struct {
	data     []byte
	pos      int
	bitBuf   uint32
	bitCount uint8
}

func (r *jpegBitReader) nextBit() (uint8, error) {
	if r.bitCount == 0 {
		if r.pos >= len(r.data) {
			return 0, newUnexpectedEof()
		}
		b := r.data[r.pos]
		r.pos++
		if b == 0xFF {
			if r.pos >= len(r.data) {
				return 0, newUnexpectedEof()
			}
			next := r.data[r.pos]
			r.pos++
			if next != 0x00 && !(next >= 0xD0 && next <= 0xD7) {
				return 0, newInvalidHeader("unexpected marker inside JPEG scan")
			}
		}
		r.bitBuf = uint32(b)
		r.bitCount = 8
	}
	r.bitCount--
	return uint8((r.bitBuf >> r.bitCount) & 1), nil
}

func (r *jpegBitReader) readBits(count uint8) (uint16, error) {
	var val uint16
	for i := uint8(0); i < count; i++ {
		bit, err := r.nextBit()
		if err != nil {
			return 0, err
		}
		val = (val << 1) | uint16(bit)
	}
	return val, nil
}

func (r *jpegBitReader) decodeHuff(table *jpegHuffTable) (uint8, error) {
	var code uint16
	for l := uint8(1); l <= 16; l++ {
		bit, err := r.nextBit()
		if err != nil {
			return 0, err
		}
		code = (code << 1) | uint16(bit)
		for _, e := range table.entries {
			if e.codeLen == l && e.codeBits == code {
				return e.symbol, nil
			}
		}
	}
	return 0, newInvalidHeader("invalid JPEG Huffman code")
}

func receiveExtend(bits uint16, category, maxCat uint8) (int32, error) {
	if category == 0 {
		return 0, nil
	}
	if category > maxCat || category > 15 {
		return 0, newInvalidHeader("JPEG coefficient category out of range")
	}
	span := uint32(1) << category
	b := uint32(bits)
	if b >= span {
		return 0, newInvalidHeader("JPEG coefficient bits out of range")
	}
	half := span >> 1
	if b < half {
		return int32(b) - int32(span-1), nil
	}
	return int32(b), nil
}

// DecodeJPEG decodes a baseline sequential SOF0 JPEG into opaque RGBA.
func DecodeJPEG(data []byte) (*Image, error) {
	if len(data) < 4 || data[0] != 0xFF || data[1] != 0xD8 {
		return nil, newInvalidHeader("missing JPEG SOI marker")
	}

	pos := 2
	var qtables [4][64]uint16
	for i := range qtables {
		for j := range qtables[i] {
			qtables[i][j] = 1
		}
	}
	var dcHuff [4]jpegHuffTable
	var acHuff [4]jpegHuffTable
	var width, height uint32
	var components []jpegComponent
	var restartInterval uint16

	for pos+2 <= len(data) {
		if data[pos] != 0xFF {
			pos++
			continue
		}
		for pos < len(data) && data[pos] == 0xFF {
			pos++
		}
		if pos >= len(data) {
			break
		}
		marker := data[pos]
		pos++

		if marker == 0xD9 { // EOI
			break
		}
		if marker == 0x00 || (marker >= 0xD0 && marker <= 0xD8) {
			continue
		}
		if pos+2 > len(data) {
			return nil, newUnexpectedEof()
		}
		segLen := int(binary.BigEndian.Uint16(data[pos : pos+2]))
		if segLen < 2 || pos+segLen > len(data) {
			return nil, newUnexpectedEof()
		}
		seg := data[pos+2 : pos+segLen]
		pos += segLen

		switch marker {
		case 0xDB: // DQT
			sp := 0
			for sp < len(seg) {
				info := seg[sp]
				sp++
				pq := info >> 4
				tq := int(info & 0x0F)
				if tq >= 4 {
					return nil, newInvalidHeader("invalid JPEG DQT index")
				}
				if pq == 0 {
					if sp+64 > len(seg) {
						return nil, newUnexpectedEof()
					}
					for i := 0; i < 64; i++ {
						qtables[tq][i] = uint16(seg[sp+i])
					}
					sp += 64
				} else {
					if sp+128 > len(seg) {
						return nil, newUnexpectedEof()
					}
					for i := 0; i < 64; i++ {
						qtables[tq][i] = binary.BigEndian.Uint16(seg[sp+i*2 : sp+i*2+2])
					}
					sp += 128
				}
			}

		case 0xC0: // SOF0
			if len(seg) < 6 {
				return nil, newInvalidHeader("short SOF0 segment")
			}
			precision := seg[0]
			if precision != 8 {
				return nil, newUnsupportedFormat("only 8-bit baseline JPEG supported")
			}
			height = uint32(binary.BigEndian.Uint16(seg[1:3]))
			width = uint32(binary.BigEndian.Uint16(seg[3:5]))
			ncomp := int(seg[5])
			if width == 0 || height == 0 || (ncomp != 1 && ncomp != 3) || len(seg) < 6+ncomp*3 {
				return nil, newInvalidHeader("invalid SOF0 dimensions")
			}
			if err := ValidateDimensions(width, height); err != nil {
				return nil, err
			}
			components = make([]jpegComponent, 0, ncomp)
			for i := 0; i < ncomp; i++ {
				base := 6 + i*3
				id := seg[base]
				samp := seg[base+1]
				qid := seg[base+2]
				h := samp >> 4
				if h == 0 {
					h = 1
				}
				v := samp & 0x0F
				if v == 0 {
					v = 1
				}
				if qid > 3 {
					qid = 3
				}
				components = append(components, jpegComponent{
					id:      id,
					hFactor: h,
					vFactor: v,
					quantID: qid,
				})
			}

		case 0xC4: // DHT
			sp := 0
			for sp+17 <= len(seg) {
				info := seg[sp]
				sp++
				tc := info >> 4
				th := int(info & 0x0F)
				if th >= 4 {
					return nil, newInvalidHeader("invalid DHT index")
				}
				var counts [16]uint8
				copy(counts[:], seg[sp:sp+16])
				sp += 16
				totalSyms := 0
				for _, c := range counts {
					totalSyms += int(c)
				}
				if sp+totalSyms > len(seg) {
					return nil, newUnexpectedEof()
				}
				syms := seg[sp : sp+totalSyms]
				sp += totalSyms
				tbl := newJpegHuffTable(&counts, syms)
				if tc == 0 {
					dcHuff[th] = tbl
				} else {
					acHuff[th] = tbl
				}
			}

		case 0xDD: // DRI
			if len(seg) >= 2 {
				restartInterval = binary.BigEndian.Uint16(seg[:2])
			}

		case 0xDA: // SOS
			if len(components) == 0 || len(seg) == 0 {
				return nil, newInvalidHeader("SOS before SOF0")
			}
			nscan := int(seg[0])
			if len(seg) < 1+nscan*2+3 {
				return nil, newInvalidHeader("short SOS segment")
			}
			for i := 0; i < nscan; i++ {
				cid := seg[1+i*2]
				tdta := seg[1+i*2+1]
				for idx := range components {
					if components[idx].id == cid {
						components[idx].dcTable = (tdta >> 4) & 0x03
						components[idx].acTable = tdta & 0x03
						break
					}
				}
			}
			return decodeJPEGScan(data, pos, width, height, components, &qtables, &dcHuff, &acHuff, restartInterval)
		}
	}

	return nil, newInvalidHeader("missing JPEG SOS scan")
}

func decodeJPEGScan(
	data []byte,
	scanPos int,
	width, height uint32,
	components []jpegComponent,
	qtables *[4][64]uint16,
	dcHuff *[4]jpegHuffTable,
	acHuff *[4]jpegHuffTable,
	restartInterval uint16,
) (*Image, error) {
	maxH := uint32(1)
	maxV := uint32(1)
	for _, c := range components {
		if uint32(c.hFactor) > maxH {
			maxH = uint32(c.hFactor)
		}
		if uint32(c.vFactor) > maxV {
			maxV = uint32(c.vFactor)
		}
	}
	mcuW := maxH * 8
	mcuH := maxV * 8
	mcusX := (width + mcuW - 1) / mcuW
	mcusY := (height + mcuH - 1) / mcuH

	reader := jpegBitReader{data: data, pos: scanPos}
	var prevDC [4]int32
	var mcuCounter uint16
	out := New(width, height)

	for my := uint32(0); my < mcusY; my++ {
		for mx := uint32(0); mx < mcusX; mx++ {
			if restartInterval > 0 && mcuCounter > 0 && mcuCounter%restartInterval == 0 {
				reader.bitCount = 0
				prevDC = [4]int32{}
				if reader.pos+2 <= len(data) && data[reader.pos] == 0xFF && (data[reader.pos+1] >= 0xD0 && data[reader.pos+1] <= 0xD7) {
					reader.pos += 2
				}
			}
			mcuCounter++

			compPlanes := make([][][64]float32, len(components))
			for ci, comp := range components {
				blocksCount := int(comp.hFactor) * int(comp.vFactor)
				blocks := make([][64]float32, 0, blocksCount)
				for b := 0; b < blocksCount; b++ {
					dcCat, err := reader.decodeHuff(&dcHuff[comp.dcTable])
					if err != nil {
						return nil, err
					}
					if dcCat > jpegMaxDCCategory {
						return nil, newInvalidHeader("JPEG coefficient category out of range")
					}
					dcBits, err := reader.readBits(dcCat)
					if err != nil {
						return nil, err
					}
					diff, err := receiveExtend(dcBits, dcCat, jpegMaxDCCategory)
					if err != nil {
						return nil, err
					}
					prevDC[ci] += diff

					var zz [64]int32
					zz[0] = prevDC[ci]
					k := 1
					for k < 64 {
						rs, err := reader.decodeHuff(&acHuff[comp.acTable])
						if err != nil {
							return nil, err
						}
						if rs == 0x00 {
							break
						}
						if rs == 0xF0 {
							k += 16
							continue
						}
						run := int(rs >> 4)
						size := rs & 0x0F
						k += run
						if k >= 64 {
							break
						}
						if size > jpegMaxACCategory {
							return nil, newInvalidHeader("JPEG coefficient category out of range")
						}
						acBits, err := reader.readBits(size)
						if err != nil {
							return nil, err
						}
						val, err := receiveExtend(acBits, size, jpegMaxACCategory)
						if err != nil {
							return nil, err
						}
						zz[k] = val
						k++
					}

					qtable := qtables[comp.quantID]
					var coeff [64]float32
					for i := 0; i < 64; i++ {
						coeff[jpegZigzag[i]] = float32(zz[i]) * float32(qtable[i])
					}
					blocks = append(blocks, idct8x8(&coeff))
				}
				compPlanes[ci] = blocks
			}

			for py := uint32(0); py < mcuH; py++ {
				imgY := my*mcuH + py
				if imgY >= height {
					continue
				}
				for px := uint32(0); px < mcuW; px++ {
					imgX := mx*mcuW + px
					if imgX >= width {
						continue
					}

					sample := func(ci int) float32 {
						comp := components[ci]
						cx := (px * uint32(comp.hFactor)) / maxH
						cy := (py * uint32(comp.vFactor)) / maxV
						bx := int(cx / 8)
						by := int(cy / 8)
						sx := int(cx % 8)
						sy := int(cy % 8)
						bIdx := by*int(comp.hFactor) + bx
						return compPlanes[ci][bIdx][sy*8+sx]
					}

					if len(components) == 1 {
						yVal := uint8(clampF32(float32(math.Round(float64(sample(0)))), 0.0, 255.0))
						out.SetPixel(imgX, imgY, yVal, yVal, yVal, 255)
					} else {
						yVal := sample(0)
						cb := sample(1) - 128.0
						cr := sample(2) - 128.0
						r := uint8(clampF32(float32(math.Round(float64(yVal+1.402*cr))), 0.0, 255.0))
						g := uint8(clampF32(float32(math.Round(float64(yVal-0.344136*cb-0.714136*cr))), 0.0, 255.0))
						b := uint8(clampF32(float32(math.Round(float64(yVal+1.772*cb))), 0.0, 255.0))
						out.SetPixel(imgX, imgY, r, g, b, 255)
					}
				}
			}
		}
	}

	return out, nil
}

func idct8x8(coeff *[64]float32) [64]float32 {
	var temp [64]float32
	invSqrt2 := float32(1.0 / math.Sqrt2)

	for y := 0; y < 8; y++ {
		for x := 0; x < 8; x++ {
			var sum float32
			for u := 0; u < 8; u++ {
				cu := float32(1.0)
				if u == 0 {
					cu = invSqrt2
				}
				angle := float32((2*x+1)*u) * float32(math.Pi) / 16.0
				sum += cu * coeff[y*8+u] * float32(math.Cos(float64(angle)))
			}
			temp[y*8+x] = sum * 0.5
		}
	}

	var out [64]float32
	for y := 0; y < 8; y++ {
		for x := 0; x < 8; x++ {
			var sum float32
			for v := 0; v < 8; v++ {
				cv := float32(1.0)
				if v == 0 {
					cv = invSqrt2
				}
				angle := float32((2*y+1)*v) * float32(math.Pi) / 16.0
				sum += cv * temp[v*8+x] * float32(math.Cos(float64(angle)))
			}
			out[y*8+x] = clampF32(sum*0.5+128.0, 0.0, 255.0)
		}
	}
	return out
}

func fdct8x8(block *[64]float32) [64]int32 {
	var temp [64]float32
	invSqrt2 := float32(1.0 / math.Sqrt2)

	for y := 0; y < 8; y++ {
		for u := 0; u < 8; u++ {
			cu := float32(1.0)
			if u == 0 {
				cu = invSqrt2
			}
			var sum float32
			for x := 0; x < 8; x++ {
				angle := float32((2*x+1)*u) * float32(math.Pi) / 16.0
				sum += (block[y*8+x] - 128.0) * float32(math.Cos(float64(angle)))
			}
			temp[y*8+u] = 0.5 * cu * sum
		}
	}

	var out [64]int32
	for v := 0; v < 8; v++ {
		cv := float32(1.0)
		if v == 0 {
			cv = invSqrt2
		}
		for u := 0; u < 8; u++ {
			var sum float32
			for y := 0; y < 8; y++ {
				angle := float32((2*y+1)*v) * float32(math.Pi) / 16.0
				sum += temp[y*8+u] * float32(math.Cos(float64(angle)))
			}
			out[v*8+u] = int32(math.Round(float64(0.5 * cv * sum)))
		}
	}
	return out
}

// EncodeJPEG encodes image as a baseline sequential SOF0 JPEG, 4:4:4 YCbCr, single scan.
func EncodeJPEG(image *Image) []byte {
	if image == nil {
		return nil
	}
	var out []byte
	out = append(out, 0xFF, 0xD8) // SOI

	// DQT table 0 (all 1s)
	out = append(out, 0xFF, 0xDB, 0x00, 67, 0x00)
	for i := 0; i < 64; i++ {
		out = append(out, 1)
	}

	w := uint16(image.Width)
	h := uint16(image.Height)
	if image.Width < 1 {
		w = 1
	} else if image.Width > 65535 {
		w = 65535
	}
	if image.Height < 1 {
		h = 1
	} else if image.Height > 65535 {
		h = 65535
	}

	// SOF0: 8-bit, 3 components, 1x1 sampling
	out = append(out, 0xFF, 0xC0, 0x00, 17, 8)
	var u16Buf [2]byte
	binary.BigEndian.PutUint16(u16Buf[:], h)
	out = append(out, u16Buf[:]...)
	binary.BigEndian.PutUint16(u16Buf[:], w)
	out = append(out, u16Buf[:]...)
	out = append(out, 3, 1, 0x11, 0, 2, 0x11, 0, 3, 0x11, 0)

	// DHT DC table 0
	dcCounts := [16]uint8{0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0}
	dcSyms := []uint8{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11}
	out = append(out, 0xFF, 0xC4, 0x00, uint8(19+len(dcSyms)), 0x00)
	out = append(out, dcCounts[:]...)
	out = append(out, dcSyms...)

	// DHT AC table 0 (162 standard symbols)
	acSyms := make([]uint8, 0, 162)
	acSyms = append(acSyms, 0x00, 0xF0)
	for run := uint8(0); run < 16; run++ {
		for size := uint8(1); size <= 10; size++ {
			acSyms = append(acSyms, (run<<4)|size)
		}
	}
	acCounts := [16]uint8{0, 0, 0, 0, 0, 0, 0, uint8(len(acSyms)), 0, 0, 0, 0, 0, 0, 0, 0}
	acSegLen := uint16(19 + len(acSyms))
	out = append(out, 0xFF, 0xC4)
	binary.BigEndian.PutUint16(u16Buf[:], acSegLen)
	out = append(out, u16Buf[:]...)
	out = append(out, 0x10)
	out = append(out, acCounts[:]...)
	out = append(out, acSyms...)

	// SOS
	out = append(out, 0xFF, 0xDA, 0x00, 12, 3, 1, 0x00, 2, 0x00, 3, 0x00, 0, 63, 0)

	var curByte uint8
	var curBits uint8

	pushBits := func(val uint16, n uint8) {
		for i := int(n) - 1; i >= 0; i-- {
			b := uint8((val >> i) & 1)
			curByte = (curByte << 1) | b
			curBits++
			if curBits == 8 {
				out = append(out, curByte)
				if curByte == 0xFF {
					out = append(out, 0x00)
				}
				curByte = 0
				curBits = 0
			}
		}
	}

	encodeMag := func(v int32) (uint8, uint16) {
		if v == 0 {
			return 0, 0
		}
		absV := uint16(absInt32(v))
		if absV > 1023 {
			absV = 1023
		}
		cat := uint8(16 - bits.LeadingZeros16(absV))
		var b uint16
		if v > 0 {
			b = absV
		} else {
			b = absV ^ ((1 << cat) - 1)
		}
		return cat, b
	}

	mcusX := (uint32(w) + 7) / 8
	mcusY := (uint32(h) + 7) / 8
	var prevDC [3]int32

	for my := uint32(0); my < mcusY; my++ {
		for mx := uint32(0); mx < mcusX; mx++ {
			var planes [3][64]float32
			for by := uint32(0); by < 8; by++ {
				for bx := uint32(0); bx < 8; bx++ {
					px := mx*8 + bx
					if px >= image.Width {
						px = image.Width - 1
					}
					py := my*8 + by
					if py >= image.Height {
						py = image.Height - 1
					}
					r, g, b, _, _ := image.Pixel(px, py)
					rf := float32(r)
					gf := float32(g)
					bf := float32(b)

					y := clampF32(0.299*rf+0.587*gf+0.114*bf, 0.0, 255.0)
					cb := clampF32(-0.168736*rf-0.331264*gf+0.5*bf+128.0, 0.0, 255.0)
					cr := clampF32(0.5*rf-0.418688*gf-0.081312*bf+128.0, 0.0, 255.0)
					idx := by*8 + bx
					planes[0][idx] = y
					planes[1][idx] = cb
					planes[2][idx] = cr
				}
			}

			for ci := 0; ci < 3; ci++ {
				dct := fdct8x8(&planes[ci])
				var zz [64]int32
				for i := 0; i < 64; i++ {
					zz[i] = dct[jpegZigzag[i]]
				}
				diff := zz[0] - prevDC[ci]
				if diff < -1023 {
					diff = -1023
				} else if diff > 1023 {
					diff = 1023
				}
				prevDC[ci] = zz[0]
				dcCat, dcBits := encodeMag(diff)
				pushBits(uint16(dcCat), 4)
				pushBits(dcBits, dcCat)

				var zeroRun uint8
				for _, acVal := range zz[1:] {
					clamped := acVal
					if clamped < -511 {
						clamped = -511
					} else if clamped > 511 {
						clamped = 511
					}

					if clamped == 0 {
						zeroRun++
					} else {
						for zeroRun >= 16 {
							pushBits(1, 8) // 0xF0 is index 1 in acSyms
							zeroRun -= 16
						}
						acCat, acBits := encodeMag(clamped)
						sym := (zeroRun << 4) | acCat
						code := uint16(0)
						for posIdx, s := range acSyms {
							if s == sym {
								code = uint16(posIdx)
								break
							}
						}
						pushBits(code, 8)
						pushBits(acBits, acCat)
						zeroRun = 0
					}
				}
				if zeroRun > 0 {
					pushBits(0, 8) // EOB is index 0 in acSyms
				}
			}
		}
	}

	if curBits > 0 {
		pad := 8 - curBits
		pushBits((1<<pad)-1, pad)
	}
	out = append(out, 0xFF, 0xD9) // EOI
	return out
}
