import { useEffect, useRef, useState } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { Series } from "../api";
import { full, num } from "../format";

/** A small line with no axes; gaps break it. */
export function Sparkline({ values, height = 28, label }: { values: Series; height?: number; label: string }) {
  const w = 120;
  const nums = values.filter((v): v is number => v !== null);
  if (nums.length < 2) return <svg className="spark" viewBox={`0 0 ${w} ${height}`} role="img" aria-label={`${label}: no data`} />;
  const lo = Math.min(...nums);
  const hi = Math.max(...nums);
  const span = hi - lo || 1;
  const step = w / Math.max(1, values.length - 1);
  let d = "";
  let pen = false;
  values.forEach((v, k) => {
    if (v === null) {
      pen = false;
      return;
    }
    const y = height - 2 - ((v - lo) / span) * (height - 4);
    d += `${pen ? "L" : "M"}${(k * step).toFixed(1)} ${y.toFixed(1)}`;
    pen = true;
  });
  return (
    <svg className="spark" viewBox={`0 0 ${w} ${height}`} preserveAspectRatio="none" role="img" aria-label={label}>
      <path d={d} vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

function cssVar(name: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** Re-renders when the OS switches light and dark: charts read colours from CSS. */
function useScheme() {
  const [dark, setDark] = useState(() => matchMedia("(prefers-color-scheme: dark)").matches);
  useEffect(() => {
    const m = matchMedia("(prefers-color-scheme: dark)");
    const f = () => setDark(m.matches);
    m.addEventListener("change", f);
    return () => m.removeEventListener("change", f);
  }, []);
  return dark;
}

export interface Limit {
  value: number;
  label: string;
}

/** A time chart: one or two lines, a crosshair with value and time, dashed limit lines. */
export function TimeChart({
  t,
  series,
  limits = [],
  format = num,
  height = 160,
}: {
  t: number[];
  series: { label: string; values: Series; color?: string }[];
  limits?: Limit[];
  format?: (v: number | null) => string;
  height?: number;
}) {
  const box = useRef<HTMLDivElement>(null);
  const tip = useRef<HTMLDivElement>(null);
  const dark = useScheme();
  // Callers pass fresh arrays and functions each render; the plot follows the data only.
  const format_ = useRef(format);
  format_.current = format;
  const limitsKey = JSON.stringify(limits);

  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const format = format_.current;
    const ink = cssVar("--muted");
    const rule = cssVar("--rule");
    const red = cssVar("--red");
    const font = `11px ${cssVar("--sans") || "sans-serif"}`;
    const colors = series.map((s, k) => s.color ?? (k === 0 ? cssVar("--accent") : cssVar("--ink-2")));

    const drawLimits = (u: uPlot) => {
      const ctx = u.ctx;
      const pr = devicePixelRatio;
      for (const lim of limits) {
        const y = u.valToPos(lim.value, "y", true);
        if (y < u.bbox.top || y > u.bbox.top + u.bbox.height) continue;
        ctx.save();
        ctx.strokeStyle = red;
        ctx.lineWidth = pr;
        ctx.setLineDash([4 * pr, 4 * pr]);
        ctx.beginPath();
        ctx.moveTo(u.bbox.left, y);
        ctx.lineTo(u.bbox.left + u.bbox.width, y);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.fillStyle = red;
        ctx.font = `${11 * pr}px ${cssVar("--sans")}`;
        ctx.textAlign = "right";
        ctx.fillText(lim.label, u.bbox.left + u.bbox.width - 4 * pr, y - 4 * pr);
        ctx.restore();
      }
    };

    const lims = limits.map((l) => l.value);
    const opts: uPlot.Options = {
      width: el.clientWidth,
      height,
      legend: { show: false },
      cursor: { y: false, points: { size: 6 }, drag: { x: false, y: false } },
      padding: [8, 4, 0, 0],
      scales: {
        x: { time: true },
        y: {
          range: (_u, lo, hi) => {
            // Keep a limit in view when it is near the data, and never a flat line.
            const all = [lo, hi, ...lims.filter((v) => v <= Math.max(hi, 0) * 1.6 + 1)];
            let a = Math.min(...all);
            let b = Math.max(...all);
            if (a === b) {
              a -= Math.abs(a) * 0.1 || 1;
              b += Math.abs(b) * 0.1 || 1;
            }
            const pad = (b - a) * 0.08;
            return [a >= 0 && a - pad < 0 ? 0 : a - pad, b + pad];
          },
        },
      },
      axes: [
        { stroke: ink, font, grid: { show: false }, ticks: { stroke: rule, size: 4 }, gap: 4, size: 24 },
        {
          stroke: ink,
          font,
          grid: { stroke: rule, width: 1 },
          ticks: { show: false },
          size: 44,
          gap: 6,
          values: (_u, vals) => vals.map((v) => format(v)),
        },
      ],
      series: [
        {},
        ...series.map((s, k) => ({
          label: s.label,
          stroke: colors[k],
          width: 1.5,
          spanGaps: false,
          points: { show: false },
        })),
      ],
      hooks: {
        draw: [drawLimits],
        setCursor: [
          (u: uPlot) => {
            const tipEl = tip.current;
            if (!tipEl) return;
            const idx = u.cursor.idx;
            if (idx === null || idx === undefined || u.cursor.left === undefined || u.cursor.left < 0) {
              tipEl.hidden = true;
              return;
            }
            const when = full((u.data[0][idx] as number) * 1000);
            tipEl.replaceChildren();
            const head = document.createElement("div");
            head.className = "tip-time";
            head.textContent = when;
            tipEl.append(head);
            series.forEach((s, k) => {
              const row = document.createElement("div");
              row.className = "tip-row";
              const sw = document.createElement("i");
              sw.style.background = colors[k];
              row.append(sw, `${series.length > 1 ? `${s.label} ` : ""}${format(u.data[k + 1][idx] as number | null)}`);
              tipEl.append(row);
            });
            tipEl.hidden = false;
            const left = Math.min(u.cursor.left + u.bbox.left / devicePixelRatio + 12, el.clientWidth - tipEl.offsetWidth - 4);
            tipEl.style.left = `${Math.max(0, left)}px`;
          },
        ],
      },
    };
    const data: uPlot.AlignedData = [t, ...series.map((s) => s.values as (number | null)[])];
    const plot = new uPlot(opts, data, el);
    const ro = new ResizeObserver(() => plot.setSize({ width: el.clientWidth, height }));
    ro.observe(el);
    return () => {
      ro.disconnect();
      plot.destroy();
    };
  }, [t, height, dark, limitsKey, ...series.map((s) => s.values)]);

  return (
    <div className="chart">
      <div ref={box} />
      <div ref={tip} className="chart-tip" hidden />
    </div>
  );
}
