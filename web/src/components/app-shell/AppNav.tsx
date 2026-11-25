import { useState, type ComponentProps } from "react";
import { Tooltip } from "../tooltip/Tooltip";
import { NavIcon } from "../dashboard/common/icons";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";

/** A place of the navigation shell (plan §F.2). */
export type AppSection = "my-work" | "review" | "watching" | "settings";

/** localStorage key of the collapsed navigation. */
export const NAV_STORAGE_KEY = "diffpilot.nav.collapsed";

const ENTRIES: { section: AppSection; name: string; tip: string }[] = [
  { section: "my-work", name: "My work", tip: "Your PRs, issues and TODOs, ranked; throughput charts" },
  {
    section: "review",
    name: "Review a PR",
    tip: "Open a pull request by URL and start or resume its analysis",
  },
  {
    section: "watching",
    name: "Watched PRs",
    tip: "Pull requests you watch: new comments are found and assessed automatically",
  },
  {
    section: "settings",
    name: "Settings",
    tip: "LLM status, GitHub App, repositories, dashboard scope, access tokens",
  },
];

export interface AppNavChild {
  icon: ComponentProps<typeof NavIcon>["name"];
  label: string;
  href: string;
  active: boolean;
}

interface AppNavProps {
  /** The section shown; null on other pages. */
  current: AppSection | null;
  /** In-app hrefs. */
  hrefs: Record<AppSection, string>;
  /** Unhidden human mentions; no count when null. */
  myWorkCount: number | null;
  settingsPages?: readonly AppNavChild[];
  onNavigate?: Navigate;
}

function stored(): boolean {
  try {
    const saved = window.localStorage.getItem(NAV_STORAGE_KEY);
    return saved === null ? (window.matchMedia?.("(max-width: 639px)").matches ?? false) : saved === "true";
  } catch {
    return false;
  }
}

/** Left navigation of the shell (mockup `.nav`): My work with its count, Review a PR, Settings, Collapse. */
export function AppNav({ current, hrefs, myWorkCount, settingsPages, onNavigate }: AppNavProps) {
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
  const activeClass =
    "data-[selected=true]:border-accent data-[selected=true]:bg-accent-bg data-[selected=true]:font-semibold";
  const mentions = `${myWorkCount} unanswered ${myWorkCount === 1 ? "mention" : "mentions"} from people`;
  const item = (centered: boolean) =>
    `flex min-h-[34px] w-full cursor-pointer items-center gap-2.5 rounded-md border-0 border-l-4 border-transparent bg-transparent py-1.5 text-left text-text no-underline hover:bg-surface-2 ${centered ? "justify-center px-0" : "px-2.5"}`;
  return (
    <nav
      aria-label="Main"
      data-collapsed={collapsed || undefined}
      className={`sticky top-12 flex h-[calc(100vh-48px)] flex-none flex-col gap-0.5 overflow-auto border-r border-border bg-surface px-2 py-2.5 ${collapsed ? "w-[52px]" : "w-[216px]"}`}
    >
      {ENTRIES.map(({ section, name, tip }) => (
        <div key={section} className="contents">
          {section === "settings" && <div role="presentation" className="mx-1 my-2 h-px bg-border" />}
          <Tooltip
            tip={section === "my-work" && myWorkCount !== null ? `${tip}; ${mentions}` : tip}
            placement="right"
            className="block"
          >
            <a
              href={hrefs[section]}
              aria-current={
                current === section && !(section === "settings" && settingsPages) ? "page" : undefined
              }
              aria-label={section === "my-work" && myWorkCount !== null ? `My work, ${mentions}` : name}
              onClick={navigateOnClick(hrefs[section], onNavigate)}
              data-selected={current === section || undefined}
              className={`${item(collapsed)} ${activeClass}`}
            >
              <NavIcon name={section} />
              {!collapsed && <span className="truncate">{name}</span>}
              {!collapsed && section === "my-work" && myWorkCount !== null && (
                <span
                  aria-hidden="true"
                  data-nav-count
                  className="ml-auto rounded-full border border-border bg-surface-2 px-1.5 text-[11px] font-bold"
                >
                  {myWorkCount}
                </span>
              )}
            </a>
          </Tooltip>
          {section === "settings" && current === "settings" && settingsPages && (
            <ul aria-label="Settings pages" className={collapsed ? "ml-1" : "ml-4"}>
              {settingsPages.map(({ icon, label, href, active }) => (
                <li key={href}>
                  <Tooltip tip={label} placement="right" className="block">
                    <a
                      href={href}
                      aria-label={label}
                      aria-current={active ? "page" : undefined}
                      onClick={navigateOnClick(href, onNavigate)}
                      data-selected={active || undefined}
                      className={`${item(collapsed)} ${activeClass}`}
                    >
                      <NavIcon name={icon} />
                      {!collapsed && <span className="truncate">{label}</span>}
                    </a>
                  </Tooltip>
                </li>
              ))}
            </ul>
          )}
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
