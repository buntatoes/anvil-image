// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"encoding/binary"
	"math/bits"
	"sort"
)

const (
	vp8lSignature    uint8  = 0x2F
	codesPerGroup    int    = 5
	literalAlphabet  int    = 256
	lengthPrefixes   int    = 24
	distanceAlphabet int    = 40
	maxCodeLength    int    = 15
	rootBits         uint32 = 8
	maxTableEntries  int    = 8 << 20
)

var codeLengthOrder = [19]int{
	17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
}

var distanceMap = [120][2]int8{
	{0, 1}, {1, 0}, {1, 1}, {-1, 1}, {0, 2}, {2, 0}, {1, 2}, {-1, 2},
	{2, 1}, {-2, 1}, {2, 2}, {-2, 2}, {0, 3}, {3, 0}, {1, 3}, {-1, 3},
	{3, 1}, {-3, 1}, {2, 3}, {-2, 3}, {3, 2}, {-3, 2}, {0, 4}, {4, 0},
	{1, 4}, {-1, 4}, {4, 1}, {-4, 1}, {3, 3}, {-3, 3}, {2, 4}, {-2, 4},
	{4, 2}, {-4, 2}, {0, 5}, {3, 4}, {-3, 4}, {4, 3}, {-4, 3}, {5, 0},
	{1, 5}, {-1, 5}, {5, 1}, {-5, 1}, {2, 5}, {-2, 5}, {5, 2}, {-5, 2},
	{4, 4}, {-4, 4}, {3, 5}, {-3, 5}, {5, 3}, {-5, 3}, {0, 6}, {6, 0},
	{1, 6}, {-1, 6}, {6, 1}, {-6, 1}, {2, 6}, {-2, 6}, {6, 2}, {-6, 2},
	{4, 5}, {-4, 5}, {5, 4}, {-5, 4}, {3, 6}, {-3, 6}, {6, 3}, {-6, 3},
	{0, 7}, {7, 0}, {1, 7}, {-1, 7}, {5, 5}, {-5, 5}, {7, 1}, {-7, 1},
	{4, 6}, {-4, 6}, {6, 4}, {-6, 4}, {2, 7}, {-2, 7}, {7, 2}, {-7, 2},
	{3, 7}, {-3, 7}, {7, 3}, {-7, 3}, {5, 6}, {-5, 6}, {6, 5}, {-6, 5},
	{8, 0}, {4, 7}, {-4, 7}, {7, 4}, {-7, 4}, {8, 1}, {8, 2}, {6, 6},
	{-6, 6}, {8, 3}, {5, 7}, {-5, 7}, {7, 5}, {-7, 5}, {8, 4}, {6, 7},
	{-6, 7}, {7, 6}, {-7, 6}, {8, 5}, {7, 7}, {-7, 7}, {8, 6}, {8, 7},
}

type vp8lBitReader struct {
	data  []byte
	pos   int
	acc   uint64
	nbits uint32
}

func newVP8LBitReader(data []byte) *vp8lBitReader {
	br := &vp8lBitReader{data: data}
	br.refill()
	return br
}

func (br *vp8lBitReader) refill() {
	for br.nbits <= 56 && br.pos < len(br.data) {
		br.acc |= uint64(br.data[br.pos]) << br.nbits
		br.pos++
		br.nbits += 8
	}
}

func (br *vp8lBitReader) peek(n uint32) uint32 {
	if br.nbits < n {
		br.refill()
	}
	return uint32(br.acc & ((1 << n) - 1))
}

func (br *vp8lBitReader) consume(n uint32) error {
	if n > br.nbits {
		br.refill()
		if n > br.nbits {
			return newUnexpectedEof()
		}
	}
	br.acc >>= n
	br.nbits -= n
	return nil
}

func (br *vp8lBitReader) read(n uint32) (uint32, error) {
	if n == 0 {
		return 0, nil
	}
	val := br.peek(n)
	if err := br.consume(n); err != nil {
		return 0, err
	}
	return val, nil
}

func (br *vp8lBitReader) readFlag() (bool, error) {
	val, err := br.read(1)
	return val == 1, err
}

type prefixEntry struct {
	bits   uint8
	symbol uint16
}

type prefixCode struct {
	singleSymbol *uint16
	root         []prefixEntry
	subTables    [][]prefixEntry
}

