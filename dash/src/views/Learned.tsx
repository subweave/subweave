import { useState } from "react";
import { api } from "../api";
import { Confirm, Panel } from "../components/ui";
import { ago, full, useNow } from "../format";
import type { ServiceProps } from "./ServiceView";

export function LearnedTab({ service, detail, reload }: ServiceProps) {
  const now = useNow(30_000);
  const [gone, setGone] = useState<Set<string>>(new Set());
  const rows = [
    ...Object.entries(detail.learned.facts).map(([name, f]) => ({ name, kind: "fact", commands: [f.command], t: f.t })),
    ...Object.entries(detail.learned.steps).map(([name, s]) => ({ name, kind: "steps", commands: s.commands, t: s.t })),
  ].filter((r) => !gone.has(r.name));

  return (
    <Panel title="Learned commands" aside={<span className="t-muted small num">{rows.length}</span>}>
      <p className="t-muted small panel-lead">
        The box learns a command once for each quoted fact and the steps that made each goal hold, then runs them with no model.
      </p>
      {rows.length === 0 ? (
        <p className="empty">Nothing learned yet.</p>
      ) : (
        <div className="table-wrap">
          <table className="table learned">
            <thead>
              <tr>
                <th>Name</th>
                <th>Kind</th>
                <th>Command</th>
                <th>Learned</th>
                <th aria-label="Actions" />
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={`${r.kind}:${r.name}`}>
                  <td className="mono nowrap">{r.name}</td>
                  <td className="t-muted">{r.kind}</td>
                  <td>
                    {r.commands.map((c, k) => (
                      <code key={k} className="cmd cmd-block">
                        {c}
                      </code>
                    ))}
                  </td>
                  <td className="t-muted nowrap num" title={full(r.t)}>
                    {ago(r.t, now)}
                  </td>
                  <td className="right">
                    <Confirm
                      label="Forget"
                      className="btn btn-small"
                      text="The box learns it again on the next check."
                      confirm="Forget"
                      danger
                      act={async () => {
                        await api.forget(service, r.name);
                        setGone((g) => new Set(g).add(r.name));
                        reload();
                      }}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}
