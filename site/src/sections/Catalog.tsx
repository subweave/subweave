import content from "virtual:content";
import { Code } from "../components/Code";
import { Section } from "../components/Section";
import { REPO } from "../links";

export function Catalog({ n, onRead }: { n: number; onRead: (name: string) => void }) {
  return (
    <Section id="examples" n={n} kicker="Examples" title="Six examples to start from.">
      <div className="catalog">
        {content.examples.map((w) => (
          <article key={w.name} className="weave-card">
            <header>
              <h3>{w.name}</h3>
              <p className="about">{w.about}</p>
              <p className="for">For {w.forWho.replace(/^./, (c) => c.toLowerCase())}.</p>
            </header>
            <Code name={`example:${w.name}`} label={`Example request for ${w.name}`} className="shell" />
            <footer>
              <a href="#weavefile" onClick={() => onRead(w.name)}>
                Read the Weavefile
              </a>
              <a href={`${REPO}/tree/main/examples/${w.name}`}>README</a>
            </footer>
          </article>
        ))}
      </div>
    </Section>
  );
}