func newPrefixCode(lengths []uint8) (*prefixCode, error) {
	alphabet := len(lengths)
	usedCount := 0
	lastUsed := 0
	for sym, l := range lengths {
		if l > 0 {
			if l > uint8(maxCodeLength) {
				return nil, newInvalidHeader("code length too long")
			}
			usedCount++
			lastUsed = sym
		}
	}

	if usedCount == 0 {
		return nil, newInvalidHeader("empty prefix code")
	}
	if usedCount == 1 {
		sym := uint16(lastUsed)
		return &prefixCode{singleSymbol: &sym}, nil
	}

	// Canonical codes
	type symLen struct {
		symbol int
		length uint8
	}
	var sorted []symLen
	for s, l := range lengths {
		if l > 0 {
			sorted = append(sorted, symLen{symbol: s, length: l})
		}
	}
	sort.Slice(sorted, func(i, j int) bool {
		if sorted[i].length == sorted[j].length {
			return sorted[i].symbol < sorted[j].symbol
		}
		return sorted[i].length < sorted[j].length
	})

	canonicalCodes := make([]uint32, alphabet)
	var nextCode uint32
	var prevLen uint8
	var totalWeight uint32

	for _, item := range sorted {
		l := item.length
		nextCode <<= (l - prevLen)
		prevLen = l
		canonicalCodes[item.symbol] = nextCode
		nextCode++
		totalWeight += 1 << (maxCodeLength - int(l))
	}
	if totalWeight != (1 << maxCodeLength) {
		return nil, newInvalidHeader("incomplete prefix code")
	}

	rootSize := 1 << rootBits
	root := make([]prefixEntry, rootSize)
	var subTables [][]prefixEntry

	for _, item := range sorted {
		sym := uint16(item.symbol)
		l := item.length
		code := canonicalCodes[sym]
		revCode := reverseBits(code, l)

		if l <= uint8(rootBits) {
			step := 1 << l
			for i := int(revCode); i < rootSize; i += step {
				root[i] = prefixEntry{bits: l, symbol: sym}
			}
		} else {
			rootPrefix := int(revCode & ((1 << rootBits) - 1))
			subBits := l - uint8(rootBits)
			subCode := int(revCode >> rootBits)

			var subIdx int
			if root[rootPrefix].bits == 0 {
				subIdx = len(subTables)
				subTables = append(subTables, make([]prefixEntry, 1<<(maxCodeLength-int(rootBits))))
				root[rootPrefix] = prefixEntry{
					bits:   uint8(rootBits) + 1,
					symbol: uint16(subIdx),
				}
			} else {
				subIdx = int(root[rootPrefix].symbol)
			}

			step := 1 << subBits
			for i := subCode; i < len(subTables[subIdx]); i += step {
				subTables[subIdx][i] = prefixEntry{bits: l, symbol: sym}
			}
		}
	}

	return &prefixCode{
		root:      root,
		subTables: subTables,
	}, nil
}

func (c *prefixCode) readSymbol(br *vp8lBitReader) (uint16, error) {
	if c.singleSymbol != nil {
		return *c.singleSymbol, nil
	}
	peek := br.peek(rootBits)
	entry := c.root[peek]
	if entry.bits <= uint8(rootBits) {
		if entry.bits == 0 {
			return 0, newInvalidHeader("invalid prefix code lookup")
		}
		if err := br.consume(uint32(entry.bits)); err != nil {
			return 0, err
		}
		return entry.symbol, nil
	}

	if err := br.consume(rootBits); err != nil {
		return 0, err
	}
	subIdx := int(entry.symbol)
	subPeek := br.peek(uint32(maxCodeLength) - rootBits)
	if subIdx >= len(c.subTables) || int(subPeek) >= len(c.subTables[subIdx]) {
		return 0, newInvalidHeader("invalid sub-table lookup")
	}
	subEntry := c.subTables[subIdx][subPeek]
	if subEntry.bits == 0 {
		return 0, newInvalidHeader("invalid sub-table code")
	}
	if err := br.consume(uint32(subEntry.bits) - rootBits); err != nil {
		return 0, err
	}
	return subEntry.symbol, nil
}

func reverseBits(val uint32, n uint8) uint32 {
	var res uint32
	for i := uint8(0); i < n; i++ {
		res = (res << 1) | ((val >> i) & 1)
	}
	return res
}

type prefixGroup struct {
	green    *prefixCode
	red      *prefixCode
	blue     *prefixCode
	alpha    *prefixCode
	distance *prefixCode
}

type vp8lDecoder struct {
	br           *vp8lBitReader
	tableEntries int
}

