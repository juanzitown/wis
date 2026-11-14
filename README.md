# wis — what i see

`wis` is a small Linux-first screen-recognition proof of concept. Use one resizable Manual area for OCR, or let Auto mode outline text blocks across the captured screen.

## What it does

- Manual mode with one draggable, resizable OCR area and live findings.
- Auto mode with thin outlines around detected text blocks across the screen.
- Persistent area coordinates and scan interval.
- Wayland screen capture through the XDG ScreenCast portal and PipeWire.
- Periodic OpenCV preprocessing and Tesseract OCR.
- Expandable live overlay finding with confidence, recognition time, and a thumbnail of the processed image sent to OCR.
- One-click × control to stop capture and return to the main window.
- No input automation and no application-specific recognition rules.

## Run on Bazzite

The included setup script creates a Fedora Distrobox with the Tauri, PipeWire, OpenCV, and Tesseract development packages:

```sh
chmod +x setup-bazzite.sh
./setup-bazzite.sh
yarn dev
```

Choose **Start manual** to position one OCR area and read its contents, or **Start auto** to detect text blocks across the screen. Both modes use the configured **Look every** interval and open the system screen-selection dialog.

## Architecture

- `crates/wis-core`: platform-independent settings model and validation.
- `src-tauri`: capture, image preprocessing, Tesseract, persistence, and window management.
- `src`: React control window and transparent area overlay.

The current capture adapter targets Linux. A macOS release should keep the OCR/settings layers and add a ScreenCaptureKit adapter plus the usual screen-recording permission, signing, and notarization work.
