import { createContext, useContext, useEffect, useMemo, useRef, useState } from "react";
import { api, setUnauthorizedHandler, type BoxInfo, type ServiceSummary, type Session } from "./api";
import { Glyph, Wordmark } from "./components/ui";
import { month, usd } from "./format";
import { startLive, stopLive, useLive } from "./live";
import { holdsOf, serviceShown } from "./model";
import { linkClick, navigate, parse, servicePath, usePath, type Route } from "./router";
import { useResource } from "./resource";
import { BoxView } from "./views/BoxView";
import { Inbox } from "./views/Inbox";
import { Login } from "./views/Login";
import { ServiceView } from "./views/ServiceView";

interface AppData {
  services: ServiceSummary[] | undefined;
  servicesError: string | null;
  box: BoxInfo | undefined;
  reloadServices: () => void;
  reloadBox: () => void;
}

const Data = createContext<AppData | null>(null);
export function useAppData(): AppData {
  return useContext(Data)!;
}

export function App() {
  const [session, setSession] = useState<Session | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setUnauthorizedHandler(() => setSession((s) => (s ? { ...s, ok: false } : s)));
    api.session().then(setSession, (e: Error) => setError(e.message));
  }, []);

  const signedIn = !!session && (session.auth === "none" || session.ok);
  useEffect(() => {
    if (!signedIn) return;
    startLive();
    return stopLive;
  }, [signedIn]);

  if (error) {
    return (
      <div className="center-page">
        <div className="login">
          <Wordmark />
          <p className="error-line">{error}</p>
          <button type="button" className="btn" onClick={() => location.reload()}>
            Try again
          </button>
        </div>
      </div>
    );
  }
  if (!session) return <div className="center-page" aria-busy="true" />;
  if (!signedIn) return <Login onSignedIn={() => setSession({ ...session, ok: true })} />;
  return (
    <Frame
      auth={session.auth}
      onSignOut={async () => {
        await api.logout().catch(() => {});
        setSession({ ...session, ok: false });
      }}
    />
  );
}

function Frame({ auth, onSignOut }: { auth: Session["auth"]; onSignOut: () => void }) {
  const path = usePath();
  const route = parse(path);
  const live = useLive();
  const services = useResource("services", api.services, live.epoch);
  const box = useResource("box", api.box, live.epoch);

  // Statuses from the event stream are newer than the list's.
  const merged = useMemo(
    () => services.data?.map((s) => ({ ...s, status: live.statuses[s.name] ?? s.status, stopped: (live.statuses[s.name] ?? s.status)?.stopped ?? s.stopped })),
    [services.data, live.statuses],
  );
  const asks = merged?.reduce((n, s) => n + (s.status?.asks.length ?? 0), 0) ?? box.data?.asks_open ?? 0;
  const spend = merged?.reduce((n, s) => n + (s.status?.spend_month_usd ?? 0), 0) ?? box.data?.spend_month_usd ?? 0;

  // The box refreshes on its own each minute and when the asks change.
  const reloadBox = box.reload;
  useEffect(() => {
    const id = window.setInterval(reloadBox, 60_000);
    return () => window.clearInterval(id);
  }, [reloadBox]);
  useEffect(() => reloadBox(), [asks, reloadBox]);

  // A service's list entry (state, stopped) changes with its status.
  const reloadServices = services.reload;
  const stoppedKey = Object.values(live.statuses)
    .map((s) => `${s.service}:${s.stopped}`)
    .join();
  useEffect(() => reloadServices(), [stoppedKey, reloadServices]);

  // `/inbox` opens over the page it came from.
  const under = useRef<Route>({ page: "box" });
  if (route.page !== "inbox") under.current = route;
  const page = under.current;

  const data: AppData = {
    services: merged,
    servicesError: services.error,
    box: box.data,
    reloadServices,
    reloadBox,
  };

  useEffect(() => {
    const where = page.page === "service" ? `${page.service} · ` : "";
    document.title = `${where}${box.data?.host ?? "Subweave"}`;
  }, [page, box.data?.host]);

  return (
    <Data.Provider value={data}>
      <div className="frame">
        <header className="topbar">
          <a href="/" onClick={linkClick} className="topbar-brand" aria-label="Box overview">
            <Wordmark host={box.data?.host} />
          </a>
          <div className="topbar-right">
            <span className={`live live-${live.conn}`} role="status">
              <Glyph state={live.conn === "live" ? "holds" : live.conn === "reconnecting" ? "waiting" : "unknown"} title={live.conn} />
              <span className="live-label">{live.conn === "live" ? "Live" : live.conn === "reconnecting" ? "Reconnecting" : "Connecting"}</span>
            </span>
            <a
              href="/inbox"
              onClick={linkClick}
              className={`pill ${asks ? "pill-amber" : ""}`}
              aria-label={`${asks} open asks`}
            >
              {asks ? (
                <>
                  <Glyph state="ask" /> {asks} {asks === 1 ? "ask" : "asks"}
                </>
              ) : (
                "Inbox"
              )}
            </a>
            <span className="spend num" title="Model spend this month, every service">
              {usd(spend)} <span className="t-muted">· {month(Date.now())}</span>
            </span>
            {auth === "password" && (
              <button type="button" className="btn btn-quiet btn-small" onClick={onSignOut}>
                Sign out
              </button>
            )}
          </div>
        </header>
        <Rail route={page} />
        <main className="main" id="main">
          {page.page === "service" ? (
            <ServiceView key={page.service} service={page.service} tab={page.tab} />
          ) : (
            <BoxView />
          )}
        </main>
        {route.page === "inbox" && (
          <Inbox
            onClose={() => {
              const back = under.current;
              navigate(back.page === "service" ? servicePath(back.service, back.tab) : "/");
            }}
          />
        )}
      </div>
    </Data.Provider>
  );
}

