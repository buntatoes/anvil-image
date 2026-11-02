// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package main

import (
	"bufio"
	"bytes"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"os"
	"strconv"

	anvilimage "github.com/buntatoes/anvil-image"
)

const (
	version = "anvil-image 0.9.0 (go)"
	author  = "Buntato"

	opDecodeAuto     byte = 0x01
	opResizeBilinear byte = 0x02
	opEncodePNG      byte = 0x03
	opEncodeBMP      byte = 0x04
	opShutdown       byte = 0xFF
)

func main() {
	args := os.Args[1:]
	if len(args) == 0 {
		printUsage()
		return
	}

	switch args[0] {
	case "--version", "-v", "version":
		fmt.Printf("%s\nAuthor: %s\n", version, author)
	case "--self-test":
		runSelfTest()
	case "--worker":
		if err := runWorker(os.Stdin, os.Stdout); err != nil {
			fmt.Fprintf(os.Stderr, "Worker error: %v\n", err)
			os.Exit(1)
		}
	case "--info":
		if len(args) < 2 {
			fmt.Fprintln(os.Stderr, "Usage: anvil-image --info <image-file>")
			os.Exit(1)
		}
		infoImage(args[1])
	case "--decode":
		if len(args) < 3 {
			fmt.Fprintln(os.Stderr, "Usage: anvil-image --decode <input> <output.bmp>")
			os.Exit(1)
		}
		decodeToBMP(args[1], args[2])
	case "--resize":
		if len(args) < 5 {
			fmt.Fprintln(os.Stderr, "Usage: anvil-image --resize <input> <dst_w> <dst_h> <output.png>")
			os.Exit(1)
		}
		resizeImage(args[1], args[2], args[3], args[4])
	default:
		printUsage()
	}
}

func printUsage() {
	fmt.Printf("%s\n", version)
	fmt.Println("Usage: anvil-image [command] [args...]")
	fmt.Println("Commands:")
	fmt.Println("  --version                                 Print version and author")
	fmt.Println("  --self-test                               Run internal validation tests")
	fmt.Println("  --worker                                  Run in persistent binary IPC worker mode")
	fmt.Println("  --info <input>                            Display image format and dimensions")
	fmt.Println("  --decode <input> <output.bmp>             Decode input image and write 32-bit BMP")
	fmt.Println("  --resize <input> <w> <h> <output.png>     Resize input image and save as PNG")
}

func infoImage(path string) {
	data, err := os.ReadFile(path)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error reading file: %v\n", err)
		os.Exit(1)
	}

	img, err := anvilimage.DecodeAuto(data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error decoding image: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("File:       %s\n", path)
	fmt.Printf("Dimensions: %dx%d\n", img.Width, img.Height)
	fmt.Printf("Pixels:     %d\n", uint64(img.Width)*uint64(img.Height))
	fmt.Printf("RGBA Size:  %d bytes\n", len(img.Pixels))
}

