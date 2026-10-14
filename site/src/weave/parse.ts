// Reads the parts of a Weavefile the page draws: ABOUT, ARGs and GOALs.
// Runs at build time (build/content.ts); the browser gets the result as JSON.

export type Level = "MUST" | "SHOULD" | "MAY";

export interface ArgOption {
  label: string;
  meaning: string;
}

export interface Arg {
  name: string;
  default: string | null;
  level: Level;
  what: string | null;
  options: ArgOption[];
}

export interface Goal {
  name: string;
  after: string[];
  every: string | null;
  tries: string | null;
  cond: string;
  quoted: boolean;
  when: string | null;
}

export interface Weave {
  name: string;
  about: string;
  args: Arg[];
  goals: Goal[];
  // NEVER and ASK lines, each on one line.
  rules: string[];
}

const HEREDOC = /<<([A-Z]+)\s*(\S.*)?$/;

// Joins `\` lines and drops heredoc bodies, so each instruction is one line.
function logicalLines(text: string): string[] {
  const out: string[] = [];
  const raw = text.split("\n");
  let end: string | null = null;
  let acc = "";
  for (const line of raw) {
    if (end !== null) {
      if (line.trim() === end) end = null;
      continue;
    }
    const doc = /^\s*[A-Z]/.test(line) ? HEREDOC.exec(line) : null;
    if (doc) end = doc[1];
    if (/^\s*#/.test(line) && acc === "") continue;
    if (line.endsWith("\\")) {
      acc = acc ? `${acc} ${line.slice(0, -1).trim()}` : line.slice(0, -1).trimEnd();
      continue;
    }
    out.push(acc ? `${acc} ${line.trim()}` : line);
    acc = "";
  }
  return out;
}

function unquote(s: string): string {
  return s.replace(/^"/, "").replace(/"$/, "");
}

export function parseWeave(name: string, text: string): Weave {
  const lines = logicalLines(text);
  let about = "";
  const args: Arg[] = [];
  const goals: Goal[] = [];
  const rules: string[] = [];
  let arg: Arg | null = null;
  for (const line of lines) {
    const head = /^(\s*)(\S+)/.exec(line);
    if (!head) {
      arg = null;
      continue;
    }
    const indented = head[1].length > 0;
    const word = head[2];
    if (arg && indented && !/^[A-Z]{2,}$/.test(word)) {
      const m = /^\s*(.+?)\s+("(?:[^"]*)")\s*$/.exec(line);
      if (m) arg.options.push({ label: m[1], meaning: unquote(m[2]) });
      continue;
    }
    arg = null;
    if (word === "ABOUT") {
      about = unquote(line.trim().slice(5).trim());
    } else if (word === "ARG") {
      const m = /^ARG\s+([a-z_][\w-]*)(?:=(\S+))?\s+(MUST|SHOULD|MAY)(?:\s+(".*"))?\s*$/.exec(line.trim());
      if (!m) throw new Error(`${name}: cannot read ${line}`);
      arg = { name: m[1], default: m[2] ?? null, level: m[3] as Level, what: m[4] ? unquote(m[4]) : null, options: [] };
      args.push(arg);
    } else if (word === "GOAL") {
      goals.push(parseGoal(name, line.trim()));
    } else if (word === "NEVER" || word === "ASK") {
      rules.push(line.trim().replace(/\s+/g, " "));
    }
  }
  return { name, about, args, goals, rules };
}

