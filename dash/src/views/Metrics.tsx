import { useMemo, useState } from "react";
import { api, type Series } from "../api";
import { TimeChart, type Limit } from "../components/charts";
import { ErrorLine, Segmented, Skeleton } from "../components/ui";
import { bytes, ms, num, rate } from "../format";
import { limitOf } from "../model";
import { useResource } from "../resource";
import { fullestMount, lastOf, useVitals } from "./BoxView";
import type { ServiceProps } from "./ServiceView";

const RANGES = ["24h", "7d", "30d"] as const;
type Range = (typeof RANGES)[number];

const pct = (v: number | null) => (v === null ? "–" : `${num(v)}%`);

function ChartCard({
  title,
  t,
  series,
  limits,
  format = num,
  sub,
}: {
  title: string;
  t: number[];
  series: { label: string; values: Series }[];
  limits?: Limit[];
  format?: (v: number | null) => string;
  sub?: string;
}) {
  const vals = series[0].values.filter((v): v is number => v !== null);
  const last = lastOf(series[0].values);
  return (
    <section className="panel chart-card">
      <header className="chart-head">
        <h3 className="mono">{title}</h3>
        {sub && <span className="t-muted small">{sub}</span>}
      </header>
      <div className="chart-nums">
        <span className="big-num num">{format(last)}</span>
        {vals.length > 0 && (
          <span className="t-muted small num">
            min {format(Math.min(...vals))} · max {format(Math.max(...vals))}
          </span>
        )}
      </div>
      {vals.length ? <TimeChart t={t} series={series} limits={limits} format={format} /> : <p className="chart-empty t-muted small">No values in this range.</p>}
    </section>
  );
}

function VitalsCharts({ range }: { range: Range }) {
  const v = useVitals(range);
  const t = useMemo(() => v.data?.t.map((x) => ms(x) / 1000) ?? [], [v.data]);
  if (v.error) return <ErrorLine error={v.error} />;
  if (!v.data) return <ChartSkeletons n={4} />;
  if (!v.data.t.length) return <p className="t-muted">Box vitals need Linux.</p>;
  const d = v.data;
  const mount = fullestMount(d);
  return (
    <div className="chart-grid">
      <ChartCard title="CPU" t={t} series={[{ label: "CPU", values: d.cpu_pct }]} format={pct} />
      <ChartCard title="Memory" t={t} series={[{ label: "Memory", values: d.mem_pct }]} format={pct} />
      {mount && <ChartCard title={`Disk ${mount}`} t={t} series={[{ label: "Disk", values: d.disk_pct[mount] }]} format={pct} />}
      <ChartCard title="Load" t={t} series={[{ label: "Load", values: d.load1 }]} sub="1 min average" />
      <ChartCard
        title="Network"
        t={t}
        series={[
          { label: "in", values: d.net_rx_bps },
          { label: "out", values: d.net_tx_bps },
        ]}
        format={rate}
        sub="green in, grey out"
      />
    </div>
  );
}

function ChartSkeletons({ n }: { n: number }) {
  return (
    <div className="chart-grid">
      {Array.from({ length: n }, (_, k) => (
        <div className="panel chart-card" key={k}>
          <Skeleton lines={1} height={12} />
          <div className="sk-chart" />
        </div>
      ))}
    </div>
  );
}

export function MetricsTab({ service, detail }: ServiceProps) {
  const [range, setRange] = useState<Range>("24h");
  const m = useResource(`metrics:${service}:${range}`, () => api.metrics(service, range));
  const t = useMemo(() => m.data?.t.map((x) => ms(x) / 1000) ?? [], [m.data]);
  const limits = useMemo(() => {
    const out: Record<string, Limit[]> = {};
    for (const g of detail.weave.goals) {
      const l = limitOf(g);
      if (l) (out[l.fact] ??= []).push({ value: l.value, label: `${g.name} ${l.op} ${num(l.value)}` });
    }
    return out;
  }, [detail.weave]);
  const facts = Object.entries(m.data?.facts ?? {}).sort(([a], [b]) => a.localeCompare(b));
  const isBytes = (name: string) => /_bytes$/.test(name);

  return (
    <div className="metrics">
      <div className="metrics-bar">
        <Segmented label="Range" value={range} options={RANGES} onChange={setRange} />
      </div>
      <h2 className="section-title">Box</h2>
      <VitalsCharts range={range} />
      <h2 className="section-title">Facts</h2>
      <ErrorLine error={m.error} />
      {m.loading && <ChartSkeletons n={4} />}
      {m.data && facts.length === 0 && <p className="empty">No numeric facts in this range. Checks write them as they run.</p>}
      <div className="chart-grid">
        {facts.map(([name, values]) => (
          <ChartCard
            key={name}
            title={name}
            t={t}
            series={[{ label: name, values }]}
            limits={limits[name]}
            format={isBytes(name) ? (v) => bytes(v) : num}
          />
        ))}
      </div>
    </div>
  );
}
