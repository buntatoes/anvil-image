// SPDX-License-Identifier: LicenseRef-PolyForm-Noncommercial-1.0.0-OR-Commercial
// Copyright (c) 2026 Buntato

package anvilimage

import (
	"errors"
	"fmt"
)

// Sentinel errors for image parsing and decoding.
var (
	ErrUnexpectedEof      = errors.New("unexpected end of image file")
	ErrUnsupportedFormat  = errors.New("unsupported image format")
	ErrInvalidHeader      = errors.New("invalid image header")
	ErrDimensionsTooLarge = errors.New("image dimensions exceed security limits")
	ErrInvalidFilter      = errors.New("invalid PNG scanline filter")
	ErrCompression        = errors.New("image decompression failed")
)

// ImageError provides detailed contextual error information.
type ImageError struct {
	Err    error
	Detail string
	Width  uint32
	Height uint32
	Filter uint8
}

// Error formats the image error message.
func (e *ImageError) Error() string {
	if e == nil {
		return ""
	}
	if errors.Is(e.Err, ErrDimensionsTooLarge) {
		return fmt.Sprintf("image dimensions %dx%d exceed security limits (%dx%d, %d pixels)",
			e.Width, e.Height, MaxImageDimension, MaxImageDimension, MaxImagePixels)
	}
	if errors.Is(e.Err, ErrInvalidFilter) {
		return fmt.Sprintf("invalid PNG scanline filter %d", e.Filter)
	}
	if e.Detail != "" {
		if e.Err != nil {
			return fmt.Sprintf("%v: %s", e.Err, e.Detail)
		}
		return e.Detail
	}
	if e.Err != nil {
		return e.Err.Error()
	}
	return "image error"
}

// Unwrap returns the underlying sentinel error.
func (e *ImageError) Unwrap() error {
	if e == nil {
		return nil
	}
	return e.Err
}

func newUnexpectedEof() error {
	return &ImageError{Err: ErrUnexpectedEof}
}

func newUnsupportedFormat(detail string) error {
	return &ImageError{Err: ErrUnsupportedFormat, Detail: detail}
}

func newInvalidHeader(detail string) error {
	return &ImageError{Err: ErrInvalidHeader, Detail: detail}
}

func newDimensionsTooLarge(w, h uint32) error {
	return &ImageError{Err: ErrDimensionsTooLarge, Width: w, Height: h}
}

func newInvalidFilter(filter uint8) error {
	return &ImageError{Err: ErrInvalidFilter, Filter: filter}
}

func newCompressionError(err error) error {
	return &ImageError{Err: ErrCompression, Detail: err.Error()}
}
