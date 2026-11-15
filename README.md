# wis — what i see

`wis` is a small Linux-first screen-recognition proof of concept. Follow the pointer with Mouse isolation, or let Auto mode outline text-like regions across the captured screen.

## What it does

- Mouse mode with a click-drag-release isolation area of any size.
- Auto mode with thin outlines around detected text lines across the screen.
- One shared text-isolation pipeline for Mouse and Auto; only their search bounds differ.
- Mouse-only previews of every retained isolation stage.
- Persistent scan interval.
- Wayland screen capture through the XDG ScreenCast portal and PipeWire.
- Periodic OpenCV text isolation with retained Mouse-mode debug stages.
- One-click × control to stop capture and return to the main window.
- No input automation and no application-specific recognition rules.

## Run on Bazzite

The included setup script creates a Fedora Distrobox with the Tauri, PipeWire, OpenCV, and Tesseract development packages:

```sh
chmod +x setup-bazzite.sh
./setup-bazzite.sh
yarn dev
```

Choose **Start mouse**, then click-drag-release around a screen area to isolate it once. Choose **Start auto** to detect text lines across the whole screen at the configured **Look every** interval. Mouse mode keeps a full-width preview of every internal isolation stage for debugging; Auto runs the same stages without retaining them.

## Architecture

- `crates/wis-core`: platform-independent settings model and validation.
- `src-tauri`: capture, image preprocessing, Tesseract, persistence, and window management.
- `src`: React control window and transparent area overlay.

The current capture adapter targets Linux. A macOS release should keep the OCR/settings layers and add a ScreenCaptureKit adapter plus the usual screen-recording permission, signing, and notarization work.
