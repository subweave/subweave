import content from "virtual:content";
import { Section } from "../components/Section";

const { draft, shipped, excerpt } = content.review;

// The worst finding on each shown line: errors before warnings before notes.
const RANK = { error: 0, warning: 1, note: 2 } as const;
const marks = new Map<number, (typeof draft.findings)[number]>();
for (const f of draft.findings) {
  const had = marks.get(f.line);
  if (!had || RANK[f.level] < RANK[had.level]) marks.set(f.line, f);
}

export function Review({ n }: { n: number }) {
  return (
    <Section
      id="review"
      n={n}
      kicker="For weave authors"
      title="Is your Weavefile any good? Ask it."
      lede={
        <>
          <code>subweave review</code> scores a Weavefile from 0 to 100 and points at the lines to fix. It changes nothing and runs no command.
        </>
      }
    >
      <div className="review">
        <figure className="review-file">
          <figcaption className="panel-head">
            <span>draft/Weavefile</span>
            <span className="review-legend">
              <span className="rv-dot is-error" /> error <span className="rv-dot is-warning" /> warning
            </span>
          </figcaption>
          <pre className="code-block review-code" tabIndex={0} aria-label="Lines 37 to 47 of a first-draft postgres Weavefile, with review findings">
            <code>
              {excerpt.map(({ n, key }) => {
                const f = marks.get(n);
                return (
                  <span key={n} className={`rv-line${f ? ` is-${f.level}` : ""}`}>
                    <span className="rv-n">{n}</span>
                    <span dangerouslySetInnerHTML={{ __html: content.code[key] }} />
                    {f && (
                      <span className="rv-finding">
                        {f.level} <b>{f.rule}</b>: {f.message}
                        {"\n"}
                      </span>
                    )}
                  </span>
                );
              })}
            </code>
          </pre>
        </figure>
        <div className="screen review-screen">
          <div className="screen-bar">
            <span>terminal</span>
          </div>
          <pre className="screen-log review-log" tabIndex={0} aria-label="Output of subweave review ./draft">
            <span className="rv-cmd">$ {draft.command}</span>
            {"\n"}
            <b>{draft.header}</b>
            {"\n\n"}
            {draft.table.map((l, i) => (
              <span key={i} className={i === 0 ? "rv-th" : undefined}>
                {l.slice(0, draft.table[0].indexOf("score") + 5)}
                {"\n"}
              </span>
            ))}
          </pre>
          <ul className="review-bars" aria-label="Score by area">
            {draft.areas.map((a) => (
              <li key={a.area}>
                <span className="rv-area">{a.area}</span>
                <span className="rv-bar" aria-hidden="true">
                  <span style={{ width: `${a.score}%` }} className={a.score >= 80 ? "is-ok" : a.score >= 50 ? "is-warning" : "is-error"} />
                </span>
                <span className="rv-score">{a.score}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>
      <p className="review-after">
        First draft <b>{draft.score}</b> → shipped postgres weave <b>{shipped.score}</b>. The draft took {draft.checks} checks for {draft.cost}; the
        shipped one, {shipped.checks} checks for {shipped.cost}.
      </p>
    </Section>
  );
}
