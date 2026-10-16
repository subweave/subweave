import { useState, type KeyboardEvent } from "react";
import { api, type Ask, type Card } from "../api";
import { ago, useNow } from "../format";
import { linkClick, servicePath } from "../router";
import { ErrorLine, errorText, FactsTable } from "./ui";

/** The card's body, as every channel shows it: the goal, the read set of its verdict, the
 * action and the rule that holds it, a part's releases, then what else it says. */
export function CardBody({ card }: { card: Card }) {
  const w = card.window;
  const a = card.action;
  const check = card.goal && ("code" in card.goal.check ? card.goal.check.code : card.goal.check.quoted);
  const reads = Object.fromEntries((card.reads ?? []).map((r) => [r.fact, r.value]));
  return (
    <div className="ask-card">
      {card.summary?.map((s, i) => (
        <p key={`s${i}`} className="ask-text">
          {s}
        </p>
      ))}
      {card.drill && (
        <p className="hint">
          Drill {card.drill.name} ({card.drill.label}): caused on purpose, to test the weave.
        </p>
      )}
      {card.goal && (
        <p className="ask-text">
          Not true now: {card.goal.sentence}{" "}
          <span className="t-muted">
            ({card.goal.name}: <code>{check}</code>
            {w && <> over {w.over}</>})
          </span>
        </p>
      )}
      {!!card.reads?.length && <FactsTable facts={reads} />}
      {w?.values?.map((s) => (
        <p key={s.fact} className="row-sub">
          <code>{s.fact}</code> in the window: {s.more ? "…, " : ""}
          {s.values.join(", ")}
        </p>
      ))}
      {w?.lines && (
        <ul className="row-sub">
          {!w.lines.kinds.length && <li>No lines in the window.</li>}
          {w.lines.kinds.map((k, i) => (
            <li key={i}>
              {k.count}× {k.known ? `known ${k.known.class} (${k.known.by})` : "not known yet"}: <code>{k.sample}</code>
            </li>
          ))}
          {!!w.lines.more && <li>…and {w.lines.more} more kinds of line.</li>}
        </ul>
      )}
      {card.parts?.map((p) => (
        <p key={p.name} className="row-sub">
          Part <code>{p.name}</code> runs {p.running}; newest {p.newest}; security release waits: {p.security}.
          {!!p.notes?.length && <> Notes: {p.notes.join(" / ")}</>}
        </p>
      ))}
      {a && (
        <p className="row-sub">
          “{a.label}”: {a.does.join("; then ")}. It changes {a.changes.length ? a.changes.join(", ") : "nothing"}.
          {a.held && (
            <>
              {" "}
              <code>{a.held.rule}</code> holds it{a.held.because ? `: ${a.held.because}` : ""}.
            </>
          )}
        </p>
      )}
      {card.undoes && <p className="row-sub">It undoes “{card.undoes}”.</p>}
      {card.lines?.map((l, i) => (
        <p key={`l${i}`} className="row-sub">
          {l}
        </p>
      ))}
    </div>
  );
}

export function AskCard({ ask, showService = false, onAnswered }: { ask: Ask; showService?: boolean; onAnswered?: () => void }) {
  const now = useNow(30_000);
  const [sent, setSent] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const buttons = ask.card.buttons ?? [];
  const keeps = buttons.some((b) => b.word === "always" || b.word === "never");

  async function answer(word: string) {
    setBusy(true);
    setError(null);
    try {
      await api.answer(ask.service, ask.n, word);
      setSent(word);
      onAnswered?.();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <article className="ask">
      <header className="ask-head">
        <span className="ask-n num">#{ask.n}</span>
        {showService && (
          <a className="ask-service" href={servicePath(ask.service)} onClick={linkClick}>
            {ask.service}
          </a>
        )}
        <span className="t-muted ask-t" title={ask.t}>
          {ago(ask.t, now)}
        </span>
      </header>
      <h4 className="ask-title">{ask.card.title}</h4>
      <CardBody card={ask.card} />
      {ask.signed ? (
        <p className="hint">Only a signed answer on Steery settles this.</p>
      ) : sent ? (
        <p className="ask-sent">
          Answered <b>{buttons.find((b) => b.word === sent)?.label ?? sent}</b>. The daemon picks it up within seconds.
        </p>
      ) : (
        <>
          <div className="ask-answers">
            {buttons.map((b) => (
              <button
                key={b.word}
                type="button"
                className={b.word === "yes" ? "btn btn-primary" : "btn"}
                disabled={busy}
                onClick={() => answer(b.word)}
              >
                {b.label}
              </button>
            ))}
          </div>
          {keeps && <p className="hint">The daemon keeps always and never for this rule and asks no more.</p>}
        </>
      )}
      <ErrorLine error={error} />
    </article>
  );
}

export function TaskComposer({ service, compact = false }: { service: string; compact?: boolean }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function send() {
    const t = text.trim();
    if (!t || busy) return;
    setBusy(true);
    setError(null);
    try {
      await api.task(service, t);
      setText("");
      setSent(true);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  function key(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void send();
    }
  }

  return (
    <form
      className={`composer ${compact ? "composer-compact" : ""}`}
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      <label className="sr-only" htmlFor={`task-${service}`}>
        A task for {service}
      </label>
      <textarea
        id={`task-${service}`}
        value={text}
        rows={compact ? 2 : 3}
        placeholder={`Give ${service} a task: vacuum the shop database`}
        onChange={(e) => {
          setText(e.target.value);
          setSent(false);
        }}
        onKeyDown={key}
      />
      <div className="composer-foot">
        <p className="hint">The coder runs it through the same rules: NEVER, ASK and SSH safety. It may ask you first.</p>
        <button type="submit" className="btn btn-primary" disabled={busy || !text.trim()}>
          Send task <kbd>⌘↵</kbd>
        </button>
      </div>
      {sent && (
        <p className="composer-sent">
          Sent. Follow it in{" "}
          <a href={servicePath(service, "activity")} onClick={linkClick}>
            Activity
          </a>
          .
        </p>
      )}
      <ErrorLine error={error} />
    </form>
  );
}
