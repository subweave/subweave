import { Code } from "../components/Code";
import { Section } from "../components/Section";

export function Learned({ n }: { n: number }) {
  return (
    <Section
      id="learned"
      n={n}
      kicker="Learned commands"
      title="The weave says what. Each box learns how, once."
      lede="The coder runs on the first install and on new trouble. After that, code runs what it learned, offline too."
    >
      <div className="learned">
        <div className="learned-pair">
          <Code name="fact" label="A quoted FACT of the postgres weave" className="weave" />
          <p className="learned-arrow" aria-hidden="true">
            ↓
          </p>
          <p className="learned-what">One read-only command. The decider checks that it changes nothing.</p>
        </div>
        <div className="learned-pair">
          <Code name="do" label="A DO step of the postgres weave" className="weave" />
          <p className="learned-arrow" aria-hidden="true">
            ↓
          </p>
          <p className="learned-what">The commands that made the goal hold, in order. Next time they run first, as code.</p>
        </div>
      </div>
      <Code name="sh:learned" label="Commands for learned steps" className="shell" />
    </Section>
  );
}
