// History API routing: `/`, `/s/<service>/<tab>`, `/inbox`.
import { useSyncExternalStore, type MouseEvent } from "react";

export const TABS = ["overview", "activity", "metrics", "logs", "learned", "weave", "settings"] as const;
export type Tab = (typeof TABS)[number];

export type Route =
  | { page: "box" }
  | { page: "service"; service: string; tab: Tab }
  | { page: "inbox" };

export function parse(path: string): Route {
  const parts = path.split("/").filter(Boolean).map(decodeURIComponent);
  if (parts[0] === "inbox") return { page: "inbox" };
  if (parts[0] === "s" && parts[1]) {
    const tab = (TABS as readonly string[]).includes(parts[2] ?? "") ? (parts[2] as Tab) : "overview";
    return { page: "service", service: parts[1], tab };
  }
  return { page: "box" };
}

const subs = new Set<() => void>();
window.addEventListener("popstate", () => subs.forEach((f) => f()));

export function navigate(path: string, replace = false) {
  if (path === location.pathname + location.search) return;
  if (replace) history.replaceState(null, "", path);
  else history.pushState(null, "", path);
  subs.forEach((f) => f());
}

export function usePath(): string {
  return useSyncExternalStore(
    (f) => {
      subs.add(f);
      return () => subs.delete(f);
    },
    () => location.pathname,
  );
}

export function servicePath(service: string, tab: Tab = "overview") {
  return `/s/${encodeURIComponent(service)}/${tab}`;
}

/** onClick for <a href>: plain clicks route in the app, modified clicks open a tab. */
export function linkClick(e: MouseEvent<HTMLAnchorElement>) {
  if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  e.preventDefault();
  navigate(e.currentTarget.getAttribute("href")!);
}
