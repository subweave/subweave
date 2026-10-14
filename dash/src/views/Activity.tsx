import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type Line } from "../api";
import { TaskComposer } from "../components/act";
import { EpisodeCard, Row } from "../components/rows";
import { ErrorLine, errorText, Skeleton } from "../components/ui";
import { useNow } from "../format";
import { useJournal, useLive } from "../live";
import { episodes, KIND_CHIPS, routineChecks } from "../model";
import type { ServiceProps } from "./ServiceView";

const PAGE = 200;

export function Activity({ service, detail }: ServiceProps) {
  const now = useNow(1000);
  const live = useLive();
  const [off, setOff] = useState<Set<string>>(new Set());
  const [goal, setGoal] = useState("");
  const [search, setSearch] = useState("");
  const [q, setQ] = useState("");
  const [paused, setPaused] = useState(false);
  const [routine, setRoutine] = useState(false);
  const [lines, setLines] = useState<Line[] | null>(null);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [held, setHeld] = useState<Line[]>([]);

  useEffect(() => {
    const id = window.setTimeout(() => setQ(search.trim()), 250);
    return () => window.clearTimeout(id);
  }, [search]);

  const kinds = useMemo(
    () => (off.size ? KIND_CHIPS.filter((c) => !off.has(c.label)).flatMap((c) => [...c.kinds]) : undefined),
    [off],
  );
  const filter = useMemo(() => ({ kind: kinds, goal: goal || undefined, q: q || undefined }), [kinds, goal, q]);

  // The first page, again whenever the filter changes or the stream reconnects.
  const gen = useRef(0);
  useEffect(() => {
    const mine = ++gen.current;
    setLines(null);
    setHeld([]);
    setError(null);
    if (kinds && kinds.length === 0) {
      setLines([]);
      setMore(false);
      return;
    }
    api.journal(service, { ...filter, limit: PAGE }).then(
      (r) => {
        if (gen.current !== mine) return;
        setLines(r.lines);
        setMore(r.more);
      },
      (e) => gen.current === mine && setError(errorText(e)),
    );
  }, [service, filter, kinds, live.epoch]);

  const loadMore = useCallback(() => {
    if (!lines?.length || !more || loadingMore) return;
    const mine = gen.current;
    setLoadingMore(true);
    api
      .journal(service, { ...filter, limit: PAGE, before: lines[lines.length - 1].i })
      .then(
        (r) => {
          if (gen.current !== mine) return;
          setLines((ls) => [...(ls ?? []), ...r.lines]);
          setMore(r.more);
        },
        (e) => gen.current === mine && setError(errorText(e)),
      )
      .finally(() => setLoadingMore(false));
  }, [service, filter, lines, more, loadingMore]);

  const sentinel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = sentinel.current;
    if (!el) return;
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && loadMore(), { rootMargin: "600px" });
    io.observe(el);
    return () => io.disconnect();
  }, [loadMore]);

  const matches = useCallback(
    (l: Line) => {
      if (kinds && !kinds.includes(l.kind)) return false;
      if (goal && l.goal !== goal) return false;
      if (q && !JSON.stringify(l).toLowerCase().includes(q.toLowerCase())) return false;
      return true;
    },
    [kinds, goal, q],
  );

  useJournal(
    service,
    useCallback(
      (_s: string, l: Line) => {
        if (!matches(l)) return;
        if (paused) setHeld((h) => [l, ...h]);
        else setLines((ls) => (ls && !ls.some((x) => x.i === l.i) ? [l, ...ls] : ls));
      },
      [matches, paused],
    ),
  );

  function resume() {
    setLines((ls) => {
      const have = new Set(ls?.map((l) => l.i));
      return [...held.filter((l) => !have.has(l.i)), ...(ls ?? [])];
    });
    setHeld([]);
    setPaused(false);
  }

  const hidden = useMemo(() => (routine || !lines ? new Set<number>() : routineChecks(lines)), [lines, routine]);
  const items = useMemo(() => (lines ? episodes(lines) : []), [lines]);
  const visible = items.filter((it) => (it.type === "line" ? !hidden.has(it.line.i) : true));
  const routineCount = hidden.size;

  const toggle = (label: string) =>
    setOff((o) => {
      const n = new Set(o);
      if (n.has(label)) n.delete(label);
      else n.add(label);
      return n;
    });

  return (
    <div className="activity">
      <TaskComposer service={service} compact />
      <div className="filters">
        <div className="chips" role="group" aria-label="Kinds">
          {KIND_CHIPS.map((c) => (
            <button key={c.label} type="button" className="chip" aria-pressed={!off.has(c.label)} onClick={() => toggle(c.label)}>
              {c.label}
            </button>
          ))}
        </div>
        <div className="filters-row">
          <label className="sr-only" htmlFor="act-goal">
            Goal
          </label>
          <select id="act-goal" value={goal} onChange={(e) => setGoal(e.target.value)}>
            <option value="">All goals</option>
            {detail.weave.goals.map((g) => (
              <option key={g.name} value={g.name}>
                {g.name}
              </option>
            ))}
          </select>
          <label className="sr-only" htmlFor="act-q">
            Search
          </label>
          <input id="act-q" type="search" placeholder="Search lines" value={search} onChange={(e) => setSearch(e.target.value)} />
          <label className="check">
            <input type="checkbox" checked={routine} onChange={(e) => setRoutine(e.target.checked)} />
            Show routine checks
          </label>
          <button type="button" className={`btn btn-small ${paused ? "btn-primary" : ""}`} onClick={() => (paused ? resume() : setPaused(true))}>
            {paused ? "Resume live" : "Pause live"}
          </button>
        </div>
      </div>
      {paused && held.length > 0 && (
        <button type="button" className="held" onClick={resume}>
          {held.length} new {held.length === 1 ? "line" : "lines"}. Show them.
        </button>
      )}
      <ErrorLine error={error} />
      {!lines && !error && <Skeleton lines={12} />}
      {lines && visible.length === 0 && <p className="empty">{lines.length ? "Only routine checks here. Show them with the switch above." : "No lines match."}</p>}
      <ol className="rows rows-activity">
        {visible.map((it) =>
          it.type === "line" ? <Row key={it.line.i} line={it.line} now={now} /> : <EpisodeCard key={it.key} ep={it} now={now} hidden={hidden} />,
        )}
      </ol>
      <div ref={sentinel} className="sentinel">
        {loadingMore && <Skeleton lines={3} />}
        {lines && !more && lines.length > 0 && (
          <p className="t-muted small end-note">
            The start of the journal.{!routine && routineCount > 0 && ` ${routineCount} routine checks hidden.`}
          </p>
        )}
      </div>
    </div>
  );
}
