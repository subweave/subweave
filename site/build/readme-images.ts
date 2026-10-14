// Writes docs/img/{hero,loop}-{light,dark}.svg for the README, from the
// same files the site reads. Run: npm run readme-images
import { mkdirSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { readContent } from "./content.ts";
import { COMMAND, heroFindings } from "../src/weave/example.ts";
import { goalRows, type Row } from "../src/weave/parse.ts";

const root = fileURLToPath(new URL("../..", import.meta.url));
const out = `${root}docs/img`;
const content = readContent(root);
const W = 880;

type Palette = Record<"bg" | "surface" | "ink" | "ink2" | "muted" | "rule" | "rule2" | "accent" | "accentInk" | "model", string>;

const LIGHT: Palette = {
  bg: "#f6f6f3",
  surface: "#efefea",
  ink: "#15171c",
  ink2: "#353942",
  muted: "#5a5f6b",
  rule: "#dcdcd4",
  rule2: "#c3c3b9",
  accent: "#0a7552",
  accentInk: "#ffffff",
  model: "#6436c4",
};

const DARK: Palette = {
  bg: "#0d0f12",
  surface: "#14171c",
  ink: "#e8e9ec",
  ink2: "#c4c7ce",
  muted: "#979ca7",
  rule: "#252a31",
  rule2: "#3a3f49",
  accent: "#3fd39c",
  accentInk: "#05130d",
  model: "#b9a2ff",
};

const MONO = "'IBM Plex Mono', ui-monospace, SFMono-Regular, 'SF Mono', Menlo, Consolas, 'Liberation Mono', monospace";
const SANS = "'IBM Plex Sans', -apple-system, BlinkMacSystemFont, 'Segoe UI', Helvetica, Arial, sans-serif";

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function svg(p: Palette, height: number, title: string, body: string): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${height}" viewBox="0 0 ${W} ${height}" role="img" aria-label="${esc(title)}">
<title>${esc(title)}</title>
<rect x="0.5" y="0.5" width="${W - 1}" height="${height - 1}" rx="12" fill="${p.bg}" stroke="${p.rule2}"/>
${body}
</svg>
`;
}

// A line of mono text from coloured runs; spaces are kept, so columns line up in any monospace font.
function line(x: number, y: number, size: number, runs: [string, string, string?][]): string {
  const spans = runs.map(([fill, text, weight]) => `<tspan fill="${fill}"${weight ? ` font-weight="${weight}"` : ""}>${esc(text)}</tspan>`);
  return `<text x="${x}" y="${y}" font-family="${MONO}" font-size="${size}" xml:space="preserve">${spans.join("")}</text>`;
}

function bar(p: Palette, label: string): string {
  return `<text x="24" y="23" font-family="${MONO}" font-size="12" fill="${p.muted}">${esc(label)}</text>
<line x1="1" x2="${W - 1}" y1="36.5" y2="36.5" stroke="${p.rule}"/>`;
}

// The goal tree, drawn as `subweave plan` draws it, every goal held.
function tree(p: Palette, rows: Row[], x0: number, y0: number, h: number): string {
  const step = 18;
  const r = 5;
  const parts: string[] = [];
  rows.forEach((row, i) => {
    const top = y0 + i * h;
    const mid = top + h / 2;
    const col = (d: number) => x0 + 7 + d * step + 0.5;
    row.rails.forEach((on, c) => {
      if (c > 0 && on) parts.push(`<line x1="${col(c - 1)}" x2="${col(c - 1)}" y1="${top}" y2="${top + h}" stroke="${p.rule2}"/>`);
    });
    const cx = col(row.depth);
    if (row.depth > 0) {
      const px = col(row.depth - 1);
      parts.push(`<path d="M${px} ${top} V${row.last ? mid : top + h} M${px} ${mid} H${cx - r}" fill="none" stroke="${p.rule2}"/>`);
    }
    if (row.kids) parts.push(`<line x1="${cx}" x2="${cx}" y1="${mid + r}" y2="${top + h}" stroke="${p.rule2}"/>`);
    parts.push(`<circle cx="${cx}" cy="${mid}" r="${r}" fill="${p.accent}" stroke="${p.accent}" stroke-width="1.5"/>`);
    parts.push(line(cx + r + 8, mid + 4.5, 13, [[p.ink, row.goal.name]]));
  });
  return parts.join("\n");
}

function hero(p: Palette): string {
  const eth = content.examples.find((w) => w.name === "ethereum")!;
  const { findings, ask } = heroFindings(eth);
  const rows = goalRows(eth.goals);
  const height = 290 + rows.length * 24 + 20;
  const parts = [bar(p, "terminal")];
  parts.push(line(24, 68, 13, [[p.muted, "$ "], [p.ink, COMMAND]]));
  findings.forEach(([level, subject, text], i) => {
    parts.push(
      line(24, 102 + i * 21, 12, [
        [level === "ask" ? p.accent : p.muted, level.padEnd(9), level === "ask" ? "600" : undefined],
        [p.ink, subject.padEnd(9)],
        [p.ink2, text],
      ]),
    );
  });
  parts.push(line(24, 224, 12, [[p.ink2, "1 question: run asks the person first."]]));
  parts.push(`<line x1="1" x2="${W - 1}" y1="246.5" y2="246.5" stroke="${p.rule}"/>`);
  parts.push(`<line x1="440.5" x2="440.5" y1="247" y2="${height - 1}" stroke="${p.rule}"/>`);
  parts.push(line(24, 278, 12, [[p.accent, `asked #1 on steery: ASK IF ${ask}`]]));
  parts.push(line(24, 300, 12, [[p.ink, "#1 yes, signed on the person's device"]]));
  parts.push(line(24, 332, 12, [[p.muted, "the daemon takes the service, and"]]));
  parts.push(line(24, 352, 12, [[p.muted, "the goals turn to held one by one"]]));
  parts.push(line(464, 278, 12, [[p.muted, "goals"]]));
  parts.push(tree(p, rows, 464, 290, 24));
  rows.forEach((_, i) => {
    parts.push(
      `<text x="${W - 24}" y="${290 + i * 24 + 16}" text-anchor="end" font-family="${MONO}" font-size="10.5" letter-spacing="0.6" fill="${p.accent}">HELD</text>`,
    );
  });
  return svg(p, height, `${COMMAND}: the request check, an ask answered on Steery, then every goal of the ethereum weave held`, parts.join("\n"));
}

