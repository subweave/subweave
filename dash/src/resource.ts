// Fetched data keyed by a string. The last value of a key stays in memory, so going back
// to a tab shows it at once while it loads again.
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "./api";

const cache = new Map<string, unknown>();

export interface Resource<T> {
  data: T | undefined;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

/** `key` null: nothing to load yet. A change of `refresh` loads again in place. */
export function useResource<T>(key: string | null, load: () => Promise<T>, refresh: unknown = 0): Resource<T> {
  const [state, setState] = useState<{ key: string | null; data: T | undefined; error: string | null }>(() => ({
    key,
    data: key === null ? undefined : (cache.get(key) as T | undefined),
    error: null,
  }));
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  loadRef.current = load;

  // A new key shows its cached value (or nothing) at once.
  let current = state;
  if (state.key !== key) {
    current = { key, data: key === null ? undefined : (cache.get(key) as T | undefined), error: null };
    setState(current);
  }

  useEffect(() => {
    if (key === null) return;
    let live = true;
    loadRef.current().then(
      (data) => {
        if (!live) return;
        cache.set(key, data);
        setState({ key, data, error: null });
      },
      (e: unknown) => {
        if (!live) return;
        setState((s) => ({ ...s, key, error: e instanceof ApiError ? e.message : String(e) }));
      },
    );
    return () => {
      live = false;
    };
  }, [key, refresh, tick]);

  const reload = useCallback(() => setTick((t) => t + 1), []);
  return {
    data: current.data,
    error: current.error,
    loading: current.data === undefined && current.error === null && key !== null,
    reload,
  };
}
