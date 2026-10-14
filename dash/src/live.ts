// One EventSource for the whole app (/api/events), held in a store that components read
// through useSyncExternalStore. Journal lines go to listeners rather than into the
// snapshot: each view keeps the lines it shows.
import { useEffect, useSyncExternalStore } from "react";
import type { Line, Status, VitalSample } from "./api";

export type Conn = "connecting" | "live" | "reconnecting";

export interface LiveSnapshot {
  conn: Conn;
  /** Newest status per service, from `status` events. */
  statuses: Record<string, Status>;
  /** Bumped on every reconnect: views refetch what they may have missed. */
  epoch: number;
  /** The newest box sample, from `vitals` events. */
  vitals: VitalSample | null;
}

type JournalListener = (service: string, line: Line) => void;

const BACKOFF = [1000, 2000, 5000, 10000];

let snap: LiveSnapshot = { conn: "connecting", statuses: {}, epoch: 0, vitals: null };
const subs = new Set<() => void>();
const journalSubs = new Set<JournalListener>();
let source: EventSource | null = null;
let retry = 0;
let timer: number | undefined;
let everLive = false;

function set(patch: Partial<LiveSnapshot>) {
  snap = { ...snap, ...patch };
  for (const f of subs) f();
}

function connect() {
  source?.close();
  const es = new EventSource("/api/events", { withCredentials: true });
  source = es;
  es.addEventListener("hello", () => {
    retry = 0;
    set({ conn: "live", epoch: everLive ? snap.epoch + 1 : snap.epoch });
    everLive = true;
  });
  es.addEventListener("status", (e) => {
    const { service, status } = JSON.parse((e as MessageEvent).data) as {
      service: string;
      status: Status;
    };
    set({ statuses: { ...snap.statuses, [service]: status } });
  });
  es.addEventListener("journal", (e) => {
    const { service, line } = JSON.parse((e as MessageEvent).data) as { service: string; line: Line };
    for (const f of journalSubs) f(service, line);
  });
  es.addEventListener("vitals", (e) => {
    set({ vitals: JSON.parse((e as MessageEvent).data) as VitalSample });
  });
  es.onerror = () => {
    es.close();
    if (source !== es) return;
    source = null;
    set({ conn: "reconnecting" });
    const wait = BACKOFF[Math.min(retry, BACKOFF.length - 1)];
    retry++;
    timer = window.setTimeout(connect, wait);
  };
}

export function startLive() {
  if (source || timer !== undefined) return;
  connect();
}

export function stopLive() {
  window.clearTimeout(timer);
  timer = undefined;
  source?.close();
  source = null;
  everLive = false;
  set({ conn: "connecting" });
}

function subscribe(f: () => void) {
  subs.add(f);
  return () => subs.delete(f);
}

export function useLive(): LiveSnapshot {
  return useSyncExternalStore(subscribe, () => snap);
}

/** Calls `f` for every live journal line (of `service`, or of every service). */
export function useJournal(service: string | null, f: JournalListener) {
  useEffect(() => {
    const g: JournalListener = (s, l) => {
      if (service === null || s === service) f(s, l);
    };
    journalSubs.add(g);
    return () => {
      journalSubs.delete(g);
    };
  }, [service, f]);
}
