import type { ReactNode } from "react";
import { Tooltip } from "../tooltip/Tooltip";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";

interface ShellTopBarProps {
  /** Name of the page after the brand (`My work`). */
  crumb?: string;
  /** In-app href of the start page (the brand link). */
  homeHref: string;
  /** In-app href of Settings (the login link). */
  settingsHref: string;
  /** Signed-in GitHub login; null when signed out. */
  login: string | null;
  /** Opens the page's keyboard help; no button without it. */
  onHelp?: () => void;
  onNavigate?: Navigate;
  /** Page actions at the right, e.g. the refresh control. */
  children?: ReactNode;
  /** Id of the main region (skip link target). */
  mainId?: string;
}

const BUTTON =
  "inline-flex h-7 min-w-7 cursor-pointer items-center justify-center rounded-md border border-border bg-surface px-1.5 text-[13px] font-semibold text-text hover:border-accent";

/** Slim top bar of the shell (mockup `.topbar`): skip link, brand, page name, page actions, help, user. */
export function ShellTopBar({
  crumb,
  homeHref,
  settingsHref,
  login,
  onHelp,
  onNavigate,
  children,
  mainId = "main",
}: ShellTopBarProps) {
  return (
    <header className="sticky top-0 z-30 flex h-12 flex-none items-center gap-3 border-b border-border bg-surface px-3.5 whitespace-nowrap">
      <a
        href={`#${mainId}`}
        className="absolute -left-[999px] top-0 z-[100] bg-surface px-2.5 py-1.5 focus:left-2 focus:top-2"
      >
        Skip to content
      </a>
      <Tooltip tip="diffpilot start page: your GitHub work">
        <a
          href={homeHref}
          onClick={navigateOnClick(homeHref, onNavigate)}
          className="text-[15px] font-bold text-text no-underline"
        >
          diff<span className="text-accent">pilot</span>
        </a>
      </Tooltip>
      {crumb && (
        <span aria-hidden className="text-[13px] text-muted">
          / {crumb}
        </span>
      )}
      <span className="flex-1" />
      {children}
      {onHelp && (
        <Tooltip tip="Keyboard shortcuts (?)">
          <button type="button" aria-label="Keyboard shortcuts" onClick={onHelp} className={BUTTON}>
            ?
          </button>
        </Tooltip>
      )}
      {login && (
        <Tooltip tip={`Signed in as ${login} (GitHub). Opens Settings.`}>
          <a
            href={settingsHref}
            aria-label={`Settings, signed in as ${login}`}
            onClick={navigateOnClick(settingsHref, onNavigate)}
            className="text-[13px] text-muted no-underline"
          >
            {login}
          </a>
        </Tooltip>
      )}
    </header>
  );
}
