import { Code } from "../components/Code";
import { Section } from "../components/Section";

export function Models({ n }: { n: number }) {
  return (
    <Section
      id="models"
      n={n}
      kicker="Models and cost"
      title="About $0.00004 a decision."
      lede="The run picks both models; a Weavefile only hints. One OpenRouter key serves both."
    >
      <div className="roles">
        <article className="role role-model">
          <p className="role-name">decide</p>
          <h3>
            <code>typesafe/jev-1.13</code>
          </h3>
          <dl className="figures">
            <div>
              <dt>a decision</dt>
              <dd>about $0.00004</dd>
            </div>
            <div>
              <dt>median latency</dt>
              <dd>0.29 s</dd>
            </div>
          </dl>
        </article>
        <article className="role">
          <p className="role-name">code</p>
          <h3>
            <code>deepseek/deepseek-v4-flash-0731</code>
          </h3>
          <dl className="figures">
            <div>
              <dt>protocol</dt>
              <dd>OpenAI or Anthropic</dd>
            </div>
            <div>
              <dt>runs</dt>
              <dd>first install, new trouble</dd>
            </div>
          </dl>
        </article>
      </div>
      <Code name="sh:models" label="Choosing models and providers" className="shell" />
    </Section>
  );
}
