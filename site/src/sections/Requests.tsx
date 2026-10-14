import content from "virtual:content";
import { Code } from "../components/Code";
import { RequestTerminal } from "../components/RequestTerminal";
import { Section } from "../components/Section";

function Plan({ text }: { text: string }) {
  const [command, ...rest] = text.split("\n");
  return (
    <pre className="plan" tabIndex={0} aria-label={command.slice(2)}>
      <code>
        <span className="prompt">$ </span>
        <span className="plan-cmd">{command.slice(2)}</span>
        {"\n"}
        {rest.map((line, i) => {
          const m = /^(error|ask|warning|info)(\s+)(\S+)(\s+)(.*?)(\s{2,}\d\.\d\d)?$/.exec(line);
          if (!m) return <span key={i} className={line.includes("error") ? "plan-end" : ""}>{line + "\n"}</span>;
          return (
            <span key={i} className="plan-line">
              <span className={`lvl lvl-${m[1]}`}>{m[1]}</span>
              {m[2]}
              <span className="plan-subject">{m[3]}</span>
              {m[4]}
              {m[5]}
              {m[6] && <span className="plan-p">{m[6]}</span>}
              {"\n"}
            </span>
          );
        })}
      </code>
    </pre>
  );
}

const RULES: [string, string, string][] = [
  ["MUST", "error", "unsaid: nothing runs"],
  ["SHOULD", "warning", "unsaid: the default, and run asks to go on"],
  ["MAY", "info", "unsaid: the default"],
  ["NEVER", "error", "asked for: refused"],
  ["ASK", "ask", "asked for: the person answers first"],
];

export function Requests({ n }: { n: number }) {
  return (
    <Section
      id="requests"
      n={n}
      kicker="The request check"
      title="Every request is checked before anything runs."
      lede="Two decider calls find each value and judge whether it fits. Code applies the levels and the rules."
    >
      <div className="check-grid">
        <ul className="levels" aria-label="What each level and rule does">
          {RULES.map(([k, level, text]) => (
            <li key={k}>
              <code className="kw">{k}</code>
              <span className={`lvl lvl-${level}`}>{level}</span>
              <span>{text}</span>
            </li>
          ))}
        </ul>
        <div className="check-demo">
          <RequestTerminal />
          <div className="check-why">
            <Code name="slot" label="The history ARG and the ASK IF rule of the ethereum weave" className="weave" />
            <p>
              The decider fills <code>blocks = 1000</code>. Code compares it with 10064: numbers are compared in code, not by the model.
            </p>
          </div>
        </div>
      </div>
      <figure className="plan-figure">
        <figcaption>A value said wrong is an error, whatever its level.</figcaption>
        <Plan text={content.plans[0]} />
      </figure>
    </Section>
  );
}
