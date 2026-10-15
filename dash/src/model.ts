// What the views derive from status, weave and journal.
import type { Goal, GoalState, Line, Status, Usage, Weave } from "./api";
import { ms } from "./format";
import type { Shown } from "./components/ui";

export type Counts = Record<GoalState, number>;

export function counts(status: Status | null | undefined): Counts {
  const c: Counts = { holds: 0, fails: 0, unknown: 0, off: 0, waiting: 0 };
  for (const g of status?.goals ?? []) c[g.state]++;
  return c;
}

/** The one glyph a service shows in lists: the worst thing about it. */
export function serviceShown(status: Status | null | undefined, stopped: boolean): Shown {
  if (stopped || !status) return "off";
  const c = counts(status);
  if (c.fails) return "fails";
  if (status.asks.length) return "ask";
  if (status.repair) return "running";
  if (c.waiting) return "waiting";
  if (c.unknown) return "unknown";
  return "holds";
}

/** Goals that count toward `holds/total`: every goal that is not off. */
export function holdsOf(status: Status | null | undefined): { holds: number; total: number } {
  const c = counts(status);
  return { holds: c.holds, total: c.holds + c.fails + c.unknown + c.waiting };
}

export interface GoalNode {
  goal: Goal;
  alsoAfter: string[];
  children: GoalNode[];
}

/** Goals as a tree by AFTER: a goal sits under its first parent, in file order. */
export function goalTree(weave: Weave): GoalNode[] {
  const nodes = new Map<string, GoalNode>();
  for (const g of weave.goals) nodes.set(g.name, { goal: g, alsoAfter: g.after.slice(1), children: [] });
  const roots: GoalNode[] = [];
  for (const g of weave.goals) {
    const parent = g.after.length ? nodes.get(g.after[0]) : undefined;
    (parent ? parent.children : roots).push(nodes.get(g.name)!);
  }
  return roots;
}

export function condText(goal: Goal): { text: string; quoted: boolean } {
  return "code" in goal.cond ? { text: goal.cond.code, quoted: false } : { text: goal.cond.quoted, quoted: true };
}

/** `fact < 85`: the number a chart draws as the goal's limit. */
export function limitOf(goal: Goal): { fact: string; op: string; value: number } | null {
  if (!("code" in goal.cond)) return null;
  const m = /^\s*([a-z_][a-z0-9_]*)\s*(<=|>=|<|>|==|!=)\s*(-?\d+(?:\.\d+)?)\s*$/i.exec(goal.cond.code);
  return m ? { fact: m[1], op: m[2], value: Number(m[3]) } : null;
}

export const KIND_CHIPS = [
  { label: "Checks", kinds: ["check"] },
  { label: "Decisions", kinds: ["decide"] },
  { label: "Gates", kinds: ["gate"] },
  { label: "Actions", kinds: ["action"] },
  { label: "Commands", kinds: ["command"] },
  { label: "Learned", kinds: ["learn"] },
  { label: "Escalations", kinds: ["ask", "answer"] },
  { label: "Messages", kinds: ["message"] },
  { label: "Notes", kinds: ["note"] },
] as const;

/** Checks that only repeat a holding state (newest-first input). */
export function routineChecks(lines: Line[]): Set<number> {
  const out = new Set<number>();
  const last = new Map<string, boolean | undefined>();
  for (let k = lines.length - 1; k >= 0; k--) {
    const l = lines[k];
    if (l.kind !== "check" || !l.goal) continue;
    if (l.ok === true && last.get(l.goal) === true) out.add(l.i);
    last.set(l.goal, l.ok);
  }
  return out;
}

/** One timeline row: a line, with the lines folded into it. */
export interface Step {
  line: Line;
  /** The gate that raised this ask. */
  gate?: Line;
  /** The message lines that sent it to the channels. */
  sent: Line[];
}

export type Outcome = "running" | "held" | "done" | "failed" | "asked" | "learned" | "waited" | "stopped";

