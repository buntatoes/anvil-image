// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 Buntato

package main

import (
	"bytes"
	"encoding/binary"
	"io"
	"testing"

	anvilimage "github.com/buntatoes/anvil-image"
)

func TestWorkerIPC(t *testing.T) {
	// Create a synthetic 4x4 test image
	src := anvilimage.New(4, 4)
	for y := uint32(0); y < 4; y++ {
		for x := uint32(0); x < 4; x++ {
			src.SetPixel(x, y, 200, 100, 50, 255)
		}
	}
	pngData := anvilimage.EncodePNG(src)

	var in bytes.Buffer

	// 1. Send OP_DECODE_AUTO (0x01)
	in.WriteByte(opDecodeAuto)
	var lenBuf [4]byte
	binary.LittleEndian.PutUint32(lenBuf[:], uint32(len(pngData)))
	in.Write(lenBuf[:])
	in.Write(pngData)

	// 2. Send OP_RESIZE_BILINEAR (0x02): 4x4 -> 2x2
	in.WriteByte(opResizeBilinear)
	var reqHdr [20]byte
	binary.LittleEndian.PutUint32(reqHdr[0:4], 4)
	binary.LittleEndian.PutUint32(reqHdr[4:8], 4)
	binary.LittleEndian.PutUint32(reqHdr[8:12], 2)
	binary.LittleEndian.PutUint32(reqHdr[12:16], 2)
	binary.LittleEndian.PutUint32(reqHdr[16:20], uint32(len(src.Pixels)))
	binary.LittleEndian.PutUint32(lenBuf[:], uint32(len(reqHdr)+len(src.Pixels)))
	in.Write(lenBuf[:])
	in.Write(reqHdr[:])
	in.Write(src.Pixels)

	// 3. Send OP_ENCODE_PNG (0x03)
	in.WriteByte(opEncodePNG)
	var encHdr [12]byte
	binary.LittleEndian.PutUint32(encHdr[0:4], 4)
	binary.LittleEndian.PutUint32(encHdr[4:8], 4)
	binary.LittleEndian.PutUint32(encHdr[8:12], uint32(len(src.Pixels)))
	binary.LittleEndian.PutUint32(lenBuf[:], uint32(len(encHdr)+len(src.Pixels)))
	in.Write(lenBuf[:])
	in.Write(encHdr[:])
	in.Write(src.Pixels)

	// 4. Send OP_ENCODE_BMP (0x04)
	in.WriteByte(opEncodeBMP)
	binary.LittleEndian.PutUint32(lenBuf[:], uint32(len(encHdr)+len(src.Pixels)))
	in.Write(lenBuf[:])
	in.Write(encHdr[:])
	in.Write(src.Pixels)

	// 5. Send OP_SHUTDOWN (0xFF)
	in.WriteByte(opShutdown)
	binary.LittleEndian.PutUint32(lenBuf[:], 0)
	in.Write(lenBuf[:])

	var out bytes.Buffer

	if err := runWorker(&in, &out); err != nil {
		t.Fatalf("runWorker failed: %v", err)
	}

	reader := bytes.NewReader(out.Bytes())

	// Read Response 1 (OP_DECODE_AUTO)
	status, err := reader.ReadByte()
	if err != nil || status != 0 {
		t.Fatalf("OP_DECODE_AUTO status error: %v, status=%d", err, status)
	}
	var resp1Hdr [12]byte
	if _, err := io.ReadFull(reader, resp1Hdr[:]); err != nil {
		t.Fatalf("failed to read resp1 header: %v", err)
	}
	w1 := binary.LittleEndian.Uint32(resp1Hdr[0:4])
	h1 := binary.LittleEndian.Uint32(resp1Hdr[4:8])
	rgbaLen1 := binary.LittleEndian.Uint32(resp1Hdr[8:12])
	if w1 != 4 || h1 != 4 || rgbaLen1 != 4*4*4 {
		t.Fatalf("OP_DECODE_AUTO mismatch: %dx%d, len=%d", w1, h1, rgbaLen1)
	}
	buf1 := make([]byte, rgbaLen1)
	if _, err := io.ReadFull(reader, buf1); err != nil {
		t.Fatalf("failed to read decoded rgba: %v", err)
	}

	// Read Response 2 (OP_RESIZE_BILINEAR)
	status, err = reader.ReadByte()
	if err != nil || status != 0 {
		t.Fatalf("OP_RESIZE_BILINEAR status error: %v, status=%d", err, status)
	}
	var resp2Hdr [12]byte
	if _, err := io.ReadFull(reader, resp2Hdr[:]); err != nil {
		t.Fatalf("failed to read resp2 header: %v", err)
	}
	w2 := binary.LittleEndian.Uint32(resp2Hdr[0:4])
	h2 := binary.LittleEndian.Uint32(resp2Hdr[4:8])
	rgbaLen2 := binary.LittleEndian.Uint32(resp2Hdr[8:12])
	if w2 != 2 || h2 != 2 || rgbaLen2 != 2*2*4 {
		t.Fatalf("OP_RESIZE_BILINEAR mismatch: %dx%d, len=%d", w2, h2, rgbaLen2)
	}
	buf2 := make([]byte, rgbaLen2)
	if _, err := io.ReadFull(reader, buf2); err != nil {
		t.Fatalf("failed to read resized rgba: %v", err)
	}

	// Read Response 3 (OP_ENCODE_PNG)
	status, err = reader.ReadByte()
	if err != nil || status != 0 {
		t.Fatalf("OP_ENCODE_PNG status error: %v, status=%d", err, status)
	}
	var pngLenBuf [4]byte
	if _, err := io.ReadFull(reader, pngLenBuf[:]); err != nil {
		t.Fatalf("failed to read png length: %v", err)
	}
	pngLen := binary.LittleEndian.Uint32(pngLenBuf[:])
	if pngLen == 0 {
		t.Fatal("encoded PNG length is 0")
	}
	pngOut := make([]byte, pngLen)
	if _, err := io.ReadFull(reader, pngOut); err != nil {
		t.Fatalf("failed to read encoded png bytes: %v", err)
	}

	// Read Response 4 (OP_ENCODE_BMP)
	status, err = reader.ReadByte()
	if err != nil || status != 0 {
		t.Fatalf("OP_ENCODE_BMP status error: %v, status=%d", err, status)
	}
	var bmpLenBuf [4]byte
	if _, err := io.ReadFull(reader, bmpLenBuf[:]); err != nil {
		t.Fatalf("failed to read bmp length: %v", err)
	}
	bmpLen := binary.LittleEndian.Uint32(bmpLenBuf[:])
	if bmpLen == 0 {
		t.Fatal("encoded BMP length is 0")
	}
	bmpOut := make([]byte, bmpLen)
	if _, err := io.ReadFull(reader, bmpOut); err != nil {
		t.Fatalf("failed to read encoded bmp bytes: %v", err)
	}

	// Read Response 5 (OP_SHUTDOWN)
	status, err = reader.ReadByte()
	if err != nil || status != 0 {
		t.Fatalf("OP_SHUTDOWN status error: %v, status=%d", err, status)
	}
}