function box(p: Palette, x: number, y: number, w: number, h: number, tone: "code" | "model" | "person", lines: [string, boolean?][]): string {
  const stroke = tone === "model" ? p.model : tone === "person" ? p.accent : p.rule2;
  const fill = tone === "model" ? p.model : tone === "person" ? p.accent : p.bg;
  const text = tone === "model" ? p.model : tone === "person" ? p.accent : p.ink;
  const rx = tone === "person" ? h / 2 : 5;
  const tinted = tone === "code" ? "" : ` fill-opacity="0.09"`;
  const mid = y + h / 2;
  const spans = lines
    .map(([t, strong], i) => {
      const dy = mid + (i - (lines.length - 1) / 2) * 16 + 4.5;
      return `<text x="${x + w / 2}" y="${dy}" text-anchor="middle" font-family="${SANS}" font-size="${strong ? 14 : 12}"${strong ? ' font-weight="600"' : ""} fill="${strong ? text : tone === "code" ? p.ink2 : text}">${esc(t)}</text>`;
    })
    .join("");
  return `<rect x="${x + 0.5}" y="${y + 0.5}" width="${w}" height="${h}" rx="${rx}" fill="${p.bg}" stroke="${stroke}" stroke-width="1.25"/>
<rect x="${x + 0.5}" y="${y + 0.5}" width="${w}" height="${h}" rx="${rx}" fill="${fill}"${tinted} stroke="none"${tone === "code" ? ' fill-opacity="0"' : ""}/>
${spans}`;
}

function arrow(p: Palette, d: string, dashed = false): string {
  return `<path d="${d}" fill="none" stroke="${p.muted}" stroke-width="1.25"${dashed ? ' stroke-dasharray="4 3"' : ""} marker-end="url(#a)"/>`;
}

