import { Code } from "../components/Code";
import { Command, Section, Step } from "../components/Section";
import { REPO } from "../links";

export function Install({ n }: { n: number }) {
  return (
    <Section id="install" n={n} kicker="Install" title="One binary: the CLI and the daemon.">
      <div className="install">
        <div className="install-main">
          <Step n={1}>Install the daemon</Step>
          <Command name="install" label="Install the Subweave daemon on Linux" />
          <Step n={2}>Run a weave</Step>
          <Code name="sh:first" label="Add a key, plan a request, run it" className="shell" />
        </div>
        <div className="install-links">
          <a className="button" href={REPO}>
            github.com/subweave/subweave
          </a>
          <p>
            Open source, MIT. The format: <a href={`${REPO}/blob/main/WEAVEFILE.md`}>WEAVEFILE.md</a>.
          </p>
        </div>
      </div>
    </Section>
  );
}
