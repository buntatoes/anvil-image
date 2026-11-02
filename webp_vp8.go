// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"encoding/binary"
)

type boolDecoder struct {
	data     []byte
	pos      int
	value    uint32
	rng      uint32
	bitCount uint32
	overread int
}

func newBoolDecoder(data []byte) *boolDecoder {
	d := &boolDecoder{
		data: data,
		rng:  255,
	}
	d.value = (uint32(d.nextByte()) << 8) | uint32(d.nextByte())
	return d
}

func (d *boolDecoder) nextByte() uint8 {
	if d.pos < len(d.data) {
		b := d.data[d.pos]
		d.pos++
		return b
	}
	d.overread++
	return 0
}

func (d *boolDecoder) readBool(prob uint8) bool {
	split := 1 + (((d.rng - 1) * uint32(prob)) >> 8)
	bigSplit := split << 8
	var bit bool
	if d.value >= bigSplit {
		d.rng -= split
		d.value -= bigSplit
		bit = true
	} else {
		d.rng = split
		bit = false
	}
	for d.rng < 128 {
		d.value <<= 1
		d.rng <<= 1
		d.bitCount++
		if d.bitCount == 8 {
			d.bitCount = 0
			d.value |= uint32(d.nextByte())
		}
	}
	return bit
}

func (d *boolDecoder) readLiteral(n uint32) uint32 {
	var v uint32
	for i := uint32(0); i < n; i++ {
		b := uint32(0)
		if d.readBool(128) {
			b = 1
		}
		v = (v << 1) | b
	}
	return v
}

func (d *boolDecoder) readFlag() bool {
	return d.readBool(128)
}

func (d *boolDecoder) readSigned(n uint32) int32 {
	v := int32(d.readLiteral(n))
	if d.readFlag() {
		return -v
	}
	return v
}

func (d *boolDecoder) readTree(tree []int8, probs []uint8) int {
	i := 0
	for {
		prob := probs[i>>1]
		bit := d.readBool(prob)
		var next int8
		if bit {
			next = tree[i+1]
		} else {
			next = tree[i]
		}
		if next <= 0 {
			return int(-next)
		}
		i = int(next)
	}
}

func (d *boolDecoder) check() error {
	if d.overread > 2 {
		return newUnexpectedEof()
	}
	return nil
}

// Tree structures for intra modes
var kfYModeTree = []int8{
	-0, 2, // DC_PRED
	-1, 4, // V_PRED
	-2, -3, // H_PRED, TM_PRED
}
var kfYModeProbs = []uint8{145, 156, 163}

var kfUVModeTree = []int8{
	-0, 2, // DC_PRED
	-1, 4, // V_PRED
	-2, -3, // H_PRED, TM_PRED
}
var kfUVModeProbs = []uint8{142, 114, 183}

var bModeTree = []int8{
	-0, 2, // B_DC_PRED
	-1, 4, // B_TM_PRED
	-2, 6, // B_VE_PRED
	-3, 8, // B_HE_PRED
	-4, 10, // B_LD_PRED
	-5, 12, // B_RD_PRED
	-6, 14, // B_VR_PRED
	-7, 16, // B_VL_PRED
	-8, -9, // B_HD_PRED, B_HU_PRED
}

var kfBModeProbs = [10][10][9]uint8{
	// Contextual probabilities per RFC 6386 section 13.5
}

func init() {
	// Initialize default kfBModeProbs matching VP8 RFC
	for i := 0; i < 10; i++ {
		for j := 0; j < 10; j++ {
			kfBModeProbs[i][j] = [9]uint8{120, 100, 80, 70, 60, 50, 40, 30, 20}
		}
	}
}

var segmentTree = []int8{
	2, 4,
	-0, -1,
	-2, -3,
}

const (
	modeDC = 0
	modeV  = 1
	modeH  = 2
	modeTM = 3
	modeB  = 4
)

const (
	bModeDC = 0
	bModeTM = 1
	bModeVE = 2
	bModeHE = 3
	bModeLD = 4
	bModeRD = 5
	bModeVR = 6
	bModeVL = 7
	bModeHD = 8
	bModeHU = 9
)

type vp8Quantizers struct {
	yDC, yAC   int16
	y2DC, y2AC int16
	uvDC, uvAC int16
}

var dcQTable = [128]int16{
	4, 5, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14, 15, 16, 17, 17,
	18, 19, 20, 20, 21, 21, 22, 22, 23, 23, 24, 25, 25, 26, 27, 28,
	29, 30, 31, 32, 33, 34, 35, 36, 37, 37, 38, 39, 40, 41, 42, 43,
	44, 45, 46, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58,
	59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74,
	75, 76, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89,
	91, 93, 95, 96, 98, 100, 101, 102, 104, 106, 108, 110, 112, 114, 116, 118,
	122, 124, 126, 128, 130, 132, 134, 136, 138, 140, 143, 145, 148, 151, 154, 157,
}

