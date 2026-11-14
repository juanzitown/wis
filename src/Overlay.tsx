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
  setOverlayMode,
  stopOcr,
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

export default function Overlay() {
  const [settings, setSettings] = useState(fallbackSettings);
  const [mode, setMode] = useState<OverlayMode>("hidden");
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [finding, setFinding] = useState<OcrUpdate | null>(null);
  const [isStopping, setIsStopping] = useState(false);
  const settingsRef = useRef(settings);
  const dragRef = useRef<DragState | null>(null);

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
      if (event.payload.phase === "starting") setFinding(null);
      if (event.payload.phase === "idle") setIsStopping(false);
    });
    const ocrSubscription = listen<OcrUpdate>("ocr-update", (event) => {
      setFinding(event.payload);
    });
    return () => {
      settingsSubscription.then((unlisten) => unlisten());
      modeSubscription.then((unlisten) => unlisten());
      statusSubscription.then((unlisten) => unlisten());
      ocrSubscription.then((unlisten) => unlisten());
    };
  }, []);

  function startDrag(event: PointerEvent, action: DragAction) {
    if (mode !== "edit") return;
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

  const region = settings.region;
  const isActive = status?.phase !== "idle" && status !== null;
  const controlsBelow = region.y < 44;
  const controlStyle: CSSProperties = {
    left: Math.min(Math.max(8, region.x + region.width - 32), Math.max(8, window.innerWidth - 40)),
    top: controlsBelow ? region.y + region.height + 8 : region.y - 38,
  };
  const popupWidth = Math.min(320, window.innerWidth - 24);
  const popupHeight = 150;
  const roomBelow = window.innerHeight - (region.y + region.height);
  const popupTop = roomBelow >= popupHeight + (controlsBelow ? 52 : 16)
    ? region.y + region.height + (controlsBelow ? 52 : 12)
    : Math.max(12, region.y - popupHeight - 12);
  const findingStyle: CSSProperties = {
    left: Math.min(Math.max(12, region.x), Math.max(12, window.innerWidth - popupWidth - 12)),
    top: popupTop,
    width: popupWidth,
  };
  return (
    <div
      className={`overlay-surface ${mode === "edit" ? "is-editing" : ""}`}
      onPointerMove={moveDrag}
      onPointerUp={finishDrag}
      onPointerCancel={finishDrag}
    >
      <div
        className="overlay-region"
        style={{ left: region.x, top: region.y, width: region.width, height: region.height }}
        onPointerDown={(event) => startDrag(event, "move")}
      >
        <span className="overlay-label">Text area</span>
        {(["nw", "ne", "sw", "se"] as DragAction[]).map((action) => (
          <span
            className={`resize-handle handle-${action}`}
            key={action}
            onPointerDown={(event) => startDrag(event, action)}
          />
        ))}
      </div>
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
      {isActive && (
        <div className={`overlay-finding ${finding?.error ? "has-error" : ""}`} style={findingStyle}>
          <div className="overlay-finding-heading">
            <span>{finding ? "Latest finding" : "Looking…"}</span>
            {finding && !finding.error && <small>{finding.confidence}%</small>}
          </div>
          <p>{finding?.error || finding?.text || (finding ? "No text found" : "Waiting for the first scan")}</p>
          {finding && <small>{finding.elapsedMs}ms · {new Date(finding.observedAtMs).toLocaleTimeString()}</small>}
        </div>
      )}
      {mode === "edit" && !isActive && (
        <>
          <div className="overlay-help">Move the area · drag a corner to resize</div>
          <div className="overlay-toolbar" onPointerDown={(event) => event.stopPropagation()}>
            <button type="button" onClick={() => setOverlayMode("preview")}>Done</button>
            <button type="button" onClick={() => setOverlayMode("hidden")}>Hide</button>
          </div>
        </>
      )}
    </div>
  );
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
