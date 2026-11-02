// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"math"
)

// ResizeBilinear resamples img to newWidth by newHeight with multi-step premultiplied-alpha
// mipmap box pre-filtering and bilinear interpolation.
func ResizeBilinear(img *Image, newWidth, newHeight uint32) (*Image, error) {
	if err := ValidateDimensions(newWidth, newHeight); err != nil {
		return nil, err
	}
	if img == nil {
		return nil, newInvalidHeader("nil image")
	}
	return img.ResizeBilinear(newWidth, newHeight), nil
}

// MipmapHalve downsamples img by 2x on both axes using a premultiplied-alpha box filter.
func MipmapHalve(img *Image) (*Image, error) {
	if img == nil {
		return nil, newInvalidHeader("nil image")
	}
	return img.MipmapHalve(true, true), nil
}

// MipmapHalve downsamples by up to 2x on each axis with a premultiplied-alpha box filter.
// Axes with length > 1 are halved ((len + 1) / 2). Semi-transparent pixels are weighted
// by their alpha channel so transparent neighbors never bleed dark RGB fringes into anti-aliased
// icon or sprite edges.
func (img *Image) MipmapHalve(halveX, halveY bool) *Image {
	if img == nil || img.Width == 0 || img.Height == 0 {
		return New(0, 0)
	}
	dstW := img.Width
	if halveX && img.Width > 1 {
		dstW = (img.Width + 1) / 2
	}
	dstH := img.Height
	if halveY && img.Height > 1 {
		dstH = (img.Height + 1) / 2
	}

	stepX := uint32(1)
	if dstW < img.Width {
		stepX = 2
	}
	stepY := uint32(1)
	if dstH < img.Height {
		stepY = 2
	}

	out := New(dstW, dstH)
	for dy := uint32(0); dy < dstH; dy++ {
		sy0 := dy * stepY
		sy1 := sy0 + stepY - 1
		if sy1 >= img.Height {
			sy1 = img.Height - 1
		}
		for dx := uint32(0); dx < dstW; dx++ {
			sx0 := dx * stepX
			sx1 := sx0 + stepX - 1
			if sx1 >= img.Width {
				sx1 = img.Width - 1
			}

			type tap struct {
				r, g, b, a uint8
			}
			taps := [4]tap{}
			taps[0].r, taps[0].g, taps[0].b, taps[0].a, _ = img.Pixel(sx0, sy0)
			taps[1].r, taps[1].g, taps[1].b, taps[1].a, _ = img.Pixel(sx1, sy0)
			taps[2].r, taps[2].g, taps[2].b, taps[2].a, _ = img.Pixel(sx0, sy1)
			taps[3].r, taps[3].g, taps[3].b, taps[3].a, _ = img.Pixel(sx1, sy1)

			var sumR, sumG, sumB, sumA float32
			for _, t := range taps {
				a := float32(t.a)
				sumR += float32(t.r) * a
				sumG += float32(t.g) * a
				sumB += float32(t.b) * a
				sumA += a
			}

			outA := uint8(clampF32(float32(math.Round(float64(sumA*0.25))), 0.0, 255.0))
			if sumA > 0.0 {
				outR := uint8(clampF32(float32(math.Round(float64(sumR/sumA))), 0.0, 255.0))
				outG := uint8(clampF32(float32(math.Round(float64(sumG/sumA))), 0.0, 255.0))
				outB := uint8(clampF32(float32(math.Round(float64(sumB/sumA))), 0.0, 255.0))
				out.SetPixel(dx, dy, outR, outG, outB, outA)
			} else {
				out.SetPixel(dx, dy, 0, 0, 0, 0)
			}
		}
	}
	return out
}

// ResizeBilinear resamples to newWidth by newHeight with multi-step premultiplied-alpha
// mipmap box pre-filtering (when downscaling by more than 2x) followed by premultiplied-alpha
// bilinear interpolation.
func (img *Image) ResizeBilinear(newWidth, newHeight uint32) *Image {
	if img == nil || newWidth == 0 || newHeight == 0 || img.Width == 0 || img.Height == 0 {
		return New(newWidth, newHeight)
	}
	if newWidth == img.Width && newHeight == img.Height {
		return img.Clone()
	}

	src := img
	for (src.Width > newWidth*2 && src.Width > 1) || (src.Height > newHeight*2 && src.Height > 1) {
		hx := src.Width > newWidth*2 && src.Width > 1
		hy := src.Height > newHeight*2 && src.Height > 1
		src = src.MipmapHalve(hx, hy)
	}
	if src.Width == newWidth && src.Height == newHeight {
		return src.Clone()
	}

	out := New(newWidth, newHeight)
	xRatio := float32(src.Width) / float32(newWidth)
	yRatio := float32(src.Height) / float32(newHeight)

	for ny := uint32(0); ny < newHeight; ny++ {
		sy := (float32(ny)+0.5)*yRatio - 0.5
		sy = clampF32(sy, 0.0, float32(src.Height-1))
		y0 := uint32(math.Floor(float64(sy)))
		y1 := y0 + 1
		if y1 >= src.Height {
			y1 = src.Height - 1
		}
		wy := sy - float32(y0)

		for nx := uint32(0); nx < newWidth; nx++ {
			sx := (float32(nx)+0.5)*xRatio - 0.5
			sx = clampF32(sx, 0.0, float32(src.Width-1))
			x0 := uint32(math.Floor(float64(sx)))
			x1 := x0 + 1
			if x1 >= src.Width {
				x1 = src.Width - 1
			}
			wx := sx - float32(x0)

			r00, g00, b00, a00Raw, _ := src.Pixel(x0, y0)
			r10, g10, b10, a10Raw, _ := src.Pixel(x1, y0)
			r01, g01, b01, a01Raw, _ := src.Pixel(x0, y1)
			r11, g11, b11, a11Raw, _ := src.Pixel(x1, y1)

			w00 := (1.0 - wx) * (1.0 - wy)
			w10 := wx * (1.0 - wy)
			w01 := (1.0 - wx) * wy
			w11 := wx * wy

			a00 := float32(a00Raw) * w00
			a10 := float32(a10Raw) * w10
			a01 := float32(a01Raw) * w01
			a11 := float32(a11Raw) * w11
			sumA := a00 + a10 + a01 + a11

			outA := uint8(clampF32(float32(math.Round(float64(sumA))), 0.0, 255.0))
			if sumA > 0.0 {
				pmR := float32(r00)*a00 + float32(r10)*a10 + float32(r01)*a01 + float32(r11)*a11
				pmG := float32(g00)*a00 + float32(g10)*a10 + float32(g01)*a01 + float32(g11)*a11
				pmB := float32(b00)*a00 + float32(b10)*a10 + float32(b01)*a01 + float32(b11)*a11

				outR := uint8(clampF32(float32(math.Round(float64(pmR/sumA))), 0.0, 255.0))
				outG := uint8(clampF32(float32(math.Round(float64(pmG/sumA))), 0.0, 255.0))
				outB := uint8(clampF32(float32(math.Round(float64(pmB/sumA))), 0.0, 255.0))
				out.SetPixel(nx, ny, outR, outG, outB, outA)
			} else {
				out.SetPixel(nx, ny, 0, 0, 0, 0)
			}
		}
	}
	return out
}

func clampF32(v, min, max float32) float32 {
	if v < min {
		return min
	}
	if v > max {
		return max
	}
	return v
}