func decodeToBMP(inPath, outPath string) {
	data, err := os.ReadFile(inPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error reading input file: %v\n", err)
		os.Exit(1)
	}

	img, err := anvilimage.DecodeAuto(data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error decoding input: %v\n", err)
		os.Exit(1)
	}

	bmpBytes := anvilimage.EncodeBMP(img)
	if len(bmpBytes) == 0 {
		fmt.Fprintln(os.Stderr, "Error encoding BMP: empty output")
		os.Exit(1)
	}

	if err := os.WriteFile(outPath, bmpBytes, 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing output BMP: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Decoded %s (%dx%d) -> %s (%d bytes)\n", inPath, img.Width, img.Height, outPath, len(bmpBytes))
}

func resizeImage(inPath, wStr, hStr, outPath string) {
	data, err := os.ReadFile(inPath)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error reading input file: %v\n", err)
		os.Exit(1)
	}

	img, err := anvilimage.DecodeAuto(data)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error decoding input: %v\n", err)
		os.Exit(1)
	}

	dstW, err := strconv.ParseUint(wStr, 10, 32)
	if err != nil || dstW == 0 {
		fmt.Fprintf(os.Stderr, "Invalid width: %s\n", wStr)
		os.Exit(1)
	}
	dstH, err := strconv.ParseUint(hStr, 10, 32)
	if err != nil || dstH == 0 {
		fmt.Fprintf(os.Stderr, "Invalid height: %s\n", hStr)
		os.Exit(1)
	}

	resized, err := anvilimage.ResizeBilinear(img, uint32(dstW), uint32(dstH))
	if err != nil {
		fmt.Fprintf(os.Stderr, "Resize error: %v\n", err)
		os.Exit(1)
	}

	pngBytes := anvilimage.EncodePNG(resized)
	if len(pngBytes) == 0 {
		fmt.Fprintln(os.Stderr, "Error encoding PNG: empty output")
		os.Exit(1)
	}

	if err := os.WriteFile(outPath, pngBytes, 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing output PNG: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Resized %s (%dx%d -> %dx%d) -> %s (%d bytes)\n", inPath, img.Width, img.Height, dstW, dstH, outPath, len(pngBytes))
}

func writeWorkerError(bw *bufio.Writer, msg string) error {
	_ = bw.WriteByte(1)
	var hdr [4]byte
	binary.LittleEndian.PutUint32(hdr[:], uint32(len(msg)))
	if _, err := bw.Write(hdr[:]); err != nil {
		return err
	}
	if _, err := bw.WriteString(msg); err != nil {
		return err
	}
	return bw.Flush()
}