func (dec *vp8lDecoder) readPrefixCode(alphabetSize int) (*prefixCode, error) {
	isSimple, err := dec.br.readFlag()
	if err != nil {
		return nil, err
	}
	if isSimple {
		numSymbols, err := dec.br.read(1)
		if err != nil {
			return nil, err
		}
		numSymbols++ // 1 or 2 symbols
		firstSymBits := uint32(1)
		if err == nil {
			is8, err := dec.br.read(1)
			if err != nil {
				return nil, err
			}
			if is8 == 1 {
				firstSymBits = 8
			}
		}
		s0, err := dec.br.read(firstSymBits)
		if err != nil {
			return nil, err
		}
		lengths := make([]uint8, alphabetSize)
		if int(s0) >= alphabetSize {
			return nil, newInvalidHeader("simple code symbol out of range")
		}
		if numSymbols == 1 {
			lengths[s0] = 1
		} else {
			s1, err := dec.br.read(8)
			if err != nil {
				return nil, err
			}
			if int(s1) >= alphabetSize {
				return nil, newInvalidHeader("simple code symbol out of range")
			}
			lengths[s0] = 1
			lengths[s1] = 1
		}
		return newPrefixCode(lengths)
	}

	// Normal prefix code: code lengths coded with a code-length code
	numCodeLengths, err := dec.br.read(4)
	if err != nil {
		return nil, err
	}
	numCodeLengths += 4
	clLengths := make([]uint8, 19)
	for i := 0; i < int(numCodeLengths); i++ {
		l, err := dec.br.read(3)
		if err != nil {
			return nil, err
		}
		clLengths[codeLengthOrder[i]] = uint8(l)
	}
	clCode, err := newPrefixCode(clLengths)
	if err != nil {
		return nil, err
	}

	lengths := make([]uint8, alphabetSize)
	maxSymbols := alphabetSize
	hasMaxSymbols, err := dec.br.readFlag()
	if err != nil {
		return nil, err
	}
	if hasMaxSymbols {
		lenBits, err := dec.br.read(3)
		if err != nil {
			return nil, err
		}
		n, err := dec.br.read(2 + 2*lenBits)
		if err != nil {
			return nil, err
		}
		maxSymbols = int(n + 2)
		if maxSymbols > alphabetSize {
			return nil, newInvalidHeader("max symbols exceeds alphabet size")
		}
	}

	var prevLen uint8 = 8
	i := 0
	for i < alphabetSize {
		if i >= maxSymbols {
			break
		}
		sym, err := clCode.readSymbol(dec.br)
		if err != nil {
			return nil, err
		}
		if sym < 16 {
			lengths[i] = uint8(sym)
			if sym != 0 {
				prevLen = uint8(sym)
			}
			i++
		} else if sym == 16 {
			repeat, err := dec.br.read(2)
			if err != nil {
				return nil, err
			}
			count := int(repeat + 3)
			for c := 0; c < count && i < alphabetSize; c++ {
				lengths[i] = prevLen
				i++
			}
		} else if sym == 17 {
			repeat, err := dec.br.read(3)
			if err != nil {
				return nil, err
			}
			count := int(repeat + 3)
			for c := 0; c < count && i < alphabetSize; c++ {
				lengths[i] = 0
				i++
			}
		} else if sym == 18 {
			repeat, err := dec.br.read(7)
			if err != nil {
				return nil, err
			}
			count := int(repeat + 11)
			for c := 0; c < count && i < alphabetSize; c++ {
				lengths[i] = 0
				i++
			}
		}
	}

	return newPrefixCode(lengths)
}

func (dec *vp8lDecoder) readPrefixGroup(cacheSize int) (*prefixGroup, error) {
	green, err := dec.readPrefixCode(literalAlphabet + lengthPrefixes + cacheSize)
	if err != nil {
		return nil, err
	}
	red, err := dec.readPrefixCode(literalAlphabet)
	if err != nil {
		return nil, err
	}
	blue, err := dec.readPrefixCode(literalAlphabet)
	if err != nil {
		return nil, err
	}
	alpha, err := dec.readPrefixCode(literalAlphabet)
	if err != nil {
		return nil, err
	}
	dist, err := dec.readPrefixCode(distanceAlphabet)
	if err != nil {
		return nil, err
	}
	return &prefixGroup{
		green:    green,
		red:      red,
		blue:     blue,
		alpha:    alpha,
		distance: dist,
	}, nil
}

type vp8lTransform interface {
	apply(pixels []uint32, width, height int) ([]uint32, error)
}

type predictorTransform struct {
	bits uint32
	data []uint32
}

