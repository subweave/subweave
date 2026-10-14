import { Marked } from "marked";
import { useMemo, type ReactNode } from "react";
import { Panel } from "../components/ui";
import type { ServiceProps } from "./ServiceView";

const KEYWORDS = new Set(
  "ABOUT ACTION ADD AFTER AND ARG ASK BEFORE CODE CPU DECIDE DISK DO EVERY FACT FOLLOW FOR FROM GB GOAL GPU IF MAY MODEL MUST NEEDS NEVER NOT OR PER PREV RAM RUN SECRET SHOULD SINCE SKILL TB THEN TRIES WHEN WITHIN".split(" "),
);

/** One line outside a heredoc: keywords, quoted text, $names, comments. `open`: the line
 *  before ended inside a quote (continued with `\\`). */
function inline(line: string, key: string, open: boolean): { nodes: ReactNode[]; open: boolean } {
  if (!open && /^\s*#/.test(line)) return { nodes: [<span key={key} className="w-comment">{line}</span>], open };
  const out: ReactNode[] = [];
  if (open) {
    const end = /^(?:[^"\\]|\\.)*"/.exec(line);
    const part = end ? end[0] : line;
    out.push(<span key={`${key}-q`} className="w-quote">{names(part, `${key}-q`)}</span>);
    if (!end) return { nodes: out, open: true };
    line = line.slice(part.length);
  }
  const re = /("(?:[^"\\]|\\.)*"?)|(\$[A-Za-z_][A-Za-z0-9_]*)|\b([A-Z]{2,})\b/g;
  let at = 0;
  let m: RegExpExecArray | null;
  let k = 0;
  let open_ = false;
  while ((m = re.exec(line))) {
    if (m.index > at) out.push(line.slice(at, m.index));
    if (m[1]) {
      out.push(<span key={`${key}-${k++}`} className="w-quote">{names(m[1], `${key}-${k}`)}</span>);
      // A quote left open runs on to the next line.
      open_ = m[1].length === 1 || !m[1].endsWith('"');
    } else if (m[2]) out.push(<span key={`${key}-${k++}`} className="w-name">{m[2]}</span>);
    else if (KEYWORDS.has(m[3])) out.push(<span key={`${key}-${k++}`} className="w-key">{m[3]}</span>);
    else out.push(m[3]);
    at = m.index + m[0].length;
  }
  if (at < line.length) out.push(line.slice(at));
  return { nodes: out, open: open_ };
}

function names(text: string, key: string): ReactNode[] {
  return text.split(/(\$[A-Za-z_][A-Za-z0-9_]*)/).map((p, k) =>
    p.startsWith("$") ? (
      <span key={`${key}-${k}`} className="w-name">
        {p}
      </span>
    ) : (
      p
    ),
  );
}

function highlight(src: string): ReactNode[] {
  const lines = src.replace(/\n$/, "").split("\n");
  const out: ReactNode[] = [];
  let heredoc: { tag: string; model: boolean } | null = null;
  let quote = false;
  lines.forEach((line, n) => {
    const key = String(n);
    let cls = "wl";
    let body: ReactNode[];
    if (heredoc) {
      if (line.trim() === heredoc.tag) {
        body = [<span key={key} className="w-key">{line}</span>];
        heredoc = null;
      } else {
        cls = heredoc.model ? "wl wl-model" : "wl wl-here";
        body = heredoc.model ? names(line, key) : [line];
      }
    } else {
      const r = inline(line, key, quote);
      body = r.nodes;
      quote = r.open;
      const h = /<<-?\s*'?([A-Za-z_]+)'?\s*$/.exec(line);
      if (h) heredoc = { tag: h[1], model: /^\s*SKILL\b/.test(line) };
    }
    out.push(
      <div key={key} className={cls}>
        <span className="wl-n" aria-hidden="true">
          {n + 1}
        </span>
        <span className="wl-t">{body.length ? body : " "}</span>
      </div>,
    );
  });
  return out;
}

// The README comes from the weave: render its Markdown, never its raw HTML.
const md = new Marked({
  gfm: true,
  renderer: {
    html: ({ text }) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"),
  },
});

export function WeaveTab({ detail }: ServiceProps) {
  const code = useMemo(() => (detail.weavefile === null ? null : highlight(detail.weavefile)), [detail.weavefile]);
  const readme = useMemo(() => (detail.readme ? (md.parse(detail.readme) as string) : null), [detail.readme]);
  return (
    <div className={`weave ${readme ? "has-readme" : ""}`}>
      <Panel title="Weavefile" className="weave-file">
        {code ? (
          <pre className="weavefile" aria-label="Weavefile">
            <code>{code}</code>
          </pre>
        ) : (
          <p className="empty">No Weavefile for this service on the box.</p>
        )}
      </Panel>
      {readme && (
        <Panel title="README" className="weave-readme">
          <div className="prose" dangerouslySetInnerHTML={{ __html: readme }} />
        </Panel>
      )}
    </div>
  );
}
