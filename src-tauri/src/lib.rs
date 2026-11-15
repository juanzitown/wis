use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use leptess::{LepTess, Variable};
use opencv::{
    core::{self, Mat, Point, Rect, Scalar, Size, Vector},
    imgcodecs, imgproc,
    prelude::*,
};
use scrcap::{CaptureConfig, CaptureDescriptor as _, Target, VideoConfig};
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use wis_core::{Region, WisSettings};

struct AppState {
    settings: Mutex<WisSettings>,
    settings_path: PathBuf,
    overlay_mode: Mutex<String>,
    capture: Mutex<CaptureRuntime>,
}

#[derive(Default)]
struct CaptureRuntime {
    phase: CapturePhase,
    mode: Option<CaptureMode>,
    stop: Option<Arc<AtomicBool>>,
    last_error: Option<String>,
    frame_size: Option<(u32, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Manual,
    Auto,
}

impl CaptureMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "manual" => Ok(Self::Manual),
            "auto" => Ok(Self::Auto),
            _ => Err("capture mode must be manual or auto".to_owned()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum CapturePhase {
    #[default]
    Idle,
    Starting,
    Running,
    Stopping,
}

impl CapturePhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopping => "stopping",
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeStatus {
    platform: &'static str,
    session_type: String,
    capture_backend: &'static str,
    running: bool,
    phase: &'static str,
    mode: Option<&'static str>,
    last_error: Option<String>,
    frame_size: Option<(u32, u32)>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DetectedLine {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OcrUpdate {
    mode: &'static str,
    text: String,
    confidence: i32,
    preview_image_data_url: Option<String>,
    elapsed_ms: u128,
    observed_at_ms: u64,
    capture_width: u32,
    capture_height: u32,
    logical_width: u32,
    logical_height: u32,
    detected_lines: Vec<DetectedLine>,
    error: Option<String>,
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<WisSettings, String> {
    state
        .settings
        .lock()
        .map(|settings| settings.clone())
        .map_err(|_| "settings lock was poisoned".to_owned())
}

#[tauri::command]
fn save_settings(
    settings: WisSettings,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<WisSettings, String> {
    settings.validate().map_err(|error| error.to_string())?;
    *state
        .settings
        .lock()
        .map_err(|_| "settings lock was poisoned".to_owned())? = settings.clone();
    persist_settings(&state.settings_path, &settings)?;
    app.emit("settings-changed", &settings)
        .map_err(|error| error.to_string())?;
    Ok(settings)
}

#[tauri::command]
fn get_overlay_mode(state: State<'_, AppState>) -> Result<String, String> {
    state
        .overlay_mode
        .lock()
        .map(|mode| mode.clone())
        .map_err(|_| "overlay mode lock was poisoned".to_owned())
}

#[tauri::command]
fn set_overlay_mode(
    mode: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    if !matches!(mode.as_str(), "hidden" | "edit" | "preview") {
        return Err("overlay mode must be hidden, edit, or preview".to_owned());
    }
    let overlay = app
        .get_webview_window("overlay")
        .ok_or_else(|| "overlay window is not available".to_owned())?;
    match mode.as_str() {
        "hidden" => overlay.hide().map_err(|error| error.to_string()),
        "edit" => {
            overlay.show().map_err(|error| error.to_string())?;
            overlay
                .set_ignore_cursor_events(false)
                .map_err(|error| error.to_string())
        }
        "preview" => {
            overlay.show().map_err(|error| error.to_string())?;
            overlay
                .set_ignore_cursor_events(true)
                .map_err(|error| error.to_string())
        }
        _ => unreachable!(),
    }?;
    *state
        .overlay_mode
        .lock()
        .map_err(|_| "overlay mode lock was poisoned".to_owned())? = mode.clone();
    app.emit_to("overlay", "overlay-mode-changed", &mode)
        .map_err(|error| error.to_string())?;
    Ok(mode)
}

fn runtime_status(state: &AppState) -> Result<RuntimeStatus, String> {
    let capture = state
        .capture
        .lock()
        .map_err(|_| "capture lock was poisoned".to_owned())?;
    Ok(RuntimeStatus {
        platform: "Linux",
        session_type: std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".to_owned()),
        capture_backend: "XDG ScreenCast portal + PipeWire",
        running: capture.phase == CapturePhase::Running,
        phase: capture.phase.as_str(),
        mode: capture.mode.map(CaptureMode::as_str),
        last_error: capture.last_error.clone(),
        frame_size: capture.frame_size,
    })
}

#[tauri::command]
fn get_runtime_status(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    runtime_status(&state)
}

#[tauri::command]
fn start_ocr(
    mode: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<RuntimeStatus, String> {
    let mode = CaptureMode::parse(&mode)?;
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut capture = state
            .capture
            .lock()
            .map_err(|_| "capture lock was poisoned".to_owned())?;
        if capture.stop.is_some() {
            return Err("text recognition is already active".to_owned());
        }
        capture.phase = CapturePhase::Starting;
        capture.mode = Some(mode);
        capture.stop = Some(stop.clone());
        capture.last_error = None;
        capture.frame_size = None;
    }
    show_capture_overlay(&app, &state)?;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }

    let worker_app = app.clone();
    thread::Builder::new()
        .name("wis-ocr".to_owned())
        .spawn(move || {
            let result = run_capture(worker_app.clone(), stop, mode);
            finish_capture(&worker_app, result.err());
        })
        .map_err(|error| {
            finish_capture(&app, Some(error.to_string()));
            error.to_string()
        })?;
    let status = runtime_status(&state)?;
    let _ = app.emit("ocr-status", &status);
    Ok(status)
}

#[tauri::command]
fn stop_ocr(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    {
        let mut capture = state
            .capture
            .lock()
            .map_err(|_| "capture lock was poisoned".to_owned())?;
        if let Some(stop) = capture.stop.as_ref() {
            stop.store(true, Ordering::Release);
            capture.phase = CapturePhase::Stopping;
        }
    }
    show_main_window(&app, &state);
    let status = runtime_status(&state)?;
    let _ = app.emit("ocr-status", &status);
    Ok(status)
}

fn show_capture_overlay(app: &tauri::AppHandle, state: &AppState) -> Result<(), String> {
    let overlay = app
        .get_webview_window("overlay")
        .ok_or_else(|| "overlay window is not available".to_owned())?;
    overlay.show().map_err(|error| error.to_string())?;
    overlay
        .set_ignore_cursor_events(false)
        .map_err(|error| error.to_string())?;
    if let Ok(mut mode) = state.overlay_mode.lock() {
        *mode = "edit".to_owned();
    }
    let _ = app.emit_to("overlay", "overlay-mode-changed", "edit");
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle, state: &AppState) {
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    if let Ok(mut mode) = state.overlay_mode.lock() {
        *mode = "hidden".to_owned();
    }
    let _ = app.emit("overlay-mode-changed", "hidden");
}

fn run_capture(
    app: tauri::AppHandle,
    stop: Arc<AtomicBool>,
    mode: CaptureMode,
) -> Result<(), String> {
    let mut tesseract = if mode == CaptureMode::Manual {
        let mut engine = LepTess::new(None, "eng")
            .map_err(|error| format!("could not initialize Tesseract English OCR: {error}"))?;
        engine
            .set_variable(Variable::TesseditPagesegMode, "6")
            .map_err(|_| "could not configure Tesseract for a text block".to_owned())?;
        Some(engine)
    } else {
        None
    };
    let capture = CaptureConfig {
        video: VideoConfig {
            channel_capacity: 2,
            hide: vec![],
            target: Target::Primary,
            fps: Some(10),
        },
        audio: None,
    }
    .create()
    .map_err(|error| format!("screen capture could not start: {error}"))?;

    set_capture_running(&app, capture.size());
    let mut last_scan = Instant::now() - Duration::from_secs(60);
    let mut auto_settle_started: Option<Instant> = None;
    while !stop.load(Ordering::Acquire) {
        let frame = capture
            .video()
            .recv()
            .map_err(|_| "the PipeWire capture stream ended".to_owned())?;
        let settings = app
            .state::<AppState>()
            .settings
            .lock()
            .map_err(|_| "settings lock was poisoned".to_owned())?
            .clone();
        if mode == CaptureMode::Auto {
            if let Some(started) = auto_settle_started {
                // At 10 fps this discards at least one complete frame after
                // the WebView removes its old borders from the compositor.
                if started.elapsed() < Duration::from_millis(160) {
                    continue;
                }
            } else {
                if last_scan.elapsed() < Duration::from_millis(settings.scan_interval_ms) {
                    continue;
                }
                app.emit_to("overlay", "auto-borders-visible", false)
                    .map_err(|error| error.to_string())?;
                auto_settle_started = Some(Instant::now());
                continue;
            }
        } else if last_scan.elapsed() < Duration::from_millis(settings.scan_interval_ms) {
            continue;
        }
        last_scan = Instant::now();
        auto_settle_started = None;
        let logical_size = overlay_dimensions(&app).unwrap_or(frame.size);
        let started = Instant::now();
        let (text, confidence, preview_image_data_url, detected_lines, error) = match mode {
            CaptureMode::Manual => {
                let (text, confidence, preview, error) = recognize_region(
                    tesseract
                        .as_mut()
                        .expect("manual capture always initializes Tesseract"),
                    &frame.vframe,
                    frame.size,
                    logical_size,
                    settings.region,
                );
                (text, confidence, preview, Vec::new(), error)
            }
            CaptureMode::Auto => match detect_text_lines(&frame.vframe, frame.size, logical_size) {
                Ok(lines) => (String::new(), 0, None, lines, None),
                Err(error) => (String::new(), 0, None, Vec::new(), Some(error)),
            },
        };
        app.emit(
            "ocr-update",
            OcrUpdate {
                mode: mode.as_str(),
                text,
                confidence,
                preview_image_data_url,
                elapsed_ms: started.elapsed().as_millis(),
                observed_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                capture_width: frame.size.0,
                capture_height: frame.size.1,
                logical_width: logical_size.0,
                logical_height: logical_size.1,
                detected_lines,
                error,
            },
        )
        .map_err(|error| error.to_string())?;
    }
    capture.terminate();
    Ok(())
}

fn detect_text_lines(
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
) -> Result<Vec<DetectedLine>, String> {
    let (frame_width, frame_height) = frame_size;
    let (logical_width, logical_height) = logical_size;
    if frame_width == 0 || frame_height == 0 || logical_width == 0 || logical_height == 0 {
        return Err("capture and logical dimensions must be non-zero".to_owned());
    }
    let expected = frame_width as usize * frame_height as usize * 4;
    if frame.len() < expected {
        return Err(format!(
            "BGRA frame is too short: expected {expected} bytes, received {}",
            frame.len()
        ));
    }

    let source = Mat::from_slice(&frame[..expected]).map_err(|error| error.to_string())?;
    let source = source
        .reshape(4, frame_height as i32)
        .map_err(|error| error.to_string())?;
    let mut bgr = Mat::default();
    imgproc::cvt_color_def(&source, &mut bgr, imgproc::COLOR_BGRA2BGR)
        .map_err(|error| error.to_string())?;

    // Smooth tiny texture/color fluctuations, then keep the strongest glyph
    // edge found in any B, G, or R channel. Looking at channels separately
    // retains colored text that can nearly disappear after grayscale mixing.
    let edge_kernel =
        imgproc::get_structuring_element(imgproc::MORPH_RECT, Size::new(3, 3), Point::new(-1, -1))
            .map_err(|error| error.to_string())?;
    let mut gradient = Mat::default();
    for channel_index in 0..3 {
        let mut channel = Mat::default();
        core::extract_channel(&bgr, &mut channel, channel_index)
            .map_err(|error| error.to_string())?;
        let mut smoothed = Mat::default();
        imgproc::gaussian_blur_def(&channel, &mut smoothed, Size::new(3, 3), 0.0)
            .map_err(|error| error.to_string())?;
        let mut channel_gradient = Mat::default();
        imgproc::morphology_ex(
            &smoothed,
            &mut channel_gradient,
            imgproc::MORPH_GRADIENT,
            &edge_kernel,
            Point::new(-1, -1),
            1,
            core::BORDER_CONSTANT,
            Scalar::all(0.0),
        )
        .map_err(|error| error.to_string())?;
        if channel_index == 0 {
            gradient = channel_gradient;
        } else {
            let mut combined = Mat::default();
            core::max(&gradient, &channel_gradient, &mut combined)
                .map_err(|error| error.to_string())?;
            gradient = combined;
        }
    }
    let mut edges = Mat::default();
    let otsu_threshold = imgproc::threshold(
        &gradient,
        &mut edges,
        0.0,
        255.0,
        imgproc::THRESH_BINARY | imgproc::THRESH_OTSU,
    )
    .map_err(|error| error.to_string())?;
    // Otsu is screen-global and can be biased upward by a few very bright UI
    // elements. Keep its adaptive baseline, but admit slightly weaker badge
    // and colored-text edges into the candidate pass.
    let relaxed_threshold = (otsu_threshold * 0.82).max(3.0);
    imgproc::threshold(
        &gradient,
        &mut edges,
        relaxed_threshold,
        255.0,
        imgproc::THRESH_BINARY,
    )
    .map_err(|error| error.to_string())?;

    // Join neighboring glyphs only along X. A one-pixel-high kernel prevents
    // adjacent rows from collapsing into a paragraph-sized block.
    const MAX_LOGICAL_LINE_GAP: u32 = 12;
    let maximum_line_gap = ((MAX_LOGICAL_LINE_GAP as u64 * frame_width as u64
        + logical_width as u64 / 2)
        / logical_width as u64)
        .max(1) as i32;
    let kernel_width = (maximum_line_gap.clamp(5, 17) | 1) as i32;
    let line_kernel = imgproc::get_structuring_element(
        imgproc::MORPH_RECT,
        Size::new(kernel_width, 1),
        Point::new(-1, -1),
    )
    .map_err(|error| error.to_string())?;
    let mut connected = Mat::default();
    imgproc::morphology_ex(
        &edges,
        &mut connected,
        imgproc::MORPH_CLOSE,
        &line_kernel,
        Point::new(-1, -1),
        1,
        core::BORDER_CONSTANT,
        Scalar::all(0.0),
    )
    .map_err(|error| error.to_string())?;

    let mut contours: Vector<Vector<Point>> = Vector::new();
    imgproc::find_contours(
        &connected,
        &mut contours,
        imgproc::RETR_EXTERNAL,
        imgproc::CHAIN_APPROX_SIMPLE,
        Point::new(0, 0),
    )
    .map_err(|error| error.to_string())?;

    let scale = |value: u32, captured: u32, logical: u32| {
        ((value as u64 * logical as u64 + captured as u64 / 2) / captured as u64) as u32
    };
    let maximum_height = (frame_height / 8).max(32) as i32;
    let mut candidates = Vec::new();
    for index in 0..contours.len() {
        let contour = contours.get(index).map_err(|error| error.to_string())?;
        let rect = imgproc::bounding_rect(&contour).map_err(|error| error.to_string())?;
        let area = rect.width.saturating_mul(rect.height);
        if rect.width < 6 || rect.height < 4 || rect.height > maximum_height || area < 40 {
            continue;
        }
        // Text has repeated internal edges and visible channel variation.
        // Flat silhouettes tend to contribute only a sparse perimeter.
        let edge_roi = Mat::roi(&edges, rect).map_err(|error| error.to_string())?;
        let edge_density = core::count_non_zero(&edge_roi).map_err(|error| error.to_string())?
            as f64
            / area as f64;
        let minimum_density = if area < 800 { 0.009 } else { 0.015 };
        if edge_density < minimum_density || edge_density > 0.72 {
            continue;
        }
        let color_roi = Mat::roi(&bgr, rect).map_err(|error| error.to_string())?;
        let mut mean = Mat::default();
        let mut deviation = Mat::default();
        core::mean_std_dev(&color_roi, &mut mean, &mut deviation, &core::no_array())
            .map_err(|error| error.to_string())?;
        let strongest_deviation = deviation
            .data_typed::<f64>()
            .map_err(|error| error.to_string())?
            .iter()
            .copied()
            .fold(0.0, f64::max);
        if strongest_deviation < 3.5 {
            continue;
        }
        // Ignore the overlay's stop button in Auto mode.
        if rect.x > frame_width.saturating_sub(80) as i32 && rect.y < 80 {
            continue;
        }

        candidates.push(rect);
    }

    candidates.sort_by_key(|rect| (rect.y, rect.x));
    let mut lines = Vec::new();
    for rect in candidates {
        let left = rect.x.saturating_sub(4).max(0) as u32;
        let top = rect.y.saturating_sub(2).max(0) as u32;
        let right = (rect.x + rect.width + 4).max(0) as u32;
        let bottom = (rect.y + rect.height + 2).max(0) as u32;
        let x = scale(left.min(frame_width), frame_width, logical_width);
        let y = scale(top.min(frame_height), frame_height, logical_height);
        let scaled_right = scale(right.min(frame_width), frame_width, logical_width);
        let scaled_bottom = scale(bottom.min(frame_height), frame_height, logical_height);
        lines.push(DetectedLine {
            x,
            y,
            width: scaled_right.saturating_sub(x).max(1),
            height: scaled_bottom.saturating_sub(y).max(1),
        });
    }
    lines.sort_by_key(|line| (line.y, line.x));
    lines.truncate(256);
    Ok(lines)
}

fn recognize_region(
    tesseract: &mut LepTess,
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    region: Region,
) -> (String, i32, Option<String>, Option<String>) {
    let (encoded, preview) = match preprocess_region(frame, frame_size, logical_size, region) {
        Ok(images) => images,
        Err(error) => return (String::new(), 0, None, Some(error)),
    };
    let preview_image_data_url = Some(format!(
        "data:image/png;base64,{}",
        BASE64.encode(preview.as_slice())
    ));
    let result = (|| -> Result<(String, i32), String> {
        tesseract
            .set_image_from_mem(encoded.as_slice())
            .map_err(|error| error.to_string())?;
        tesseract.set_source_resolution(300);
        let raw = tesseract
            .get_utf8_text()
            .map_err(|error| error.to_string())?;
        let text = raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Ok((text, tesseract.mean_text_conf()))
    })();
    match result {
        Ok((text, confidence)) => (text, confidence, preview_image_data_url, None),
        Err(error) => (String::new(), 0, preview_image_data_url, Some(error)),
    }
}

fn preprocess_region(
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    region: Region,
) -> Result<(Vector<u8>, Vector<u8>), String> {
    let (frame_width, frame_height) = frame_size;
    let (logical_width, logical_height) = logical_size;
    if frame_width == 0 || frame_height == 0 || logical_width == 0 || logical_height == 0 {
        return Err("capture and logical dimensions must be non-zero".to_owned());
    }
    let expected = frame_width as usize * frame_height as usize * 4;
    if frame.len() < expected {
        return Err(format!(
            "BGRA frame is too short: expected {expected} bytes, received {}",
            frame.len()
        ));
    }

    let scale = |value: u32, captured: u32, logical: u32| {
        ((value as u64 * captured as u64 + logical as u64 / 2) / logical as u64) as u32
    };
    let left = scale(region.x, frame_width, logical_width).min(frame_width);
    let top = scale(region.y, frame_height, logical_height).min(frame_height);
    let right = scale(
        region.x.saturating_add(region.width),
        frame_width,
        logical_width,
    )
    .min(frame_width);
    let bottom = scale(
        region.y.saturating_add(region.height),
        frame_height,
        logical_height,
    )
    .min(frame_height);
    if right <= left || bottom <= top {
        return Err("the text area is outside the captured screen".to_owned());
    }

    let source = Mat::from_slice(&frame[..expected]).map_err(|error| error.to_string())?;
    let source = source
        .reshape(4, frame_height as i32)
        .map_err(|error| error.to_string())?;
    let roi = Mat::roi(
        &source,
        Rect::new(
            left as i32,
            top as i32,
            (right - left) as i32,
            (bottom - top) as i32,
        ),
    )
    .map_err(|error| error.to_string())?;

    let mut enlarged = Mat::default();
    imgproc::resize(
        &roi,
        &mut enlarged,
        Size::new(roi.cols() * 2, roi.rows() * 2),
        0.0,
        0.0,
        imgproc::INTER_CUBIC,
    )
    .map_err(|error| error.to_string())?;
    let mut gray = Mat::default();
    imgproc::cvt_color_def(&enlarged, &mut gray, imgproc::COLOR_BGRA2GRAY)
        .map_err(|error| error.to_string())?;
    let mut binary = Mat::default();
    imgproc::threshold(
        &gray,
        &mut binary,
        0.0,
        255.0,
        imgproc::THRESH_BINARY | imgproc::THRESH_OTSU,
    )
    .map_err(|error| error.to_string())?;
    if core::count_non_zero(&binary).map_err(|error| error.to_string())?
        < (binary.rows() * binary.cols()) / 2
    {
        let mut inverted = Mat::default();
        core::bitwise_not(&binary, &mut inverted, &core::no_array())
            .map_err(|error| error.to_string())?;
        binary = inverted;
    }

    let mut padded = Mat::default();
    core::copy_make_border(
        &binary,
        &mut padded,
        16,
        16,
        16,
        16,
        core::BORDER_CONSTANT,
        Scalar::all(255.0),
    )
    .map_err(|error| error.to_string())?;
    let mut encoded = Vector::new();
    imgcodecs::imencode(".png", &padded, &mut encoded, &Vector::new())
        .map_err(|error| error.to_string())?;

    let preview_scale = (480.0 / padded.cols() as f64)
        .min(160.0 / padded.rows() as f64)
        .min(1.0);
    let preview_size = Size::new(
        ((padded.cols() as f64 * preview_scale).round() as i32).max(1),
        ((padded.rows() as f64 * preview_scale).round() as i32).max(1),
    );
    let mut preview_mat = Mat::default();
    imgproc::resize(
        &padded,
        &mut preview_mat,
        preview_size,
        0.0,
        0.0,
        imgproc::INTER_AREA,
    )
    .map_err(|error| error.to_string())?;
    let mut preview = Vector::new();
    imgcodecs::imencode(".png", &preview_mat, &mut preview, &Vector::new())
        .map_err(|error| error.to_string())?;
    Ok((encoded, preview))
}

fn overlay_dimensions(app: &tauri::AppHandle) -> Option<(u32, u32)> {
    let size = app.get_webview_window("overlay")?.inner_size().ok()?;
    (size.width > 0 && size.height > 0).then_some((size.width, size.height))
}

fn set_capture_running(app: &tauri::AppHandle, frame_size: (u32, u32)) {
    let state = app.state::<AppState>();
    if let Ok(mut capture) = state.capture.lock() {
        capture.phase = CapturePhase::Running;
        capture.frame_size = Some(frame_size);
    }
    if let Ok(status) = runtime_status(&state) {
        let _ = app.emit("ocr-status", status);
    }
}

fn finish_capture(app: &tauri::AppHandle, error: Option<String>) {
    let should_return_to_app = error.is_some();
    let state = app.state::<AppState>();
    if let Ok(mut capture) = state.capture.lock() {
        capture.phase = CapturePhase::Idle;
        capture.mode = None;
        capture.stop = None;
        capture.last_error = error;
    }
    if should_return_to_app {
        show_main_window(app, &state);
    }
    if let Ok(status) = runtime_status(&state) {
        let _ = app.emit("ocr-status", status);
    }
}

fn load_settings(path: &Path) -> WisSettings {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<WisSettings>(&bytes).ok())
        .filter(|settings| settings.validate().is_ok())
        .unwrap_or_default()
}

fn persist_settings(path: &Path, settings: &WisSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let json = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    fs::write(path, json).map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let settings = load_settings(&settings_path);
            app.manage(AppState {
                settings: Mutex::new(settings),
                settings_path,
                overlay_mode: Mutex::new("hidden".to_owned()),
                capture: Mutex::new(CaptureRuntime::default()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            get_overlay_mode,
            set_overlay_mode,
            get_runtime_status,
            start_ocr,
            stop_ocr,
        ])
        .run(tauri::generate_context!())
        .expect("error while running wis");
}
