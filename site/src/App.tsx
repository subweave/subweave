import { useState } from "react";
import { Hero } from "./components/Hero";
import { REPO } from "./links";
import { Architecture } from "./sections/Architecture";
import { Catalog } from "./sections/Catalog";
import { Channels } from "./sections/Channels";
import { Costs } from "./sections/Costs";
import { Graph } from "./sections/Graph";
import { Idea } from "./sections/Idea";
import { Install } from "./sections/Install";
import { Learned } from "./sections/Learned";
import { Loop } from "./sections/Loop";
import { Requests } from "./sections/Requests";
import { Review } from "./sections/Review";
import { Weavefile } from "./sections/Weavefile";

export function App() {
  const [weave, setWeave] = useState("postgres");

  // Sections in page order; each is numbered by its place here.
  const sections = [
    (n: number) => <Idea key="idea" n={n} />,
    (n: number) => <Architecture key="architecture" n={n} />,
    (n: number) => <Graph key="graph" n={n} />,
    (n: number) => <Loop key="loop" n={n} />,
    (n: number) => <Requests key="requests" n={n} />,
    (n: number) => <Weavefile key="weavefile" n={n} selected={weave} onSelect={setWeave} />,
    (n: number) => <Learned key="learned" n={n} />,
    (n: number) => <Channels key="channels" n={n} />,
    (n: number) => <Catalog key="examples" n={n} onRead={setWeave} />,
    (n: number) => <Review key="review" n={n} />,
    (n: number) => <Costs key="costs" n={n} />,
    (n: number) => <Install key="install" n={n} />,
  ];

  return (
    <>
      <a className="skip" href="#main">
        Skip to content
      </a>
      <nav className="nav" aria-label="Site">
        <div className="wrap nav-inner">
          <a className="wordmark" href="#top" aria-label="Subweave, top of page">
            <svg width="22" height="22" viewBox="0 0 22 22" aria-hidden="true">
              <path d="M2 6 H20 M2 11 H20 M2 16 H20" className="mark-weft" />
              <path d="M6 3 V19 M11 3 V19 M16 3 V19" className="mark-warp" />
            </svg>
            subweave
          </a>
          <ul>
            <li>
              <a href="#architecture">How it works</a>
            </li>
            <li>
              <a href="#weavefile">Weavefile</a>
            </li>
            <li>
              <a href="#examples">Examples</a>
            </li>
            <li>
              <a href={REPO}>GitHub</a>
            </li>
          </ul>
        </div>
      </nav>
      <Hero />
      <main id="main">{sections.map((render, i) => render(i + 1))}</main>
      <footer className="footer">
        <div className="wrap footer-inner">
          <p>Subweave. MIT license.</p>
          <ul>
            <li>
              <a href={REPO}>GitHub</a>
            </li>
            <li>
              <a href={`${REPO}/blob/main/WEAVEFILE.md`}>WEAVEFILE.md</a>
            </li>
            <li>
              <a href={`${REPO}/blob/main/LIMITS.md`}>LIMITS.md</a>
            </li>
          </ul>
        </div>
      </footer>
    </>
  );
}