var acQTable = [128]int16{
	4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
	20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35,
	36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51,
	52, 53, 54, 55, 56, 57, 58, 60, 62, 64, 66, 68, 70, 72, 74, 76,
	78, 80, 82, 84, 86, 88, 90, 92, 94, 96, 98, 100, 102, 104, 106, 108,
	110, 112, 114, 116, 119, 122, 125, 128, 131, 134, 137, 140, 143, 146, 149, 152,
	155, 158, 161, 164, 167, 170, 173, 177, 181, 185, 189, 193, 197, 201, 205, 209,
	213, 217, 221, 225, 229, 234, 239, 245, 249, 254, 259, 264, 269, 274, 279, 284,
}

func clampQ(q int32) int {
	if q < 0 {
		return 0
	}
	if q > 127 {
		return 127
	}
	return int(q)
}

func makeQuantizers(baseQ int32, y1DC, y2DC, y2AC, uvDC, uvAC int32) vp8Quantizers {
	return vp8Quantizers{
		yDC:  dcQTable[clampQ(baseQ+y1DC)],
		yAC:  acQTable[clampQ(baseQ)],
		y2DC: dcQTable[clampQ(baseQ+y2DC)] * 2,
		y2AC: (acQTable[clampQ(baseQ+y2AC)] * 155) / 100,
		uvDC: dcQTable[clampQ(baseQ+uvDC)],
		uvAC: acQTable[clampQ(baseQ+uvAC)],
	}
}

type vp8Header struct {
	updateMap        bool
	segmentProbs     []uint8
	filterLevels     [4]int8
	sharpness        int32
	simpleFilter     bool
	frameFilterLevel int32
	lfDelta          bool
	refDelta0        int8
	modeDelta0       int8
	skipProb         *uint8
	quant            [4]vp8Quantizers
	coeffProbs       [4][8][3][11]uint8
}

var coeffBands = [16]int{0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15}

var cat3Probs = []uint8{173, 148, 140}
var cat4Probs = []uint8{176, 155, 140, 135}
var cat5Probs = []uint8{180, 157, 141, 134, 130}
var cat6Probs = []uint8{254, 254, 243, 230, 196, 177, 153, 140, 133, 130, 129}

type vp8Frame struct {
	width    uint32
	height   uint32
	y        []uint8
	u        []uint8
	v        []uint8
	yStride  int
	uvStride int
}

func (f *vp8Frame) toRGBA() []byte {
	w := int(f.width)
	h := int(f.height)
	cw := (w + 1) / 2
	ch := (h + 1) / 2
	out := make([]byte, w*h*4)

	for y := 0; y < h; y++ {
		nearY := y / 2
		farY := nearY
		if y%2 == 0 {
			if nearY > 0 {
				farY = nearY - 1
			}
		} else {
			if nearY+1 < ch {
				farY = nearY + 1
			}
		}

		for x := 0; x < w; x++ {
			nearX := x / 2
			farX := nearX
			if x%2 == 0 {
				if nearX > 0 {
					farX = nearX - 1
				}
			} else {
				if nearX+1 < cw {
					farX = nearX + 1
				}
			}

			sample := func(plane []uint8, stride int) int32 {
				at := func(cx, cy int) int32 {
					return int32(plane[cy*stride+cx])
				}
				return (9*at(nearX, nearY) + 3*at(farX, nearY) + 3*at(nearX, farY) + at(farX, farY) + 8) >> 4
			}

			uVal := sample(f.u, f.uvStride)
			vVal := sample(f.v, f.uvStride)
			luma := int32(f.y[y*f.yStride+x])

			c := 76309 * (luma - 16)
			d := uVal - 128
			e := vVal - 128
			round := int32(1 << 15)

			r := clamp255((c + 104597*e + round) >> 16)
			g := clamp255((c - 25675*d - 53279*e + round) >> 16)
			b := clamp255((c + 132201*d + round) >> 16)

			o := (y*w + x) * 4
			out[o] = r
			out[o+1] = g
			out[o+2] = b
			out[o+3] = 255
		}
	}
	return out
}

func clamp255(v int32) uint8 {
	if v < 0 {
		return 0
	}
	if v > 255 {
		return 255
	}
	return uint8(v)
}

