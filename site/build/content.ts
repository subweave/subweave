// `virtual:content`: what the page shows from the repo's files, read at build time,
// so the page says what the examples and the spec say. All code is coloured here
// (build/shiki.ts), so the browser gets HTML and no highlighter.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import type { Plugin } from "vite";
import { highlight, type Lang } from "./shiki.ts";
import { DOCKERFILE, SHELL } from "./snippets.ts";
import { parseWeave, type Weave } from "../src/weave/parse.ts";

export const EXAMPLES = ["ethereum", "postgres", "valkey", "minecraft", "vllm", "store-edge"] as const;

export interface WeaveCard extends Weave {
  forWho: string;
  example: string;
  lines: number;
}

export interface Finding {
  line: number;
  level: "error" | "warning" | "note";
  rule: string;
  message: string;
}

// `subweave review` output, as printed: the header, the area table, the findings.
export interface Review {
  command: string;
  header: string;
  table: string[];
  areas: { area: string; score: number }[];
  findings: Finding[];
  name: string;
  score: number;
  checks: number;
  cost: string;
}

export interface Content {
  examples: WeaveCard[];
  // The hero's short Weavefile (site/hero.Weavefile), parsed.
  hero: Weave;
  plans: string[];
  deciderRequest: string;
  // site/review: a first-draft postgres weave, its review, and the shipped weave's review.
  review: { draft: Review; shipped: Review; excerpt: { n: number; key: string }[] };
  // Code to colour, by key: each weave by name, `hero`, quoted excerpts, shell, JSON and a Dockerfile.
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

// Parses `subweave review` text: "name  54/100   44 rules, 114 checks, $0.0008", the area
// table up to the first blank line, then one finding per "Weavefile:N" line.
function parseReview(command: string, text: string): Review {
  const lines = text.replace(/\n$/, "").split("\n");
  const head = /^(\S+)\s+(\d+)\/100\s+\d+ rules, (\d+) checks, (\$[\d.]+)$/.exec(lines[0]);
  if (!head) throw new Error(`review: bad header ${lines[0]}`);
  const blank = lines.indexOf("", 2);
  const table = lines.slice(2, blank === -1 ? undefined : blank);
  const areas = table.slice(1).map((l) => {
    const [area, score] = l.trim().split(/\s+/);
    return { area, score: Number(score) };
  });
  const findings: Finding[] = [];
  for (const l of lines) {
    const m = /^Weavefile:(\d+)\s+(error|warning|note)\s+(\S+)\s{2,}.*?\s{2,}(.+?)(?:\s{2,}[\d.]+)?$/.exec(l);
    if (m) findings.push({ line: Number(m[1]), level: m[2] as Finding["level"], rule: m[3], message: m[4] });
  }
  return { command, header: lines[0], table, areas, findings, name: head[1], score: Number(head[2]), checks: Number(head[3]), cost: head[4] };
}

// The draft lines the page shows, by number; the ACTIONs where the review finds the most.
const EXCERPT = [37, 47] as const;

export function readContent(root: string, files: string[] = []): Content {
  const read = (path: string) => {
    files.push(resolve(root, path));
    return readFileSync(resolve(root, path), "utf8");
  };
  const sources: Record<string, { lang: Lang; text: string }> = {};
  const weavefile = (key: string, text: string) => (sources[key] = { lang: "weavefile", text: text.replace(/\n$/, "") });
  const examples = EXAMPLES.map((name): WeaveCard => {
    const readme = read(`examples/${name}/README.md`);
    const text = read(`examples/${name}/Weavefile`);
    weavefile(name, text);
    const forLine = one(readme.split("\n").filter((l) => l.startsWith("**For:**")), `For line in ${name}`);
    return {
      ...parseWeave(name, text),
      forWho: forLine.replace("**For:**", "").split(". Example:")[0].trim().replace(/\.$/, ""),
      example: one(blocks(readme).filter((b) => b.info === ""), `example in ${name}`).body,
      lines: text.replace(/\n$/, "").split("\n").length,
    };
  });
  for (const w of examples) sources[`example:${w.name}`] = { lang: "shellscript", text: w.example };
  for (const [key, text] of Object.entries(SHELL)) sources[`sh:${key}`] = { lang: "shellscript", text };
  sources.dockerfile = { lang: "dockerfile", text: DOCKERFILE };

  const heroText = read("site/hero.Weavefile");
  weavefile("hero", heroText);
  const hero = parseWeave("hero", heroText);
  for (const g of hero.goals)
    for (const a of g.after)
      if (!hero.goals.some((h) => h.name === a)) throw new Error(`hero.Weavefile: ${g.name} AFTER ${a}, no such goal`);

  // The ethereum ARG with a slot, and the code rule that reads the slot.
  const eth = examples.find((w) => w.name === "ethereum")!;
  const history = one(eth.args.filter((a) => a.name === "history"), "ethereum history ARG");
  const slot = one(history.options.filter((o) => o.label.includes("<blocks>")), "history slot");
  const askIf = one(eth.rules.filter((r) => r.startsWith("ASK IF") && r.includes("blocks")), "ASK IF blocks");
  weavefile("slot", `ARG ${history.name}=${history.default}  ${history.level}\n    ${slot.label}  "${slot.meaning}"\n\n${askIf}`);

  // A quoted FACT and a DO step of the postgres weave, for "Learned commands".
  const pg = sources.postgres.text.split("\n");
  weavefile("fact", one(pg.filter((l) => l.startsWith("FACT disk ")), "FACT disk").replace(/^FACT disk\s+/, "FACT disk "));
  const action = pg.findIndex((l) => l.startsWith("ACTION free-disk"));
  weavefile("do", one(pg.slice(action).filter((l) => /^\s+DO\s/.test(l)), "free-disk DO").trim().replace(/^DO\s+/, "DO "));

  // The postgres weave's update rule, for "What about costs?": the version ARG, the goal, the action.
  const line = (prefix: string) => one(pg.filter((l) => l.startsWith(prefix)), prefix);
  const patch = pg.findIndex((l) => l.startsWith("ACTION patch "));
  weavefile("intent", [line("ARG version="), "", line("GOAL patched "), "", ...pg.slice(patch, patch + 3)].join("\n"));

  const spec = blocks(read("WEAVEFILE.md"));
  const journal = one(spec.filter((b) => b.info === "json" && b.body.includes('"kind":"decide"')), "journal");
  const plans = spec.filter((b) => b.body.startsWith("$ subweave plan postgres")).map((b) => b.body);
  if (plans.length !== 2) throw new Error(`site content: want 2 postgres plans, found ${plans.length}`);
  const deciderRequest = one(spec.filter((b) => b.info === "json" && b.body.includes('"questions"')), "decider request").body;
  sources.request = { lang: "json", text: deciderRequest };
  sources.journal = { lang: "json", text: one(journal.body.split("\n").filter((l) => l.includes('"kind":"decide"')), "decide line") };

  const draftText = read("site/review/draft/Weavefile").split("\n");
  const excerpt: { n: number; key: string }[] = [];
  for (let n = EXCERPT[0]; n <= EXCERPT[1]; n++) {
    const key = `draft:${n}`;
    sources[key] = { lang: "weavefile", text: draftText[n - 1] };
    excerpt.push({ n, key });
  }
  const review = {
    draft: parseReview("subweave review ./draft", read("site/review/review-draft.txt")),
    shipped: parseReview("subweave review examples/postgres", read("site/review/review-postgres.txt")),
    excerpt,
  };
  if (review.draft.areas.length !== 9) throw new Error(`review: want 9 areas, found ${review.draft.areas.length}`);
  return { examples, hero, plans, deciderRequest, review, sources };
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
