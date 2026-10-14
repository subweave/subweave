import { Section } from "../components/Section";

type Station = "check" | "decide" | "act";

// Three of the postgres weave's goals, each kept by the same small loop, each needing
// judgement a script lacks; `says` is what each station does in that loop.
const LOOPS: { title: string; goal: string; when: string; action: string; says: Record<Station, string> }[] = [
  {
    title: "Self-healing",
    goal: "running",
    when: "a restart did not bring it back",
    action: "fix",
    says: { check: "crash loop", decide: "find the cause", act: "fix the config" },
  },
  {
    title: "Self-configuring",
    goal: "configured",
    when: "you ask for private network access",
    action: "configure",
    says: { check: "access changed", decide: "reconfigure", act: "TLS + firewall" },
  },
  {
    title: "Self-upgrading",
    goal: "current",
    when: "you ask for PostgreSQL 18 on a 17 cluster",
    action: "upgrade",
    says: { check: "17, wants 18", decide: "upgrade", act: "migrate data" },
  },
];

const STATIONS: { label: Station; angle: number; tone: string }[] = [
  { label: "check", angle: -90, tone: "code" },
  { label: "decide", angle: 30, tone: "model" },
  { label: "act", angle: 150, tone: "code" },
];

const R = 52;

function Ring({ goal, says, delay }: { goal: string; says: Record<Station, string>; delay: number }) {
  const at = (deg: number) => {
    const a = (deg * Math.PI) / 180;
    return [100 + R * Math.cos(a), 100 + R * Math.sin(a)];
  };
  return (
    <svg className="ring" viewBox="0 0 200 172" aria-hidden="true">
      <circle className="ring-track" cx="100" cy="100" r={R} />
      <g className="ring-orbit" style={{ animationDelay: `${delay}s` }}>
        <circle className="ring-dot" cx="100" cy={100 - R} r="5" />
      </g>
      {STATIONS.map((s) => {
        const [x, y] = at(s.angle);
        // The label and its words sit above the top station and below the others.
        const top = y < 100;
        const ly = top ? y - 36 : y + 21;
        return (
          <g key={s.label} className={`station station-${s.tone}`}>
            <circle cx={x} cy={y} r="8" />
            <text x={x} y={ly} textAnchor="middle" dominantBaseline="central">
              {s.label}
            </text>
            <text className="station-says" x={x} y={ly + 14} textAnchor="middle" dominantBaseline="central">
              {says[s.label]}
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
            <Ring goal={l.goal} says={l.says} delay={i * -2} />
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
