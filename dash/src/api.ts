// The dashboard API (see the server's /api routes). Shapes follow src/service.rs and the
// parsed weave of `subweave plan --json`.

export type GoalState = "holds" | "fails" | "unknown" | "off" | "waiting";

export interface GoalStatus {
  name: string;
  state: GoalState;
  since: string | null;
}

export interface Status {
  service: string;
  t: string;
  goals: GoalStatus[];
  repair: string | null;
  asks: number[];
  spend_month_usd: number;
  decide: string;
  code: string[];
  stopped: boolean;
}

export interface Spec {
  name: string;
  weave: string;
  request: string;
  args: Record<string, string>;
  decide: string;
  code: string[];
  created: string;
  stopped: boolean;
}

export type Kind =
  | "check"
  | "decide"
  | "gate"
  | "action"
  | "command"
  | "learn"
  | "ask"
  | "answer"
  | "message"
  | "note";

export interface Line {
  i: number;
  t: string;
  kind: Kind;
  goal?: string;
  action?: string;
  ok?: boolean;
  text?: string;
  facts?: Record<string, string>;
  p?: Record<string, number>;
  cost_usd?: number;
  n?: number;
  answers?: string[];
  by?: string;
  output?: string;
  /** `ask`: the ask's explanation. */
  body?: string;
}

export interface Learned {
  facts: Record<string, { command: string; key: string; t: string }>;
  steps: Record<string, { commands: string[]; key: string; t: string }>;
}

export interface Ask {
  n: number;
  service: string;
  goal?: string | null;
  action?: string | null;
  rule?: string | null;
  title: string;
  text: string;
  facts: Record<string, string>;
  answers: string[];
  t: string;
}

export type Level = "MUST" | "SHOULD" | "MAY";

export interface Arg {
  name: string;
  default: string | null;
  level: Level;
  kind: { value: { what: string } } | { options: { label: string; meaning: string; slots: string[] }[] };
  line: number;
}

export interface Fact {
  name: string;
  follow: boolean;
  source: { command: string } | { quoted: string };
  line: number;
}

export interface Goal {
  name: string;
  after: string[];
  every: string | null;
  tries: string | null;
  cond: { code: string } | { quoted: string };
  when: string | null;
  line: number;
}

export interface Action {
  name: string;
  for_goals: string[];
  within: string | null;
  when_if: string | null;
  when: string;
  line: number;
}

export interface Weave {
  name: string;
  about: string | null;
  args: Arg[];
  facts: Fact[];
  goals: Goal[];
  actions: Action[];
  model: { decide: string | null; code: string[] };
}

export interface ServiceSummary {
  name: string;
  weave: string;
  request: string;
  stopped: boolean;
  state: "running" | "stopped" | "starting";
  status: Status | null;
}

export interface ServiceDetail {
  spec: Spec;
  status: Status | null;
  weave: Weave;
  weavefile: string;
  readme: string | null;
  learned: Learned;
  asks: Ask[];
}

export interface BoxInfo {
  host: string;
  os: string;
  kernel: string;
  cpus: number;
  mem_bytes: number;
  disks: { mount: string; size_bytes: number; used_bytes: number }[];
  uptime_s: number;
  version: string;
  daemon: { active: boolean; since?: string | null };
  spend_month_usd: number;
  asks_open: number;
}

export type Series = (number | null)[];

export interface Vitals {
  step_s: number;
  t: string[];
  cpu_pct: Series;
  mem_pct: Series;
  load1: Series;
  disk_pct: Record<string, Series>;
  net_rx_bps: Series;
  net_tx_bps: Series;
}

export interface VitalSample {
  t: string;
  cpu_pct: number | null;
  mem_pct: number | null;
  load1: number | null;
  disk_pct: Record<string, number | null>;
  net_rx_bps: number | null;
  net_tx_bps: number | null;
}

export interface Metrics {
  step_s: number;
  t: string[];
  facts: Record<string, Series>;
}

