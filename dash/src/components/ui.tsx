import { useEffect, useRef, useState, type ReactNode } from "react";
import { ApiError, type GoalState } from "../api";

export function Wordmark({ host }: { host?: string }) {
  return (
    <span className="wordmark">
      <svg width="20" height="20" viewBox="0 0 22 22" aria-hidden="true">
        <path d="M2 6 H20 M2 11 H20 M2 16 H20" className="mark-weft" />
        <path d="M6 3 V19 M11 3 V19 M16 3 V19" className="mark-warp" />
      </svg>
      <span className="wordmark-name">subweave</span>
      {host && <span className="wordmark-host">/ {host}</span>}
    </span>
  );
}

/** A goal or service state: the shape says it without the colour. `running`: a repair runs. */
export type Shown = GoalState | "running" | "ask";

const LABEL: Record<Shown, string> = {
  holds: "holds",
  suspect: "failed once",
  fails: "fails",
  unknown: "unknown",
  off: "off",
  waiting: "waiting",
  running: "running now",
  ask: "escalation open",
};

export function Glyph({ state, title }: { state: Shown; title?: string }) {
  return <span className={`glyph glyph-${state}`} role="img" aria-label={title ?? LABEL[state]} title={title ?? LABEL[state]} />;
}

export function Skeleton({ lines = 3, height = 14 }: { lines?: number; height?: number }) {
  return (
    <div className="skeleton" aria-busy="true" aria-label="Loading">
      {Array.from({ length: lines }, (_, i) => (
        <span key={i} style={{ height, width: `${92 - ((i * 17) % 40)}%` }} />
      ))}
    </div>
  );
}

export function Panel({
  title,
  aside,
  children,
  className = "",
}: {
  title?: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`panel ${className}`}>
      {(title || aside) && (
        <header className="panel-head">
          {title && <h3>{title}</h3>}
          {aside}
        </header>
      )}
      {children}
    </section>
  );
}

export function ErrorLine({ error }: { error: string | null | undefined }) {
  if (!error) return null;
  return (
    <p className="error-line" role="alert">
      {error}
    </p>
  );
}

export function errorText(e: unknown): string {
  return e instanceof ApiError ? e.message : String(e);
}

/** A button that asks first in a small popover, then runs `act`; errors show under it. */
export function Confirm({
  label,
  text,
  confirm,
  act,
  danger = false,
  className = "btn",
  disabled = false,
}: {
  label: ReactNode;
  text: ReactNode;
  confirm: string;
  act: () => Promise<unknown>;
  danger?: boolean;
  className?: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", down);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", down);
      document.removeEventListener("keydown", key);
    };
  }, [open]);

  async function run() {
    setBusy(true);
    setError(null);
    try {
      await act();
      setOpen(false);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="confirm" ref={ref}>
      <button type="button" className={className} disabled={disabled} aria-expanded={open} onClick={() => setOpen(!open)}>
        {label}
      </button>
      {open && (
        <div className="popover" role="dialog">
          <div className="popover-text">{text}</div>
          <div className="popover-actions">
            <button type="button" className="btn btn-quiet" onClick={() => setOpen(false)}>
              Cancel
            </button>
            <button
              type="button"
              className={danger ? "btn btn-danger" : "btn btn-primary"}
              disabled={busy}
              autoFocus
              onClick={run}
            >
              {confirm}
            </button>
          </div>
        </div>
      )}
      <ErrorLine error={error} />
    </div>
  );
}

export function FactsTable({ facts }: { facts: Record<string, string> }) {
  const entries = Object.entries(facts);
  if (!entries.length) return null;
  return (
    <table className="facts">
      <tbody>
        {entries.map(([k, v]) => (
          <tr key={k}>
            <th>{k}</th>
            <td>
              <div className={v.includes("\n") ? "fact-v fact-lines" : "fact-v"}>{v}</div>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: readonly T[];
  onChange: (v: T) => void;
  label: string;
}) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button key={o} type="button" role="radio" aria-checked={o === value} onClick={() => onChange(o)}>
          {o}
        </button>
      ))}
    </div>
  );
}