func (t *predictorTransform) apply(pixels []uint32, width, height int) ([]uint32, error) {
	inversePredictor(pixels, width, height, t.bits, t.data)
	return pixels, nil
}

type crossColorTransform struct {
	bits uint32
	data []uint32
}

func (t *crossColorTransform) apply(pixels []uint32, width, height int) ([]uint32, error) {
	inverseCrossColor(pixels, width, t.bits, t.data)
	return pixels, nil
}

type subtractGreenTransform struct{}

func (t *subtractGreenTransform) apply(pixels []uint32, width, height int) ([]uint32, error) {
	for i := range pixels {
		p := pixels[i]
		green := (p >> 8) & 0xFF
		red := ((p >> 16) + green) & 0xFF
		blue := (p + green) & 0xFF
		pixels[i] = (p & 0xFF00FF00) | (red << 16) | blue
	}
	return pixels, nil
}

type colorIndexingTransform struct {
	packedWidth int
	bits        uint32
	palette     []uint32
}

func (t *colorIndexingTransform) apply(pixels []uint32, width, height int) ([]uint32, error) {
	return inverseColorIndexing(pixels, t.packedWidth, width, height, t.bits, t.palette), nil
}

func (dec *vp8lDecoder) decodeImageStream(width, height int) ([]uint32, error) {
	var transforms []vp8lTransform
	currentWidth := width
	currentHeight := height

	// Read transforms
	for {
		hasTransform, err := dec.br.readFlag()
		if err != nil {
			return nil, err
		}
		if !hasTransform {
			break
		}
		tType, err := dec.br.read(2)
		if err != nil {
			return nil, err
		}
		switch tType {
		case 0: // Predictor
			bits, err := dec.br.read(3)
			if err != nil {
				return nil, err
			}
			bits += 2
			blockW := divRoundUp(currentWidth, bits)
			blockH := divRoundUp(currentHeight, bits)
			data, err := dec.decodeImageStream(blockW, blockH)
			if err != nil {
				return nil, err
			}
			transforms = append(transforms, &predictorTransform{bits: bits, data: data})

		case 1: // Cross-color
			bits, err := dec.br.read(3)
			if err != nil {
				return nil, err
			}
			bits += 2
			blockW := divRoundUp(currentWidth, bits)
			blockH := divRoundUp(currentHeight, bits)
			data, err := dec.decodeImageStream(blockW, blockH)
			if err != nil {
				return nil, err
			}
			transforms = append(transforms, &crossColorTransform{bits: bits, data: data})

		case 2: // Subtract green
			transforms = append(transforms, &subtractGreenTransform{})

		case 3: // Color indexing
			numColorsRaw, err := dec.br.read(8)
			if err != nil {
				return nil, err
			}
			numColors := int(numColorsRaw) + 1
			palette, err := dec.decodeImageStream(numColors, 1)
			if err != nil {
				return nil, err
			}
			var bits uint32
			if numColors <= 2 {
				bits = 3
			} else if numColors <= 4 {
				bits = 2
			} else if numColors <= 16 {
				bits = 1
			} else {
				bits = 0
			}
			packedWidth := divRoundUp(currentWidth, bits)
			transforms = append(transforms, &colorIndexingTransform{
				packedWidth: packedWidth,
				bits:        bits,
				palette:     palette,
			})
			currentWidth = packedWidth
		}
	}

	// Read color cache size
	hasCache, err := dec.br.readFlag()
	if err != nil {
		return nil, err
	}
	cacheBits := uint32(0)
	cacheSize := 0
	if hasCache {
		b, err := dec.br.read(4)
		if err != nil {
			return nil, err
		}
		if b < 1 || b > 11 {
			return nil, newInvalidHeader("color cache bits out of range")
		}
		cacheBits = b
		cacheSize = 1 << b
	}

	// Read meta prefix codes
	hasMeta, err := dec.br.readFlag()
	if err != nil {
		return nil, err
	}
	var metaBits uint32
	var metaWidth int
	var metaImage []uint32
	numGroups := 1

	if hasMeta {
		b, err := dec.br.read(3)
		if err != nil {
			return nil, err
		}
		metaBits = b + 2
		metaWidth = divRoundUp(currentWidth, metaBits)
		metaHeight := divRoundUp(currentHeight, metaBits)
		metaImage, err = dec.decodeImageStream(metaWidth, metaHeight)
		if err != nil {
			return nil, err
		}
		maxGroup := 0
		for _, p := range metaImage {
			g := int((p >> 8) & 0xFFFF)
			if g > maxGroup {
				maxGroup = g
			}
		}
		numGroups = maxGroup + 1
	}

	groups := make([]*prefixGroup, numGroups)
	for g := 0; g < numGroups; g++ {
		grp, err := dec.readPrefixGroup(cacheSize)
		if err != nil {
			return nil, err
		}
		groups[g] = grp
	}

	// Decode pixels
	count := currentWidth * currentHeight
	pixels := make([]uint32, count)
	colorCache := make([]uint32, cacheSize)

	insertCache := func(p uint32) {
		if cacheSize > 0 {
			idx := (0x1E35A7BD * p) >> (32 - cacheBits)
			colorCache[idx] = p
		}
	}

	pixelIdx := 0
	for pixelIdx < count {
		grp := groups[0]
		if hasMeta {
			x := pixelIdx % currentWidth
			y := pixelIdx / currentWidth
			metaIdx := (y>>metaBits)*metaWidth + (x >> metaBits)
			if metaIdx < len(metaImage) {
				gIdx := int((metaImage[metaIdx] >> 8) & 0xFFFF)
				if gIdx < len(groups) {
					grp = groups[gIdx]
				}
			}
		}

		sym, err := grp.green.readSymbol(dec.br)
		if err != nil {
			return nil, err
		}

		if int(sym) < literalAlphabet {
			// Literal green
			green := uint8(sym)
			redSym, err := grp.red.readSymbol(dec.br)
			if err != nil {
				return nil, err
			}
			blueSym, err := grp.blue.readSymbol(dec.br)
			if err != nil {
				return nil, err
			}
			alphaSym, err := grp.alpha.readSymbol(dec.br)
			if err != nil {
				return nil, err
			}
			p := (uint32(alphaSym) << 24) | (uint32(redSym) << 16) | (uint32(green) << 8) | uint32(blueSym)
			pixels[pixelIdx] = p
			insertCache(p)
			pixelIdx++

		} else if int(sym) < literalAlphabet+lengthPrefixes {
			// LZ77 length + distance
			lenPrefix := int(sym) - literalAlphabet
			length, err := prefixValue(dec.br, lenPrefix)
			if err != nil {
				return nil, err
			}

			distSym, err := grp.distance.readSymbol(dec.br)
			if err != nil {
				return nil, err
			}
			distVal, err := prefixValue(dec.br, int(distSym))
			if err != nil {
				return nil, err
			}
			dist := planeDistance(currentWidth, distVal)

			if dist < 1 || pixelIdx < dist {
				return nil, newInvalidHeader("backward reference before image start")
			}

			for k := 0; k < length && pixelIdx < count; k++ {
				p := pixels[pixelIdx-dist]
				pixels[pixelIdx] = p
				insertCache(p)
				pixelIdx++
			}

		} else {
			// Color cache hit
			cacheIdx := int(sym) - (literalAlphabet + lengthPrefixes)
			if cacheIdx >= cacheSize {
				return nil, newInvalidHeader("color cache index out of range")
			}
			p := colorCache[cacheIdx]
			pixels[pixelIdx] = p
			insertCache(p)
			pixelIdx++
		}
	}

	// Apply inverse transforms in reverse order
	for i := len(transforms) - 1; i >= 0; i-- {
		var err error
		pixels, err = transforms[i].apply(pixels, width, height)
		if err != nil {
			return nil, err
		}
	}

	return pixels, nil
}

