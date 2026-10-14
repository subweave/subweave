import { useEffect, useState } from "react";
import content from "virtual:content";
import { goalRows } from "../weave/parse";
import { useReducedMotion } from "../motion";
import { REPO } from "../links";
import { Code } from "./Code";
import { Command, Step } from "./Section";

type State = "pending" | "held" | "fail" | "wait";

const rows = goalRows(content.hero.goals);
const FAILING = "running";

// Every goal whose AFTER chain reaches `name`.
function after(name: string): string[] {
  const out: string[] = [];
  for (const r of rows) if (r.goal.after.some((a) => a === name || out.includes(a))) out.push(r.goal.name);
  return out;
}
const WAITING = after(FAILING);

// One pass: goals turn held top-down, `running` fails, the loop repairs it, the goals after it follow.
type Frame = { states: Record<string, State>; step: number };
function frames(): { frame: Frame; ms: number }[] {
  const out: { frame: Frame; ms: number }[] = [];
  const states: Record<string, State> = Object.fromEntries(rows.map((r) => [r.goal.name, "pending" as State]));
  const snap = (step: number, ms: number) => out.push({ frame: { states: { ...states }, step }, ms });
  snap(0, 700);
  for (const r of rows) {
    states[r.goal.name] = "held";
    snap(0, 380);
  }
  out[out.length - 1].ms = 1600;
  states[FAILING] = "fail";
  for (const n of WAITING) states[n] = "wait";
  snap(1, 1500);
  snap(2, 1600);
  snap(3, 1400);
  states[FAILING] = "held";
  snap(4, 500);
  for (const n of WAITING) {
    states[n] = "held";
    snap(4, 380);
  }
  out[out.length - 1].ms = 3600;
  return out;
}
const FRAMES = frames();
const FINAL: Frame = { states: Object.fromEntries(rows.map((r) => [r.goal.name, "held" as State])), step: 0 };

const W = 360;
const ROW = 62;
const CARD = 46;

function Graph({ frame }: { frame: Frame }) {
  const height = rows.length * ROW + 8;
  return (
    <svg className="hero-graph" viewBox={`0 0 ${W} ${height}`} aria-hidden="true">
      {rows.map((row, i) => {
        if (row.depth === 0) return null;
        const parent = rows.findIndex((r) => r.goal.name === row.goal.after[0]);
        const x = 8 + (row.depth - 1) * 26 + 16;
        return (
          <path
            key={`e-${row.goal.name}`}
            className="hg-edge"
            d={`M${x} ${parent * ROW + 4 + CARD} V${i * ROW + 4 + CARD / 2} H${x + 10}`}
          />
        );
      })}
      {rows.map((row, i) => {
        const g = row.goal;
        const x = 8 + row.depth * 26;
        const y = i * ROW + 4;
        const state = frame.states[g.name];
        const cond = state === "wait" ? `waits for ${g.after[0]}` : g.quoted ? `"${g.cond}"` : g.cond;
        return (
          <g key={g.name} className={`hg-card hg-${state}`}>
            <rect x={x + 0.5} y={y + 0.5} width={W - x - 9} height={CARD} rx={7} />
            <circle cx={x + 18} cy={y + CARD / 2} r={5} />
            <text className="hg-name" x={x + 32} y={y + 19}>
              {g.name}
            </text>
            <text className="hg-cond" x={x + 32} y={y + 36}>
              {cond}
            </text>
            <text className="hg-state" x={W - 20} y={y + 19} textAnchor="end">
              {state === "fail" ? "fails" : state === "held" ? "held" : state === "wait" ? "waits" : ""}
            </text>
            {g.when && (
              <text className="hg-if" x={W - 20} y={y + 36} textAnchor="end">
                IF {g.when}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}

const STEPS = [
  { who: "code", text: `${FAILING} fails` },
  { who: "model", text: "restart or restore? The decider picks restart, 0.98" },
  { who: "code", text: "RUN systemctl restart postgresql@18-main" },
  { who: "code", text: `${FAILING} holds within 2m; ${WAITING.join(" and ")} are checked again` },
];

export function Hero() {
  const reduced = useReducedMotion();
  const [i, setI] = useState(0);

  useEffect(() => {
    if (reduced) return;
    const id = setTimeout(() => setI((n) => (n + 1) % FRAMES.length), FRAMES[i].ms);
    return () => clearTimeout(id);
  }, [i, reduced]);

  const frame = reduced ? FINAL : FRAMES[i].frame;
  const step = reduced ? 0 : frame.step;

  return (
    <header className="hero" id="top">
      <div className="wrap">
        <div className="hero-copy">
          <div className="hero-title">
            <h1>
              DevOps
              <br />
              in a box...
            </h1>
            <p className="hero-sub">Like Docker containers, but powered by LLMs and decision models.</p>
          </div>
          <div className="hero-side">
            <p className="hero-line">
              Write a runbook for a service in human language. Subweave installs it on your Linux box and maintains it
              indefinitely.
            </p>
            <ol className="hero-commands">
              <li>
                <Step n={1}>Install the daemon</Step>
                <Command name="install" label="Install the Subweave daemon on Linux" />
              </li>
              <li>
                <Step n={2}>Run a weave</Step>
                <Command name="run" label="Run the postgres weave" />
              </li>
            </ol>
            <p className="hero-links">
              Linux, open source, MIT · <a href={REPO}>GitHub</a> · <a href="#weavefile">Six weaves</a>
            </p>
          </div>
        </div>

        <figure className="hero-panel">
          <div className="hero-file">
            <p className="panel-label">
              <span>Weavefile</span>
              <span className="panel-dim">postgres, shortened</span>
            </p>
            <Code name="hero" label="A short Weavefile for PostgreSQL" numbered />
          </div>
          <div className="hero-goals">
            <p className="panel-label">
              <span>Its goals</span>
              <span className="panel-dim">AFTER makes the graph</span>
            </p>
            <Graph frame={frame} />
            <ol className="hero-steps" aria-hidden="true">
              {STEPS.map((s, n) => (
                <li key={s.text} className={`hs-${s.who}${step === n + 1 ? " on" : ""}${step > n + 1 ? " done" : ""}`}>
                  {s.text}
                </li>
              ))}
            </ol>
            <p className="sr-only">
              The goals of this Weavefile: {rows.map((r) => r.goal.name).join(", ")}. When {FAILING} fails, the
              decider picks restart over restore, code runs systemctl restart, and the goal holds again within 2 minutes.
            </p>
          </div>
        </figure>
      </div>
    </header>
  );
}
