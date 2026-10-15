import { useCallback, useEffect, useMemo, useState } from "react";
import { api, type Line, type Series, type ServiceSummary, type Vitals } from "../api";
import { useAppData } from "../App";
import { Sparkline } from "../components/charts";
import { EpisodeRow, StepRow } from "../components/rows";
import { ErrorLine, Glyph, Panel, Skeleton } from "../components/ui";
import { bytes, clock, dur, ms, num, rate, usd, useNow } from "../format";
import { useJournal, useLive } from "../live";
import { counts, holdsOf, serviceShown, timeline } from "../model";
import { useResource } from "../resource";
import { linkClick, servicePath } from "../router";

/** Box vitals for a range; the 1 h range grows with live samples. */
export function useVitals(range: string) {
  const live = useLive();
  const res = useResource(`vitals:${range}`, () => api.vitals(range), live.epoch);
  const [extra, setExtra] = useState<Vitals | null>(null);
  useEffect(() => setExtra(null), [res.data]);
  const sample = live.vitals;
  useEffect(() => {
    if (range !== "1h" || !sample || !res.data || !res.data.t.length) return;
    setExtra((prev) => {
      const base = prev ?? res.data!;
      if (base.t.length && ms(base.t[base.t.length - 1]) >= ms(sample.t)) return prev;
      const keep = (a: Series, v: number | null) => [...a.slice(1), v];
      const disk: Record<string, Series> = {};
      for (const [m, a] of Object.entries(base.disk_pct)) disk[m] = keep(a, sample.disk_pct[m] ?? null);
      return {
        step_s: base.step_s,
        t: [...base.t.slice(1), sample.t],
        cpu_pct: keep(base.cpu_pct, sample.cpu_pct),
        mem_pct: keep(base.mem_pct, sample.mem_pct),
        load1: keep(base.load1, sample.load1),
        disk_pct: disk,
        net_rx_bps: keep(base.net_rx_bps, sample.net_rx_bps),
        net_tx_bps: keep(base.net_tx_bps, sample.net_tx_bps),
      };
    });
  }, [sample, range, res.data]);
  return { ...res, data: extra ?? res.data };
}

export function lastOf(a: Series | undefined): number | null {
  if (!a) return null;
  for (let k = a.length - 1; k >= 0; k--) if (a[k] !== null) return a[k];
  return null;
}

export function fullestMount(v: Vitals): string | null {
  let best: string | null = null;
  let max = -1;
  for (const [m, a] of Object.entries(v.disk_pct)) {
    const x = lastOf(a) ?? -1;
    if (x > max) {
      max = x;
      best = m;
    }
  }
  return best;
}

function VitalsStrip() {
  const { box } = useAppData();
  const v = useVitals("1h");
  if (v.error) return <ErrorLine error={v.error} />;
  if (!v.data) {
    return (
      <div className="vitals">
        {Array.from({ length: 5 }, (_, k) => (
          <div className="vital" key={k}>
            <Skeleton lines={2} />
          </div>
        ))}
      </div>
    );
  }
  if (!v.data.t.length) return <p className="t-muted vitals-none">Box vitals need Linux.</p>;
  const d = v.data;
  const mount = fullestMount(d);
  const disk = box?.disks.find((x) => x.mount === mount);
  const net = d.net_rx_bps.map((x, k) => (x === null && d.net_tx_bps[k] === null ? null : (x ?? 0) + (d.net_tx_bps[k] ?? 0)));
  const items = [
    { name: "CPU", value: `${num(lastOf(d.cpu_pct))}%`, sub: box ? `${box.cpus} cores` : "", series: d.cpu_pct },
    {
      name: "Memory",
      value: `${num(lastOf(d.mem_pct))}%`,
      sub: box ? `of ${bytes(box.mem_bytes)}` : "",
      series: d.mem_pct,
    },
    {
      name: "Disk",
      value: `${num(mount ? lastOf(d.disk_pct[mount]) : null)}%`,
      sub: disk ? `${mount} · ${bytes(disk.size_bytes)}` : (mount ?? ""),
      series: mount ? d.disk_pct[mount] : [],
    },
    { name: "Load", value: num(lastOf(d.load1)), sub: "1 min average", series: d.load1 },
    {
      name: "Network",
      value: rate(lastOf(net)),
      sub: `in ${rate(lastOf(d.net_rx_bps))} · out ${rate(lastOf(d.net_tx_bps))}`,
      series: net,
    },
  ];
  return (
    <div className="vitals">
      {items.map((it) => (
        <div className="vital" key={it.name}>
          <div className="vital-top">
            <span className="vital-name">{it.name}</span>
            <span className="vital-value num">{it.value}</span>
          </div>
          <Sparkline values={it.series} label={`${it.name}, last hour`} />
          <span className="vital-sub t-muted num">{it.sub}</span>
        </div>
      ))}
    </div>
  );
}

export function StateBar({ s }: { s: ServiceSummary }) {
  const c = counts(s.status);
  const total = c.holds + c.fails + c.waiting + c.unknown + c.off;
  if (!total) return <div className="statebar statebar-empty" />;
  const parts = (["fails", "waiting", "unknown", "holds", "off"] as const).filter((k) => c[k]);
  return (
    <div className="statebar" role="img" aria-label={parts.map((k) => `${c[k]} ${k}`).join(", ")}>
      {parts.map((k) => (
        <span key={k} className={`seg seg-${k}`} style={{ flexGrow: c[k] }} title={`${c[k]} ${k}`} />
      ))}
    </div>
  );
}

