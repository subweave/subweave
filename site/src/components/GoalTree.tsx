import type { Row } from "../weave/parse";

export type GoalState = "held" | "fail" | "wait" | "off";

const STEP = 20;
const HEIGHT = 40;
const R = 6;

// One SVG per row: the rails of its ancestors, its own elbow, its node.
function Rail({ row, state }: { row: Row; state: GoalState }) {
  const x = (depth: number) => 8 + depth * STEP;
  const mid = HEIGHT / 2;
  const cx = x(row.depth);
  const width = cx + R + 2;
  return (
    <svg className="rail" width={width} height={HEIGHT} viewBox={`0 0 ${width} ${HEIGHT}`} aria-hidden="true">
      {row.rails.map((on, c) => (c > 0 && on ? <line key={c} x1={x(c - 1)} x2={x(c - 1)} y1={0} y2={HEIGHT} /> : null))}
      {row.depth > 0 && (
        <path d={`M${x(row.depth - 1)} 0 V${row.last ? mid : HEIGHT} M${x(row.depth - 1)} ${mid} H${cx - R}`} />
      )}
      {row.kids && <line x1={cx} x2={cx} y1={mid + R} y2={HEIGHT} />}
      <circle className={`node node-${state}`} cx={cx} cy={mid} r={R} />
    </svg>
  );
}

const WORD: Record<GoalState, string> = { held: "holds", fail: "fails", wait: "waits", off: "off" };

export function GoalTree({
  rows,
  states,
  words,
  label,
}: {
  rows: Row[];
  states: Record<string, GoalState>;
  words: Record<string, string>;
  label: string;
}) {
  return (
    <ul className="tree" aria-label={label}>
      {rows.map((row) => {
        const g = row.goal;
        const state = states[g.name] ?? "held";
        return (
          <li key={g.name} className={`tree-row is-${state}`}>
            <span className="tree-goal">
              <Rail row={row} state={state} />
              <span className="tree-name">{g.name}</span>
              {g.when && <span className="tree-if">IF {g.when}</span>}
            </span>
            <span className="tree-words">{words[g.name] ?? ""}</span>
            <code className="tree-cond">{g.quoted ? "quoted: the decider judges it" : g.cond}</code>
            <span className="tree-state">{WORD[state]}</span>
          </li>
        );
      })}
    </ul>
  );
}
