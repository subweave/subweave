import type { ReactNode } from "react";
import { Section } from "../components/Section";

type Box = { x: number; y: number; w: number; h: number };
type Layout = {
  view: [number, number];
  boxes: Record<"weave" | "daemon" | "decider" | "coder" | "channels", Box>;
  // One path and one badge point per step, in step order.
  edges: { d: string; at: [number, number] }[];
};

const DESKTOP: Layout = {
  view: [1100, 470],
  boxes: {
    weave: { x: 10, y: 135, w: 240, h: 200 },
    daemon: { x: 340, y: 135, w: 330, h: 200 },
    decider: { x: 770, y: 10, w: 320, h: 130 },
    coder: { x: 770, y: 170, w: 320, h: 120 },
    channels: { x: 770, y: 320, w: 320, h: 140 },
  },
  edges: [
    { d: "M250 235 H338", at: [295, 235] },
    { d: "M670 165 C720 165 720 55 768 55", at: [714, 112] },
    { d: "M770 105 C725 105 725 190 672 190", at: [732, 150] },
    { d: "M670 215 H768", at: [720, 215] },
    { d: "M770 255 H672", at: [720, 255] },
    { d: "M670 290 C720 290 720 370 768 370", at: [714, 330] },
    { d: "M770 420 C725 420 725 315 672 315", at: [736, 390] },
  ],
};

const MOBILE: Layout = {
  view: [360, 760],
  boxes: {
    weave: { x: 1, y: 1, w: 170, h: 200 },
    decider: { x: 189, y: 1, w: 170, h: 200 },
    daemon: { x: 1, y: 280, w: 358, h: 200 },
    coder: { x: 1, y: 560, w: 170, h: 198 },
    channels: { x: 189, y: 560, w: 170, h: 198 },
  },
  edges: [
    { d: "M86 201 V278", at: [86, 240] },
    { d: "M240 280 V203", at: [240, 240] },
    { d: "M310 201 V278", at: [310, 240] },
    { d: "M60 480 V558", at: [60, 520] },
    { d: "M120 560 V482", at: [120, 520] },
    { d: "M240 480 V558", at: [240, 520] },
    { d: "M310 560 V482", at: [310, 520] },
  ],
};

const STEPS = [
  "The daemon loads the weave.",
  "It reads the facts that due goals need, and sends them to the decider.",
  "The decider judges the goals and picks an action. Code gates it with the rules.",
  "A DO step goes to the coder, one gated shell command at a time.",
  "The commands that worked are kept, and run as code from then on.",
  "A rule that asks, a pick under 0.5, or used-up tries: the daemon asks the person.",
  "The answer comes back. A Steery answer is signed by the person's device.",
];

const CONTENT: Record<keyof Layout["boxes"], { tone: string; kicker: string; title: string; body: ReactNode }> = {
  weave: {
    tone: "code",
    kicker: "Weavefile",
    title: "The weave",
    body: (
      <ul className="chips">
        <li>GOALs: a graph</li>
        <li>FACTs</li>
        <li>ACTIONs</li>
        <li>ARG levels</li>
        <li>NEVER, ASK</li>
        <li>SKILLs</li>
      </ul>
    ),
  },
  daemon: {
    tone: "code",
    kicker: "on your box, as root",
    title: "The daemon",
    body: (
      <ul className="chips">
        <li>reads facts</li>
        <li>checks goals</li>
        <li>gates with rules</li>
        <li>runs steps</li>
        <li>learned commands</li>
        <li>journal</li>
      </ul>
    ),
  },
  decider: {
    tone: "model",
    kicker: "decision model",
    title: "Jev",
    body: <p>Judges every request, goal, pick and command, with probabilities.</p>,
  },
  coder: {
    tone: "model",
    kicker: "coding model",
    title: "DeepSeek V4 Flash",
    body: <p>Writes a box's commands once.</p>,
  },
  channels: {
    tone: "person",
    kicker: "the person",
    title: "Channels",
    body: (
      <ul className="chips">
        <li>Steery</li>
        <li>Telegram</li>
        <li>Slack</li>
        <li>email</li>
        <li>webhook</li>
      </ul>
    ),
  },
};

