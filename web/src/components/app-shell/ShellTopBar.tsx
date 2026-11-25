import type { ReactNode } from "react";
import { Tooltip } from "../tooltip/Tooltip";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";

interface ShellTopBarProps {
  /** Name of the page after the brand (`My work`). */
  crumb?: string;
  /** In-app href of the start page (the brand link). */
  homeHref: string;
  userMenu?: ReactNode;
  onNavigate?: Navigate;
  /** Page actions at the right, e.g. the refresh control. */
  children?: ReactNode;
  /** Id of the main region (skip link target). */
  mainId?: string;
}

/** Slim top bar of the shell (mockup `.topbar`): skip link, brand, page name, page actions, help, user. */
export function ShellTopBar({
  crumb,
  homeHref,
  userMenu,
  onNavigate,
  children,
  mainId = "main",
}: ShellTopBarProps) {
  return (
    <header className="sticky top-0 z-30 flex h-12 flex-none items-center gap-2 border-b border-border bg-surface px-3.5 whitespace-nowrap sm:gap-3">
      <a
        href={`#${mainId}`}
        className="absolute -left-[999px] top-0 z-[100] bg-surface px-2.5 py-1.5 focus:left-2 focus:top-2"
      >
        Skip to content
      </a>
      <nav aria-label="Breadcrumb" className="flex items-center gap-1.5">
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
          <>
            <span aria-hidden className="hidden text-muted sm:inline">
              ›
            </span>
            <span aria-current="page" className="sr-only text-[13px] text-muted sm:not-sr-only">
              {crumb}
            </span>
          </>
        )}
      </nav>
      <span className="flex-1" />
      {children}
      {userMenu}
    </header>
  );
}
