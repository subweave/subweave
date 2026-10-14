// Times, durations, numbers: one place so every view says them the same way.
import { useEffect, useState } from "react";

export function ms(t: string | number): number {
  return typeof t === "number" ? (t < 1e12 ? t * 1000 : t) : Date.parse(t);
}

/** `12s`, `4m`, `2h 10m`, `3d`. */
export function dur(msec: number): string {
  const s = Math.max(0, Math.round(msec / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return h % 24 && d < 7 ? `${d}d ${h % 24}h` : `${d}d`;
}

/** `2m 03s` for a running clock. */
export function clock(msec: number): string {
  const s = Math.max(0, Math.floor(msec / 1000));
  const m = Math.floor(s / 60);
  if (m === 0) return `${s}s`;
  if (m < 60) return `${m}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

export function ago(t: string | number, now: number): string {
  return `${dur(now - ms(t))} ago`;
}

const timeFmt = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
const dayFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const fullFmt = new Intl.DateTimeFormat(undefined, {
  year: "numeric",
  month: "short",
  day: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});
const monthFmt = new Intl.DateTimeFormat(undefined, { month: "short" });

export const time = (t: string | number) => timeFmt.format(ms(t));
export const day = (t: string | number) => dayFmt.format(ms(t));
export const full = (t: string | number) => fullFmt.format(ms(t));
export const month = (t: number) => monthFmt.format(t);
export const utc = (t: string | number) => new Date(ms(t)).toISOString().replace(".000Z", "Z");

export function usd(v: number): string {
  if (v === 0) return "$0";
  if (v < 0.01) return `$${v.toPrecision(2).replace(/0+$/, "")}`;
  return `$${v.toFixed(2)}`;
}

/** A fact value or metric, short: 1234567 → 1.23M. */
export function num(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return "–";
  const a = Math.abs(v);
  if (a >= 1e9) return `${+(v / 1e9).toPrecision(3)}B`;
  if (a >= 1e6) return `${+(v / 1e6).toPrecision(3)}M`;
  if (a >= 1e4) return `${+(v / 1e3).toPrecision(3)}k`;
  if (Number.isInteger(v)) return String(v);
  return String(+v.toPrecision(3));
}

/** Token counts: 2822 → 2.8k. */
export function tokens(v: number): string {
  if (v < 1000) return String(v);
  if (v < 1e6) return `${+(v / 1e3).toFixed(v < 1e4 ? 1 : 0)}k`;
  return `${+(v / 1e6).toFixed(1)}M`;
}

/** `deepseek/deepseek-v4-flash-0731@novita` → `deepseek-v4-flash`: no vendor, provider or date. */
export function modelName(id: string): string {
  return (id.split("@")[0].split("/").pop() ?? id).replace(/-\d{4}$/, "");
}

export function bytes(v: number | null | undefined): string {
  if (v === null || v === undefined) return "–";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  let x = v;
  while (x >= 1024 && i < u.length - 1) {
    x /= 1024;
    i++;
  }
  return `${x >= 100 || i === 0 ? Math.round(x) : x.toFixed(1)} ${u[i]}`;
}

export function rate(v: number | null | undefined): string {
  return v === null || v === undefined ? "–" : `${bytes(v)}/s`;
}

/** Re-renders every `every` ms; returns now. */
export function useNow(every = 1000): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), every);
    return () => window.clearInterval(id);
  }, [every]);
  return now;
}
