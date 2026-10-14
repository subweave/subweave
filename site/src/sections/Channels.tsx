import { Code } from "../components/Code";
import { Section } from "../components/Section";

const CHANNELS: [string, string][] = [
  ["steery", "asks as cards; answers signed"],
  ["telegram", "every message; answers with buttons"],
  ["slack", "every message"],
  ["email", "every message"],
  ["webhook", "every message, as JSON"],
  ["heartbeat", "a ping each tick; silence alerts"],
];

export function Channels({ n }: { n: number }) {
  return (
    <Section
      id="channels"
      n={n}
      kicker="Channels"
      title="When it should not decide alone, it asks you."
      lede="The first answer settles an ask. The box needs no open port."
    >
      <ul className="channels">
        {CHANNELS.map(([kind, text]) => (
          <li key={kind} className="channel">
            <code>{kind}</code>
            <span>{text}</span>
          </li>
        ))}
      </ul>
      <Code name="sh:channels" label="Adding channels" className="shell" />
    </Section>
  );
}
