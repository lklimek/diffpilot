import { useCallback, useMemo, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Navigate, Outlet, useLocation, useOutletContext } from "react-router";
import { useMe } from "../../api/queries";
import type { AppSection } from "../../components/app-shell/AppNav";
import { MY_WORK_PATH, REVIEW_START_PATH, SETTINGS_PATH } from "./paths";
import { ShellLayout } from "./ShellLayout";
import { DashboardScreen } from "./DashboardScreen";

/** What a page inside the shell may set: My work's count (kept while other pages show), help, top-bar actions. */
export interface ShellSlots {
  /** Where a page renders its top-bar actions (`createPortal`); null until the bar is mounted. */
  actions: HTMLElement | null;
  setCount: (count: number | null) => void;
  /** The page's keyboard help; null removes the help button. */
  setHelp: (help: (() => void) | null) => void;
}

const PAGES: Record<string, { section: AppSection; crumb: string }> = {
  [MY_WORK_PATH]: { section: "my-work", crumb: "My work" },
  [REVIEW_START_PATH]: { section: "review", crumb: "Review a PR" },
  [SETTINGS_PATH]: { section: "settings", crumb: "Settings" },
};

/** Layout route of `/`, `/review` and `/settings` (plan §F.2): the shell stays while its pages change. */
export function ShellRoute() {
  const { pathname } = useLocation();
  const me = useMe();
  const page = PAGES[pathname] ?? PAGES[MY_WORK_PATH]!;
  const [actions, setActions] = useState<HTMLElement | null>(null);
  const [count, setCount] = useState<number | null>(null);
  const [help, setHelp] = useState<{ open: () => void } | null>(null);
  const setPageHelp = useCallback((open: (() => void) | null) => setHelp(open ? { open } : null), []);
  // Stable setters: pages call them from effects; a new function per render would loop.
  const slots = useMemo<ShellSlots>(
    () => ({ actions, setCount, setHelp: setPageHelp }),
    [actions, setPageHelp],
  );
  return (
    <ShellLayout
      current={page.section}
      crumb={page.crumb}
      login={me.data?.user?.login ?? null}
      myWorkCount={count}
      onHelp={help?.open}
      actions={<span ref={setActions} className="contents" />}
    >
      <Outlet context={slots} />
    </ShellLayout>
  );
}

/** The shell's slots for the page shown in it. */
export const useShellSlots = () => useOutletContext<ShellSlots>();

/** Top-bar actions of the page, rendered into the shell's bar. */
export function ShellActions({ children }: { children: ReactNode }) {
  const { actions } = useShellSlots();
  return actions ? createPortal(children, actions) : null;
}

/**
 * `/`: My work; with `pr` or `login_error` in the query (old deep links, the OAuth error redirect) the review start
 * page with the same query.
 */
export function StartRoute() {
  const { search } = useLocation();
  const query = new URLSearchParams(search);
  if (query.has("pr") || query.has("login_error"))
    return <Navigate to={{ pathname: REVIEW_START_PATH, search }} replace />;
  return <DashboardScreen />;
}
