import { useState } from "react";
import { Tooltip } from "../tooltip/Tooltip";
import { NavIcon } from "../dashboard/common/icons";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";

/** A place of the navigation shell (plan §F.2). */
export type AppSection = "my-work" | "review" | "settings";

/** localStorage key of the collapsed navigation. */
export const NAV_STORAGE_KEY = "smart-review.nav.collapsed";

const ENTRIES: { section: AppSection; name: string; tip: string }[] = [
  { section: "my-work", name: "My work", tip: "Your PRs, issues and TODOs, ranked; throughput charts" },
  {
    section: "review",
    name: "Review a PR",
    tip: "Open a pull request by URL and start or resume its analysis",
  },
  {
    section: "settings",
    name: "Settings",
    tip: "LLM status, GitHub App, repositories, dashboard scope, access tokens",
  },
];

interface AppNavProps {
  /** The section shown; null on other pages. */
  current: AppSection | null;
  /** In-app hrefs (My work is `/my-work` until the switch, then `/`). */
  hrefs: Record<AppSection, string>;
  /** Ranked items of the latest snapshot; no count when null. */
  myWorkCount: number | null;
  onNavigate?: Navigate;
}

function stored(): boolean {
  try {
    return window.localStorage.getItem(NAV_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

/** Left navigation of the shell (mockup `.nav`): My work with its count, Review a PR, Settings, Collapse. */
export function AppNav({ current, hrefs, myWorkCount, onNavigate }: AppNavProps) {
  const [collapsed, setCollapsed] = useState(stored);
  const toggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    try {
      window.localStorage.setItem(NAV_STORAGE_KEY, String(next));
    } catch {
      // Storage unavailable: the state lives for this page only.
    }
  };
  const item = (centered: boolean) =>
    `flex min-h-[34px] w-full cursor-pointer items-center gap-2.5 rounded-md border-0 border-l-4 border-transparent bg-transparent px-2.5 py-1.5 text-left text-text no-underline hover:bg-surface-2 ${centered ? "justify-center px-0" : ""}`;
  return (
    <nav
      aria-label="Main"
      data-collapsed={collapsed || undefined}
      className={`sticky top-12 flex h-[calc(100vh-48px)] flex-none flex-col gap-0.5 overflow-auto border-r border-border bg-surface px-2 py-2.5 ${collapsed ? "w-[52px]" : "w-[216px]"}`}
    >
      {ENTRIES.map(({ section, name, tip }) => (
        <div key={section} className="contents">
          {section === "settings" && <div role="presentation" className="mx-1 my-2 h-px bg-border" />}
          <Tooltip tip={tip} placement="right" className="block">
            <a
              href={hrefs[section]}
              aria-current={current === section ? "page" : undefined}
              aria-label={collapsed ? name : undefined}
              onClick={navigateOnClick(hrefs[section], onNavigate)}
              className={`${item(collapsed)} aria-[current=page]:border-accent aria-[current=page]:bg-accent-bg aria-[current=page]:font-semibold`}
            >
              <NavIcon name={section} />
              {!collapsed && <span className="truncate">{name}</span>}
              {!collapsed && section === "my-work" && myWorkCount !== null && (
                <span
                  data-nav-count
                  className="ml-auto rounded-full border border-border bg-surface-2 px-1.5 text-[11px] font-bold"
                >
                  {myWorkCount}
                </span>
              )}
            </a>
          </Tooltip>
        </div>
      ))}
      <div className="mt-auto">
        <Tooltip
          tip={collapsed ? "Show the navigation labels" : "Hide the navigation labels"}
          placement="right"
          className="block"
        >
          <button
            type="button"
            aria-expanded={!collapsed}
            aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
            onClick={toggle}
            className={item(collapsed)}
          >
            <NavIcon name={collapsed ? "expand" : "collapse"} />
            {!collapsed && <span>Collapse</span>}
          </button>
        </Tooltip>
      </div>
    </nav>
  );
}