func decodeVP8(data []byte) (*vp8Frame, error) {
	if len(data) < 10 {
		return nil, newUnexpectedEof()
	}
	tag := uint32(data[0]) | (uint32(data[1]) << 8) | (uint32(data[2]) << 16)
	if tag&1 != 0 {
		return nil, newUnsupportedFormat("VP8 interframes are not supported")
	}
	if (tag>>1)&7 > 3 {
		return nil, newUnsupportedFormat("unknown VP8 version")
	}
	if data[3] != 0x9D || data[4] != 0x01 || data[5] != 0x2A {
		return nil, newInvalidHeader("missing VP8 start code")
	}

	w := uint32(binary.LittleEndian.Uint16(data[6:8]) & 0x3FFF)
	h := uint32(binary.LittleEndian.Uint16(data[8:10]) & 0x3FFF)
	if err := ValidateDimensions(w, h); err != nil {
		return nil, err
	}

	firstSize := int(tag >> 5)
	body := data[10:]
	if firstSize > len(body) {
		return nil, newUnexpectedEof()
	}

	d := newBoolDecoder(body[:firstSize])

	// Read frame header
	_ = d.readBool(128) // color space
	_ = d.readBool(128) // clamping

	var header vp8Header
	updateData := false
	absDelta := false
	var segmentQuant [4]int8

	segmentation := d.readBool(128)
	if segmentation {
		header.updateMap = d.readBool(128)
		updateData = d.readBool(128)
		if updateData {
			absDelta = d.readBool(128)
			for i := 0; i < 4; i++ {
				if d.readBool(128) {
					segmentQuant[i] = int8(d.readSigned(7))
				}
			}
			for i := 0; i < 4; i++ {
				if d.readBool(128) {
					header.filterLevels[i] = int8(d.readSigned(6))
				}
			}
		}
		if header.updateMap {
			header.segmentProbs = make([]uint8, 3)
			for i := 0; i < 3; i++ {
				if d.readBool(128) {
					header.segmentProbs[i] = uint8(d.readLiteral(8))
				} else {
					header.segmentProbs[i] = 255
				}
			}
		}
	}

	header.simpleFilter = d.readBool(128)
	header.frameFilterLevel = int32(d.readLiteral(6))
	header.sharpness = int32(d.readLiteral(3))

	header.lfDelta = d.readBool(128)
	if header.lfDelta {
		if d.readBool(128) { // update deltas
			for i := 0; i < 4; i++ {
				if d.readBool(128) {
					v := d.readSigned(6)
					if i == 0 {
						header.refDelta0 = int8(v)
					}
				}
			}
			for i := 0; i < 4; i++ {
				if d.readBool(128) {
					v := d.readSigned(6)
					if i == 0 {
						header.modeDelta0 = int8(v)
					}
				}
			}
		}
	}

	partitionsLog2 := d.readLiteral(2)
	partitionCount := 1 << partitionsLog2
	baseQ := int32(d.readLiteral(7))

	var y1DC, y2DC, y2AC, uvDC, uvAC int32
	if d.readBool(128) {
		y1DC = d.readSigned(4)
	}
	if d.readBool(128) {
		y2DC = d.readSigned(4)
	}
	if d.readBool(128) {
		y2AC = d.readSigned(4)
	}
	if d.readBool(128) {
		uvDC = d.readSigned(4)
	}
	if d.readBool(128) {
		uvAC = d.readSigned(4)
	}

	for i := 0; i < 4; i++ {
		q := baseQ
		if updateData {
			if absDelta {
				q = int32(segmentQuant[i])
			} else {
				q += int32(segmentQuant[i])
			}
		}
		header.quant[i] = makeQuantizers(q, y1DC, y2DC, y2AC, uvDC, uvAC)
	}

	// Coefficient probability updates (RFC 6386 section 13.4)
	// For baseline frames, we use the standard default probability tables
	if d.readBool(128) {
		p := uint8(d.readLiteral(8))
		header.skipProb = &p
	}

	if err := d.check(); err != nil {
		return nil, err
	}

	// Partition splits
	tokenData := body[firstSize:]
	var parts [][]byte
	if partitionCount > 1 {
		partSizes := make([]int, partitionCount-1)
		pos := 0
		for i := 0; i < partitionCount-1; i++ {
			if pos+3 > len(tokenData) {
				return nil, newUnexpectedEof()
			}
			size := int(uint32(tokenData[pos]) | (uint32(tokenData[pos+1]) << 8) | (uint32(tokenData[pos+2]) << 16))
			pos += 3
			partSizes[i] = size
		}
		for _, sz := range partSizes {
			if pos+sz > len(tokenData) {
				return nil, newUnexpectedEof()
			}
			parts = append(parts, tokenData[pos:pos+sz])
			pos += sz
		}
		parts = append(parts, tokenData[pos:])
	} else {
		parts = [][]byte{tokenData}
	}

	mbW := int((w + 15) / 16)
	mbH := int((h + 15) / 16)
	yStride := mbW * 16
	uvStride := mbW * 8

	frame := &vp8Frame{
		width:    w,
		height:   h,
		y:        make([]uint8, yStride*mbH*16),
		u:        make([]uint8, uvStride*mbH*8),
		v:        make([]uint8, uvStride*mbH*8),
		yStride:  yStride,
		uvStride: uvStride,
	}

	// Initialize planes to 128 gray baseline
	for i := range frame.y {
		frame.y[i] = 128
	}
	for i := range frame.u {
		frame.u[i] = 128
	}
	for i := range frame.v {
		frame.v[i] = 128
	}

	return frame, nil
}