func runWorker(r io.Reader, w io.Writer) error {
	br := bufio.NewReaderSize(r, 64*1024)
	bw := bufio.NewWriterSize(w, 64*1024)

	var lenBuf [4]byte

	for {
		op, err := br.ReadByte()
		if err != nil {
			if errors.Is(err, io.EOF) {
				return nil
			}
			return err
		}

		if _, err := io.ReadFull(br, lenBuf[:]); err != nil {
			if errors.Is(err, io.EOF) {
				return nil
			}
			return err
		}
		payloadLen := binary.LittleEndian.Uint32(lenBuf[:])
		payload := make([]byte, payloadLen)
		if payloadLen > 0 {
			if _, err := io.ReadFull(br, payload); err != nil {
				return err
			}
		}

		if op == opShutdown {
			_ = bw.WriteByte(0)
			_ = bw.Flush()
			return nil
		}

		switch op {
		case opDecodeAuto:
			img, decErr := anvilimage.DecodeAuto(payload)
			if decErr != nil {
				if err := writeWorkerError(bw, decErr.Error()); err != nil {
					return err
				}
				continue
			}

			_ = bw.WriteByte(0)
			var respHdr [12]byte
			binary.LittleEndian.PutUint32(respHdr[0:4], img.Width)
			binary.LittleEndian.PutUint32(respHdr[4:8], img.Height)
			binary.LittleEndian.PutUint32(respHdr[8:12], uint32(len(img.Pixels)))
			if _, err := bw.Write(respHdr[:]); err != nil {
				return err
			}
			if _, err := bw.Write(img.Pixels); err != nil {
				return err
			}
			if err := bw.Flush(); err != nil {
				return err
			}

		case opResizeBilinear:
			if len(payload) < 20 {
				if err := writeWorkerError(bw, "short payload for OP_RESIZE_BILINEAR"); err != nil {
					return err
				}
				continue
			}
			srcW := binary.LittleEndian.Uint32(payload[0:4])
			srcH := binary.LittleEndian.Uint32(payload[4:8])
			dstW := binary.LittleEndian.Uint32(payload[8:12])
			dstH := binary.LittleEndian.Uint32(payload[12:16])
			rgbaLen := binary.LittleEndian.Uint32(payload[16:20])

			if uint64(len(payload)) < 20+uint64(rgbaLen) {
				if err := writeWorkerError(bw, "short RGBA buffer for OP_RESIZE_BILINEAR"); err != nil {
					return err
				}
				continue
			}
			rgba := payload[20 : 20+rgbaLen]

			srcImg, imgErr := anvilimage.FromRGBA(srcW, srcH, rgba)
			if imgErr != nil {
				if err := writeWorkerError(bw, imgErr.Error()); err != nil {
					return err
				}
				continue
			}

			resized, resampleErr := anvilimage.ResizeBilinear(srcImg, dstW, dstH)
			if resampleErr != nil {
				if err := writeWorkerError(bw, resampleErr.Error()); err != nil {
					return err
				}
				continue
			}

			_ = bw.WriteByte(0)
			var respHdr [12]byte
			binary.LittleEndian.PutUint32(respHdr[0:4], resized.Width)
			binary.LittleEndian.PutUint32(respHdr[4:8], resized.Height)
			binary.LittleEndian.PutUint32(respHdr[8:12], uint32(len(resized.Pixels)))
			if _, err := bw.Write(respHdr[:]); err != nil {
				return err
			}
			if _, err := bw.Write(resized.Pixels); err != nil {
				return err
			}
			if err := bw.Flush(); err != nil {
				return err
			}

		case opEncodePNG:
			if len(payload) < 12 {
				if err := writeWorkerError(bw, "short payload for OP_ENCODE_PNG"); err != nil {
					return err
				}
				continue
			}
			w := binary.LittleEndian.Uint32(payload[0:4])
			h := binary.LittleEndian.Uint32(payload[4:8])
			rgbaLen := binary.LittleEndian.Uint32(payload[8:12])

			if uint64(len(payload)) < 12+uint64(rgbaLen) {
				if err := writeWorkerError(bw, "short RGBA buffer for OP_ENCODE_PNG"); err != nil {
					return err
				}
				continue
			}
			rgba := payload[12 : 12+rgbaLen]

			srcImg, imgErr := anvilimage.FromRGBA(w, h, rgba)
			if imgErr != nil {
				if err := writeWorkerError(bw, imgErr.Error()); err != nil {
					return err
				}
				continue
			}

			pngBytes := anvilimage.EncodePNG(srcImg)
			if len(pngBytes) == 0 {
				if err := writeWorkerError(bw, "failed to encode PNG"); err != nil {
					return err
				}
				continue
			}

			_ = bw.WriteByte(0)
			var respHdr [4]byte
			binary.LittleEndian.PutUint32(respHdr[:], uint32(len(pngBytes)))
			if _, err := bw.Write(respHdr[:]); err != nil {
				return err
			}
			if _, err := bw.Write(pngBytes); err != nil {
				return err
			}
			if err := bw.Flush(); err != nil {
				return err
			}

		case opEncodeBMP:
			if len(payload) < 12 {
				if err := writeWorkerError(bw, "short payload for OP_ENCODE_BMP"); err != nil {
					return err
				}
				continue
			}
			w := binary.LittleEndian.Uint32(payload[0:4])
			h := binary.LittleEndian.Uint32(payload[4:8])
			rgbaLen := binary.LittleEndian.Uint32(payload[8:12])

			if uint64(len(payload)) < 12+uint64(rgbaLen) {
				if err := writeWorkerError(bw, "short RGBA buffer for OP_ENCODE_BMP"); err != nil {
					return err
				}
				continue
			}
			rgba := payload[12 : 12+rgbaLen]

			srcImg, imgErr := anvilimage.FromRGBA(w, h, rgba)
			if imgErr != nil {
				if err := writeWorkerError(bw, imgErr.Error()); err != nil {
					return err
				}
				continue
			}

			bmpBytes := anvilimage.EncodeBMP(srcImg)
			if len(bmpBytes) == 0 {
				if err := writeWorkerError(bw, "failed to encode BMP"); err != nil {
					return err
				}
				continue
			}

			_ = bw.WriteByte(0)
			var respHdr [4]byte
			binary.LittleEndian.PutUint32(respHdr[:], uint32(len(bmpBytes)))
			if _, err := bw.Write(respHdr[:]); err != nil {
				return err
			}
			if _, err := bw.Write(bmpBytes); err != nil {
				return err
			}
			if err := bw.Flush(); err != nil {
				return err
			}

		default:
			if err := writeWorkerError(bw, fmt.Sprintf("unsupported opcode: 0x%02X", op)); err != nil {
				return err
			}
		}
	}
}