function Rail({ route }: { route: Route }) {
  const { services, box, servicesError } = useAppData();
  const current = route.page === "service" ? route.service : "";
  return (
    <nav className="rail" aria-label="Services">
      <div className="rail-pick">
        <label className="sr-only" htmlFor="rail-select">
          Page
        </label>
        <select
          id="rail-select"
          value={current}
          onChange={(e) => navigate(e.target.value ? servicePath(e.target.value, route.page === "service" ? route.tab : "overview") : "/")}
        >
          <option value="">Box</option>
          {services?.map((s) => (
            <option key={s.name} value={s.name}>
              {s.name}
              {s.stopped ? " (stopped)" : ""}
            </option>
          ))}
        </select>
      </div>
      <div className="rail-list">
        <a href="/" onClick={linkClick} className={`rail-item ${route.page === "box" ? "is-current" : ""}`} aria-current={route.page === "box" ? "page" : undefined}>
          <svg className="rail-icon" viewBox="0 0 12 12" aria-hidden="true">
            <rect x="1.5" y="1.5" width="9" height="9" rx="1.5" />
            <path d="M1.5 5h9M5 5v5.5" />
          </svg>
          <span>Box</span>
        </a>
        <h2 className="rail-head">Services</h2>
        {servicesError && <p className="error-line rail-note">{servicesError}</p>}
        {!services && !servicesError && (
          <div className="rail-note">
            <span className="sk-bar" />
            <span className="sk-bar" />
          </div>
        )}
        {services?.length === 0 && <p className="rail-note t-muted">No services yet. Start one with subweave run.</p>}
        {services?.map((s) => {
          const { holds, total } = holdsOf(s.status);
          const here = s.name === current;
          return (
            <a
              key={s.name}
              href={servicePath(s.name, route.page === "service" && here ? route.tab : "overview")}
              onClick={linkClick}
              className={`rail-item ${here ? "is-current" : ""} ${s.stopped ? "is-stopped" : ""}`}
              aria-current={here ? "page" : undefined}
            >
              <Glyph state={serviceShown(s.status, s.stopped)} />
              <span className="rail-name">{s.name}</span>
              <span className="rail-meta num">{s.stopped ? "stopped" : s.status ? `${holds}/${total}` : "starting"}</span>
            </a>
          );
        })}
      </div>
      <footer className="rail-foot">
        {box ? (
          <>
            <span>
              <Glyph state={box.daemon.active ? "holds" : "fails"} /> daemon {box.daemon.active ? "up" : "down"}
            </span>
            <span className="t-muted num">v{box.version}</span>
          </>
        ) : (
          <span className="sk-bar" />
        )}
      </footer>
    </nav>
  );
}
