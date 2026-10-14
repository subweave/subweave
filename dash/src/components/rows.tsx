import { useState, type ReactNode } from "react";
import type { Kind, Line } from "../api";
import { ago, clock, day, ms, time, usd, utc } from "../format";
import type { Episode } from "../model";
import { FactsTable, Glyph } from "./ui";

const ICON: Record<Kind, ReactNode> = {
  check: null,
  decide: <path d="M6 1.5 10.5 6 6 10.5 1.5 6Z" />,
  gate: <path d="M6 1 10 2.6V6c0 2.4-1.7 4.2-4 5-2.3-.8-4-2.6-4-5V2.6Z" />,
  action: <path d="M3.5 2v8L10 6Z" />,
  command: <path d="M2 3l3 3-3 3M6.5 9.5H10" fill="none" />,
  learn: <path d="M3 1.5h6v9L6 8.5 3 10.5Z" />,
  ask: <path d="M4.3 4.4a1.8 1.8 0 1 1 2.6 1.6c-.6.3-.9.7-.9 1.3M6 9.4v.1" fill="none" />,
  answer: <path d="M5 2.5 2 5.5l3 3M2.5 5.5H7a3 3 0 0 1 3 3v1" fill="none" />,
  message: <path d="M1.5 3h9v6h-9ZM1.5 3 6 6.5 10.5 3" fill="none" />,
  note: <circle cx="6" cy="6" r="1.8" />,
};

/** Lines a model decided or did: violet (the Weavefile's "quoted means model"). */
export function byModel(l: Line): boolean {
  if (l.kind === "decide" || l.kind === "gate") return true;
  if (l.kind === "command") return l.cost_usd !== undefined;
  if (l.kind === "note") return !!l.action;
  if (l.kind === "learn") return true;
  return false;
}

export function KindGlyph({ line }: { line: Line }) {
  if (line.kind === "check") {
    return <Glyph state={line.ok === true ? "holds" : line.ok === false ? "fails" : "unknown"} />;
  }
  const tone =
    line.kind === "gate"
      ? gateTone(line)
      : line.kind === "ask"
        ? "amber"
        : line.kind === "command" && line.ok === false
          ? "red"
          : byModel(line)
            ? "model"
            : "ink";
  return (
    <svg className={`kind kind-${tone}`} viewBox="0 0 12 12" aria-label={line.kind} role="img">
      {ICON[line.kind]}
    </svg>
  );
}

function gateTone(l: Line): "red" | "amber" {
  return /^ASK\b/.test(l.text ?? "") ? "amber" : "red";
}

export function Time({ t, now }: { t: string; now: number }) {
  const sameDay = new Date(ms(t)).toDateString() === new Date(now).toDateString();
  return (
    <time className="row-time" dateTime={t} title={`${utc(t)} · ${ago(t, now)}`}>
      {sameDay ? time(t) : day(t)}
      {!sameDay && <span className="row-time-2">{time(t)}</span>}
    </time>
  );
}

function Expand({ children, label }: { children: ReactNode; label: string }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" className="disclose" aria-expanded={open} onClick={() => setOpen(!open)}>
        {label}
      </button>
      {open && <div className="row-more">{children}</div>}
    </>
  );
}

function Command({ text }: { text: string }) {
  const [all, setAll] = useState(false);
  const long = text.split("\n").length > 2 || text.length > 180;
  return (
    <code className={`cmd ${all ? "" : "cmd-clamp"}`} onClick={long ? () => setAll(true) : undefined} title={long && !all ? "Show all" : undefined}>
      {text}
    </code>
  );
}

function Choices({ p, pick }: { p: Record<string, number>; pick?: string }) {
  const sorted = Object.entries(p).sort((a, b) => b[1] - a[1]);
  return (
    <span className="choices">
      {sorted.map(([name, v]) => (
        <span key={name} className={`choice ${name === pick ? "is-pick" : ""}`}>
          {name}
          <span className="num">{v.toFixed(2)}</span>
          <span className="bar" aria-hidden="true">
            <span style={{ width: `${Math.round(v * 100)}%` }} />
          </span>
        </span>
      ))}
    </span>
  );
}

