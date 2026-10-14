import { useEffect, useState, type KeyboardEvent } from "react";
import { api, type Arg, type TellResult } from "../api";
import { Confirm, ErrorLine, errorText, Panel } from "../components/ui";
import type { ServiceProps } from "./ServiceView";

function argWhat(a: Arg): string {
  if ("value" in a.kind) return a.kind.value.what;
  return a.kind.options.map((o) => `${o.label}: ${o.meaning}`).join(" · ");
}

function Tell({ service }: { service: string }) {
  const [text, setText] = useState("");
  const [checked, setChecked] = useState<{ request: string; result: TellResult } | null>(null);
  const [busy, setBusy] = useState<"check" | "apply" | null>(null);
  const [applied, setApplied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(apply: boolean) {
    const request = apply && checked ? checked.request : text.trim();
    if (!request) return;
    setBusy(apply ? "apply" : "check");
    setError(null);
    try {
      const result = await api.tell(service, request, apply);
      setChecked({ request, result });
      setApplied(apply && result.ready);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(null);
    }
  }

  function key(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void run(false);
    }
  }

  const r = checked?.result;
  const changed = r
    ? [...new Set([...Object.keys(r.args_before), ...Object.keys(r.args_after)])].filter((k) => r.args_before[k] !== r.args_after[k])
    : [];
  const stale = checked && checked.request !== text.trim();

  return (
    <Panel title="Change settings in words" className="tell" aside={<span id="tell" />}>
      <p className="t-muted small panel-lead">Say what should change. The check reads it against the weave's ARGs first and changes nothing.</p>
      <textarea
        rows={3}
        value={text}
        placeholder="Keep 14 daily backups"
        aria-label="What should change"
        onChange={(e) => {
          setText(e.target.value);
          setApplied(false);
        }}
        onKeyDown={key}
      />
      <div className="row-buttons">
        <button type="button" className="btn" disabled={!text.trim() || busy !== null} onClick={() => run(false)}>
          {busy === "check" ? "Checking" : "Check"}
        </button>
        {r && !applied && (
          <button
            type="button"
            className="btn btn-primary"
            disabled={!r.ready || busy !== null || !!stale}
            title={stale ? "The text changed: check it again." : r.ready ? undefined : "Fix the errors first."}
            onClick={() => run(true)}
          >
            {busy === "apply" ? "Applying" : "Apply"}
          </button>
        )}
      </div>
      <ErrorLine error={error} />
      {applied && <p className="ok-line">Applied. The daemon takes the new settings on its next tick.</p>}
      {r && (
        <div className="tell-result">
          {r.findings.length > 0 && (
            <ul className="findings">
              {r.findings.map((f, k) => (
                <li key={k} className={`finding finding-${f.level}`}>
                  <span className="finding-level">{f.level}</span>
                  <span>{f.text}</span>
                </li>
              ))}
            </ul>
          )}
          {changed.length > 0 ? (
            <div className="table-wrap">
              <table className="table diff">
                <thead>
                  <tr>
                    <th>ARG</th>
                    <th>Before</th>
                    <th aria-hidden="true" />
                    <th>After</th>
                  </tr>
                </thead>
                <tbody>
                  {changed.map((k) => (
                    <tr key={k}>
                      <td className="mono">{k}</td>
                      <td className="mono t-muted">{r.args_before[k] ?? <i>unset</i>}</td>
                      <td className="t-muted">→</td>
                      <td className="mono w-6">{r.args_after[k] ?? <i>unset</i>}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="t-muted small">No ARG changes.</p>
          )}
          {!r.ready && <p className="t-red small">Not ready: fix the errors above and check again.</p>}
        </div>
      )}
    </Panel>
  );
}

export function Settings({ service, detail, status }: ServiceProps) {
  const stopped = status?.stopped ?? detail.spec.stopped;
  const [sent, setSent] = useState<string | null>(null);

  useEffect(() => {
    if (location.hash) document.getElementById(location.hash.slice(1))?.scrollIntoView({ block: "start" });
  }, []);

  return (
    <div className="settings">
      <Panel title="ARGs">
        <div className="table-wrap">
          <table className="table args">
            <thead>
              <tr>
                <th>Name</th>
                <th>Value</th>
                <th>Level</th>
                <th>What</th>
              </tr>
            </thead>
            <tbody>
              {detail.weave.args.map((a) => {
                const v = detail.spec.args[a.name];
                return (
                  <tr key={a.name}>
                    <td className="mono nowrap">{a.name}</td>
                    <td className="mono">
                      {v ?? <span className="t-muted">unset</span>}
                      {v !== undefined && v === a.default && <span className="t-muted small"> default</span>}
                    </td>
                    <td>
                      <span className={`tag level-${a.level.toLowerCase()}`}>{a.level}</span>
                    </td>
                    <td className="t-muted">{argWhat(a)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Panel>
      <Tell service={service} />
      <Panel title="Actions" aside={<span id="actions" />}>
        <ul className="actions">
          {detail.weave.actions.map((a) => (
            <li key={a.name} className="action">
              <div className="action-main">
                <div className="action-name">
                  <b className="mono">{a.name}</b>
                  {a.for_goals.length > 0 && <span className="t-muted small"> for {a.for_goals.join(", ")}</span>}
                  {a.within && <span className="tag tag-muted">WITHIN {a.within}</span>}
                  {a.when_if && <span className="tag tag-muted">IF {a.when_if}</span>}
                </div>
                <p className="action-when t-model">{a.when}</p>
              </div>
              <Confirm
                label="Run now"
                className="btn btn-small"
                disabled={stopped}
                text={
                  <>
                    Run <b>{a.name}</b> now, through the same rules. It is meant for: <span className="t-model">{a.when}</span>
                  </>
                }
                confirm="Run now"
                act={async () => {
                  await api.run(service, a.name);
                  setSent(a.name);
                }}
              />
            </li>
          ))}
        </ul>
        {sent && <p className="ok-line">Sent {sent}. Follow it in Activity.</p>}
      </Panel>
      <Panel title="Service" aside={<span id="service" />}>
        <div className="service-power">
          <p className="t-muted small">
            {stopped
              ? "Stopped: the daemon checks nothing and repairs nothing. Start runs the checks again."
              : "Stop makes the daemon stop checking and repairing this service. The service itself keeps running."}
          </p>
          {stopped ? (
            <Confirm label="Start" text={`Start checking and repairing ${service} again.`} confirm="Start" act={() => api.start(service)} />
          ) : (
            <Confirm label="Stop" text={`Stop checking and repairing ${service}.`} confirm="Stop" danger act={() => api.stop(service)} />
          )}
        </div>
      </Panel>
    </div>
  );
}