func prefixValue(br *vp8lBitReader, prefix int) (int, error) {
	if prefix < 4 {
		return prefix + 1, nil
	}
	extraBits := uint32((prefix - 2) >> 1)
	offset := (2 + (prefix & 1)) << extraBits
	extra, err := br.read(extraBits)
	if err != nil {
		return 0, err
	}
	return int(offset+int(extra)) + 1, nil
}

func planeDistance(width int, distCode int) int {
	if distCode <= 120 {
		entry := distanceMap[distCode-1]
		dx := int(entry[0])
		dy := int(entry[1])
		d := dy*width + dx
		if d < 1 {
			return 1
		}
		return d
	}
	return distCode - 120
}

func divRoundUp(n int, bits uint32) int {
	return (n + (1 << bits) - 1) >> bits
}

func addPixels(a, b uint32) uint32 {
	ag := (a & 0xFF00FF00) + (b & 0xFF00FF00)
	rb := (a & 0x00FF00FF) + (b & 0x00FF00FF)
	return (ag & 0xFF00FF00) | (rb & 0x00FF00FF)
}

func average2(a, b uint32) uint32 {
	return (((a ^ b) & 0xFEFEFEFE) >> 1) + (a & b)
}

func channel(p uint32, shift uint32) int32 {
	return int32((p >> shift) & 0xFF)
}

