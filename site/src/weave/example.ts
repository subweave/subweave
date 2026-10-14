// The hero's example: `subweave run ethereum` with a request whose history
// is below reth's minimum, so `ASK IF blocks < 10064` holds. The findings are
// built from the ethereum weave's own ARGs and rules, in `subweave plan` order.
import type { Weave } from "./parse.ts";

export const REQUEST = "mainnet full node holding only 1000 blocks, RPC on the private network";
export const COMMAND = `subweave run ethereum "${REQUEST}"`;
const BLOCKS = "1000";

export type Finding = ["ask" | "info", string, string];

export function heroFindings(eth: Weave): { findings: Finding[]; ask: string } {
  const meaning = (name: string, label: string) => {
    const option = eth.args.find((a) => a.name === name)?.options.find((o) => o.label === label);
    if (!option) throw new Error(`ethereum has no ${name} option ${label}`);
    return option.meaning;
  };
  const rule = eth.rules.find((r) => r.startsWith("ASK IF") && r.includes("blocks"));
  const updates = eth.args.find((a) => a.name === "updates");
  if (!rule || !updates?.default) throw new Error("ethereum: no ASK IF on blocks, or no updates default");
  const ask = rule.slice("ASK IF ".length).replace("blocks", BLOCKS);
  return {
    ask,
    findings: [
      ["ask", "ASK IF", ask],
      ["info", "network", `mainnet: ${meaning("network", "mainnet")}`],
      ["info", "history", `last ${BLOCKS} blocks: ${meaning("history", "last <blocks> blocks")}`],
      ["info", "rpc", `private: ${meaning("rpc", "private")}`],
      ["info", "updates", `unsaid, default ${updates.default}: ${meaning("updates", updates.default)}`],
    ],
  };
}
