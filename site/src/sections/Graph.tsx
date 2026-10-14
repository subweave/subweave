import { useEffect, useMemo, useState } from "react";
import content from "virtual:content";
import { GoalTree, type GoalState } from "../components/GoalTree";
import { Section } from "../components/Section";
import { useReducedMotion } from "../motion";
import { goalRows, offGoals } from "../weave/parse";

const pg = content.weaves.find((w) => w.name === "postgres")!;
const rows = goalRows(pg.goals);
const BUCKET = "s3://acme-pg/db1";
const FAILING = "archiving";

// A few words per goal of the postgres weave; the code condition sits beside them.
const WORDS: Record<string, string> = {
  installed: "PostgreSQL and wal-g are installed",
  configured: "access is what the request asked",
  running: "accepts connections",
  archiving: "WAL reaches the bucket",
  backed: "a base backup in the last day",
  drilled: "a restore drill this month",
  connections: "under 80% of max connections",
  "short-tx": "no session idle in a transaction",
  frozen: "transaction IDs far from wraparound",
  patched: "no minor release pending",
  current: "the major release asked for",
  quiet: "the logs show nothing to act on",
  room: "the disk is under 85% full",
};

const waiting = (() => {
  const out: string[] = [];
  for (const r of rows) if (r.goal.after.some((a) => a === FAILING || out.includes(a))) out.push(r.goal.name);
  return out;
})();

const STEPS = [
  `${FAILING} fails. ${waiting.join(" and ")} wait: they come after it.`,
  "The decider picks archive over fix.",
  "The coder runs its DO step. The commands that work are kept.",
  `${FAILING} holds. ${waiting.join(" and ")} are checked again.`,
];
// [step shown, ms], step 0 = all hold
const TIMELINE: [number, number][] = [
  [0, 2200],
  [1, 2200],
  [2, 2000],
  [3, 2200],
  [4, 2600],
];

export function Graph({ n }: { n: number }) {
  const reduced = useReducedMotion();
  const [bucket, setBucket] = useState(true);
  const [t, setT] = useState(0);
  const off = useMemo(() => offGoals(pg.goals, bucket ? { bucket: BUCKET } : {}), [bucket]);
  const playing = bucket && !reduced;

  useEffect(() => {
    if (!playing) return;
    const id = setTimeout(() => setT((x) => (x + 1) % TIMELINE.length), TIMELINE[t][1]);
    return () => clearTimeout(id);
  }, [t, playing]);

  const step = playing ? TIMELINE[t][0] : 0;
  const states: Record<string, GoalState> = {};
  for (const g of pg.goals) {
    if (off.has(g.name)) states[g.name] = "off";
    else if (step >= 1 && step <= 3 && g.name === FAILING) states[g.name] = "fail";
    else if (step >= 1 && step <= 3 && waiting.includes(g.name)) states[g.name] = "wait";
    else states[g.name] = "held";
  }

  return (
    <Section
      id="graph"
      n={n}
      kicker="The goal graph"
      title="A goal is checked only while the goals before it hold."
      lede="The postgres weave's goals, read from its Weavefile at build time."
    >
      <div className="graph">
        <div className="graph-tree">
          <fieldset className="toggle">
            <legend>The request says</legend>
            <label>
              <input type="radio" name="bucket" checked={bucket} onChange={() => setBucket(true)} />
              <span>
                backups to <code>{BUCKET}</code>
              </span>
            </label>
            <label>
              <input type="radio" name="bucket" checked={!bucket} onChange={() => setBucket(false)} />
              <span>no bucket</span>
            </label>
          </fieldset>
          <GoalTree rows={rows} states={states} words={WORDS} label="Goals of the postgres weave" />
        </div>
        <aside className="graph-side" aria-live="polite">
          {bucket ? (
            <>
              <p className="side-title">When a goal fails</p>
              <ol className="repair">
                {STEPS.map((s, i) => (
                  <li key={s} className={reduced || step === i + 1 ? "on" : step > i + 1 ? "done" : ""}>
                    {s}
                  </li>
                ))}
              </ol>
            </>
          ) : (
            <>
              <p className="side-title">No bucket</p>
              <p className="side-text">
                <code className="kw">IF bucket</code> is false, so {[...off].join(", ")} are off. Nothing checks them.
              </p>
            </>
          )}
        </aside>
      </div>
    </Section>
  );
}
