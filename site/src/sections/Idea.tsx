import { Section } from "../components/Section";

// Three of the postgres weave's goals, each kept by the same small loop.
const LOOPS = [
  { title: "Self-healing", goal: "running", when: "postgres was killed", action: "restart" },
  { title: "Self-maintained", goal: "backed", when: "the newest backup is a day old", action: "backup" },
  { title: "Self-updating", goal: "patched", when: "a minor release is out", action: "patch" },
];

const STATIONS: { label: string; angle: number; tone: string }[] = [
  { label: "check", angle: -90, tone: "code" },
  { label: "decide", angle: 30, tone: "model" },
  { label: "act", angle: 150, tone: "code" },
];

function Ring({ goal, delay }: { goal: string; delay: number }) {
  const r = 62;
  const at = (deg: number, radius = r) => {
    const a = (deg * Math.PI) / 180;
    return [100 + radius * Math.cos(a), 100 + radius * Math.sin(a)];
  };
  return (
    <svg className="ring" viewBox="0 0 200 200" aria-hidden="true">
      <circle className="ring-track" cx="100" cy="100" r={r} />
      <g className="ring-orbit" style={{ animationDelay: `${delay}s` }}>
        <circle className="ring-dot" cx="100" cy={100 - r} r="5" />
      </g>
      {STATIONS.map((s) => {
        const [x, y] = at(s.angle);
        const [lx, ly] = at(s.angle, r + 22);
        return (
          <g key={s.label} className={`station station-${s.tone}`}>
            <circle cx={x} cy={y} r="8" />
            <text x={lx} y={ly} textAnchor="middle" dominantBaseline="central">
              {s.label}
            </text>
          </g>
        );
      })}
      <text className="ring-goal" x="100" y="100" textAnchor="middle" dominantBaseline="central">
        {goal}
      </text>
    </svg>
  );
}

export function Idea({ n }: { n: number }) {
  return (
    <Section
      id="idea"
      n={n}
      kicker="What it is"
      title="Small agentic loops keep one service healthy."
      lede="A decision model, Jev, reads what the box shows and picks the next step. A cheap coding model writes the commands once; code runs them after that."
    >
      <div className="loops">
        {LOOPS.map((l, i) => (
          <figure key={l.title} className="loop-card">
            <Ring goal={l.goal} delay={i * -2} />
            <figcaption>
              <h3>{l.title}</h3>
              <p>
                {l.when} <span className="arrow">→</span> <code>{l.action}</code>
              </p>
            </figcaption>
          </figure>
        ))}
      </div>
    </Section>
  );
}
