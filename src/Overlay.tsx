import { PointerEvent, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getOverlayMode,
  getSettings,
  OverlayMode,
  Region,
  saveSettings,
  setOverlayMode,
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
  const settingsRef = useRef(settings);
  const dragRef = useRef<DragState | null>(null);

  function updateSettings(next: WisSettings) {
    settingsRef.current = next;
    setSettings(next);
  }

  useEffect(() => {
    Promise.all([getSettings(), getOverlayMode()]).then(([loaded, loadedMode]) => {
      updateSettings(loaded);
      setMode(loadedMode);
    });
    const settingsSubscription = listen<WisSettings>("settings-changed", (event) => {
      if (!dragRef.current) updateSettings(event.payload);
    });
    const modeSubscription = listen<OverlayMode>("overlay-mode-changed", (event) => {
      setMode(event.payload);
    });
    return () => {
      settingsSubscription.then((unlisten) => unlisten());
      modeSubscription.then((unlisten) => unlisten());
    };
  }, []);

  function startDrag(event: PointerEvent, action: DragAction) {
    if (mode !== "edit") return;
    event.preventDefault();
    event.stopPropagation();
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

  const region = settings.region;
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
      {mode === "edit" && (
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