function splitGate(text: string): { rule: string; command: string | null } {
  const m = /^((?:NEVER|ASK)\b[^"]*"[^"]*"|(?:NEVER|ASK)[^:]*):\s*([\s\S]*)$/.exec(text);
  return m ? { rule: m[1], command: m[2] } : { rule: text, command: null };
}

function Body({ line: l }: { line: Line }) {
  const goal = l.goal ? <span className="row-goal">{l.goal}</span> : null;
  switch (l.kind) {
    case "check":
      return (
        <>
          <span className="row-main">
            {goal} {l.ok === true ? "holds" : l.ok === false ? <span className="t-red">fails</span> : <span className="t-muted">unknown</span>}
          </span>
          {l.facts && Object.keys(l.facts).length > 0 && (
            <Expand label="Facts">
              <FactsTable facts={l.facts} />
            </Expand>
          )}
        </>
      );
    case "decide":
      return (
        <>
          <span className="row-main">
            {goal && <>{goal} → </>}
            <span className="t-model">{l.text}</span>
          </span>
          {l.p && <Choices p={l.p} pick={l.text} />}
        </>
      );
    case "gate": {
      const g = splitGate(l.text ?? "");
      const tone = gateTone(l);
      const p = l.p ? Object.values(l.p)[0] : undefined;
      return (
        <>
          <span className="row-main">
            <span className={tone === "red" ? "t-red" : "t-amber"}>{tone === "red" ? "Refused" : "Asked"}</span>{" "}
            <span className="t-model">{g.rule}</span>
            {p !== undefined && <span className="tag">p {p.toFixed(2)}</span>}
          </span>
          {g.command && <Command text={g.command} />}
        </>
      );
    }
    case "command":
      return (
        <>
          <span className="row-main row-cmd">
            <Command text={l.text ?? ""} />
            <span className={`tag ${l.ok === false ? "tag-red" : ""}`}>{l.ok === false ? "failed" : "ok"}</span>
          </span>
          {l.output && (
            <Expand label="Output">
              <pre className="output">{l.output}</pre>
            </Expand>
          )}
        </>
      );
    case "action": {
      if (l.action === "task") {
        return (
          <span className="row-main">
            {l.ok === undefined ? <>Task from {l.by ?? "a person"}: {l.text}</> : <>Task {l.ok ? "done" : <span className="t-red">failed</span>}{l.text && <span className="t-muted">: {l.text}</span>}</>}
          </span>
        );
      }
      return (
        <span className="row-main">
          <b className="w-6">{l.action}</b>
          {l.goal && <> for {goal}</>}
          {l.ok === undefined ? " started" : l.ok ? " ended, the goal holds" : <span className="t-red"> ended, the goal still fails</span>}
          {l.by && <span className="t-muted"> · {l.by}</span>}
          {l.text && <span className="t-muted"> · {l.text}</span>}
        </span>
      );
    }
    case "learn": {
      const m = /^([^:]+):\s*([\s\S]*)$/.exec(l.text ?? "");
      return m ? (
        <>
          <span className="row-main">Learned {m[1]}</span>
          <Command text={m[2]} />
        </>
      ) : (
        <span className="row-main">Learned {l.text}</span>
      );
    }
    case "ask":
      return (
        <>
          <span className="row-main">
            <span className="t-amber">Ask #{l.n}</span> {l.text}
          </span>
          {l.body && <span className="row-sub">{l.body}</span>}
          {l.facts && Object.keys(l.facts).length > 0 && (
            <Expand label="Facts">
              <FactsTable facts={l.facts} />
            </Expand>
          )}
        </>
      );
    case "answer":
      // The daemon closes an ask whose goal holds again: by "daemon: <goal> holds again".
      if (l.text === "withdrawn") {
        return (
          <span className="row-main">
            Ask #{l.n} withdrawn
            {l.by && <span className="t-muted">: {l.by.replace(/^daemon:\s*/, "")}</span>}
          </span>
        );
      }
      return (
        <span className="row-main">
          Ask #{l.n} answered <b className="w-6">{l.text}</b>
          {l.by && <span className="t-muted"> by {l.by}</span>}
        </span>
      );
    default:
      return (
        <span className="row-main">
          {goal && <>{goal}: </>}
          {l.text}
          {l.by && <span className="t-muted"> · {l.by}</span>}
        </span>
      );
  }
}

export function Row({ line, now, service }: { line: Line; now: number; service?: string }) {
  return (
    <li className="row">
      <Time t={line.t} now={now} />
      <span className="row-glyph">
        <KindGlyph line={line} />
      </span>
      <div className="row-body">
        {service && <span className="row-service">{service}</span>}
        <Body line={line} />
      </div>
      {line.cost_usd !== undefined && <span className="row-cost">{usd(line.cost_usd)}</span>}
    </li>
  );
}

export function EpisodeCard({ ep, now, hidden }: { ep: Episode; now: number; hidden: Set<number> }) {
  const running = ep.outcome === null;
  const [open, setOpen] = useState(running);
  const took = (ep.end ? ms(ep.end) : now) - ms(ep.start);
  const tone = running ? "running" : ep.outcome === "holds" ? "holds" : "fails";
  const shown = ep.lines.filter((l) => !hidden.has(l.i));
  return (
    <li className={`episode episode-${tone}`}>
      <button type="button" className="episode-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Time t={ep.start} now={now} />
        <span className="row-glyph">
          <Glyph state={tone === "running" ? "running" : tone} />
        </span>
        <span className="episode-title">
          {ep.task ? (
            <>
              Task from {ep.by ?? "a person"}: <span className="w-5">{ep.text}</span>
            </>
          ) : (
            <>
              <b className="w-6">{ep.goal}</b> <span className="t-red">fails</span>
              {ep.actions.map((a) => (
                <span key={a}>
                  {" "}
                  → <span className="t-model">{a}</span>
                </span>
              ))}{" "}
              → {running ? <span className="t-model">now</span> : <span className="t-accent">holds</span>}
            </>
          )}
          <span className="t-muted num"> · {clock(took)}</span>
          {ep.task && !running && <span className={ep.outcome === "holds" ? "t-accent" : "t-red"}> · {ep.outcome === "holds" ? "done" : "failed"}</span>}
        </span>
        <span className="episode-count t-muted num">{ep.lines.length} lines</span>
      </button>
      {open && (
        <ol className="rows">
          {shown.map((l) => (
            <Row key={l.i} line={l} now={now} />
          ))}
        </ol>
      )}
    </li>
  );
}
