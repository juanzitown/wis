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
        setMessage("Choose a Mouse region or full-screen Auto isolation.");
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
      if (event.payload.mode !== "auto") {
        setFindings((current) => [event.payload, ...current].slice(0, 12));
      }
    });
    return () => {
      settingsSubscription.then((unlisten) => unlisten());
      statusSubscription.then((unlisten) => unlisten());
      ocrSubscription.then((unlisten) => unlisten());
    };
  }, []);

  async function save() {
    try {
      const saved = await saveSettings(settings);
      setSettings(saved);
      setMessage("Scan interval saved.");
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
        mode === "mouse"
          ? "Drag around a screen region; isolation starts when you release the mouse."
          : "Auto isolation is scanning the whole screen.",
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
          <p className="lede">Drag around one region or isolate text-like glyphs across the whole screen.</p>
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

          <div className="interval-row">
            <div>
              <strong>Look every</strong>
              <small>Auto mode scan interval; Mouse runs when a drag selection is released</small>
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
            <button className="primary" type="button" disabled={!isIdle} onClick={() => beginWatching("mouse")}>Start mouse</button>
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
              <h2>Latest isolation</h2>
            </div>
            {latest && <time>{new Date(latest.observedAtMs).toLocaleTimeString()}</time>}
          </div>

          <div className={`latest-finding ${latest?.error ? "has-error" : ""}`}>
            {!latest ? (
              <p>Nothing seen yet.</p>
            ) : latest.error ? (
              <p>{latest.error}</p>
            ) : (
              <pre>{latest.detectedLines.length} text-like region{latest.detectedLines.length === 1 ? "" : "s"} found</pre>
            )}
            {latest && !latest.error && (
              <small>{latest.isolationSteps.length} retained steps · {formatDuration(latest.elapsedMicros)}</small>
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
                <p>{finding.error || `${finding.detectedLines.length} text-like regions`}</p>
                <span>{finding.error ? "error" : formatDuration(finding.elapsedMicros)}</span>
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

function formatDuration(micros: number): string {
  if (micros < 1) return "<1µs";
  if (micros < 1_000) return `${Math.round(micros)}µs`;
  if (micros < 10_000) return `${(micros / 1_000).toFixed(2)}ms`;
  return `${(micros / 1_000).toFixed(1)}ms`;
}