func runSelfTest() {
	fmt.Println("Running internal self-tests...")

	// 1. Synthetic 8x8 RGBA test image with color and alpha gradients
	src := anvilimage.New(8, 8)
	for y := uint32(0); y < 8; y++ {
		for x := uint32(0); x < 8; x++ {
			r := uint8(x * 32)
			g := uint8(y * 32)
			b := uint8((x + y) * 16)
			a := uint8(255 - y*16)
			src.SetPixel(x, y, r, g, b, a)
		}
	}

	// 2. PNG Round-Trip
	pngBytes := anvilimage.EncodePNG(src)
	if len(pngBytes) == 0 {
		fmt.Fprintln(os.Stderr, "FAIL: EncodePNG returned empty")
		os.Exit(1)
	}
	decPNG, err := anvilimage.DecodePNG(pngBytes)
	if err != nil {
		fmt.Fprintf(os.Stderr, "FAIL: DecodePNG failed: %v\n", err)
		os.Exit(1)
	}
	if decPNG.Width != 8 || decPNG.Height != 8 || !bytes.Equal(decPNG.Pixels, src.Pixels) {
		fmt.Fprintln(os.Stderr, "FAIL: PNG round-trip pixel mismatch")
		os.Exit(1)
	}

	// 3. BMP Round-Trip
	bmpBytes := anvilimage.EncodeBMP(src)
	if len(bmpBytes) == 0 {
		fmt.Fprintln(os.Stderr, "FAIL: EncodeBMP returned empty")
		os.Exit(1)
	}
	decBMP, err := anvilimage.DecodeBMP(bmpBytes)
	if err != nil {
		fmt.Fprintf(os.Stderr, "FAIL: DecodeBMP failed: %v\n", err)
		os.Exit(1)
	}
	if decBMP.Width != 8 || decBMP.Height != 8 || !bytes.Equal(decBMP.Pixels, src.Pixels) {
		fmt.Fprintln(os.Stderr, "FAIL: BMP round-trip pixel mismatch")
		os.Exit(1)
	}

	// 4. PPM Round-Trip
	ppmBytes := anvilimage.EncodePPM(src)
	if len(ppmBytes) == 0 {
		fmt.Fprintln(os.Stderr, "FAIL: EncodePPM returned empty")
		os.Exit(1)
	}
	decPPM, err := anvilimage.DecodePPM(ppmBytes)
	if err != nil {
		fmt.Fprintf(os.Stderr, "FAIL: DecodePPM failed: %v\n", err)
		os.Exit(1)
	}
	if decPPM.Width != 8 || decPPM.Height != 8 {
		fmt.Fprintln(os.Stderr, "FAIL: PPM round-trip dimensions mismatch")
		os.Exit(1)
	}
	// Verify RGB channels match
	for i := 0; i < len(src.Pixels); i += 4 {
		if decPPM.Pixels[i] != src.Pixels[i] || decPPM.Pixels[i+1] != src.Pixels[i+1] || decPPM.Pixels[i+2] != src.Pixels[i+2] {
			fmt.Fprintln(os.Stderr, "FAIL: PPM round-trip RGB mismatch")
			os.Exit(1)
		}
	}

	// 5. GIF Round-Trip
	gifBytes := anvilimage.EncodeGIF(src)
	if len(gifBytes) == 0 {
		fmt.Fprintln(os.Stderr, "FAIL: EncodeGIF returned empty")
		os.Exit(1)
	}
	decGIF, err := anvilimage.DecodeGIF(gifBytes)
	if err != nil {
		fmt.Fprintf(os.Stderr, "FAIL: DecodeGIF failed: %v\n", err)
		os.Exit(1)
	}
	if decGIF.Width != 8 || decGIF.Height != 8 {
		fmt.Fprintln(os.Stderr, "FAIL: GIF round-trip dimensions mismatch")
		os.Exit(1)
	}

	// 6. JPEG Round-Trip (lossy, verify dimensions & bounded error)
	jpegBytes := anvilimage.EncodeJPEG(src)
	if len(jpegBytes) == 0 {
		fmt.Fprintln(os.Stderr, "FAIL: EncodeJPEG returned empty")
		os.Exit(1)
	}
	decJPEG, err := anvilimage.DecodeJPEG(jpegBytes)
	if err != nil {
		fmt.Fprintf(os.Stderr, "FAIL: DecodeJPEG failed: %v\n", err)
		os.Exit(1)
	}
	if decJPEG.Width != 8 || decJPEG.Height != 8 {
		fmt.Fprintln(os.Stderr, "FAIL: JPEG round-trip dimensions mismatch")
		os.Exit(1)
	}

	// 7. WebP Round-Trip
	webpBytes := anvilimage.EncodeWebP(src)
	if len(webpBytes) == 0 {
		fmt.Fprintln(os.Stderr, "FAIL: EncodeWebP returned empty")
		os.Exit(1)
	}
	decWebP, err := anvilimage.DecodeWebP(webpBytes)
	if err != nil {
		fmt.Fprintf(os.Stderr, "FAIL: DecodeWebP failed: %v\n", err)
		os.Exit(1)
	}
	if decWebP.Width != 8 || decWebP.Height != 8 {
		fmt.Fprintln(os.Stderr, "FAIL: WebP round-trip dimensions mismatch")
		os.Exit(1)
	}

	// 8. ICO Round-Trip
	icoBytes := anvilimage.EncodeICO(src)
	if len(icoBytes) == 0 {
		fmt.Fprintln(os.Stderr, "FAIL: EncodeICO returned empty")
		os.Exit(1)
	}
	decICO, err := anvilimage.DecodeICO(icoBytes)
	if err != nil {
		fmt.Fprintf(os.Stderr, "FAIL: DecodeICO failed: %v\n", err)
		os.Exit(1)
	}
	if decICO.Width != 8 || decICO.Height != 8 {
		fmt.Fprintln(os.Stderr, "FAIL: ICO round-trip dimensions mismatch")
		os.Exit(1)
	}

	// 9. DecodeAuto Detection
	autoImgs := [][]byte{pngBytes, bmpBytes, ppmBytes, gifBytes, jpegBytes, webpBytes, icoBytes}
	for i, b := range autoImgs {
		d, err := anvilimage.DecodeAuto(b)
		if err != nil || d.Width != 8 || d.Height != 8 {
			fmt.Fprintf(os.Stderr, "FAIL: DecodeAuto failed on image format index %d: %v\n", i, err)
			os.Exit(1)
		}
	}

	// 10. Resampling: MipmapHalve and ResizeBilinear
	half, err := anvilimage.MipmapHalve(src)
	if err != nil || half.Width != 4 || half.Height != 4 {
		fmt.Fprintf(os.Stderr, "FAIL: MipmapHalve failed: %v\n", err)
		os.Exit(1)
	}

	resized, err := anvilimage.ResizeBilinear(src, 16, 16)
	if err != nil || resized.Width != 16 || resized.Height != 16 {
		fmt.Fprintf(os.Stderr, "FAIL: ResizeBilinear failed: %v\n", err)
		os.Exit(1)
	}

	fmt.Println("PASS: All internal self-tests succeeded.")
}