export interface Episode {
  type: "episode";
  key: string;
  /** A goal's repair, a person's task, or a learn outside a job. */
  what: "repair" | "task" | "learn";
  goal?: string;
  /** A repair's actions, in order. */
  actions: string[];
  /** A task's words; what a learn learned. */
  text?: string;
  by?: string;
  outcome: Outcome;
  start: string;
  /** Its last line once it ended; null while it runs. */
  end: string | null;
  cost: number;
  usage: Usage | null;
  /** Messages about the episode with no one line to sit on. */
  sent: Line[];
  /** Oldest first. */
  steps: Step[];
}

export type Item = { type: "step"; step: Step } | Episode;

const TASK = "task";
const WINDOW = 5000;
const HAND = /^do (\S+): asked by hand$/;
const META = ["wait", "ask", "steered"];

function push<K, V>(m: Map<K, V[]>, k: K, v: V) {
  const a = m.get(k);
  if (a) a.push(v);
  else m.set(k, [v]);
}

/** The line nearest `chrono[k]` within `window` ms that `f` takes, earlier first on a tie. */
function nearest(chrono: Line[], ts: number[], k: number, f: (l: Line) => boolean, window: number): Line | null {
  for (let d = 1; d < chrono.length; d++) {
    let inside = false;
    for (const j of [k - d, k + d]) {
      if (j < 0 || j >= chrono.length || Math.abs(ts[j] - ts[k]) > window) continue;
      inside = true;
      if (f(chrono[j])) return chrono[j];
    }
    if (!inside) return null;
  }
  return null;
}

/** The line a message line announces: by ask number, else by what it says. */
function announced(chrono: Line[], ts: number[], k: number, gone: Set<number>): Line | null {
  const m = chrono[k];
  const rest = (m.text ?? "").replace(/^[a-z]+:\s*/, "");
  const n = /^#(\d+)\b/.exec(rest);
  const asked = n && { kind: rest.endsWith(" withdrawn") ? "answer" : "ask", n: Number(n[1]) };
  const learned = /^learned (.+)$/.exec(rest)?.[1];
  const fails = /^(\S+) fails$/.exec(rest)?.[1];
  const holds = /^(\S+) holds again$/.exec(rest)?.[1];
  return nearest(
    chrono,
    ts,
    k,
    (l) => {
      if (l.kind === "message" || gone.has(l.i) || (m.ep && l.ep && l.ep !== m.ep)) return false;
      if (asked) return l.kind === asked.kind && l.n === asked.n;
      if (learned) return l.kind === "learn" && (l.text === learned || (l.text ?? "").startsWith(`${learned}:`));
      if (fails) return l.kind === "check" && l.goal === fails && l.ok === false;
      if (holds) return l.goal === holds && (l.kind === "check" || l.kind === "action") && l.ok === true;
      return l.text === rest || l.text === m.text;
    },
    asked ? Infinity : WINDOW,
  );
}

function addUsage(a: Usage | null, b: Usage): Usage {
  if (!a) return { ...b };
  return {
    model: b.model,
    calls: a.calls + b.calls,
    input: a.input + b.input,
    cached: a.cached + b.cached,
    output: a.output + b.output,
    reasoning: (a.reasoning ?? 0) + (b.reasoning ?? 0) || undefined,
  };
}

/** Older journals have no `ep`: a goal from its failing check to its next holding check,
 *  and a person's task from start to end. Chronological in and out. */
function legacy(chrono: Line[]): ({ lines: Line[]; held: boolean } | Line)[] {
  const open = new Map<string, { lines: Line[]; held: boolean }>();
  let task: { lines: Line[]; held: boolean } | null = null;
  const out: ({ lines: Line[]; held: boolean } | Line)[] = [];
  const failing = new Map<string, boolean>();
  for (const l of chrono) {
    if (l.action === TASK) {
      if (!task || (l.kind === "action" && l.ok === undefined)) {
        task = { lines: [], held: false };
        out.push(task);
      }
      task.lines.push(l);
      if (l.kind === "action" && l.ok !== undefined) task = null;
      continue;
    }
    const g = l.goal;
    if (l.kind === "check" && g) {
      const ep = open.get(g);
      if (l.ok === false && !ep && !failing.get(g)) {
        const e = { lines: [l], held: false };
        open.set(g, e);
        out.push(e);
        failing.set(g, true);
        continue;
      }
      if (ep) {
        ep.lines.push(l);
        if (l.ok === true) {
          ep.held = true;
          open.delete(g);
        }
      } else out.push(l);
      if (l.ok !== undefined) failing.set(g, l.ok === false);
      continue;
    }
    const ep = g ? open.get(g) : undefined;
    if (ep) ep.lines.push(l);
    else out.push(l);
  }
  // A goal's episode of one check says no more than the line.
  return out.map((it) => ("lines" in it && it.lines.length === 1 && it.lines[0].action !== TASK ? it.lines[0] : it));
}

