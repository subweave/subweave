import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Line, type ServiceDetail, type Status, type Weave } from "../api";
import { useAppData } from "../App";
import { TaskComposer } from "../components/act";
import { ErrorLine, Glyph, Skeleton } from "../components/ui";
import { dur, ms, useNow } from "../format";
import { useJournal, useLive } from "../live";
import { counts } from "../model";
import { useResource } from "../resource";
import { linkClick, navigate, servicePath, TABS, type Tab } from "../router";
import { Activity } from "./Activity";
import { LearnedTab } from "./Learned";
import { Logs } from "./Logs";
import { MetricsTab } from "./Metrics";
import { Overview } from "./Overview";
import { Settings } from "./Settings";
import { WeaveTab } from "./Weave";

const TAB_LABEL: Record<Tab, string> = {
  overview: "Overview",
  activity: "Activity",
  metrics: "Metrics",
  logs: "Logs",
  learned: "Learned",
  weave: "Weave",
  settings: "Settings",
};

export interface ServiceProps {
  service: string;
  detail: ServiceDetail & { weave: Weave };
  status: Status | null;
  reload: () => void;
}

export function ServiceView({ service, tab }: { service: string; tab: Tab }) {
  const live = useLive();
  const { services } = useAppData();
  const [bump, setBump] = useState(0);
  const detail = useResource(`svc:${service}`, () => api.service(service), `${live.epoch}:${bump}`);
  const now = useNow(10_000);

  // Asks, answers, learned commands and settings live in the detail: load it again when
  // a line says one changed.
  const timer = useRef<number | undefined>(undefined);
  useJournal(
    service,
    useCallback((_s: string, l: Line) => {
      if (!["ask", "answer", "learn", "note", "action"].includes(l.kind)) return;
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setBump((b) => b + 1), 400);
    }, []),
  );
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const status = live.statuses[service] ?? detail.data?.status ?? null;
  const known = services?.some((s) => s.name === service);
  const reload = useCallback(() => setBump((b) => b + 1), []);

  if (detail.error && !detail.data) {
    return (
      <div className="page">
        <h1>{service}</h1>
        <ErrorLine error={known === false ? `No service named ${service} on this box.` : detail.error} />
      </div>
    );
  }

  const c = counts(status);
  const stopped = status?.stopped ?? detail.data?.spec.stopped ?? false;
  const staleFor = status && !stopped ? now - ms(status.t) : 0;
  const d = detail.data;
  // A Weavefile that does not parse still has the goals its status names.
  const weave: Weave | null = d
    ? (d.weave ?? {
        name: service,
        about: null,
        args: [],
        facts: [],
        goals: (status?.goals ?? []).map((g) => ({ name: g.name, after: [], every: null, tries: null, cond: { code: "" }, when: null, line: 0 })),
        actions: [],
        model: { decide: null, code: [] },
      })
    : null;
  const props = d && weave ? { service, detail: { ...d, weave }, status, reload } : null;

  return (
    <div className="page page-service">
      <header className="svc-head">
        <div className="svc-head-main">
          <h1>
            {service}
            {stopped && <span className="tag tag-muted">stopped</span>}
          </h1>
          {d ? (
            <p className="page-sub t-muted">
              {d.spec.request} · weave <span className="mono">{d.spec.weave.split("/").pop()}</span>
            </p>
          ) : (
            <Skeleton lines={1} />
          )}
          {status && (
            <p className="svc-counts num">
              {(["fails", "waiting", "unknown", "holds", "off"] as const)
                .filter((k) => c[k])
                .map((k) => (
                  <span key={k}>
                    <Glyph state={k} /> {c[k]} {k}
                  </span>
                ))}
              {status.repair && (
                <span className="t-model">
                  <Glyph state="running" /> {status.repair}
                </span>
              )}
            </p>
          )}
        </div>
        <HeadActions service={service} stopped={stopped} />
      </header>
      {staleFor > 120_000 && (
        <p className="stale" role="status">
          The daemon has not written status for {dur(staleFor)}. Is <code>subweave daemon</code> running?
        </p>
      )}
      {d && !d.weave && <p className="stale">The Weavefile on this box does not parse. Goals show from the daemon's status only.</p>}
      {!status && d && !stopped && <p className="stale">No status yet. The daemon writes it after its first check.</p>}
      <nav className="tabs" aria-label={`${service} sections`}>
        {TABS.map((t) => (
          <a
            key={t}
            href={servicePath(service, t)}
            onClick={linkClick}
            className={t === tab ? "is-current" : ""}
            aria-current={t === tab ? "page" : undefined}
          >
            {TAB_LABEL[t]}
          </a>
        ))}
      </nav>
      <div className="tab-body">
        {!props ? (
          <Skeleton lines={8} />
        ) : tab === "overview" ? (
          <Overview {...props} />
        ) : tab === "activity" ? (
          <Activity {...props} />
        ) : tab === "metrics" ? (
          <MetricsTab {...props} />
        ) : tab === "logs" ? (
          <Logs {...props} />
        ) : tab === "learned" ? (
          <LearnedTab {...props} />
        ) : tab === "weave" ? (
          <WeaveTab {...props} />
        ) : (
          <Settings {...props} />
        )}
      </div>
    </div>
  );
}

function HeadActions({ service, stopped }: { service: string; stopped: boolean }) {
  const [open, setOpen] = useState<"task" | "menu" | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(null);
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(null);
    document.addEventListener("pointerdown", down);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", down);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  const go = (path: string) => {
    setOpen(null);
    navigate(path);
  };
  return (
    <div className="svc-actions" ref={ref}>
      <button type="button" className="btn" aria-expanded={open === "task"} onClick={() => setOpen(open === "task" ? null : "task")}>
        Task
      </button>
      <button
        type="button"
        className="btn btn-icon"
        aria-label="More"
        aria-expanded={open === "menu"}
        onClick={() => setOpen(open === "menu" ? null : "menu")}
      >
        <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
          <circle cx="3.5" cy="8" r="1.3" />
          <circle cx="8" cy="8" r="1.3" />
          <circle cx="12.5" cy="8" r="1.3" />
        </svg>
      </button>
      {open === "task" && (
        <div className="popover popover-wide" role="dialog" aria-label={`A task for ${service}`}>
          <TaskComposer service={service} />
        </div>
      )}
      {open === "menu" && (
        <div className="popover menu" role="menu">
          <button type="button" role="menuitem" onClick={() => go(servicePath(service, "settings") + "#actions")}>
            Run an action
          </button>
          <button type="button" role="menuitem" onClick={() => go(servicePath(service, "settings") + "#tell")}>
            Change settings in words
          </button>
          <button type="button" role="menuitem" onClick={() => go(servicePath(service, "settings") + "#service")}>
            {stopped ? "Start the service" : "Stop the service"}
          </button>
        </div>
      )}
    </div>
  );
}
