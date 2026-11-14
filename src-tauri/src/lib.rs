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

use leptess::{LepTess, Variable};
use opencv::{
    core::{self, Mat, Rect, Scalar, Size, Vector},
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
    stop: Option<Arc<AtomicBool>>,
    last_error: Option<String>,
    frame_size: Option<(u32, u32)>,
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
    last_error: Option<String>,
    frame_size: Option<(u32, u32)>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OcrUpdate {
    text: String,
    confidence: i32,
    elapsed_ms: u128,
    observed_at_ms: u64,
    capture_width: u32,
    capture_height: u32,
    logical_width: u32,
    logical_height: u32,
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
        last_error: capture.last_error.clone(),
        frame_size: capture.frame_size,
    })
}

#[tauri::command]
fn get_runtime_status(state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
    runtime_status(&state)
}

#[tauri::command]
fn start_ocr(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<RuntimeStatus, String> {
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
            let result = run_ocr(worker_app.clone(), stop);
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

fn run_ocr(app: tauri::AppHandle, stop: Arc<AtomicBool>) -> Result<(), String> {
    let mut tesseract = LepTess::new(None, "eng")
        .map_err(|error| format!("could not initialize Tesseract English OCR: {error}"))?;
    tesseract
        .set_variable(Variable::TesseditPagesegMode, "6")
        .map_err(|_| "could not configure Tesseract for a text block".to_owned())?;
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
        if last_scan.elapsed() < Duration::from_millis(settings.scan_interval_ms) {
            continue;
        }
        last_scan = Instant::now();
        let logical_size = overlay_dimensions(&app).unwrap_or(frame.size);
        let started = Instant::now();
        let result = recognize_region(
            &mut tesseract,
            &frame.vframe,
            frame.size,
            logical_size,
            settings.region,
        );
        let (text, confidence, error) = match result {
            Ok((text, confidence)) => (text, confidence, None),
            Err(error) => (String::new(), 0, Some(error)),
        };
        app.emit(
            "ocr-update",
            OcrUpdate {
                text,
                confidence,
                elapsed_ms: started.elapsed().as_millis(),
                observed_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                capture_width: frame.size.0,
                capture_height: frame.size.1,
                logical_width: logical_size.0,
                logical_height: logical_size.1,
                error,
            },
        )
        .map_err(|error| error.to_string())?;
    }
    capture.terminate();
    Ok(())
}

fn recognize_region(
    tesseract: &mut LepTess,
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    region: Region,
) -> Result<(String, i32), String> {
    let encoded = preprocess_region(frame, frame_size, logical_size, region)?;
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
}

fn preprocess_region(
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    region: Region,
) -> Result<Vector<u8>, String> {
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
    Ok(encoded)
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
