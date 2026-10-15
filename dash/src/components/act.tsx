import { useState, type KeyboardEvent } from "react";
import { api, type Ask } from "../api";
import { ago, useNow } from "../format";
import { linkClick, servicePath } from "../router";
import { ErrorLine, errorText, FactsTable } from "./ui";

/** Short fact sets show at once; long ones (a whole log, a config) open on a click. */
function AskFacts({ facts }: { facts: Record<string, string> }) {
  const n = Object.keys(facts).length;
  const long = n > 4 || Object.values(facts).some((v) => v.split("\n").length > 3);
  const [open, setOpen] = useState(!long);
  if (!n) return null;
  return (
    <div className="ask-facts">
      {long && (
        <button type="button" className="disclose" aria-expanded={open} onClick={() => setOpen(!open)}>
          {n} facts the decider read
        </button>
      )}
      {open && <FactsTable facts={facts} />}
    </div>
  );
}

export function AskCard({ ask, showService = false, onAnswered }: { ask: Ask; showService?: boolean; onAnswered?: () => void }) {
  const now = useNow(30_000);
  const [sent, setSent] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const keeps = ask.answers.some((a) => a === "always" || a === "never");

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
      <h4 className="ask-title">{ask.title}</h4>
      {ask.text && <p className="ask-text">{ask.text}</p>}
      {(ask.goal || ask.action || ask.rule) && (
        <p className="ask-meta t-muted">
          {[ask.action && `action ${ask.action}`, ask.goal && `goal ${ask.goal}`, ask.rule && `rule ${ask.rule}`].filter(Boolean).join(" · ")}
        </p>
      )}
      <AskFacts facts={ask.facts} />
      {ask.signed ? (
        <p className="hint">Only a signed answer on Steery settles this.</p>
      ) : sent ? (
        <p className="ask-sent">
          Answered <b>{ask.labels[sent] ?? sent}</b>. The daemon picks it up within seconds.
        </p>
      ) : (
        <>
          <div className="ask-answers">
            {ask.answers.map((w) => (
              <button
                key={w}
                type="button"
                className={w === "yes" ? "btn btn-primary" : "btn"}
                disabled={busy}
                onClick={() => answer(w)}
              >
                {ask.labels[w] ?? w}
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
