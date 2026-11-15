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
use scrcap::{CaptureConfig, CaptureDescriptor as _, Target, VideoConfig};
use serde::Serialize;
use tauri::{Emitter, Manager, State};
use wis_core::{Region, WisSettings};

pub mod text_clarity;
mod text_isolation;

struct AppState {
    settings: Mutex<WisSettings>,
    settings_path: PathBuf,
    overlay_mode: Mutex<String>,
    mouse_selection: Mutex<MouseSelectionState>,
    capture: Mutex<CaptureRuntime>,
}

#[derive(Default, Clone, Copy)]
struct MouseSelectionState {
    region: Option<Region>,
    revision: u64,
}

#[derive(Clone, Copy)]
struct PendingIsolation {
    region: Region,
    mouse_revision: Option<u64>,
    started: Instant,
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
    Auto,
    Mouse,
}

impl CaptureMode {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "auto" => Ok(Self::Auto),
            "mouse" => Ok(Self::Mouse),
            _ => Err("capture mode must be auto or mouse".to_owned()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Mouse => "mouse",
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
struct OcrUpdate {
    mode: &'static str,
    text: String,
    confidence: i32,
    preview_image_data_url: Option<String>,
    elapsed_ms: u128,
    elapsed_micros: u128,
    observed_at_ms: u64,
    capture_width: u32,
    capture_height: u32,
    logical_width: u32,
    logical_height: u32,
    detected_lines: Vec<text_isolation::DetectedLine>,
    isolation_steps: Vec<text_isolation::IsolationStep>,
    recognized_lines: Vec<RecognizedLine>,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecognizedLine {
    region: text_isolation::DetectedLine,
    text: String,
    confidence: i32,
    preview_image_data_url: Option<String>,
    elapsed_micros: u128,
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
fn submit_mouse_region(region: Region, state: State<'_, AppState>) -> Result<u64, String> {
    if region.width < 2 || region.height < 2 {
        return Err("drag a region at least 2×2 pixels".to_owned());
    }
    region
        .x
        .checked_add(region.width)
        .and_then(|_| region.y.checked_add(region.height))
        .ok_or_else(|| "mouse region coordinates overflow".to_owned())?;
    let mut selection = state
        .mouse_selection
        .lock()
        .map_err(|_| "mouse selection lock was poisoned".to_owned())?;
    selection.region = Some(region);
    selection.revision = selection.revision.wrapping_add(1).max(1);
    Ok(selection.revision)
}

#[tauri::command]
fn clear_mouse_region(state: State<'_, AppState>) -> Result<u64, String> {
    let mut selection = state
        .mouse_selection
        .lock()
        .map_err(|_| "mouse selection lock was poisoned".to_owned())?;
    selection.region = None;
    selection.revision = selection.revision.wrapping_add(1).max(1);
    Ok(selection.revision)
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
    if mode == CaptureMode::Mouse {
        *state
            .mouse_selection
            .lock()
            .map_err(|_| "mouse selection lock was poisoned".to_owned())? =
            MouseSelectionState::default();
    }
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
    let mut tesseract = if mode == CaptureMode::Mouse {
        let mut engine = LepTess::new(None, "eng")
            .map_err(|error| format!("could not initialize Tesseract English OCR: {error}"))?;
        engine
            .set_variable(Variable::TesseditPagesegMode, "7")
            .map_err(|_| "could not configure Tesseract for single-line OCR".to_owned())?;
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
    let mut last_mouse_revision = 0_u64;
    let mut pending_isolation: Option<PendingIsolation> = None;
    while !stop.load(Ordering::Acquire) {
        let frame = capture
            .video()
            .recv()
            .map_err(|_| "the PipeWire capture stream ended".to_owned())?;
        let logical_size = overlay_dimensions(&app).unwrap_or(frame.size);
        if let Some(pending) = pending_isolation {
            // At 10 fps this discards at least one complete frame after
            // the WebView removes its old borders from the compositor.
            if pending.started.elapsed() < Duration::from_millis(160) {
                continue;
            }
        } else {
            let trigger = match mode {
                CaptureMode::Auto => {
                    let interval = app
                        .state::<AppState>()
                        .settings
                        .lock()
                        .map_err(|_| "settings lock was poisoned".to_owned())?
                        .scan_interval_ms;
                    if last_scan.elapsed() < Duration::from_millis(interval) {
                        continue;
                    }
                    PendingIsolation {
                        region: Region {
                            x: 0,
                            y: 0,
                            width: logical_size.0,
                            height: logical_size.1,
                        },
                        mouse_revision: None,
                        started: Instant::now(),
                    }
                }
                CaptureMode::Mouse => {
                    let selection = *app
                        .state::<AppState>()
                        .mouse_selection
                        .lock()
                        .map_err(|_| "mouse selection lock was poisoned".to_owned())?;
                    let Some(region) = selection.region else {
                        continue;
                    };
                    if selection.revision <= last_mouse_revision {
                        continue;
                    }
                    PendingIsolation {
                        region,
                        mouse_revision: Some(selection.revision),
                        started: Instant::now(),
                    }
                }
            };
            app.emit_to("overlay", "isolation-borders-visible", false)
                .map_err(|error| error.to_string())?;
            pending_isolation = Some(trigger);
            continue;
        }
        let pending = pending_isolation
            .take()
            .expect("a settled isolation must have pending bounds");
        if let Some(revision) = pending.mouse_revision {
            let selection = *app
                .state::<AppState>()
                .mouse_selection
                .lock()
                .map_err(|_| "mouse selection lock was poisoned".to_owned())?;
            if selection.revision != revision || selection.region != Some(pending.region) {
                continue;
            }
        }
        last_scan = Instant::now();
        let started = Instant::now();
        let (preview_image_data_url, detected_lines, isolation_steps, error) =
            match text_isolation::isolate_text(
                &frame.vframe,
                frame.size,
                logical_size,
                pending.region,
                mode == CaptureMode::Mouse,
            ) {
                Ok(result) => {
                    let preview = result
                        .steps
                        .last()
                        .map(|step| step.preview_image_data_url.clone());
                    (preview, result.lines, result.steps, None)
                }
                Err(error) => (None, Vec::new(), Vec::new(), Some(error)),
            };
        let recognized_lines = if error.is_none() && mode == CaptureMode::Mouse {
            recognize_isolated_lines(
                tesseract
                    .as_mut()
                    .expect("Mouse capture always initializes Tesseract"),
                &frame.vframe,
                frame.size,
                logical_size,
                &detected_lines,
            )
        } else {
            Vec::new()
        };
        let recognized_text = recognized_lines
            .iter()
            .map(|line| line.text.trim())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let confident_lines = recognized_lines
            .iter()
            .filter(|line| line.error.is_none() && !line.text.trim().is_empty())
            .collect::<Vec<_>>();
        let confidence = if confident_lines.is_empty() {
            0
        } else {
            confident_lines
                .iter()
                .map(|line| line.confidence as i64)
                .sum::<i64>()
                .checked_div(confident_lines.len() as i64)
                .unwrap_or_default() as i32
        };
        if let Some(revision) = pending.mouse_revision {
            last_mouse_revision = revision;
        }
        let elapsed = started.elapsed();
        app.emit(
            "ocr-update",
            OcrUpdate {
                mode: mode.as_str(),
                text: recognized_text,
                confidence,
                preview_image_data_url,
                elapsed_ms: elapsed.as_millis(),
                elapsed_micros: elapsed.as_micros(),
                observed_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                capture_width: frame.size.0,
                capture_height: frame.size.1,
                logical_width: logical_size.0,
                logical_height: logical_size.1,
                detected_lines,
                isolation_steps,
                recognized_lines,
                error,
            },
        )
        .map_err(|error| error.to_string())?;
    }
    capture.terminate();
    Ok(())
}

fn recognize_isolated_lines(
    tesseract: &mut LepTess,
    frame: &[u8],
    frame_size: (u32, u32),
    logical_size: (u32, u32),
    lines: &[text_isolation::DetectedLine],
) -> Vec<RecognizedLine> {
    lines
        .iter()
        .take(64)
        .map(|line| {
            let started = Instant::now();
            let (text, confidence, preview_image_data_url, error) = text_clarity::recognize_region(
                tesseract,
                frame,
                frame_size,
                logical_size,
                Region {
                    x: line.x,
                    y: line.y,
                    width: line.width,
                    height: line.height,
                },
            );
            RecognizedLine {
                region: line.clone(),
                text,
                confidence,
                preview_image_data_url,
                elapsed_micros: started.elapsed().as_micros(),
                error,
            }
        })
        .collect()
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
                mouse_selection: Mutex::new(MouseSelectionState::default()),
                capture: Mutex::new(CaptureRuntime::default()),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            save_settings,
            submit_mouse_region,
            clear_mouse_region,
            get_overlay_mode,
            set_overlay_mode,
            get_runtime_status,
            start_ocr,
            stop_ocr,
        ])
        .run(tauri::generate_context!())
        .expect("error while running wis");
}
