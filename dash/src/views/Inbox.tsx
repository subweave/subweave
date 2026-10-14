import { useEffect, useRef } from "react";
import { api, type Ask } from "../api";
import { useAppData } from "../App";
import { AskCard } from "../components/act";
import { ErrorLine, Skeleton } from "../components/ui";
import { useLive } from "../live";
import { useResource } from "../resource";

export function Inbox({ onClose }: { onClose: () => void }) {
  const { services } = useAppData();
  const live = useLive();
  const names = services?.map((s) => s.name) ?? null;
  // Refetch when any service's open asks change.
  const asksKey = services?.map((s) => `${s.name}:${s.status?.asks.join(".") ?? ""}`).join() ?? "";
  const asks = useResource<Ask[]>(
    names ? `inbox:${names.join()}` : null,
    async () => {
      const all = await Promise.all(names!.map((n) => api.service(n)));
      return all.flatMap((d) => d.asks).sort((a, b) => b.t.localeCompare(a.t));
    },
    `${asksKey}:${live.epoch}`,
  );
  const panel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    panel.current?.focus();
    const key = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", key);
    return () => document.removeEventListener("keydown", key);
  }, [onClose]);

  return (
    <div className="drawer-wrap">
      <div className="drawer-scrim" onClick={onClose} />
      <aside className="drawer" role="dialog" aria-modal="true" aria-label="Inbox" tabIndex={-1} ref={panel}>
        <header className="drawer-head">
          <h2>Inbox</h2>
          <button type="button" className="btn btn-quiet btn-icon" onClick={onClose} aria-label="Close">
            <svg viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
              <path d="M2 2l8 8M10 2l-8 8" />
            </svg>
          </button>
        </header>
        <div className="drawer-body">
          <ErrorLine error={asks.error} />
          {asks.loading && <Skeleton lines={6} />}
          {asks.data?.length === 0 && <p className="empty">Nothing waits for you.</p>}
          {asks.data?.map((a) => (
            <AskCard key={`${a.service}-${a.n}`} ask={a} showService />
          ))}
        </div>
      </aside>
    </div>
  );
}
