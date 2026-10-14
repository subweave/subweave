// `virtual:content`: what the page shows from the repo's files, read at build time,
// so the page says what the weaves and the spec say. All code is coloured here
// (build/shiki.ts), so the browser gets HTML and no highlighter.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Plugin } from "vite";
import { highlight, type Lang } from "./shiki.ts";
import { SHELL } from "./snippets.ts";
import { parseWeave, type Weave } from "../src/weave/parse.ts";

export const WEAVES = ["ethereum", "postgres", "valkey", "minecraft", "vllm", "store-edge"] as const;

export interface WeaveCard extends Weave {
  forWho: string;
  example: string;
  lines: number;
}

export interface Content {
  weaves: WeaveCard[];
  // The hero's short Weavefile (site/hero.Weavefile), parsed.
  hero: Weave;
  plans: string[];
  deciderRequest: string;
  // Code to colour, by key: each weave by name, `hero`, quoted excerpts, shell and JSON.
  sources: Record<string, { lang: Lang; text: string }>;
}

// What the page imports: the sources replaced by their coloured HTML.
export type SiteContent = Omit<Content, "sources"> & { code: Record<string, string> };

function blocks(markdown: string): { info: string; body: string }[] {
  const out: { info: string; body: string }[] = [];
  const re = /^(`{3,})([^\n`]*)\n([\s\S]*?)\n\1\s*$/gm;
  for (let m = re.exec(markdown); m; m = re.exec(markdown)) out.push({ info: m[2].trim(), body: m[3] });
  return out;
}

function one<T>(list: T[], what: string): T {
  if (list.length === 0) throw new Error(`site content: no ${what}`);
  return list[0];
}

export function readContent(root: string, files: string[] = []): Content {
  const read = (path: string) => {
    files.push(resolve(root, path));
    return readFileSync(resolve(root, path), "utf8");
  };
  const sources: Record<string, { lang: Lang; text: string }> = {};
  const weavefile = (key: string, text: string) => (sources[key] = { lang: "weavefile", text: text.replace(/\n$/, "") });
  const weaves = WEAVES.map((name): WeaveCard => {
    const readme = read(`weaves/${name}/README.md`);
    const text = read(`weaves/${name}/Weavefile`);
    weavefile(name, text);
    const forLine = one(readme.split("\n").filter((l) => l.startsWith("**For:**")), `For line in ${name}`);
    return {
      ...parseWeave(name, text),
      forWho: forLine.replace("**For:**", "").split(". Example:")[0].trim().replace(/\.$/, ""),
      example: one(blocks(readme).filter((b) => b.info === ""), `example in ${name}`).body,
      lines: text.replace(/\n$/, "").split("\n").length,
    };
  });
  for (const w of weaves) sources[`example:${w.name}`] = { lang: "shellscript", text: w.example };
  for (const [key, text] of Object.entries(SHELL)) sources[`sh:${key}`] = { lang: "shellscript", text };

  const heroText = read("site/hero.Weavefile");
  weavefile("hero", heroText);
  const hero = parseWeave("hero", heroText);
  for (const g of hero.goals)
    for (const a of g.after)
      if (!hero.goals.some((h) => h.name === a)) throw new Error(`hero.Weavefile: ${g.name} AFTER ${a}, no such goal`);

  // The ethereum ARG with a slot, and the code rule that reads the slot.
  const eth = weaves.find((w) => w.name === "ethereum")!;
  const history = one(eth.args.filter((a) => a.name === "history"), "ethereum history ARG");
  const slot = one(history.options.filter((o) => o.label.includes("<blocks>")), "history slot");
  const askIf = one(eth.rules.filter((r) => r.startsWith("ASK IF") && r.includes("blocks")), "ASK IF blocks");
  weavefile("slot", `ARG ${history.name}=${history.default}  ${history.level}\n    ${slot.label}  "${slot.meaning}"\n\n${askIf}`);

  // A quoted FACT and a DO step of the postgres weave, for "Learned commands".
  const pg = sources.postgres.text.split("\n");
  weavefile("fact", one(pg.filter((l) => l.startsWith("FACT disk ")), "FACT disk").replace(/^FACT disk\s+/, "FACT disk "));
  const action = pg.findIndex((l) => l.startsWith("ACTION free-disk"));
  weavefile("do", one(pg.slice(action).filter((l) => /^\s+DO\s/.test(l)), "free-disk DO").trim().replace(/^DO\s+/, "DO "));

  const spec = blocks(read("WEAVEFILE.md"));
  const journal = one(spec.filter((b) => b.info === "json" && b.body.includes('"kind":"decide"')), "journal");
  const plans = spec.filter((b) => b.body.startsWith("$ subweave plan postgres")).map((b) => b.body);
  if (plans.length !== 2) throw new Error(`site content: want 2 postgres plans, found ${plans.length}`);
  const deciderRequest = one(spec.filter((b) => b.info === "json" && b.body.includes('"questions"')), "decider request").body;
  sources.request = { lang: "json", text: deciderRequest };
  sources.journal = { lang: "json", text: one(journal.body.split("\n").filter((l) => l.includes('"kind":"decide"')), "decide line") };
  return { weaves, hero, plans, deciderRequest, sources };
}

export function content(root: string): Plugin {
  const id = "virtual:content";
  return {
    name: "subweave-content",
    resolveId: (source) => (source === id ? `\0${id}` : undefined),
    async load(loaded) {
      if (loaded !== `\0${id}`) return;
      const files: string[] = [];
      const { sources, ...data } = readContent(root, files);
      const grammar = resolve(root, "editors/weavefile.tmLanguage.json");
      files.push(grammar);
      const code = await highlight(grammar, sources);
      for (const f of files) this.addWatchFile(f);
      return `export default ${JSON.stringify({ ...data, code })};`;
    },
  };
}