export interface LogStream {
  name: string;
  title: string;
  source: "fact" | "box";
}

export interface Finding {
  level: "error" | "warning" | "info";
  text: string;
}

export interface TellResult {
  ready: boolean;
  findings: Finding[];
  args_before: Record<string, string>;
  args_after: Record<string, string>;
}

export interface Session {
  auth: "none" | "password";
  ok: boolean;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

// Set by the session store: a 401 anywhere means the session ended.
let onUnauthorized: () => void = () => {};
export function setUnauthorizedHandler(f: () => void) {
  onUnauthorized = f;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(path, {
      method,
      credentials: "same-origin",
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiError(0, "The dashboard server does not answer.");
  }
  if (res.status === 401 && path !== "/api/login") onUnauthorized();
  if (!res.ok) {
    let message = `${res.status} ${res.statusText}`;
    try {
      const j = (await res.json()) as { error?: string };
      if (j.error) message = j.error;
    } catch {
      // Not JSON: keep the status line.
    }
    throw new ApiError(res.status, message);
  }
  if (res.status === 204 || res.status === 202) {
    const text = await res.text();
    return (text ? JSON.parse(text) : undefined) as T;
  }
  return (await res.json()) as T;
}

const enc = encodeURIComponent;

export const api = {
  session: () => request<Session>("GET", "/api/session"),
  login: (password: string) => request<void>("POST", "/api/login", { password }),
  logout: () => request<void>("POST", "/api/logout", {}),
  box: () => request<BoxInfo>("GET", "/api/box"),
  vitals: (range: string) => request<Vitals>("GET", `/api/vitals?range=${range}`),
  services: () => request<ServiceSummary[]>("GET", "/api/services"),
  service: (s: string) => request<ServiceDetail>("GET", `/api/services/${enc(s)}`),
  journal: (
    s: string,
    q: { before?: number; limit?: number; kind?: string[]; goal?: string; action?: string; q?: string },
  ) => {
    const p = new URLSearchParams();
    if (q.before !== undefined) p.set("before", String(q.before));
    p.set("limit", String(q.limit ?? 200));
    if (q.kind?.length) p.set("kind", q.kind.join(","));
    if (q.goal) p.set("goal", q.goal);
    if (q.action) p.set("action", q.action);
    if (q.q) p.set("q", q.q);
    return request<{ lines: Line[]; more: boolean }>("GET", `/api/services/${enc(s)}/journal?${p}`);
  },
  metrics: (s: string, range: string) =>
    request<Metrics>("GET", `/api/services/${enc(s)}/metrics?range=${range}`),
  logs: (s: string) => request<LogStream[]>("GET", `/api/services/${enc(s)}/logs`),
  answer: (s: string, n: number, word: string) =>
    request<void>("POST", `/api/services/${enc(s)}/answer`, { n, word }),
  run: (s: string, action: string) => request<void>("POST", `/api/services/${enc(s)}/do`, { action }),
  task: (s: string, text: string) => request<void>("POST", `/api/services/${enc(s)}/task`, { text }),
  tell: (s: string, req: string, apply: boolean) =>
    request<TellResult>("POST", `/api/services/${enc(s)}/tell`, { request: req, apply }),
  stop: (s: string) => request<void>("POST", `/api/services/${enc(s)}/stop`, {}),
  start: (s: string) => request<void>("POST", `/api/services/${enc(s)}/start`, {}),
  forget: (s: string, name: string) =>
    request<void>("POST", `/api/services/${enc(s)}/forget`, { name }),
};

export function logStreamUrl(s: string, name: string, q?: Record<string, string>) {
  const p = q ? new URLSearchParams(Object.entries(q).filter(([, v]) => v)) : null;
  const qs = p && [...p].length ? `?${p}` : "";
  return `/api/services/${enc(s)}/logs/${enc(name)}${qs}`;
}