function loop(p: Palette): string {
  const y = 118;
  const h = 52;
  const w = 136;
  const gap = 38;
  const x = (i: number) => 24 + i * (w + gap);
  const cx = (i: number) => x(i) + w / 2 + 0.5;
  const parts = [
    `<defs><marker id="a" viewBox="0 0 8 8" refX="7" refY="4" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0 L8 4 L0 8 z" fill="${p.muted}"/></marker></defs>`,
  ];
  const main: ["code" | "model", string, string][] = [
    ["code", "check", "the goals that are due"],
    ["model", "decide", "the decider picks"],
    ["code", "gate", "the rules, in code"],
    ["code", "act", "the steps run"],
    ["code", "hold", "within WITHIN?"],
  ];
  main.forEach(([tone, title, sub], i) => {
    parts.push(box(p, x(i), y, w, h, tone, [[title, true], [sub]]));
    if (i > 0) parts.push(arrow(p, `M${x(i - 1) + w + 1} ${y + h / 2 + 0.5} H${x(i) - 1}`));
  });
  // decide: ask
  parts.push(box(p, x(1) - 6, 30, w + 12, 44, "person", [["under 0.5, or steered:"], ["ask the person"]]));
  parts.push(arrow(p, `M${cx(1)} ${y} V${76}`));
  // gate: ASK above, NEVER below
  parts.push(box(p, x(2), 38, w, 30, "person", [["ASK: ask the person"]]));
  parts.push(arrow(p, `M${cx(2)} ${y} V${70}`));
  parts.push(box(p, x(2), 204, w, 30, "code", [["NEVER: pick again"]]));
  parts.push(arrow(p, `M${cx(2)} ${y + h + 1} V${202}`));
  // act
  parts.push(
    `<text x="${cx(3)}" y="${y + h + 26}" text-anchor="middle" font-family="${SANS}" font-size="11.5" fill="${p.muted}">learned steps first,</text>`,
    `<text x="${cx(3)}" y="${y + h + 42}" text-anchor="middle" font-family="${SANS}" font-size="11.5" fill="${p.muted}">else RUN, DO, ADD</text>`,
  );
  // hold: yes above, no below
  parts.push(box(p, x(4), 38, w, 30, "code", [["yes: learn, tell"]]));
  parts.push(arrow(p, `M${cx(4)} ${y} V${70}`));
  parts.push(box(p, x(4), 204, w, 30, "model", [["no: pick again"]]));
  parts.push(arrow(p, `M${cx(4)} ${y + h + 1} V${202}`));
  parts.push(box(p, x(4), 244, w, 30, "person", [["TRIES used: ask"]]));
  // the next tick
  parts.push(arrow(p, `M${cx(4)} 38 V16 H${cx(0)} V${y - 1}`, true));
  parts.push(`<text x="${(cx(0) + cx(1)) / 2}" y="12" text-anchor="middle" font-family="${MONO}" font-size="10.5" fill="${p.muted}">next tick</text>`);
  // legend
  const legend: [string, string, number][] = [
    ["code", p.rule2, 0],
    ["model", p.model, 1],
    ["the person", p.accent, 2],
  ];
  legend.forEach(([label, color, i]) => {
    const lx = 24 + i * 96;
    parts.push(
      `<rect x="${lx + 0.5}" y="250.5" width="14" height="10" rx="${i === 2 ? 5 : 3}" fill="${i === 0 ? p.bg : color}" fill-opacity="${i === 0 ? 1 : 0.12}" stroke="${color}" stroke-width="1.25"/>`,
      `<text x="${lx + 22}" y="260" font-family="${SANS}" font-size="12" fill="${p.muted}">${label}</text>`,
    );
  });
  return svg(
    p,
    290,
    "The Subweave loop: check the goals, the decider picks an action, the rules gate it, the steps run, the goal must hold within its time",
    parts.join("\n"),
  );
}

mkdirSync(out, { recursive: true });
const images: Record<string, (p: Palette) => string> = { hero, loop };
for (const [name, draw] of Object.entries(images)) {
  writeFileSync(`${out}/${name}-light.svg`, draw(LIGHT));
  writeFileSync(`${out}/${name}-dark.svg`, draw(DARK));
}
console.log(`wrote ${Object.keys(images).length * 2} files to docs/img`);
