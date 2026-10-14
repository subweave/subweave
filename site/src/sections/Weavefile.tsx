import { useRef, type KeyboardEvent } from "react";
import content from "virtual:content";
import { Code } from "../components/Code";
import { Section } from "../components/Section";
import { LIBRARY } from "../links";

export function Weavefile({ n, selected, onSelect }: { n: number; selected: string; onSelect: (name: string) => void }) {
  const tabs = useRef<Record<string, HTMLButtonElement | null>>({});
  const names = content.examples.map((w) => w.name);
  const weave = content.examples.find((w) => w.name === selected)!;

  const onKey = (e: KeyboardEvent) => {
    const i = names.indexOf(selected);
    const next =
      e.key === "ArrowRight"
        ? names[(i + 1) % names.length]
        : e.key === "ArrowLeft"
          ? names[(i - 1 + names.length) % names.length]
          : e.key === "Home"
            ? names[0]
            : e.key === "End"
              ? names[names.length - 1]
              : null;
    if (!next) return;
    e.preventDefault();
    onSelect(next);
    tabs.current[next]?.focus();
  };

  return (
    <Section
      id="weavefile"
      n={n}
      kicker="The Weavefile"
      title="Each weave is one file. Read them in full."
      lede="Keywords in capitals, as in a Dockerfile. Quoted text goes to a model; everything else is code. Say what, not how: each box learns the how."
    >
      <div className="tabs">
        <div className="tabs-head">
          <div className="tablist" role="tablist" aria-label="Examples" onKeyDown={onKey}>
            {names.map((name) => (
              <button
                key={name}
                ref={(el) => {
                  tabs.current[name] = el;
                }}
                type="button"
                role="tab"
                id={`tab-${name}`}
                aria-selected={name === selected}
                aria-controls="weave-panel"
                tabIndex={name === selected ? 0 : -1}
                onClick={() => onSelect(name)}
              >
                {name}
              </button>
            ))}
          </div>
          <p className="legend">
            <span className="w-keyword w-b">KEYWORD</span>
            <span className="w-model">"to a model"</span>
            <span className="w-code">code</span>
            <span className="w-comment"># comment</span>
          </p>
        </div>
        <div className="tabpanel" role="tabpanel" id="weave-panel" aria-labelledby={`tab-${selected}`}>
          <div className="panel-head">
            <p>
              <span className="panel-name">examples/{selected}/Weavefile</span>
              <span className="panel-about">{weave.about}</span>
            </p>
            <a href={`${LIBRARY}/blob/main/${selected}/Weavefile`}>{weave.lines} lines on GitHub</a>
          </div>
          <Code key={selected} name={selected} label={`The ${selected} Weavefile, in full`} numbered className="weave" />
        </div>
      </div>
    </Section>
  );
}
