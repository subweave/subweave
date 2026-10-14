import { useCallback, useMemo, useState, type CSSProperties } from "react";
import { api, type GoalState, type GoalStatus, type Line } from "../api";
import { AskCard, TaskComposer } from "../components/act";
import { Sparkline } from "../components/charts";
import { ErrorLine, FactsTable, Glyph, Panel, Skeleton } from "../components/ui";
import { ago, clock, dur, ms, num, usd, useNow } from "../format";
import { useJournal } from "../live";
import { condText, goalTree, repairNow, type GoalNode } from "../model";
import { useResource } from "../resource";
import { linkClick, servicePath } from "../router";
import type { ServiceProps } from "./ServiceView";

/** The newest lines of a service, kept live. */
export function useRecentLines(service: string, query: { kind?: string[]; limit: number }, refresh: unknown) {
  const key = `lines:${service}:${query.kind?.join(",") ?? ""}:${query.limit}`;
  const res = useResource(key, () => api.journal(service, query).then((r) => r.lines), refresh);
  const [fresh, setFresh] = useState<Line[]>([]);
  const kinds = query.kind?.join(",");
  useJournal(
    service,
    useCallback(
      (_s: string, l: Line) => {
        if (kinds && !kinds.split(",").includes(l.kind)) return;
        setFresh((f) => [l, ...f].slice(0, 200));
      },
      [kinds],
    ),
  );
  return useMemo(() => {
    const base = res.data ?? [];
    const top = base[0]?.i ?? -1;
    const extra = fresh.filter((l) => l.i > top);
    return { lines: res.data ? [...extra, ...base] : undefined, error: res.error };
  }, [res.data, res.error, fresh]);
}

function NowStrip({ repair, lines, now }: { repair: string; lines: Line[] | undefined; now: number }) {
  const r = repairNow(repair, lines ?? []);
  return (
    <div className="now-strip" role="status">
      <Glyph state="running" />
      <span>
        <b className="w-6">{repair}</b>
        {r.start && <span className="num"> · {clock(now - ms(r.start))}</span>}
        <span className="num"> · {r.commands} {r.commands === 1 ? "command" : "commands"}</span>
        {r.last && (
          <>
            {" "}
            · last: <code className="mono">{r.last}</code>
          </>
        )}
      </span>
    </div>
  );
}

function GoalRow({
  node,
  depth,
  states,
  lastCheck,
  now,
}: {
  node: GoalNode;
  depth: number;
  states: Map<string, GoalStatus>;
  lastCheck: Map<string, Line>;
  now: number;
}) {
  const g = node.goal;
  const st = states.get(g.name);
  const state: GoalState = st?.state ?? "unknown";
  const check = lastCheck.get(g.name);
  const [open, setOpen] = useState(state === "fails");
  const cond = condText(g);
  const facts = check?.facts && Object.keys(check.facts).length ? check.facts : null;
  return (
    <>
      <li className={`goal goal-${state}`} style={{ "--depth": depth } as CSSProperties}>
        <button type="button" className="goal-line" aria-expanded={facts ? open : undefined} disabled={!facts} onClick={() => setOpen(!open)}>
          <span className="goal-who">
            {depth > 0 && <span className="goal-elbow" aria-hidden="true" />}
            <Glyph state={state} />
            <span className="goal-name">{g.name}</span>
          </span>
          <span className="goal-what">
            <span className={`goal-cond ${cond.quoted ? "t-model" : ""}`} title={cond.text}>
              {cond.quoted ? `"${cond.text}"` : cond.text}
            </span>
            {g.every && <span className="tag tag-muted">EVERY {g.every}</span>}
            {g.when && <span className="tag tag-muted" title={`IF ${g.when}`}>IF {g.when}</span>}
            {node.alsoAfter.length > 0 && <span className="t-muted goal-also small">also after {node.alsoAfter.join(", ")}</span>}
          </span>
          <span className={`goal-since num t-${state === "fails" ? "red" : "muted"}`}>
            {st?.since ? `${state === "off" ? "off" : state} for ${dur(now - ms(st.since))}` : state}
          </span>
          <span className="goal-checked num t-muted" title={check ? check.t : "No check in the newest lines"}>
            {check ? ago(check.t, now) : "–"}
          </span>
        </button>
        {open && facts && (
          <div className="goal-facts">
            <FactsTable facts={facts} />
          </div>
        )}
      </li>
      {node.children.map((c) => (
        <GoalRow key={c.goal.name} node={c} depth={depth + 1} states={states} lastCheck={lastCheck} now={now} />
      ))}
    </>
  );
}