func selectPredictor(left, top, topLeft uint32) uint32 {
	var toLeft, toTop int32
	for _, shift := range []uint32{24, 16, 8, 0} {
		l := channel(left, shift)
		t := channel(top, shift)
		tl := channel(topLeft, shift)
		toLeft += absInt32(t - tl)
		toTop += absInt32(l - tl)
	}
	if toLeft < toTop {
		return left
	}
	return top
}

func clampAddSubtractFull(a, b, c uint32) uint32 {
	var out uint32
	for _, shift := range []uint32{24, 16, 8, 0} {
		v := channel(a, shift) + channel(b, shift) - channel(c, shift)
		if v < 0 {
			v = 0
		} else if v > 255 {
			v = 255
		}
		out |= uint32(v) << shift
	}
	return out
}

func clampAddSubtractHalf(a, b uint32) uint32 {
	var out uint32
	for _, shift := range []uint32{24, 16, 8, 0} {
		ca := channel(a, shift)
		cb := channel(b, shift)
		v := ca + (ca-cb)/2
		if v < 0 {
			v = 0
		} else if v > 255 {
			v = 255
		}
		out |= uint32(v) << shift
	}
	return out
}

func predict(mode uint32, left, top, topLeft, topRight uint32) uint32 {
	switch mode {
	case 0:
		return 0xFF000000
	case 1:
		return left
	case 2:
		return top
	case 3:
		return topRight
	case 4:
		return topLeft
	case 5:
		return average2(average2(left, topRight), top)
	case 6:
		return average2(left, topLeft)
	case 7:
		return average2(left, top)
	case 8:
		return average2(topLeft, top)
	case 9:
		return average2(top, topRight)
	case 10:
		return average2(average2(left, topLeft), average2(top, topRight))
	case 11:
		return selectPredictor(left, top, topLeft)
	case 12:
		return clampAddSubtractFull(left, top, topLeft)
	case 13:
		return clampAddSubtractHalf(average2(left, top), topLeft)
	default:
		return 0xFF000000
	}
}

func inversePredictor(pixels []uint32, width, height int, bits uint32, data []uint32) {
	if len(pixels) == 0 {
		return
	}
	blocksWide := divRoundUp(width, bits)
	pixels[0] = addPixels(pixels[0], 0xFF000000)
	for x := 1; x < width; x++ {
		pixels[x] = addPixels(pixels[x], pixels[x-1])
	}
	for y := 1; y < height; y++ {
		row := y * width
		pixels[row] = addPixels(pixels[row], pixels[row-width])
		blockRow := (y >> bits) * blocksWide
		for x := 1; x < width; x++ {
			mode := (data[blockRow+(x>>bits)] >> 8) & 0x0F
			i := row + x
			left := pixels[i-1]
			top := pixels[i-width]
			topLeft := pixels[i-width-1]
			topRight := pixels[i-width+1]
			pred := predict(mode, left, top, topLeft, topRight)
			pixels[i] = addPixels(pixels[i], pred)
		}
	}
}

func colorDelta(t, c uint8) int32 {
	return (int32(int8(t)) * int32(int8(c))) >> 5
}

func inverseCrossColor(pixels []uint32, width int, bits uint32, data []uint32) {
	blocksWide := divRoundUp(width, bits)
	for i := range pixels {
		x := i % width
		y := i / width
		element := data[(y>>bits)*blocksWide+(x>>bits)]
		g2r := uint8(element)
		g2b := uint8(element >> 8)
		r2b := uint8(element >> 16)
		p := pixels[i]
		green := uint8(p >> 8)
		red := int32((p >> 16) & 0xFF)
		blue := int32(p & 0xFF)

		red += colorDelta(g2r, green)
		redByte := uint8(red & 0xFF)
		blue += colorDelta(g2b, green)
		blue += colorDelta(r2b, redByte)
		pixels[i] = (p & 0xFF00FF00) | (uint32(redByte) << 16) | uint32(blue&0xFF)
	}
}

func inverseColorIndexing(packed []uint32, packedWidth, width, height int, bits uint32, palette []uint32) []uint32 {
	out := make([]uint32, width*height)
	perByteBits := 8 >> bits
	indexMask := uint32((1 << perByteBits) - 1)
	xMask := (1 << bits) - 1
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			source := packed[y*packedWidth+(x>>bits)]
			green := (source >> 8) & 0xFF
			index := (green >> (uint32(x&xMask) * uint32(perByteBits))) & indexMask
			if int(index) < len(palette) {
				out[y*width+x] = palette[index]
			}
		}
	}
	return out
}

