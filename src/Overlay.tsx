import { CSSProperties, PointerEvent, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getOverlayMode,
  getRuntimeStatus,
  getSettings,
  OcrUpdate,
  OverlayMode,
  Region,
  RuntimeStatus,
  saveSettings,
  stopOcr,
  updateMousePosition,
  WisSettings,
  fallbackSettings,
} from "./tauri";

type DragAction = "move" | "nw" | "ne" | "sw" | "se";
type DragState = {
  action: DragAction;
  startX: number;
  startY: number;
  initial: Region;
};

const minimumSize = 32;
const mouseAreaWidth = 240;
const mouseAreaHeight = 120;

export default function Overlay() {
  const [settings, setSettings] = useState(fallbackSettings);
  const [mode, setMode] = useState<OverlayMode>("hidden");
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [finding, setFinding] = useState<OcrUpdate | null>(null);
  const [autoBordersVisible, setAutoBordersVisible] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const [mousePosition, setMousePosition] = useState(() => ({
    x: window.innerWidth / 2,
    y: window.innerHeight / 2,
  }));
  const settingsRef = useRef(settings);
  const dragRef = useRef<DragState | null>(null);
  const lastMouseUpdateRef = useRef(0);
  const pendingMouseUpdateRef = useRef<number | null>(null);

  function updateSettings(next: WisSettings) {
    settingsRef.current = next;
    setSettings(next);
  }

  useEffect(() => {
    Promise.all([getSettings(), getOverlayMode(), getRuntimeStatus()]).then(([loaded, loadedMode, loadedStatus]) => {
      updateSettings(loaded);
      setMode(loadedMode);
      setStatus(loadedStatus);
    });
    const settingsSubscription = listen<WisSettings>("settings-changed", (event) => {
      if (!dragRef.current) updateSettings(event.payload);
    });
    const modeSubscription = listen<OverlayMode>("overlay-mode-changed", (event) => {
      setMode(event.payload);
    });
    const statusSubscription = listen<RuntimeStatus>("ocr-status", (event) => {
      setStatus(event.payload);
      if (event.payload.phase === "starting") {
        setFinding(null);
        setAutoBordersVisible(false);
        if (event.payload.mode === "mouse") {
          const center = { x: window.innerWidth / 2, y: window.innerHeight / 2 };
          setMousePosition(center);
          updateMousePosition(center.x, center.y).catch(() => undefined);
        }
      }
      if (event.payload.phase === "idle") {
        setIsStopping(false);
        if (pendingMouseUpdateRef.current !== null) {
          window.clearTimeout(pendingMouseUpdateRef.current);
          pendingMouseUpdateRef.current = null;
        }
      }
    });
    const ocrSubscription = listen<OcrUpdate>("ocr-update", (event) => {
      setFinding(event.payload);
      if (event.payload.mode === "auto") setAutoBordersVisible(true);
    });
    const autoVisibilitySubscription = listen<boolean>("auto-borders-visible", (event) => {
      setAutoBordersVisible(event.payload);
    });
    return () => {
      settingsSubscription.then((unlisten) => unlisten());
      modeSubscription.then((unlisten) => unlisten());
      statusSubscription.then((unlisten) => unlisten());
      ocrSubscription.then((unlisten) => unlisten());
      autoVisibilitySubscription.then((unlisten) => unlisten());
      if (pendingMouseUpdateRef.current !== null) {
        window.clearTimeout(pendingMouseUpdateRef.current);
      }
    };
  }, []);

  function startDrag(event: PointerEvent, action: DragAction) {
    if (mode !== "edit" || (status?.phase !== "idle" && status?.mode !== "manual")) return;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(event.pointerId);
    dragRef.current = {
      action,
      startX: event.clientX,
      startY: event.clientY,
      initial: { ...settingsRef.current.region },
    };
  }

  function moveDrag(event: PointerEvent) {
    const drag = dragRef.current;
    if (!drag) return;
    const region = resizeRegion(
      drag.initial,
      drag.action,
      Math.round(event.clientX - drag.startX),
      Math.round(event.clientY - drag.startY),
    );
    updateSettings({ ...settingsRef.current, region });
  }

  function movePointer(event: PointerEvent) {
    moveDrag(event);
    if (status?.mode !== "mouse" || status.phase === "idle") return;
    const next = {
      x: Math.max(0, Math.min(window.innerWidth, event.clientX)),
      y: Math.max(0, Math.min(window.innerHeight, event.clientY)),
    };
    setMousePosition(next);
    const now = performance.now();
    const remaining = 50 - (now - lastMouseUpdateRef.current);
    if (pendingMouseUpdateRef.current !== null) {
      window.clearTimeout(pendingMouseUpdateRef.current);
      pendingMouseUpdateRef.current = null;
    }
    if (remaining <= 0) {
      lastMouseUpdateRef.current = now;
      updateMousePosition(next.x, next.y).catch(() => undefined);
    } else {
      pendingMouseUpdateRef.current = window.setTimeout(() => {
        pendingMouseUpdateRef.current = null;
        lastMouseUpdateRef.current = performance.now();
        updateMousePosition(next.x, next.y).catch(() => undefined);
      }, remaining);
    }
  }

  function finishDrag() {
    if (!dragRef.current) return;
    dragRef.current = null;
    saveSettings(settingsRef.current).catch(() => getSettings().then(updateSettings));
  }

  async function closeCapture() {
    if (isStopping) return;
    setIsStopping(true);
    try {
      await stopOcr();
    } catch {
      setIsStopping(false);
    }
  }

  const isActive = status?.phase !== "idle" && status !== null;
  const isManualActive = isActive && status?.mode === "manual";
  const isAutoActive = isActive && status?.mode === "auto";
  const isMouseActive = isActive && status?.mode === "mouse";
  const mouseRegion: Region = {
    x: mouseAxisStart(mousePosition.x, mouseAreaWidth, window.innerWidth),
    y: mouseAxisStart(mousePosition.y, mouseAreaHeight, window.innerHeight),
    width: Math.min(mouseAreaWidth, window.innerWidth),
    height: Math.min(mouseAreaHeight, window.innerHeight),
  };
  const region = isMouseActive ? mouseRegion : settings.region;
  const isReadingActive = isManualActive || isMouseActive;
  const controlsBelow = region.y < 44;
  const controlStyle: CSSProperties = isAutoActive || isMouseActive
    ? { right: 18, top: 18 }
    : {
        left: Math.min(Math.max(8, region.x + region.width - 32), Math.max(8, window.innerWidth - 40)),
        top: controlsBelow ? region.y + region.height + 8 : region.y - 38,
      };
  const popupWidth = Math.min(400, window.innerWidth - 24);
  const popupHeight = Math.min(360, window.innerHeight - 24);
  const belowTop = region.y + region.height + (controlsBelow ? 52 : 12);
  const roomBelow = window.innerHeight - belowTop - 12;
  const roomAbove = region.y - 24;
  const placeBelow = roomBelow >= Math.min(220, popupHeight) || roomBelow >= roomAbove;
  const availableHeight = Math.max(120, Math.min(popupHeight, placeBelow ? roomBelow : roomAbove));
  const popupTop = placeBelow ? belowTop : Math.max(12, region.y - availableHeight - 12);
  const findingStyle: CSSProperties = {
    left: Math.min(Math.max(12, region.x), Math.max(12, window.innerWidth - popupWidth - 12)),
    top: popupTop,
    width: popupWidth,
    maxHeight: availableHeight,
  };
  return (
    <div
      className={`overlay-surface ${mode === "edit" ? "is-editing" : ""} ${isMouseActive ? "is-mouse" : ""}`}
      onPointerMove={movePointer}
      onPointerUp={finishDrag}
      onPointerCancel={finishDrag}
    >
      {!isAutoActive && !isMouseActive && (
        <div
          className="overlay-region"
          style={{ left: region.x, top: region.y, width: region.width, height: region.height }}
          onPointerDown={(event) => startDrag(event, "move")}
        >
          <span className="overlay-label">Manual area</span>
          {mode === "edit" && (["nw", "ne", "sw", "se"] as DragAction[]).map((action) => (
            <span
              className={`resize-handle handle-${action}`}
              key={action}
              onPointerDown={(event) => startDrag(event, action)}
            />
          ))}
        </div>
      )}
      {isMouseActive && (
        <div
          className="mouse-capture-region"
          style={{ left: region.x, top: region.y, width: region.width, height: region.height }}
        >
          <span className="overlay-label">Mouse 240×120</span>
        </div>
      )}
      {isAutoActive && autoBordersVisible && finding?.detectedLines.map((line, index) => (
        <div
          className="auto-detected-line"
          key={`${index}-${line.x}-${line.y}-${line.width}-${line.height}`}
          style={{ left: line.x, top: line.y, width: line.width, height: line.height }}
        />
      ))}
      {isActive && (
        <button
          aria-label="Stop OCR and return to wis"
          className="overlay-close"
          disabled={isStopping}
          style={controlStyle}
          type="button"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={closeCapture}
        >
          ×
        </button>
      )}
      {isReadingActive && (
        <div
          aria-live="polite"
          className={`overlay-finding ${finding?.error ? "has-error" : ""}`}
          style={findingStyle}
          tabIndex={0}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <div className="overlay-finding-heading">
            <span>{finding ? "Latest finding" : "Looking…"}</span>
            {finding && !finding.error && <small>{finding.confidence}%</small>}
          </div>
          {finding?.previewImageDataUrl && (
            <figure className="overlay-preview">
              <img src={finding.previewImageDataUrl} alt="Latest image sent to OCR" />
              <figcaption>OCR input</figcaption>
            </figure>
          )}
          <p>{finding?.error || finding?.text || (finding ? "No text found" : "Waiting for the first scan")}</p>
          {finding && <small>{finding.elapsedMs}ms · {new Date(finding.observedAtMs).toLocaleTimeString()}</small>}
        </div>
      )}
    </div>
  );
}

function mouseAxisStart(position: number, size: number, screenSize: number): number {
  if (screenSize <= size) return 0;
  if (position + size <= screenSize) return position;
  return Math.max(0, position - size);
}

function resizeRegion(initial: Region, action: DragAction, deltaX: number, deltaY: number): Region {
  if (action === "move") {
    return {
      ...initial,
      x: Math.max(0, initial.x + deltaX),
      y: Math.max(0, initial.y + deltaY),
    };
  }
  let left = initial.x;
  let top = initial.y;
  let right = initial.x + initial.width;
  let bottom = initial.y + initial.height;
  if (action.includes("n")) top = Math.min(bottom - minimumSize, Math.max(0, top + deltaY));
  if (action.includes("s")) bottom = Math.max(top + minimumSize, bottom + deltaY);
  if (action.includes("w")) left = Math.min(right - minimumSize, Math.max(0, left + deltaX));
  if (action.includes("e")) right = Math.max(left + minimumSize, right + deltaX);
  return { x: left, y: top, width: right - left, height: bottom - top };
}
