import { Code } from "../components/Code";
import { Command, Section } from "../components/Section";
import { REPO } from "../links";

export function Install({ n }: { n: number }) {
  return (
    <Section id="install" n={n} kicker="Install" title="One binary: the CLI and the daemon.">
      <div className="install">
        <div className="install-main">
          <Command name="install" label="Install Subweave on Linux" />
          <Code name="sh:first" label="First commands" className="shell" />
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