function FactMetrics({ service }: { service: string }) {
  const m = useResource(`metrics:${service}:24h`, () => api.metrics(service, "24h"));
  const top = useMemo(() => {
    if (!m.data) return [];
    return Object.entries(m.data.facts)
      .map(([name, vals]) => {
        const n = vals.filter((v): v is number => v !== null);
        if (n.length < 2) return null;
        const lo = Math.min(...n);
        const hi = Math.max(...n);
        const scale = Math.max(Math.abs(hi), Math.abs(lo)) || 1;
        return { name, vals, last: n[n.length - 1], change: (hi - lo) / scale };
      })
      .filter((x): x is NonNullable<typeof x> => x !== null)
      .sort((a, b) => b.change - a.change)
      .slice(0, 4);
  }, [m.data]);
  return (
    <Panel
      title="Metrics"
      aside={
        <a className="panel-link" href={servicePath(service, "metrics")} onClick={linkClick}>
          All metrics
        </a>
      }
    >
      <ErrorLine error={m.error} />
      {m.loading && <Skeleton lines={4} />}
      {m.data && top.length === 0 && <p className="t-muted small">No numeric facts in the last 24 hours.</p>}
      <ul className="mini-metrics">
        {top.map((x) => (
          <li key={x.name}>
            <span className="mono mini-name">{x.name}</span>
            <span className="num mini-value">{num(x.last)}</span>
            <Sparkline values={x.vals} label={`${x.name}, last 24 hours`} height={22} />
          </li>
        ))}
      </ul>
    </Panel>
  );
}

export function Overview({ service, detail, status }: ServiceProps) {
  const now = useNow(1000);
  const lines = useRecentLines(service, { limit: 300 }, 0);
  const checks = useRecentLines(service, { kind: ["check"], limit: 500 }, 0);
  const tree = useMemo(() => goalTree(detail.weave), [detail.weave]);
  const states = useMemo(() => new Map((status?.goals ?? []).map((g) => [g.name, g])), [status]);
  const lastCheck = useMemo(() => {
    const m = new Map<string, Line>();
    for (const l of checks.lines ?? []) if (l.goal && !m.has(l.goal)) m.set(l.goal, l);
    return m;
  }, [checks.lines]);

  return (
    <div className="overview">
      <div className="overview-main">
        {status?.repair && <NowStrip repair={status.repair} lines={lines.lines} now={now} />}
        <Panel title="Goals" aside={<span className="t-muted small num">{detail.weave.goals.length} goals</span>}>
          <ErrorLine error={checks.error} />
          {detail.weave.goals.length === 0 ? (
            <p className="t-muted small">This weave has no goals.</p>
          ) : (
            <ul className="goals">
              {tree.map((n) => (
                <GoalRow key={n.goal.name} node={n} depth={0} states={states} lastCheck={lastCheck} now={now} />
              ))}
            </ul>
          )}
        </Panel>
      </div>
      <aside className="overview-side">
        {detail.asks.length > 0 && (
          <Panel title={`Asks · ${detail.asks.length}`} className="panel-flat">
            <div className="ask-list">
              {detail.asks.map((a) => (
                <AskCard key={a.n} ask={a} />
              ))}
            </div>
          </Panel>
        )}
        <Panel title="Task">
          <TaskComposer service={service} compact />
        </Panel>
        <FactMetrics service={service} />
        <Panel title="Spend">
          <p className="big-num num">{usd(status?.spend_month_usd ?? 0)}</p>
          <p className="t-muted small">This month, decider and coder together.</p>
        </Panel>
        <Panel title="Models">
          <dl className="kv">
            <dt>Decide</dt>
            <dd className="mono">{status?.decide ?? detail.spec.decide}</dd>
            <dt>Code</dt>
            <dd className="mono">
              {(status?.code ?? detail.spec.code).map((c, k) => (
                <span key={c} className="block">
                  {k > 0 && <span className="t-muted">then </span>}
                  {c}
                </span>
              ))}
            </dd>
          </dl>
        </Panel>
      </aside>
    </div>
  );
}
