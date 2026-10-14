import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api, logStreamUrl } from "../api";
import { ErrorLine, Skeleton } from "../components/ui";
import { time, utc } from "../format";
import { useResource } from "../resource";
import type { ServiceProps } from "./ServiceView";

const MAX = 5000;

interface LogLine {
  k: number;
  t: string;
  text: string;
  level: "error" | "warning" | "info";
}

export function Logs({ service }: ServiceProps) {
  const streams = useResource(`logs:${service}`, () => api.logs(service));
  const [pick, setPick] = useState<string | null>(null);
  const current = pick ?? streams.data?.[0]?.name ?? null;
  const stream = streams.data?.find((s) => s.name === current);

  return (
    <div className="logs">
      <nav className="log-streams" aria-label="Streams">
        {streams.loading && <Skeleton lines={3} />}
        <ErrorLine error={streams.error} />
        {streams.data?.length === 0 && <p className="t-muted small">No streams.</p>}
        {streams.data?.map((s) => (
          <button
            key={s.name}
            type="button"
            className={`log-stream ${s.name === current ? "is-current" : ""}`}
            aria-current={s.name === current}
            onClick={() => setPick(s.name)}
          >
            <span className="log-stream-name">{s.source === "box" ? "Box journal" : s.name}</span>
            <span className="log-stream-title t-muted">{s.source === "box" ? "systemd journal" : s.title}</span>
          </button>
        ))}
      </nav>
      {stream ? <Viewer key={stream.name} service={service} name={stream.name} box={stream.source === "box"} /> : <div className="viewer" />}
    </div>
  );
}

function Viewer({ service, name, box }: { service: string; name: string; box: boolean }) {
  const [lines, setLines] = useState<LogLine[]>([]);
  const [state, setState] = useState<"connecting" | "open" | "retrying" | "ended">("connecting");
  const [endText, setEndText] = useState<string | null>(null);
  const [wrap, setWrap] = useState(true);
  const [paused, setPaused] = useState(false);
  const [filter, setFilter] = useState("");
  const [unit, setUnit] = useState("");
  const [priority, setPriority] = useState("");
  const [atBottom, setAtBottom] = useState(true);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const pending = useRef<LogLine[]>([]);
  const seq = useRef(0);
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setLines([]);
    setEndText(null);
    setState("connecting");
    pending.current = [];
    const es = new EventSource(logStreamUrl(service, name, box ? { unit, priority } : undefined), { withCredentials: true });
    let flush: number | undefined;
    const push = () => {
      flush = undefined;
      if (pausedRef.current || !pending.current.length) return;
      const add = pending.current;
      pending.current = [];
      setLines((ls) => {
        const next = ls.concat(add);
        return next.length > MAX ? next.slice(next.length - MAX) : next;
      });
    };
    es.onopen = () => setState("open");
    es.addEventListener("line", (e) => {
      const l = JSON.parse((e as MessageEvent).data) as Omit<LogLine, "k">;
      pending.current.push({ ...l, k: seq.current++ });
      if (pending.current.length > MAX) pending.current.splice(0, pending.current.length - MAX);
      flush ??= window.setTimeout(push, 100);
    });
    es.addEventListener("end", (e) => {
      const end: { text: string } = JSON.parse((e as MessageEvent).data);
      setEndText(end.text);
      setState("ended");
      es.close();
      push();
    });
    es.onerror = () => {
      if (es.readyState !== EventSource.CLOSED) {
        setState("retrying");
        return;
      }
      // Refused outright (a 429 for too many open streams, or the session ended): the
      // browser gives no body, so say it plainly.
      setEndText("The stream closed. Close other log tabs, or reload the page.");
      setState("ended");
    };
    return () => {
      es.close();
      window.clearTimeout(flush);
    };
  }, [service, name, box, unit, priority]);

  // Resuming shows what came while paused.
  useEffect(() => {
    if (paused || !pending.current.length) return;
    const add = pending.current;
    pending.current = [];
    setLines((ls) => ls.concat(add).slice(-MAX));
  }, [paused]);

  const shown = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return f ? lines.filter((l) => l.text.toLowerCase().includes(f)) : lines;
  }, [lines, filter]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && atBottom) el.scrollTop = el.scrollHeight;
  }, [shown, atBottom, wrap]);

  const jump = () => {
    const el = scroller.current;
    if (el) el.scrollTop = el.scrollHeight;
    setAtBottom(true);
  };

  return (
    <div className="viewer">
      <div className="viewer-bar">
        <input type="search" placeholder="Filter lines" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter lines" />
        {box && (
          <>
            <select value={unit} onChange={(e) => setUnit(e.target.value)} aria-label="Unit">
              <option value="">All units</option>
              <option value="subweave.service">subweave</option>
              <option value="subweave-dash.service">subweave-dash</option>
              <option value={`${service}*`}>{service}*</option>
            </select>
            <select value={priority} onChange={(e) => setPriority(e.target.value)} aria-label="Priority">
              <option value="">Any priority</option>
              <option value="warning">Warning and worse</option>
              <option value="err">Error and worse</option>
            </select>
          </>
        )}
        <label className="check">
          <input type="checkbox" checked={wrap} onChange={(e) => setWrap(e.target.checked)} />
          Wrap
        </label>
        <button type="button" className={`btn btn-small ${paused ? "btn-primary" : ""}`} onClick={() => setPaused(!paused)}>
          {paused ? "Resume" : "Pause"}
        </button>
        <span className={`viewer-state small ${state === "retrying" ? "t-amber" : "t-muted"}`}>
          {state === "connecting" ? "Connecting" : state === "retrying" ? "Reconnecting" : state === "ended" ? "Ended" : `${shown.length} lines`}
        </span>
      </div>
      <div
        className={`viewer-lines ${wrap ? "is-wrap" : ""}`}
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          setAtBottom(el.scrollHeight - el.scrollTop - el.clientHeight < 24);
        }}
        role="log"
        aria-live="off"
      >
        {state === "connecting" && lines.length === 0 && <Skeleton lines={6} height={12} />}
        {state !== "connecting" && shown.length === 0 && !endText && <p className="t-muted viewer-empty">No lines yet. New lines appear here as they come.</p>}
        {shown.map((l) => (
          <div key={l.k} className={`log-line log-${l.level}`}>
            <time className="log-time" title={utc(l.t)}>
              {time(l.t)}
            </time>
            <span className="log-text">{l.text}</span>
          </div>
        ))}
        {endText && <p className="viewer-end t-muted">{endText}</p>}
      </div>
      {!atBottom && (
        <button type="button" className="btn btn-small jump" onClick={jump}>
          Jump to newest
        </button>
      )}
    </div>
  );
}
