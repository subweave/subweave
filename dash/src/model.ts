// What the views derive from status, weave and journal.
import type { Goal, GoalState, Line, Status, Weave } from "./api";
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
  { label: "Asks", kinds: ["ask", "answer"] },
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

export interface Episode {
  type: "episode";
  key: string;
  /** A goal's repair, or a person's task. */
  task: boolean;
  goal?: string;
  by?: string;
  text?: string;
  actions: string[];
  /** `holds` / `fails` once it ended; null while it runs. */
  outcome: "holds" | "fails" | null;
  start: string;
  end: string | null;
  lines: Line[];
}

export type Item = { type: "line"; line: Line } | Episode;

/** Folds lines into episodes: a goal from its failing check to its next holding check,
 *  and a person's task from start to end. Input and output are newest first. */
export function episodes(lines: Line[]): Item[] {
  const chrono = [...lines].reverse();
  const open = new Map<string, Episode>();
  let task: Episode | null = null;
  const items: Item[] = [];
  const failing = new Map<string, boolean>();

  for (const l of chrono) {
    if (l.action === "task") {
      if (!task || (l.kind === "action" && l.ok === undefined)) {
        task = {
          type: "episode",
          key: `task-${l.i}`,
          task: true,
          by: l.by,
          text: l.text,
          actions: [],
          outcome: null,
          start: l.t,
          end: null,
          lines: [],
        };
        items.push(task);
      }
      task.lines.push(l);
      if (l.kind === "action" && l.ok !== undefined) {
        task.outcome = l.ok ? "holds" : "fails";
        task.end = l.t;
        if (l.text) task.text = task.text ? task.text : l.text;
        task = null;
      }
      continue;
    }
    const g = l.goal;
    if (l.kind === "check" && g) {
      const ep = open.get(g);
      if (l.ok === false && !ep && !failing.get(g)) {
        const e: Episode = {
          type: "episode",
          key: `ep-${l.i}`,
          task: false,
          goal: g,
          actions: [],
          outcome: null,
          start: l.t,
          end: null,
          lines: [l],
        };
        open.set(g, e);
        items.push(e);
        failing.set(g, true);
        continue;
      }
      if (ep) {
        ep.lines.push(l);
        if (l.ok === true) {
          ep.outcome = "holds";
          ep.end = l.t;
          open.delete(g);
        }
      } else items.push({ type: "line", line: l });
      if (l.ok !== undefined) failing.set(g, l.ok === false);
      continue;
    }
    const ep = g ? open.get(g) : undefined;
    if (ep) {
      ep.lines.push(l);
      if (l.action && !ep.actions.includes(l.action) && (l.kind === "decide" || l.kind === "action" || l.kind === "command"))
        ep.actions.push(l.action);
      if (l.kind === "decide" && l.text && !ep.actions.includes(l.text) && l.text in (l.p ?? {}) && !["wait", "ask", "steered"].includes(l.text))
        ep.actions.push(l.text);
    } else items.push({ type: "line", line: l });
  }
  // An episode of one check with nothing after it says no more than the line.
  return items
    .map((it): Item => (it.type === "episode" && !it.task && it.lines.length === 1 ? { type: "line", line: it.lines[0] } : it))
    .map((it) => (it.type === "episode" ? { ...it, lines: [...it.lines].reverse() } : it))
    .reverse();
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
