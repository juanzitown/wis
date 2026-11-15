Features

- [x] make the latest findings popup scrollable
- [x] add Mouse mode with a 240x120 pointer-anchored OCR area, findings, and thumbnail
- [ ] detect and process complete text lines
  - [x] add Manual and Auto modes while preserving the single draggable Manual area
  - [x] group glyph fragments horizontally without merging neighboring rows
  - [x] keep every horizontally connected contour as an independent line candidate
  - [x] show thin Auto-mode borders around detected lines
  - [x] hide previous line borders before sampling the next frame
  - [ ] auto-slice detected lines and send each to Tesseract
  - [ ] show a popup preview for every detected line

Improvements

- Colored texts almost gets transparent in the OCR input thumbnail
  - need for find a way to make the thumbnail show other colored texts as strong as white is today
  - [x] preserve colored glyph edges per B/G/R channel in Auto detection
- Auto detection false positives
  - [x] smooth small texture noise before edge detection
  - [x] reject sparse-edge and nearly flat-color candidate regions
  - [x] relax Otsu edge strength and density gates for repeated low-contrast badges
  - [x] reject image-like candidates using quantized 3x3 local color diversity