// BitWriter for VP8L encoding
type vp8lBitWriter struct {
	out   []byte
	acc   uint64
	count uint32
}

func (w *vp8lBitWriter) put(value uint32, count uint32) {
	mask := uint64(0)
	if count > 0 {
		mask = (1 << count) - 1
	}
	w.acc |= (uint64(value) & mask) << w.count
	w.count += count
	for w.count >= 8 {
		w.out = append(w.out, uint8(w.acc))
		w.acc >>= 8
		w.count -= 8
	}
}

func (w *vp8lBitWriter) putCode(code uint32, l uint8) {
	for i := int(l) - 1; i >= 0; i-- {
		w.put((code>>i)&1, 1)
	}
}

func (w *vp8lBitWriter) finish() []byte {
	if w.count > 0 {
		w.out = append(w.out, uint8(w.acc))
	}
	return w.out
}

type encodeCode struct {
	lengths []uint8
	codes   []uint32
}

func newEncodeCode(size int) *encodeCode {
	short := uint32(0)
	for (1 << (short + 1)) <= size {
		short++
	}
	a := (1 << (short + 1)) - size
	lengths := make([]uint8, size)
	for s := 0; s < size; s++ {
		if s < a {
			lengths[s] = uint8(short)
		} else {
			lengths[s] = uint8(short + 1)
		}
	}
	codes := make([]uint32, size)
	var next uint32
	var prevLen uint8

	type symOrder struct {
		symbol int
		length uint8
	}
	order := make([]symOrder, size)
	for s := 0; s < size; s++ {
		order[s] = symOrder{symbol: s, length: lengths[s]}
	}
	sort.Slice(order, func(i, j int) bool {
		if order[i].length == order[j].length {
			return order[i].symbol < order[j].symbol
		}
		return order[i].length < order[j].length
	})

	for _, item := range order {
		s := item.symbol
		next <<= (lengths[s] - prevLen)
		prevLen = lengths[s]
		codes[s] = next
		next++
	}
	return &encodeCode{lengths: lengths, codes: codes}
}

func (c *encodeCode) writeHeader(w *vp8lBitWriter) {
	short := c.lengths[0]
	longUsed := false
	for _, l := range c.lengths {
		if l != short {
			longUsed = true
			break
		}
	}
	zero := short
	one := short + 1
	if !longUsed {
		zero = 0
		one = short
	}
	w.put(0, 1)  // normal code
	w.put(15, 4) // 19 - 4 = 15
	for _, sym := range codeLengthOrder {
		used := (sym == int(zero) || sym == int(one))
		if used {
			w.put(1, 3)
		} else {
			w.put(0, 3)
		}
	}
	w.put(0, 1) // no max symbols limit
	for _, l := range c.lengths {
		if l != zero {
			w.put(1, 1)
		} else {
			w.put(0, 1)
		}
	}
}

func (c *encodeCode) put(w *vp8lBitWriter, symbol int) {
	w.putCode(c.codes[symbol], c.lengths[symbol])
}

func encodePrefix(value int) (sym int, extraBits uint32, extraVal uint32) {
	d := uint32(value - 1)
	if d < 4 {
		return int(d), 0, 0
	}
	high := uint32(31 - bits.LeadingZeros32(d))
	second := (d >> (high - 1)) & 1
	nBits := high - 1
	return int(2*high + second), nBits, d & ((1 << nBits) - 1)
}