function Diagram({ layout, className }: { layout: Layout; className: string }) {
  const [w, h] = layout.view;
  return (
    <svg className={`arch ${className}`} viewBox={`0 0 ${w} ${h}`} aria-hidden="true">
      <defs>
        <marker id={`arch-arrow-${className}`} viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
          <path d="M0 0 L8 4 L0 8 z" className="arch-head" />
        </marker>
      </defs>
      {layout.edges.map((e, i) => (
        <g key={i} className="arch-edge" style={{ ["--i" as string]: i }}>
          <path className="arch-line" d={e.d} markerEnd={`url(#arch-arrow-${className})`} />
          <path className="arch-flow" d={e.d} />
          <circle className="arch-badge" cx={e.at[0]} cy={e.at[1]} r="10" />
          <text className="arch-n" x={e.at[0]} y={e.at[1]} textAnchor="middle" dominantBaseline="central">
            {i + 1}
          </text>
        </g>
      ))}
      {(Object.keys(layout.boxes) as (keyof Layout["boxes"])[]).map((k) => {
        const b = layout.boxes[k];
        const c = CONTENT[k];
        return (
          <foreignObject key={k} x={b.x} y={b.y} width={b.w} height={b.h}>
            <div className={`arch-box arch-${c.tone}`}>
              <p className="arch-kicker">{c.kicker}</p>
              <p className="arch-title">{c.title}</p>
              {c.body}
            </div>
          </foreignObject>
        );
      })}
    </svg>
  );
}

function Chips({ items, note }: { items: string[]; note?: string }) {
  return (
    <div className="why-chips">
      {items.map((c, i) => (
        <span key={i} className="why-chip">
          {c}
        </span>
      ))}
      {note && <span className="why-note">{note}</span>}
    </div>
  );
}

// A sysadmin's map: watch, try, then call someone.
function MapTree() {
  return (
    <svg className="why-map" viewBox="0 0 300 92" aria-hidden="true">
      <path className="why-map-edge" d="M14 20 V46 H34 M14 46 V72 H58" />
      <circle className="why-map-dot is-code" cx="14" cy="20" r="5" />
      <text x="28" y="24">watch this</text>
      <circle className="why-map-dot is-model" cx="38" cy="46" r="5" />
      <text x="52" y="50">if it breaks, try that</text>
      <circle className="why-map-dot is-person" cx="62" cy="72" r="5" />
      <text x="76" y="76">if that fails, call someone</text>
    </svg>
  );
}

const COSTS: [string, string, string, number][] = [
  ["checks", "code, free", "code", 4],
  ["picks", "Jev, about $0.00004", "model", 22],
  ["new trouble", "coder", "coder", 100],
];

function Costs() {
  return (
    <ul className="why-costs">
      {COSTS.map(([what, who, tone, w]) => (
        <li key={what} className={`why-cost is-${tone}`}>
          <span className="why-cost-what">{what}</span>
          <span className="why-cost-arrow" aria-hidden="true">
            →
          </span>
          <span className="why-cost-who">{who}</span>
          <span className="why-bar" aria-hidden="true">
            <span style={{ width: `${w}%` }} />
          </span>
        </li>
      ))}
    </ul>
  );
}

const WHY: { title: string; body: string; visual: ReactNode }[] = [
  {
    title: "Skills are too loose.",
    body: "A skill file is a pile of tips. It doesn't say what to watch, what to do when something breaks, or when to stop and ask you.",
    visual: <Chips items={["tip", "tip", "tip", "tip"]} note="what to watch? when to ask?" />,
  },
  {
    title: "Agents are heavy.",
    body: "A coding agent wakes a big model for every step. Subweave checks with plain code, picks the next step with a tiny decision model, and calls the coder only when something new breaks. A fix that worked is saved and replayed with no model at all.",
    visual: <Costs />,
  },
  {
    title: "DevOps know-how is a map.",
    body: "Ask a good sysadmin how they run Postgres and you get a map: watch this, if it breaks try that, if that fails call someone. That map is the Weavefile. Skills are the know-how each step needs.",
    visual: <MapTree />,
  },
  {
    title: "Write the boring parts once.",
    body: "Every service needs settings, secrets, alerts, a way to ask you and a log of what happened. Subweave does those for every weave, so a Weavefile holds only what's special about your service.",
    visual: <Chips items={["ARGs", "secrets", "channels", "asks", "journal"]} note="built in" />,
  },
];

export function Architecture({ n }: { n: number }) {
  return (
    <Section id="architecture" n={n} kicker="Why Subweave" title="Why not just hand an agent a skill file?">
      <ul className="why-grid">
        {WHY.map((w) => (
          <li key={w.title} className="why-card">
            <div className="why-visual">{w.visual}</div>
            <h3>{w.title}</h3>
            <p>{w.body}</p>
          </li>
        ))}
      </ul>
      <h3 className="arch-heading">How it fits together</h3>
      <div className="arch-grid">
        <figure className="arch-figure">
          <Diagram layout={DESKTOP} className="arch-wide" />
          <Diagram layout={MOBILE} className="arch-narrow" />
        </figure>
        <ol className="arch-steps">
          {STEPS.map((s, i) => (
            <li key={s} style={{ ["--i" as string]: i }}>
              <span className="arch-step-n">{i + 1}</span>
              {s}
            </li>
          ))}
        </ol>
      </div>
    </Section>
  );
}