/** The timeline: lines folded into episodes and rows. Lines in, newest first; items out,
 *  newest first. `running` is the status's repair: an open job episode runs only then. */
export function timeline(lines: Line[], running: string | null): Item[] {
  const chrono = [...lines].reverse();
  const ts = chrono.map((l) => ms(l.t));
  const gone = new Set<number>();

  // An ask and the gate that raised it are one row.
  const gateOf = new Map<number, Line>();
  chrono.forEach((g, k) => {
    if (g.kind !== "gate" || g.n === undefined) return;
    const ask = nearest(chrono, ts, k, (l) => l.kind === "ask" && l.n === g.n && !gateOf.has(l.i), WINDOW);
    if (!ask) return;
    gateOf.set(ask.i, g);
    gone.add(g.i);
  });

  // A message is a mark on the line it announces, else on its episode.
  const sentOn = new Map<number, Line[]>();
  const sentEp = new Map<string, Line[]>();
  chrono.forEach((m, k) => {
    if (m.kind !== "message") return;
    const to = announced(chrono, ts, k, gone);
    if (to) push(sentOn, to.i, m);
    else if (m.ep) push(sentEp, m.ep, m);
    else return;
    gone.add(m.i);
  });

  const groups = new Map<string, Line[]>();
  for (const l of chrono) if (l.ep) push(groups, l.ep, l);

  // Lines other tasks wrote for an episode: "do X: asked by hand" just before it, and its
  // goal's checks while it ran (and the failing one that began it).
  const spans = [...groups.entries()].map(([ep, ls]) => ({
    ep,
    ls,
    goal: ls.find((l) => l.goal)?.goal,
    t0: ms(ls[0].t),
    t1: ms(ls[ls.length - 1].t),
    hand: false,
  }));
  const adopted = new Set<number>();
  chrono.forEach((l, k) => {
    if (l.ep || gone.has(l.i)) return;
    const t = ts[k];
    let s: (typeof spans)[number] | undefined;
    const hand = l.kind === "note" ? HAND.exec(l.text ?? "") : null;
    if (hand) {
      s = spans.find((s) => !s.hand && s.t0 >= t - 1000 && s.t0 - t <= 10_000 && s.ls.some((x) => x.action === hand[1]));
      if (s) s.hand = true;
    } else if (l.kind === "check" && l.goal) {
      s = spans.find((s) => s.goal === l.goal && t <= s.t1 && (t >= s.t0 || (l.ok === false && s.t0 - t <= 60_000)));
    }
    if (!s) return;
    s.ls.push(l);
    adopted.add(l.i);
  });

  function build(key: string, all: Line[], ep: string | undefined, held: boolean): Episode {
    all.sort((a, b) => a.i - b.i);
    const task = all.some((l) => l.action === TASK);
    const start = task ? all.find((l) => l.kind === "action" && l.action === TASK && l.ok === undefined) : undefined;
    const hand = all.find((l) => l.kind === "note" && !l.ep && HAND.test(l.text ?? ""));
    const learns = all.filter((l) => l.kind === "learn");
    const cannot = all.find((l) => l.kind === "note" && /^cannot learn /.test(l.text ?? ""));
    const job = all.some((l) => l.kind === "decide" || l.kind === "command" || l.kind === "action" || l.kind === "gate" || l.kind === "ask");
    const what = task ? "task" : !job && (learns.length || cannot) ? "learn" : "repair";
    const end = [...all].reverse().find((l) => l.kind === "action" && l.ok !== undefined);

    const actions: string[] = [];
    let usage: Usage | null = null;
    let cost = 0;
    for (const l of all) {
      const a =
        l.kind === "decide"
          ? l.text && l.text in (l.p ?? {}) && !META.includes(l.text)
            ? l.text
            : undefined
          : l.kind === "action" || l.kind === "command" || l.kind === "gate" || l.kind === "ask"
            ? l.action
            : undefined;
      if (a && a !== TASK && !actions.includes(a)) actions.push(a);
      if (l.kind === "note" && l.usage) usage = addUsage(usage, l.usage);
      cost += l.cost_usd ?? 0;
      const gate = gateOf.get(l.i);
      if (gate && !gate.ep) cost += gate.cost_usd ?? 0;
    }

    let outcome: Outcome | null = null;
    if (held) outcome = "held";
    else if (end) outcome = end.ok ? (task ? "done" : "held") : "failed";
    else if (what === "learn") outcome = learns.length ? "learned" : cannot ? "failed" : null;
    else if (all.some((l) => l.kind === "ask")) outcome = "asked";

    const fact = learns[0]?.text?.split(":")[0] ?? (cannot ? /^cannot learn ([^:]+)/.exec(cannot.text ?? "")?.[1] : undefined);
    return {
      type: "episode",
      key,
      what,
      goal: all.find((l) => l.goal)?.goal,
      actions,
      text: task ? (start?.text ?? undefined) : what === "learn" ? fact : undefined,
      by: start?.by ?? hand?.by ?? (hand ? "hand" : undefined),
      outcome: outcome ?? "running",
      start: all[0].t,
      end: outcome ? all[all.length - 1].t : null,
      cost,
      usage,
      sent: ep ? (sentEp.get(ep) ?? []) : [],
      steps: all
        .filter((l) => !gone.has(l.i) && l !== start && l !== hand && !(l.kind === "note" && l.usage))
        .map((l) => ({ line: l, gate: gateOf.get(l.i), sent: sentOn.get(l.i) ?? [] })),
    };
  }

  const items: { at: number; item: Item }[] = [];
  for (const s of spans) items.push({ at: Math.min(...s.ls.map((l) => l.i)), item: build(`ep-${s.ep}`, s.ls, s.ep, false) });
  const rest = chrono.filter((l) => !l.ep && !gone.has(l.i) && !adopted.has(l.i));
  // Guessed episodes only for lines older than the first with an `ep`; after it, a line with
  // none belongs to no episode.
  const first = chrono.find((l) => l.ep)?.i ?? Infinity;
  const older = legacy(rest.filter((l) => l.i < first));
  for (const it of [...older, ...rest.filter((l) => l.i >= first)]) {
    if ("lines" in it) items.push({ at: it.lines[0].i, item: build(`lg-${it.lines[0].i}`, it.lines, undefined, it.held) });
    else items.push({ at: it.i, item: { type: "step", step: { line: it, gate: gateOf.get(it.i), sent: sentOn.get(it.i) ?? [] } } });
  }
  items.sort((a, b) => b.at - a.at);

  // Open episodes: the newest job runs while the status names a repair, a learn while it is
  // the newest item; the rest stopped (or chose to wait).
  let job = running !== null;
  for (const { item: e } of items) {
    if (e.type !== "episode" || e.end !== null) continue;
    if (e.what === "learn" ? e === items[0].item : job) {
      if (e.what !== "learn") job = false;
      continue;
    }
    if (e.what !== "learn") job = false;
    e.outcome = e.steps.some((s) => s.line.kind === "decide" && s.line.text === "wait") ? "waited" : "stopped";
    e.end = e.steps.length ? e.steps[e.steps.length - 1].line.t : e.start;
  }
  return items.map((x) => x.item);
}

/** The repair running now: `configure for configured` and the journal lines behind it. */
export function repairNow(repair: string, lines: Line[]) {
  const action = repair.split(" for ")[0];
  const mine: Line[] = [];
  for (const l of lines) {
    if (l.action !== action) {
      if (mine.length && l.kind === "decide" && l.text === action) mine.push(l);
      continue;
    }
    if (l.kind === "action" && l.ok !== undefined) break;
    mine.push(l);
    if (l.kind === "action") break;
  }
  const commands = mine.filter((l) => l.kind === "command");
  return {
    action,
    start: mine.length ? mine[mine.length - 1].t : null,
    commands: commands.length,
    last: commands[0]?.text ?? null,
  };
}
