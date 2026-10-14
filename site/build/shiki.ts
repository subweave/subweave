// Colours the page's code at build time: Weavefiles with the repo's TextMate grammar
// (editors/weavefile.tmLanguage.json), shell and JSON with Shiki's own grammars.
// The theme names roles; build output is classes (styles.css colours them for
// light and dark). Text for a model keeps its own colour, apart from code.
import { readFileSync } from "node:fs";
import { createHighlighterCore, type LanguageRegistration, type ThemeRegistration } from "shiki/core";
import { createOnigurumaEngine } from "shiki/engine/oniguruma";
import json from "shiki/langs/json.mjs";
import shellscript from "shiki/langs/shellscript.mjs";

export type Lang = "weavefile" | "shellscript" | "json";

const role = (name: string) => `var(--w-${name})`;

const THEME: ThemeRegistration = {
  name: "subweave",
  type: "dark",
  colors: { "editor.foreground": role("code"), "editor.background": "transparent" },
  tokenColors: [
    { scope: ["comment"], settings: { foreground: role("comment") } },
    // Weavefile
    { scope: ["keyword.control.instruction.weavefile"], settings: { foreground: role("keyword"), fontStyle: "bold" } },
    { scope: ["keyword.other.weavefile", "storage.modifier.level.weavefile"], settings: { foreground: role("keyword") } },
    { scope: ["entity.name"], settings: { foreground: role("name"), fontStyle: "bold" } },
    {
      scope: ["string.quoted.double.model.weavefile", "string.unquoted.model.weavefile"],
      settings: { foreground: role("model") },
    },
    { scope: ["markup.inline.raw.weavefile"], settings: { foreground: role("model"), fontStyle: "bold" } },
    { scope: ["variable.other.weavefile", "variable.parameter.slot.weavefile"], settings: { foreground: role("name") } },
    { scope: ["constant.numeric", "meta.embedded"], settings: { foreground: role("code") } },
    // Shell: a quoted request goes to the decider, so it is model text too.
    { scope: ["entity.name.command.shell"], settings: { foreground: role("name"), fontStyle: "bold" } },
    { scope: ["keyword.control.shell", "keyword.control.do.shell"], settings: { foreground: role("keyword") } },
    { scope: ["string.quoted.double.shell", "string.quoted.single.shell"], settings: { foreground: role("model") } },
    { scope: ["variable.other.normal.shell"], settings: { foreground: role("name") } },
    { scope: ["constant.other.option", "keyword.operator.pipe.shell", "keyword.operator.redirect.shell"], settings: { foreground: role("comment") } },
    // JSON: the decider's request
    { scope: ["support.type.property-name.json"], settings: { foreground: role("name") } },
    { scope: ["string.quoted.double.json"], settings: { foreground: role("model") } },
    { scope: ["constant.numeric.json"], settings: { foreground: role("keyword") } },
  ],
};

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

// Returns, per key, the inner HTML of a <code>: one <span class="line"> per line.
export async function highlight(
  grammarPath: string,
  items: Record<string, { lang: Lang; text: string }>,
): Promise<Record<string, string>> {
  const grammar = JSON.parse(readFileSync(grammarPath, "utf8")) as LanguageRegistration;
  const highlighter = await createHighlighterCore({
    themes: [THEME],
    langs: [shellscript, json, { ...grammar, name: "weavefile" }],
    engine: createOnigurumaEngine(import("shiki/wasm")),
  });
  const out: Record<string, string> = {};
  for (const [key, { lang, text }] of Object.entries(items)) {
    const lines = highlighter.codeToTokensBase(text, { lang, theme: "subweave" });
    out[key] = lines
      .map((tokens) => {
        // Runs of one colour become one span; plain code needs none, the <pre> is code-coloured.
        const runs: { cls: string; text: string }[] = [];
        for (const t of tokens) {
          const name = /--w-([a-z]+)/.exec(t.color ?? "")?.[1] ?? "code";
          const bold = ((t.fontStyle ?? 0) & 2) !== 0;
          const cls = name === "code" && !bold ? "" : `w-${name}${bold ? " w-b" : ""}`;
          const last = runs[runs.length - 1];
          if (last && last.cls === cls) last.text += t.content;
          else runs.push({ cls, text: t.content });
        }
        const html = runs.map((r) => (r.cls ? `<span class="${r.cls}">${esc(r.text)}</span>` : esc(r.text))).join("");
        return `<span class="line">${html}\n</span>`;
      })
      .join("");
  }
  highlighter.dispose();
  return out;
}
