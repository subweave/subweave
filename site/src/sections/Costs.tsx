import type { ReactNode } from "react";
import { Code } from "../components/Code";
import { Section } from "../components/Section";

const CARDS: { figure: string; note: string; title: string; body: string }[] = [
  {
    figure: "$0.0011",
    note: "a test service installed itself, was killed and came back",
    title: "Built for cheap models.",
    body: "A small decision model and a cheap coding model do the thinking; code does the rest. Any OpenAI- or Anthropic-compatible model works, on your own GPU too.",
  },
  {
    figure: "24/7",
    note: "watch, repair, back up, patch, drill restores",
    title: "Saves a full-time DevOps engineer.",
    body: "Each goal is checked on its schedule, day and night. Subweave repairs, backs up, patches and drills restores, and asks you only where the weave says so.",
  },
];

// What a 2024 Dockerfile pinned, and what changed since; checked on Docker Hub and GitHub.
const STALE: ReactNode[] = [
  <>
    <code>16.4</code> is 11 minor releases behind <code>16.15</code>.
  </>,
  <>
    Bump wal-g to <code>v3.0.9</code> and the download is a 404: the file was renamed.
  </>,
];

const LASTS: ReactNode[] = [
  "It names no release: the box finds the pending one.",
  "Each box learns the commands, and learns them again when they stop working.",
];

export function Costs({ n }: { n: number }) {
  return (
    <Section
      id="costs"
      n={n}
      kicker="Costs"
      title="What about costs?"
      lede="Subweave is built for cheap models. It does the routine work of a full-time DevOps engineer, and a weave rarely needs an update."
    >
      <ul className="cost-grid">
        {CARDS.map((c) => (
          <li key={c.title} className="cost-card">
            <p className="cost-figure">{c.figure}</p>
            <p className="cost-note">{c.note}</p>
            <h3>{c.title}</h3>
            <p>{c.body}</p>
          </li>
        ))}
      </ul>
      <h3 className="stale-heading">Commands go stale. Intent does not.</h3>
      <div className="stale">
        <figure className="stale-side is-pinned">
          <figcaption>
            <span>Dockerfile, 2024</span>
            <span className="stale-tag">pinned commands</span>
          </figcaption>
          <Code name="dockerfile" label="A Dockerfile from 2024 that pins versions and URLs" />
          <ul>
            {STALE.map((s, i) => (
              <li key={i}>{s}</li>
            ))}
          </ul>
        </figure>
        <figure className="stale-side is-intent">
          <figcaption>
            <span>Weavefile</span>
            <span className="stale-tag">intent</span>
          </figcaption>
          <Code name="intent" label="The postgres weave's update rule" />
          <ul>
            {LASTS.map((s, i) => (
              <li key={i}>{s}</li>
            ))}
          </ul>
        </figure>
      </div>
    </Section>
  );
}