function ServiceCard({ s, now }: { s: ServiceSummary; now: number }) {
  const c = counts(s.status);
  const { holds, total } = holdsOf(s.status);
  const weave = s.weave.split("/").pop();
  return (
    <a className={`card svc-card ${s.stopped ? "is-stopped" : ""}`} href={servicePath(s.name)} onClick={linkClick}>
      <div className="svc-card-top">
        <Glyph state={serviceShown(s.status, s.stopped)} />
        <span className="svc-card-name">{s.name}</span>
        <span className="t-muted svc-card-weave">{weave}</span>
        <span className="svc-card-holds num">{s.stopped ? "stopped" : `${holds}/${total} hold`}</span>
      </div>
      <p className="svc-card-request t-muted">{s.request}</p>
      <StateBar s={s} />
      <div className="svc-card-meta">
        {c.fails > 0 && <span className="t-red">{c.fails} fails</span>}
        {s.status?.repair && (
          <span className="t-model">
            Running now: {s.status.repair}
          </span>
        )}
        {!!s.status?.asks.length && <span className="t-amber">{s.status.asks.length} open {s.status.asks.length === 1 ? "escalation" : "escalations"}</span>}
        {s.status && <span className="t-muted num">{usd(s.status.spend_month_usd)} this month</span>}
        {s.status && !s.stopped && now - ms(s.status.t) > 120_000 && (
          <span className="t-amber">no status for {dur(now - ms(s.status.t))}</span>
        )}
      </div>
    </a>
  );
}

function Recent({ services, now }: { services: ServiceSummary[]; now: number }) {
  const live = useLive();
  const names = services.map((s) => s.name);
  const key = names.join();
  const res = useResource(
    `recent:${key}`,
    async () => {
      const all = await Promise.all(names.map((n) => api.journal(n, { limit: 30 }).then((r) => r.lines.map((l) => ({ service: n, line: l })))));
      return all.flat();
    },
    live.epoch,
  );
  const [fresh, setFresh] = useState<{ service: string; line: Line }[]>([]);
  useEffect(() => setFresh([]), [res.data]);
  useJournal(
    null,
    useCallback((service: string, line: Line) => setFresh((f) => [{ service, line }, ...f].slice(0, 30)), []),
  );
  const repairs = services.map((s) => s.status?.repair ?? null).join("\n");
  const rows = useMemo(() => {
    const by = new Map<string, Map<number, Line>>();
    for (const { service, line } of [...fresh, ...(res.data ?? [])]) {
      const m = by.get(service) ?? new Map<number, Line>();
      if (!m.has(line.i)) m.set(line.i, line);
      by.set(service, m);
    }
    const repair = repairs.split("\n");
    return [...by]
      .flatMap(([service, m]) =>
        timeline(
          [...m.values()].sort((a, b) => b.i - a.i),
          repair[names.indexOf(service)] || null,
        ).map((item) => ({ service, item, t: ms(item.type === "step" ? item.step.line.t : item.start) })),
      )
      .sort((a, b) => b.t - a.t)
      .slice(0, 20);
  }, [fresh, res.data, repairs, key]);
  const many = names.length > 1;
  return (
    <Panel title="Recent activity">
      <ErrorLine error={res.error} />
      {res.loading ? (
        <Skeleton lines={8} />
      ) : rows.length === 0 ? (
        <p className="empty">Nothing happened yet.</p>
      ) : (
        <ol className="rows">
          {rows.map(({ service, item }) =>
            item.type === "step" ? (
              <StepRow key={`${service}-${item.step.line.i}`} step={item.step} now={now} service={many ? service : undefined} />
            ) : (
              <EpisodeRow key={`${service}-${item.key}`} ep={item} now={now} hidden={NONE} service={many ? service : undefined} />
            ),
          )}
        </ol>
      )}
    </Panel>
  );
}

const NONE = new Set<number>();

export function BoxView() {
  const { services, box, servicesError } = useAppData();
  const now = useNow(5000);
  return (
    <div className="page">
      <header className="page-head">
        <div>
          <h1>{box?.host ?? "Box"}</h1>
          {box ? (
            <p className="page-sub t-muted num">
              {box.os} · {box.kernel} · {box.cpus} CPU · {bytes(box.mem_bytes)} · up {dur(box.uptime_s * 1000)}
              {box.daemon.since && box.daemon.active && <> · daemon up {clock(now - ms(box.daemon.since))}</>}
            </p>
          ) : (
            <Skeleton lines={1} />
          )}
        </div>
      </header>
      {box && !box.daemon.active && (
        <p className="stale">The daemon does not run. Start it with <code>subweave daemon --install</code> or <code>systemctl start subweave</code>.</p>
      )}
      <VitalsStrip />
      <h2 className="section-title">Services</h2>
      <ErrorLine error={servicesError} />
      {!services && !servicesError && (
        <div className="cards">
          <div className="card">
            <Skeleton lines={3} />
          </div>
        </div>
      )}
      {services?.length === 0 && <p className="empty">No services on this box yet. Start one with <code>subweave run</code>.</p>}
      <div className="cards">
        {services?.map((s) => (
          <ServiceCard key={s.name} s={s} now={now} />
        ))}
      </div>
      {services && services.length > 0 && <Recent services={services} now={now} />}
    </div>
  );
}
