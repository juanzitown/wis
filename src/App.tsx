import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  fallbackSettings,
  getRuntimeStatus,
  getSettings,
  CaptureMode,
  OcrUpdate,
  RuntimeStatus,
  saveSettings,
  startOcr,
  stopOcr,
  WisSettings,
} from "./tauri";

type RegionKey = "x" | "y" | "width" | "height";

function NumberField({
  label,
  value,
  min,
  max,
  step,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max?: number;
  step?: number;
  onChange: (value: number) => void;
}) {
  return (
    <label className="field">
      <span>{label}</span>
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        step={step}
        onChange={(event) => onChange(Number(event.target.value))}
      />
    </label>
  );
}

export default function App() {
  const [settings, setSettings] = useState<WisSettings>(fallbackSettings);
  const [status, setStatus] = useState<RuntimeStatus | null>(null);
  const [findings, setFindings] = useState<OcrUpdate[]>([]);
  const [message, setMessage] = useState("Getting the text area ready…");

  useEffect(() => {
    Promise.all([getSettings(), getRuntimeStatus()]).then(
      ([loadedSettings, loadedStatus]) => {
        setSettings(loadedSettings);
        setStatus(loadedStatus);
        setMessage("Choose manual OCR or automatic text-line detection.");
      },
    );
    const settingsSubscription = listen<WisSettings>("settings-changed", (event) => {
      setSettings(event.payload);
    });
    const statusSubscription = listen<RuntimeStatus>("ocr-status", (event) => {
      setStatus(event.payload);
      if (event.payload.lastError) setMessage(event.payload.lastError);
    });
    const ocrSubscription = listen<OcrUpdate>("ocr-update", (event) => {
      if (event.payload.mode === "manual") {
        setFindings((current) => [event.payload, ...current].slice(0, 12));
      }
    });
    return () => {
      settingsSubscription.then((unlisten) => unlisten());
      statusSubscription.then((unlisten) => unlisten());
      ocrSubscription.then((unlisten) => unlisten());
    };
  }, []);

  function updateRegion(key: RegionKey, value: number) {
    if (!Number.isFinite(value)) return;
    const minimum = key === "width" || key === "height" ? 1 : 0;
    setSettings((current) => ({
      ...current,
      region: { ...current.region, [key]: Math.max(minimum, Math.round(value)) },
    }));
  }

  async function save() {
    try {
      const saved = await saveSettings(settings);
      setSettings(saved);
      setMessage("Text area and scan interval saved.");
    } catch (error) {
      setMessage(String(error));
    }
  }

  async function beginWatching(mode: CaptureMode) {
    try {
      const saved = await saveSettings(settings);
      setSettings(saved);
      setFindings([]);
      const next = await startOcr(mode);
      setStatus(next);
      setMessage(
        mode === "manual"
          ? "Manual OCR is active. Drag or resize the area on screen."
          : "Auto detection is scanning the screen for text lines.",
      );
    } catch (error) {
      setMessage(String(error));
    }
  }

  async function endWatching() {
    try {
      setStatus(await stopOcr());
      setMessage("Stopping screen capture…");
    } catch (error) {
      setMessage(String(error));
    }
  }

  const latest = findings[0];
  const isIdle = !status || status.phase === "idle";

  return (
    <main className="shell">
      <header className="hero">
        <div>
          <p className="wordmark">wis <span>/ what i see</span></p>
          <h1>Watch a piece of your screen.</h1>
          <p className="lede">Read one area manually, or let wis outline every text line it can find.</p>
        </div>
        <div className={`runtime-badge ${status?.running ? "is-running" : ""}`}>
          <span />
          {status?.phase === "running" ? "watching" : status?.phase ?? "idle"}
        </div>
      </header>

      <section className="workspace">
        <div className="panel setup-panel">
          <div className="section-heading">
            <div>
              <p className="kicker">Capture mode</p>
              <h2>How should I look?</h2>
            </div>
            <span className="mode">{status?.mode ?? "ready"}</span>
          </div>

          <div className="field-grid">
            <NumberField label="X" value={settings.region.x} min={0} onChange={(value) => updateRegion("x", value)} />
            <NumberField label="Y" value={settings.region.y} min={0} onChange={(value) => updateRegion("y", value)} />
            <NumberField label="Width" value={settings.region.width} min={32} max={8192} onChange={(value) => updateRegion("width", value)} />
            <NumberField label="Height" value={settings.region.height} min={32} max={8192} onChange={(value) => updateRegion("height", value)} />
          </div>

          <div className="interval-row">
            <div>
              <strong>Look every</strong>
              <small>How often Manual OCR or Auto detection scans</small>
            </div>
            <div className="interval-input">
              <input
                aria-label="Scan interval in seconds"
                type="number"
                min={0.25}
                max={60}
                step={0.25}
                value={settings.scanIntervalMs / 1000}
                onChange={(event) => setSettings((current) => ({
                  ...current,
                  scanIntervalMs: Math.round(Number(event.target.value) * 1000),
                }))}
              />
              <span>sec</span>
            </div>
          </div>

          <div className="button-row">
            <button className="primary" type="button" disabled={!isIdle} onClick={() => beginWatching("manual")}>Start manual</button>
            <button className="secondary" type="button" disabled={!isIdle} onClick={() => beginWatching("auto")}>Start auto</button>
            <button className="secondary" type="button" disabled={isIdle} onClick={endWatching}>Stop</button>
            <button className="quiet" type="button" disabled={!isIdle} onClick={save}>Save</button>
          </div>
          <p className="message">{message}</p>
        </div>

        <aside className="panel findings-panel">
          <div className="section-heading">
            <div>
              <p className="kicker">Findings</p>
              <h2>Latest text</h2>
            </div>
            {latest && <time>{new Date(latest.observedAtMs).toLocaleTimeString()}</time>}
          </div>

          <div className={`latest-finding ${latest?.error ? "has-error" : ""}`}>
            {!latest ? (
              <p>Nothing seen yet.</p>
            ) : latest.error ? (
              <p>{latest.error}</p>
            ) : (
              <pre>{latest.text || "No text found"}</pre>
            )}
            {latest && !latest.error && (
              <small>{latest.confidence}% confidence · {latest.elapsedMs}ms</small>
            )}
          </div>

          <div className="history">
            <div className="history-heading">
              <span>Recent attempts</span>
              <span>{findings.length}</span>
            </div>
            {findings.slice(1).map((finding) => (
              <div className="history-item" key={`${finding.observedAtMs}-${finding.elapsedMs}`}>
                <time>{new Date(finding.observedAtMs).toLocaleTimeString()}</time>
                <p>{finding.error || finding.text || "No text found"}</p>
                <span>{finding.error ? "error" : `${finding.confidence}%`}</span>
              </div>
            ))}
          </div>

          <dl className="diagnostics">
            <div><dt>Capture</dt><dd>{status?.frameSize?.join(" × ") ?? "—"}</dd></div>
            <div><dt>Session</dt><dd>{status?.sessionType ?? "—"}</dd></div>
          </dl>
        </aside>
      </section>
    </main>
  );
}
