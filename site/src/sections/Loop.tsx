import { Code } from "../components/Code";
import { Section } from "../components/Section";

type Tone = "code" | "model" | "person";

const SPINE_X = 28;
const SPINE_W = 172;
const SIDE_X = 226;
const SIDE_W = 128;
const H = 34;

function Box({ x, y, w, h = H, tone, lines }: { x: number; y: number; w: number; h?: number; tone: Tone; lines: string[] }) {
  const mid = y + h / 2;
  return (
    <g className={`box box-${tone}`}>
      <rect x={x} y={y} width={w} height={h} rx={tone === "person" ? h / 2 : 4} />
      {lines.map((l, i) => (
        <text key={l} x={x + w / 2} y={mid + (i - (lines.length - 1) / 2) * 13} dominantBaseline="central">
          {l}
        </text>
      ))}
    </g>
  );
}

function Down({ y1, y2, label }: { y1: number; y2: number; label?: string }) {
  const x = SPINE_X + SPINE_W / 2;
  return (
    <g>
      <line className="edge" x1={x} x2={x} y1={y1} y2={y2 - 2} markerEnd="url(#arrow)" />
      {label && (
        <text className="edge-label" x={x + 8} y={(y1 + y2) / 2} dominantBaseline="central">
          {label}
        </text>
      )}
    </g>
  );
}

function Right({ y, label }: { y: number; label?: string }) {
  return (
    <g>
      <line className="edge" x1={SPINE_X + SPINE_W} x2={SIDE_X - 2} y1={y} y2={y} markerEnd="url(#arrow)" />
      {label && (
        <text className="edge-label" x={SPINE_X + SPINE_W + 6} y={y - 7}>
          {label}
        </text>
      )}
    </g>
  );
}

// The loop of WEAVEFILE.md "The loop", drawn as a flow chart.
function Diagram() {
  const row = [12, 76, 140, 212, 284, 356, 428, 500];
  return (
    <svg className="loop" viewBox="0 0 360 548" role="img" aria-labelledby="loop-svg-title">
      <title id="loop-svg-title">
        The loop: check the goals; a goal fails; the decider picks an action, or asks the person when its confidence
        is under 0.5 or the output on the box tried to steer it; the rules gate the action: NEVER makes it pick again,
        ASK asks the person; the steps run; if the goal holds within its time, the steps are learned and the loop checks
        again; if not and tries are left, it picks again; else it asks the person.
      </title>
      <defs>
        <marker id="arrow" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M0 0 L8 4 L0 8 z" className="arrowhead" />
        </marker>
      </defs>

      <Box x={SPINE_X} y={row[0]} w={SPINE_W} tone="code" lines={["check the goals"]} />
      <Down y1={row[0] + H} y2={row[1]} />
      <Box x={SPINE_X} y={row[1]} w={SPINE_W} tone="code" lines={["a goal fails"]} />
      <Down y1={row[1] + H} y2={row[2]} />
      <Box x={SPINE_X} y={row[2]} w={SPINE_W} tone="model" lines={["the decider picks"]} />
      <Right y={row[2] + H / 2} />
      <Box x={SIDE_X} y={row[2]} w={SIDE_W} tone="person" lines={["under 0.5, or steered:", "ask the person"]} />
      <Down y1={row[2] + H} y2={row[3]} />
      <Box x={SPINE_X} y={row[3]} w={SPINE_W} tone="code" lines={["the rules gate it"]} />
      <path className="edge" d={`M${SPINE_X + SPINE_W} ${row[3] + H / 2} H${SIDE_X - 12} V${row[3] - 2} H${SIDE_X - 2}`} markerEnd="url(#arrow)" />
      <path className="edge" d={`M${SIDE_X - 12} ${row[3] + H / 2} V${row[3] + H + 2} H${SIDE_X - 2}`} markerEnd="url(#arrow)" />
      <Box x={SIDE_X} y={row[3] - 15} w={SIDE_W} h={26} tone="code" lines={["NEVER: pick again"]} />
      <Box x={SIDE_X} y={row[3] + H - 11} w={SIDE_W} h={26} tone="person" lines={["ASK: ask the person"]} />
      <Down y1={row[3] + H} y2={row[4]} />
      <Box x={SPINE_X} y={row[4]} w={SPINE_W} tone="code" lines={["the steps run"]} />
      <Down y1={row[4] + H} y2={row[5]} />
      <Box x={SPINE_X} y={row[5]} w={SPINE_W} tone="code" lines={["holds WITHIN its time?"]} />
      <Right y={row[5] + H / 2} label="yes" />
      <Box x={SIDE_X} y={row[5]} w={SIDE_W} tone="code" lines={["learn the steps,", "tell, check again"]} />
      <path
        className="edge edge-back"
        d={`M${SPINE_X} ${row[5] + H / 2} H12 V${row[0] + H / 2} H${SPINE_X - 2}`}
        markerEnd="url(#arrow)"
      />
      <Down y1={row[5] + H} y2={row[6]} label="no" />
      <Box x={SPINE_X} y={row[6]} w={SPINE_W} tone="code" lines={["TRIES left?"]} />
      <Right y={row[6] + H / 2} label="yes" />
      <Box x={SIDE_X} y={row[6]} w={SIDE_W} tone="model" lines={["pick again"]} />
      <Down y1={row[6] + H} y2={row[7]} label="no" />
      <Box x={SPINE_X} y={row[7]} w={SPINE_W} tone="person" lines={["ask the person"]} />
    </svg>
  );
}

const STATS: { n: string; text: string }[] = [
  { n: "0.81", text: "A log line told the operator to restore. Read as plain facts, it steered the pick to restore." },
  { n: "0.97", text: "The steered question catches that line. At 0.5 or more, nothing runs and the person is asked." },
  { n: "<0.5", text: "Every decision under 0.5 confidence asks the person. Code asks, not the model." },
  { n: "signed", text: "A Steery answer is signed by the person's device. The daemon checks it before it acts." },
];

export function Loop({ n }: { n: number }) {
  return (
    <Section
      id="loop"
      n={n}
      kicker="The decision model"
      title="Jev judges. Code decides what follows."
      lede="Every request, quoted goal, pick and coder command is a typed question to the decider. Its answers come with probabilities, and thresholds in code act on them."
    >
      <div className="loop-grid">
        <figure className="loop-figure">
          <Diagram />
          <figcaption>
            <span className="swatch swatch-code" /> code <span className="swatch swatch-model" /> model{" "}
            <span className="swatch swatch-person" /> the person
          </figcaption>
        </figure>
        <div className="loop-io">
          <p className="io-title">
            Request: <code>running</code> fails on postgres
          </p>
          <Code name="request" label="The decider request" className="json" />
          <p className="io-title">Answer</p>
          <p className="answer">
            <span className="a-key">next</span>
            <span className="a-pick">restart</span>
            <span className="a-p">0.98</span>
          </p>
          <Code name="journal" label="The journal line" className="json" />
        </div>
      </div>
      <div className="stats">
        {STATS.map((s) => (
          <div className="stat" key={s.n}>
            <p className={`stat-n${s.n === "signed" ? " stat-word" : ""}`}>{s.n}</p>
            <p>{s.text}</p>
          </div>
        ))}
      </div>
    </Section>
  );
}