func encodeVP8L(width, height uint32, rgba []byte) []byte {
	w := width
	if w < 1 {
		w = 1
	} else if w > 16384 {
		w = 16384
	}
	h := height
	if h < 1 {
		h = 1
	} else if h > 16384 {
		h = 16384
	}
	count := int(w) * int(h)

	var bw vp8lBitWriter
	bw.put(uint32(vp8lSignature), 8)
	bw.put(w-1, 14)
	bw.put(h-1, 14)

	hasAlpha := false
	for i := 0; i < count && i*4+3 < len(rgba); i++ {
		if rgba[i*4+3] != 255 {
			hasAlpha = true
			break
		}
	}
	if hasAlpha {
		bw.put(1, 1)
	} else {
		bw.put(0, 1)
	}
	bw.put(0, 3) // version 0

	// Subtract-green transform
	bw.put(1, 1) // has transform
	bw.put(2, 2) // subtract green
	bw.put(0, 1) // end of transforms

	argb := make([]uint32, count)
	for i := 0; i < count; i++ {
		base := i * 4
		var rRaw, g, bRaw, a uint8 = 0, 0, 0, 255
		if base < len(rgba) {
			rRaw = rgba[base]
		}
		if base+1 < len(rgba) {
			g = rgba[base+1]
		}
		if base+2 < len(rgba) {
			bRaw = rgba[base+2]
		}
		if base+3 < len(rgba) {
			a = rgba[base+3]
		}
		r := rRaw - g
		b := bRaw - g
		argb[i] = (uint32(a) << 24) | (uint32(r) << 16) | (uint32(g) << 8) | uint32(b)
	}

	const cacheBits uint32 = 6
	const cacheSize int = 1 << cacheBits
	bw.put(1, 1) // has cache
	bw.put(cacheBits, 4)
	bw.put(0, 1) // single group

	green := newEncodeCode(literalAlphabet + lengthPrefixes + cacheSize)
	red := newEncodeCode(literalAlphabet)
	blue := newEncodeCode(literalAlphabet)
	alpha := newEncodeCode(literalAlphabet)
	dist := newEncodeCode(distanceAlphabet)

	green.writeHeader(&bw)
	red.writeHeader(&bw)
	blue.writeHeader(&bw)
	alpha.writeHeader(&bw)
	dist.writeHeader(&bw)

	colorCache := make([]uint32, cacheSize)
	var cacheUsed uint64
	insert := func(px uint32) {
		idx := (0x1E35A7BD * px) >> (32 - cacheBits)
		colorCache[idx] = px
		cacheUsed |= 1 << idx
	}

	stride := int(w)
	n := len(argb)
	i := 0
	for i < n {
		px := argb[i]
		bestLen := 0
		bestDist := 1
		for _, candD := range []int{1, stride} {
			if candD > 0 && i >= candD && argb[i-candD] == px {
				run := 1
				for i+run < n && run < 4096 && argb[i+run] == argb[i+run-candD] {
					run++
				}
				if run > bestLen {
					bestLen = run
					bestDist = candD
				}
			}
		}

		if bestLen > 0 {
			lsym, lnb, lextra := encodePrefix(bestLen)
			green.put(&bw, literalAlphabet+lsym)
			bw.put(lextra, lnb)
			dsym, dnb, dextra := encodePrefix(bestDist + 120)
			dist.put(&bw, dsym)
			bw.put(dextra, dnb)
			for k := 0; k < bestLen; k++ {
				insert(argb[i+k])
			}
			i += bestLen
			continue
		}

		idx := (0x1E35A7BD * px) >> (32 - cacheBits)
		if (cacheUsed&(1<<idx)) != 0 && colorCache[idx] == px {
			green.put(&bw, literalAlphabet+lengthPrefixes+int(idx))
			i++
			continue
		}

		green.put(&bw, int((px>>8)&0xFF))
		red.put(&bw, int((px>>16)&0xFF))
		blue.put(&bw, int(px&0xFF))
		alpha.put(&bw, int(px>>24))
		insert(px)
		i++
	}

	return bw.finish()
}

func decodeVP8L(data []byte) (width, height uint32, argb []uint32, err error) {
	if len(data) < 5 {
		return 0, 0, nil, newUnexpectedEof()
	}
	if data[0] != vp8lSignature {
		return 0, 0, nil, newInvalidHeader("invalid VP8L signature")
	}
	bitsVal := binary.LittleEndian.Uint32(data[1:5])
	w := (bitsVal & 0x3FFF) + 1
	h := ((bitsVal >> 14) & 0x3FFF) + 1
	if (bitsVal >> 29) != 0 {
		return 0, 0, nil, newUnsupportedFormat("VP8L version is not 0")
	}
	if err := ValidateDimensions(w, h); err != nil {
		return 0, 0, nil, err
	}

	dec := &vp8lDecoder{
		br: newVP8LBitReader(data[5:]),
	}
	pixels, err := dec.decodeImageStream(int(w), int(h))
	if err != nil {
		return 0, 0, nil, err
	}
	return w, h, pixels, nil
}

func decodeVP8LHeaderless(data []byte, width, height uint32) ([]uint32, error) {
	if err := ValidateDimensions(width, height); err != nil {
		return nil, err
	}
	dec := &vp8lDecoder{
		br: newVP8LBitReader(data),
	}
	return dec.decodeImageStream(int(width), int(height))
}
