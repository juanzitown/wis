# wis — what i see

`wis` is a small Linux-first OCR proof of concept. Place one resizable area over any part of the screen, choose how often to scan it, and watch the recognized text appear in a rolling findings feed.

## What it does

- Transparent text-area overlay that stays draggable and resizable while OCR runs.
- Persistent area coordinates and scan interval.
- Wayland screen capture through the XDG ScreenCast portal and PipeWire.
- Periodic OpenCV preprocessing and Tesseract OCR.
- Live overlay finding with confidence and recognition time, plus recent attempts in the main window.
- One-click × control to stop capture and return to the main window.
- No input automation and no application-specific recognition rules.

## Run on Bazzite

The included setup script creates a Fedora Distrobox with the Tauri, PipeWire, OpenCV, and Tesseract development packages:

```sh
chmod +x setup-bazzite.sh
./setup-bazzite.sh
yarn dev
```

In the app, select **Edit area on screen**, drag and resize the box, and choose **Done**. Then select **Start watching** and grant access to the screen containing the text.

## Architecture

- `crates/wis-core`: platform-independent settings model and validation.
- `src-tauri`: capture, image preprocessing, Tesseract, persistence, and window management.
- `src`: React control window and transparent area overlay.

The current capture adapter targets Linux. A macOS release should keep the OCR/settings layers and add a ScreenCaptureKit adapter plus the usual screen-recording permission, signing, and notarization work.