function parseGoal(weave: string, line: string): Goal {
  let rest = line.slice(4).trim();
  const take = (re: RegExp): RegExpExecArray | null => {
    const m = re.exec(rest);
    if (m) rest = rest.slice(m[0].length).trim();
    return m;
  };
  const name = take(/^([a-z][\w-]*)/);
  if (!name) throw new Error(`${weave}: cannot read ${line}`);
  let after: string[] = [];
  let every: string | null = null;
  let tries: string | null = null;
  for (;;) {
    const a = take(/^AFTER\s+([a-z][\w-]*(?:\s*,\s*[a-z][\w-]*)*)/);
    if (a) {
      after = a[1].split(/\s*,\s*/);
      continue;
    }
    const e = take(/^EVERY\s+(\S+)/);
    if (e) {
      every = e[1];
      continue;
    }
    const t = take(/^TRIES\s+(\d+\s+PER\s+\S+)/);
    if (t) {
      tries = t[1];
      continue;
    }
    break;
  }
  let cond: string;
  let when: string | null = null;
  const quoted = rest.startsWith('"');
  if (quoted) {
    const close = rest.indexOf('"', 1);
    cond = rest.slice(1, close);
    const tail = rest.slice(close + 1).trim();
    if (tail.startsWith("IF ")) when = tail.slice(3).trim();
  } else {
    const at = rest.lastIndexOf(" IF ");
    if (at >= 0) {
      when = rest.slice(at + 4).trim();
      rest = rest.slice(0, at);
    }
    cond = rest.trim().replace(/\s+/g, " ");
  }
  return { name: name[1], after, every, tries, cond, quoted, when };
}

export interface Row {
  goal: Goal;
  depth: number;
  // For each ancestor level, whether a vertical line continues past this row.
  rails: boolean[];
  last: boolean;
  kids: boolean;
}

// The tree `subweave plan` draws: each goal under its first AFTER parent, file order.
export function goalRows(goals: Goal[]): Row[] {
  const rows: Row[] = [];
  const walk = (parent: string | null, depth: number, rails: boolean[]) => {
    const list = goals.filter((g) => (parent === null ? g.after.length === 0 : g.after[0] === parent));
    list.forEach((goal, i) => {
      const last = i === list.length - 1;
      rows.push({ goal, depth, rails, last, kids: goals.some((g) => g.after[0] === goal.name) });
      walk(goal.name, depth + 1, [...rails, !last]);
    });
  };
  walk(null, 0, []);
  return rows;
}

// A goal is off while its IF is false, or while any goal it comes AFTER is off.
export function offGoals(goals: Goal[], args: Record<string, string>): Set<string> {
  const off = new Set<string>();
  for (let changed = true; changed; ) {
    changed = false;
    for (const g of goals) {
      if (off.has(g.name)) continue;
      if ((g.when !== null && !evalIf(g.when, args)) || g.after.some((a) => off.has(a))) {
        off.add(g.name);
        changed = true;
      }
    }
  }
  return off;
}

// The IF forms the weaves use: names, 'strings', numbers, == != < <= > >=, AND OR NOT, parentheses.
export function evalIf(expr: string, args: Record<string, string>): boolean {
  const tokens = expr.match(/'[^']*'|[A-Za-z_][\w-]*|\d+(?:\.\d+)?|==|!=|<=|>=|[<>()]/g) ?? [];
  let i = 0;
  const peek = () => tokens[i];
  const value = (t: string): string => (t.startsWith("'") ? t.slice(1, -1) : /^\d/.test(t) ? t : (args[t] ?? ""));
  const term = (): boolean => {
    if (peek() === "NOT") {
      i++;
      return !term();
    }
    if (peek() === "(") {
      i++;
      const v = or();
      i++;
      return v;
    }
    const left = tokens[i++];
    const op = peek();
    if (op && ["==", "!=", "<", "<=", ">", ">="].includes(op)) {
      i++;
      const a = value(left);
      const b = value(tokens[i++]);
      const na = Number(a);
      const nb = Number(b);
      const num = a !== "" && b !== "" && !Number.isNaN(na) && !Number.isNaN(nb);
      switch (op) {
        case "==":
          return num ? na === nb : a === b;
        case "!=":
          return num ? na !== nb : a !== b;
        case "<":
          return num && na < nb;
        case "<=":
          return num && na <= nb;
        case ">":
          return num && na > nb;
        default:
          return num && na >= nb;
      }
    }
    return value(left) !== "";
  };
  const and = (): boolean => {
    let v = term();
    while (peek() === "AND") {
      i++;
      v = term() && v;
    }
    return v;
  };
  const or = (): boolean => {
    let v = and();
    while (peek() === "OR") {
      i++;
      v = and() || v;
    }
    return v;
  };
  return or();
}
