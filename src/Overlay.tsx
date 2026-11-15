import { CSSProperties, PointerEvent, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  getRuntimeStatus,
  OcrUpdate,
  Region,
  RuntimeStatus,
  stopOcr,
  submitMouseRegion,
} from "./tauri";

type DragSelection = {
  startX: number;
  startY: number;
};

export default function Overlay() {
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [finding, setFinding] = useState<OcrUpdate | null>(null);
  const [selection, setSelection] = useState<Region | null>(null);
  const [draftSelection, setDraftSelection] = useState<Region | null>(null);
  const [selectionError, setSelectionError] = useState<string | null>(null);
  const [isSelecting, setIsSelecting] = useState(false);
  const [isolationBordersVisible, setIsolationBordersVisible] = useState(false);
  const [isStopping, setIsStopping] = useState(false);
  const dragRef = useRef<DragSelection | null>(null);

  useEffect(() => {
    getRuntimeStatus().then(setStatus);
    const statusSubscription = listen<RuntimeStatus>("ocr-status", (event) => {
      setStatus(event.payload);
      if (event.payload.phase === "starting") {
        setFinding(null);
        setSelection(null);
        setDraftSelection(null);
        setSelectionError(null);
        setIsolationBordersVisible(false);
      }
      if (event.payload.phase === "idle") {
        setIsStopping(false);
        dragRef.current = null;
        setIsSelecting(false);
      }
    });
    const isolationSubscription = listen<OcrUpdate>("ocr-update", (event) => {
      setFinding(event.payload);
      setIsolationBordersVisible(true);
    });
    const visibilitySubscription = listen<boolean>("isolation-borders-visible", (event) => {
      setIsolationBordersVisible(event.payload);
    });
    return () => {
      statusSubscription.then((unlisten) => unlisten());
      isolationSubscription.then((unlisten) => unlisten());
      visibilitySubscription.then((unlisten) => unlisten());
    };
  }, []);

  const isActive = status?.phase !== "idle" && status !== null;
  const isAutoActive = isActive && status?.mode === "auto";
  const isMouseActive = isActive && status?.mode === "mouse";

  function startSelection(event: PointerEvent<HTMLDivElement>) {
    if (!isMouseActive || event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    const point = clampPoint(event.clientX, event.clientY);
    dragRef.current = { startX: point.x, startY: point.y };
    setDraftSelection({ x: point.x, y: point.y, width: 1, height: 1 });
    setSelectionError(null);
    setFinding(null);
    setIsSelecting(true);
    setIsolationBordersVisible(true);
  }

  function moveSelection(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag) return;
    const point = clampPoint(event.clientX, event.clientY);
    setDraftSelection(regionBetween(drag.startX, drag.startY, point.x, point.y));
  }

  function finishSelection(event: PointerEvent<HTMLDivElement>) {
    const drag = dragRef.current;
    if (!drag) return;
    const point = clampPoint(event.clientX, event.clientY);
    const region = regionBetween(drag.startX, drag.startY, point.x, point.y);
    dragRef.current = null;
    setDraftSelection(null);
    setIsSelecting(false);
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    if (region.width < 2 || region.height < 2) {
      setSelectionError("Click and drag to select an area.");
      return;
    }
    setSelection(region);
    setIsolationBordersVisible(true);
    submitMouseRegion(region).catch((error) => setSelectionError(String(error)));
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

  const visibleSelection = draftSelection ?? selection;
  const controlsBelow = selection ? selection.y < 44 : false;
  const popupWidth = Math.min(440, window.innerWidth - 24);
  const popupHeight = Math.min(520, window.innerHeight - 24);
  const belowTop = selection ? selection.y + selection.height + (controlsBelow ? 52 : 12) : 12;
  const roomBelow = window.innerHeight - belowTop - 12;
  const roomAbove = selection ? selection.y - 24 : 0;
  const placeBelow = roomBelow >= Math.min(260, popupHeight) || roomBelow >= roomAbove;
  const availableHeight = Math.max(140, Math.min(popupHeight, placeBelow ? roomBelow : roomAbove));
  const popupTop = selection && !placeBelow
    ? Math.max(12, selection.y - availableHeight - 12)
    : belowTop;
  const findingStyle: CSSProperties = {
    left: selection
      ? Math.min(Math.max(12, selection.x), Math.max(12, window.innerWidth - popupWidth - 12))
      : 12,
    top: popupTop,
    width: popupWidth,
    maxHeight: availableHeight,
  };

  return (
    <div
      className={`overlay-surface ${isMouseActive ? "is-mouse" : ""} ${isSelecting ? "is-selecting" : ""}`}
      onPointerDown={startSelection}
      onPointerMove={moveSelection}
      onPointerUp={finishSelection}
      onPointerCancel={finishSelection}
    >
      {isMouseActive && !selection && !isSelecting && (
        <div className="overlay-help">Click and drag around the text you want to isolate</div>
      )}
      {isMouseActive && visibleSelection && (isSelecting || isolationBordersVisible) && (
        <div
          className="mouse-capture-region"
          style={{
            left: visibleSelection.x,
            top: visibleSelection.y,
            width: visibleSelection.width,
            height: visibleSelection.height,
          }}
        >
          <span className="overlay-label">
            {isSelecting ? "Selecting" : "Selected"} {visibleSelection.width}×{visibleSelection.height}
          </span>
        </div>
      )}
      {(isAutoActive || isMouseActive) && isolationBordersVisible && finding?.detectedLines.map((line, index) => (
        <div
          className="auto-detected-line"
          key={`${index}-${line.x}-${line.y}-${line.width}-${line.height}`}
          style={{ left: line.x, top: line.y, width: line.width, height: line.height }}
        />
      ))}
      {isActive && (
        <button
          aria-label="Stop isolation and return to wis"
          className="overlay-close"
          disabled={isStopping}
          style={{ right: 18, top: 18 }}
          type="button"
          onPointerDown={(event) => event.stopPropagation()}
          onClick={closeCapture}
        >
          ×
        </button>
      )}
      {isMouseActive && selection && !isSelecting && (
        <div
          aria-live="polite"
          className={`overlay-finding ${finding?.error || selectionError ? "has-error" : ""}`}
          style={findingStyle}
          onPointerDown={(event) => event.stopPropagation()}
        >
          <div className="overlay-finding-heading">
            <span>{finding ? "Isolation pipeline" : "Processing selection…"}</span>
            {finding && !finding.error && <small>{finding.detectedLines.length} regions</small>}
          </div>
          {finding?.isolationSteps.length ? (
            <div className="isolation-steps">
              {finding.isolationSteps.map((step, index) => (
                <figure className="overlay-preview" key={`${index}-${step.name}`}>
                  <img src={step.previewImageDataUrl} alt={`Isolation stage: ${step.name}`} />
                  <figcaption>
                    <span>{step.name}</span>
                    {step.elapsedMicros !== null && <small>{formatDuration(step.elapsedMicros)}</small>}
                  </figcaption>
                </figure>
              ))}
            </div>
          ) : null}
          <p>
            {selectionError
              || finding?.error
              || (finding ? `${finding.detectedLines.length} text-like regions found` : "Waiting for this selection")}
          </p>
          {finding && (
            <small>{formatDuration(finding.elapsedMicros)} total · {new Date(finding.observedAtMs).toLocaleTimeString()}</small>
          )}
        </div>
      )}
    </div>
  );
}

function clampPoint(x: number, y: number) {
  return {
    x: Math.round(Math.max(0, Math.min(window.innerWidth, x))),
    y: Math.round(Math.max(0, Math.min(window.innerHeight, y))),
  };
}

function regionBetween(startX: number, startY: number, endX: number, endY: number): Region {
  return {
    x: Math.min(startX, endX),
    y: Math.min(startY, endY),
    width: Math.max(1, Math.abs(endX - startX)),
    height: Math.max(1, Math.abs(endY - startY)),
  };
}

function formatDuration(micros: number): string {
  if (micros < 1) return "<1µs";
  if (micros < 1_000) return `${Math.round(micros)}µs`;
  if (micros < 10_000) return `${(micros / 1_000).toFixed(2)}ms`;
  return `${(micros / 1_000).toFixed(1)}ms`;
}