func TestWorkerUnknownOpcodeStreamSync(t *testing.T) {
	// Create a synthetic 2x2 test image
	src := anvilimage.New(2, 2)
	src.SetPixel(0, 0, 10, 20, 30, 255)
	src.SetPixel(1, 1, 40, 50, 60, 255)
	pngData := anvilimage.EncodePNG(src)

	var in bytes.Buffer
	var lenBuf [4]byte

	// 1. Send unsupported opcode 0x77 with a 4-byte payload
	in.WriteByte(0x77)
	binary.LittleEndian.PutUint32(lenBuf[:], 4)
	in.Write(lenBuf[:])
	in.Write([]byte{1, 2, 3, 4})

	// 2. Send legitimate OP_DECODE_AUTO (0x01)
	in.WriteByte(opDecodeAuto)
	binary.LittleEndian.PutUint32(lenBuf[:], uint32(len(pngData)))
	in.Write(lenBuf[:])
	in.Write(pngData)

	// 3. Send OP_SHUTDOWN (0xFF)
	in.WriteByte(opShutdown)
	binary.LittleEndian.PutUint32(lenBuf[:], 0)
	in.Write(lenBuf[:])

	var out bytes.Buffer
	if err := runWorker(&in, &out); err != nil {
		t.Fatalf("runWorker failed: %v", err)
	}

	reader := bytes.NewReader(out.Bytes())

	// Response 1 should be an error (status != 0) with "unsupported opcode"
	status1, err := reader.ReadByte()
	if err != nil || status1 == 0 {
		t.Fatalf("expected error status for unsupported opcode, got status=%d, err=%v", status1, err)
	}
	var errLenBuf [4]byte
	if _, err := io.ReadFull(reader, errLenBuf[:]); err != nil {
		t.Fatalf("failed to read error len: %v", err)
	}
	errLen := binary.LittleEndian.Uint32(errLenBuf[:])
	errMsg := make([]byte, errLen)
	if _, err := io.ReadFull(reader, errMsg); err != nil {
		t.Fatalf("failed to read error msg: %v", err)
	}

	// Response 2 should succeed (status == 0) and return the 2x2 decoded image
	status2, err := reader.ReadByte()
	if err != nil || status2 != 0 {
		t.Fatalf("expected status 0 for subsequent OP_DECODE_AUTO, got status=%d, err=%v", status2, err)
	}
	var resp2Hdr [12]byte
	if _, err := io.ReadFull(reader, resp2Hdr[:]); err != nil {
		t.Fatalf("failed to read resp2 header: %v", err)
	}
	w := binary.LittleEndian.Uint32(resp2Hdr[0:4])
	h := binary.LittleEndian.Uint32(resp2Hdr[4:8])
	if w != 2 || h != 2 {
		t.Fatalf("OP_DECODE_AUTO mismatch after unsupported opcode: %dx%d", w, h)
	}
}
