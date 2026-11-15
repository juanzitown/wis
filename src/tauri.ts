import { invoke } from "@tauri-apps/api/core";

export type Region = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type WisSettings = {
  region: Region;
  scanIntervalMs: number;
};

export type OverlayMode = "hidden" | "edit" | "preview";
export type CaptureMode = "auto" | "mouse";

export type IsolationStep = {
  name: string;
  previewImageDataUrl: string;
  elapsedMicros: number | null;
};

export type RecognizedLine = {
  region: Region;
  text: string;
  confidence: number;
  previewImageDataUrl: string | null;
  elapsedMicros: number;
  error: string | null;
};

export type RuntimeStatus = {
  platform: string;
  sessionType: string;
  captureBackend: string;
  running: boolean;
  phase: "idle" | "starting" | "running" | "stopping";
  mode: CaptureMode | null;
  lastError: string | null;
  frameSize: [number, number] | null;
};

export type OcrUpdate = {
  mode: CaptureMode;
  text: string;
  confidence: number;
  previewImageDataUrl: string | null;
  elapsedMs: number;
  elapsedMicros: number;
  observedAtMs: number;
  captureWidth: number;
  captureHeight: number;
  logicalWidth: number;
  logicalHeight: number;
  detectedLines: Region[];
  isolationSteps: IsolationStep[];
  recognizedLines: RecognizedLine[];
  error: string | null;
};

export const fallbackSettings: WisSettings = {
  region: { x: 240, y: 180, width: 640, height: 240 },
  scanIntervalMs: 1500,
};

export async function getSettings(): Promise<WisSettings> {
  try {
    return await invoke<WisSettings>("get_settings");
  } catch {
    return fallbackSettings;
  }
}

export function saveSettings(settings: WisSettings) {
  return invoke<WisSettings>("save_settings", { settings });
}

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  try {
    return await invoke<RuntimeStatus>("get_runtime_status");
  } catch {
    return {
      platform: "browser preview",
      sessionType: "unknown",
      captureBackend: "Tauri runtime required",
      running: false,
      phase: "idle",
      mode: null,
      lastError: null,
      frameSize: null,
    };
  }
}

export function startOcr(mode: CaptureMode) {
  return invoke<RuntimeStatus>("start_ocr", { mode });
}

export function stopOcr() {
  return invoke<RuntimeStatus>("stop_ocr");
}

export function submitMouseRegion(region: Region) {
  return invoke<number>("submit_mouse_region", {
    region: {
      x: Math.round(region.x),
      y: Math.round(region.y),
      width: Math.round(region.width),
      height: Math.round(region.height),
    },
  });
}

export function clearMouseRegion() {
  return invoke<number>("clear_mouse_region");
}

export async function getOverlayMode(): Promise<OverlayMode> {
  try {
    return await invoke<OverlayMode>("get_overlay_mode");
  } catch {
    return "hidden";
  }
}

export function setOverlayMode(mode: OverlayMode) {
  return invoke<OverlayMode>("set_overlay_mode", { mode });
}
