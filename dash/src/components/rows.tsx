import { useState, type ReactNode } from "react";
import type { Kind, Line, Usage } from "../api";
import { ago, clock, day, ms, modelName, time, tokens, usd, utc } from "../format";
import type { Episode, Outcome, Step } from "../model";
import { FactsTable, Glyph, type Shown } from "./ui";

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

/** A command on one line: its first line, cut at the edge, never wrapped. */
function Cmd({ text }: { text: string }) {
  const first = text.split("\n")[0];
  return (
    <code className="cmd-line" title={text.length > 200 ? undefined : text}>
      {first}
      {first !== text && <span className="t-muted"> …</span>}
    </code>
  );
}

/** A command too long for its line, whole. */
function long(text: string): boolean {
  return text.includes("\n") || text.length > 90;
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

function Facts({ facts }: { facts?: Record<string, string> }) {
  return facts && Object.keys(facts).length > 0 ? <FactsTable facts={facts} /> : null;
}

/** A step's one line, the tags at its end, and what opens under it. */
function describe({ line: l, gate }: Step): { main: ReactNode; tags?: ReactNode; more?: ReactNode } {
  const goal = l.goal ? <span className="row-goal">{l.goal}</span> : null;
  switch (l.kind) {
    case "check":
      return {
        main: (
          <>
            {goal} {l.ok === true ? "holds" : l.ok === false ? <span className="t-red">fails</span> : <span className="t-muted">unknown</span>}
          </>
        ),
        more: <Facts facts={l.facts} />,
      };
    case "decide": {
      const p = l.text !== undefined ? l.p?.[l.text] : undefined;
      return {
        main: (
          <>
            {goal && <>{goal} → </>}
            <span className="t-model">{l.text}</span>
          </>
        ),
        tags: p !== undefined && <span className="tag">p {p.toFixed(2)}</span>,
        more: l.p && <Choices p={l.p} pick={l.text} />,
      };
    }
    case "gate": {
      const g = splitGate(l.text ?? "");
      const tone = gateTone(l);
      const p = l.p ? Object.values(l.p)[0] : undefined;
      return {
        main: (
          <>
            <span className={tone === "red" ? "t-red" : "t-amber"}>{tone === "red" ? "Refused" : "Asked"}</span>{" "}
            <span className="t-model">{g.rule}</span>
            {g.command && (
              <>
                {" "}
                <Cmd text={g.command} />
              </>
            )}
          </>
        ),
        tags: p !== undefined && <span className="tag">p {p.toFixed(2)}</span>,
        more: g.command && long(g.command) && <pre className="output">{g.command}</pre>,
      };
    }
    case "command": {
      const text = l.text ?? "";
      return {
        main: <Cmd text={text} />,
        tags: (
          <span className={`tag ${l.ok === false ? "tag-red" : "tag-ok"}`} title={l.ok === false ? "The command exited with a code other than 0." : undefined}>
            exit {l.ok === false ? "≠0" : "0"}
          </span>
        ),
        more: (long(text) || l.output) && (
          <>
            {long(text) && <pre className="output">{text}</pre>}
            {l.output && <pre className="output">{l.output}</pre>}
          </>
        ),
      };
    }
    case "action":
      if (l.action === "task") {
        return {
          main:
            l.ok === undefined ? (
              <>Task from {l.by ?? "a person"}: {l.text}</>
            ) : (
              <>
                Task {l.ok ? <span className="t-accent">done</span> : <span className="t-red">failed</span>}
                {l.text && <span className="t-muted">: {l.text}</span>}
              </>
            ),
          more: l.text && l.text.length > 120 && <p className="row-sub">{l.text}</p>,
        };
      }
      return {
        main: (
          <>
            <b className="w-6">{l.action}</b>
            {l.goal && <> for {goal}</>}
            {l.ok === undefined ? " started" : l.ok ? <span className="t-accent"> ended, the goal holds</span> : <span className="t-red"> ended, the goal still fails</span>}
            {l.by && <span className="t-muted"> · {l.by}</span>}
            {l.text && <span className="t-muted"> · {l.text}</span>}
          </>
        ),
      };
    case "learn": {
      const m = /^([^:]+):\s*([\s\S]*)$/.exec(l.text ?? "");
      return m
        ? {
            main: (
              <>
                Learned <span className="w-5">{m[1]}</span> <Cmd text={m[2]} />
              </>
            ),
            more: long(m[2]) && <pre className="output">{m[2]}</pre>,
          }
        : { main: <>Learned {l.text}</> };
    }
    case "ask": {
      const rule = gate && splitGate(gate.text ?? "");
      return {
        main: (
          <>
            <span className="t-amber">Ask #{l.n}</span> {l.text}
          </>
        ),
        tags: rule && <span className="tag tag-amber">{rule.rule}</span>,
        more: (
          <>
            {l.body && <p className="row-sub">{l.body}</p>}
            {rule?.command && <pre className="output">{rule.command}</pre>}
            <Facts facts={l.facts} />
          </>
        ),
      };
    }
    case "answer":
      // The daemon closes an ask whose goal holds again: by "daemon: <goal> holds again".
      if (l.text === "withdrawn") {
        return {
          main: (
            <>
              Ask #{l.n} withdrawn
              {l.by && <span className="t-muted">: {l.by.replace(/^daemon:\s*/, "")}</span>}
            </>
          ),
        };
      }
      return {
        main: (
          <>
            Ask #{l.n} answered <b className="w-6">{l.text}</b>
            {l.by && <span className="t-muted"> by {l.by}</span>}
          </>
        ),
      };
    default:
      return {
        main: (
          <>
            {goal && <>{goal}: </>}
            {l.text}
            {l.by && <span className="t-muted"> · {l.by}</span>}
          </>
        ),
        more: (l.text?.length ?? 0) > 140 && <p className="row-sub">{l.text}</p>,
      };
  }
}

function Sent({ lines }: { lines: Line[] }) {
  if (!lines.length) return null;
  return (
    <span className="sent" title={`Sent to the channels:\n${lines.map((m) => m.text).join("\n")}`}>
      <svg className="kind" viewBox="0 0 12 12" aria-hidden="true">
        {ICON.message}
      </svg>
      sent
    </span>
  );
}

/** The coder's usage: model, calls, tokens in (cached share), out, reasoning. */
function UsageMark({ u }: { u: Usage }) {
  const cached = u.input ? Math.round((u.cached / u.input) * 100) : 0;
  const title = `${u.model}: ${u.calls} calls, ${u.input} tokens in (${u.cached} cached), ${u.output} out${u.reasoning ? `, ${u.reasoning} reasoning` : ""}`;
  return (
    <span className="usage num" title={title}>
      {modelName(u.model)} · {u.calls} {u.calls === 1 ? "call" : "calls"} · {tokens(u.input)} in, {cached}% cached · {tokens(u.output)} out
      {!!u.reasoning && <> · {tokens(u.reasoning)} reasoning</>}
    </span>
  );
}

/** One step: absolute time, or its offset from `base` inside an episode. `ep`: the
 *  episode this one step stands for, whose usage, cost and marks it carries. */
export function StepRow({ step, now, base, service, ep }: { step: Step; now: number; base?: number; service?: string; ep?: Episode }) {
  const [open, setOpen] = useState(false);
  const d = describe(step);
  const l = step.line;
  const cost = ep ? ep.cost : (l.cost_usd ?? 0) + (step.gate?.cost_usd ?? 0);
  const has = l.cost_usd !== undefined || step.gate?.cost_usd !== undefined || (ep && ep.cost > 0);
  const sent = ep ? [...step.sent, ...ep.sent] : step.sent;
  const line = (
    <>
      {service && <span className="row-service">{service}</span>}
      <span className="row-main">{d.main}</span>
      {d.tags}
      {ep?.by && <span className="t-muted nowrap small">by {ep.by}</span>}
      {ep?.usage && <UsageMark u={ep.usage} />}
      <Sent lines={sent} />
    </>
  );
  return (
    <li className="row">
      {base === undefined ? (
        <Time t={l.t} now={now} />
      ) : (
        <time className="row-time" dateTime={l.t} title={`${utc(l.t)} · ${ago(l.t, now)}`}>
          +{clock(ms(l.t) - base)}
        </time>
      )}
      <span className="row-glyph">
        <KindGlyph line={l} />
      </span>
      <div className="row-body">
        {d.more ? (
          <button type="button" className="row-line is-more" aria-expanded={open} onClick={() => setOpen(!open)}>
            {line}
          </button>
        ) : (
          <div className="row-line">{line}</div>
        )}
        {open && d.more && <div className="row-more">{d.more}</div>}
      </div>
      <span className="row-cost num">{has ? usd(cost) : ""}</span>
    </li>
  );
}

const OUTCOME: Record<Outcome, { glyph: Shown; tone: string; border: string }> = {
  running: { glyph: "running", tone: "t-model", border: "running" },
  held: { glyph: "holds", tone: "t-accent", border: "holds" },
  done: { glyph: "holds", tone: "t-accent", border: "holds" },
  learned: { glyph: "holds", tone: "t-accent", border: "holds" },
  failed: { glyph: "fails", tone: "t-red", border: "fails" },
  asked: { glyph: "ask", tone: "t-amber", border: "asked" },
  waited: { glyph: "unknown", tone: "t-muted", border: "quiet" },
  stopped: { glyph: "unknown", tone: "t-muted", border: "quiet" },
};

function Title({ ep }: { ep: Episode }) {
  if (ep.what === "task") {
    return (
      <>
        <span className="t-muted">Task</span> <span className="w-5">{ep.text ?? "from a person"}</span>
      </>
    );
  }
  if (ep.what === "learn") {
    return (
      <>
        {ep.outcome === "failed" ? "Could not learn" : "Learn"} <span className="w-5">{ep.text}</span>
      </>
    );
  }
  return (
    <>
      <b className="w-6">{ep.goal ?? "repair"}</b>
      {ep.actions.map((a) => (
        <span key={a}>
          {" "}
          → <span className="t-model">{a}</span>
        </span>
      ))}
    </>
  );
}

/** An episode: one row with what it was and how it ended; it opens to its steps. One with
 *  a single step is that step's row. */
export function EpisodeRow({ ep, now, hidden, service }: { ep: Episode; now: number; hidden: Set<number>; service?: string }) {
  const steps = ep.steps.filter((s) => !hidden.has(s.line.i));
  const o = OUTCOME[ep.outcome];
  const [open, setOpen] = useState(ep.outcome === "running" || ep.outcome === "failed" || ep.outcome === "asked");
  if (steps.length === 1) return <StepRow step={steps[0]} now={now} ep={ep} service={service} />;
  const base = ms(ep.start);
  const took = (ep.end ? ms(ep.end) : now) - base;
  return (
    <li className={`episode episode-${o.border}`}>
      <button type="button" className="episode-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <Time t={ep.start} now={now} />
        <span className="row-glyph">
          <Glyph state={o.glyph} />
        </span>
        <span className="row-line">
          {service && <span className="row-service">{service}</span>}
          <span className="row-main">
            <Title ep={ep} />
          </span>
          <span className={`${o.tone} w-5 nowrap`}>{ep.outcome}</span>
          <span className="t-muted num nowrap">{clock(took)}</span>
          {ep.by && <span className="t-muted nowrap small">by {ep.by}</span>}
          {ep.usage && <UsageMark u={ep.usage} />}
          <Sent lines={ep.sent} />
          <span className="t-muted num nowrap small episode-count">{steps.length} steps</span>
        </span>
        <span className="row-cost num">{ep.cost > 0 ? usd(ep.cost) : ""}</span>
      </button>
      {open && (
        <ol className="rows">
          {steps.map((s) => (
            <StepRow key={s.line.i} step={s} now={now} base={base} />
          ))}
        </ol>
      )}
    </li>
  );
}
