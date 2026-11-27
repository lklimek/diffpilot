import type { ReactNode } from "react";
import { useNavigate } from "react-router";
import { AppNav, type AppSection, type AppNavChild } from "../../components/app-shell/AppNav";
import { ShellTopBar } from "../../components/app-shell/ShellTopBar";
import { APP_NAV_HREFS, MY_WORK_PATH } from "./paths";

interface ShellLayoutProps {
  /** The section shown. */
  current: AppSection;
  /** Page name after the brand. */
  crumb: string;
  /** Account menu of the signed-in reader. */
  userMenu?: ReactNode;
  /** Unhidden human mentions; no count when null. */
  myWorkCount: number | null;
  /** Page actions in the top bar (the refresh control). */
  actions?: ReactNode;
  settingsPages?: readonly AppNavChild[];
  children: ReactNode;
}

/**
 * The navigation shell (plan §F.2): slim top bar, left navigation and the main region of `/`, `/prs` and
 * `/settings` (`ShellRoute`).
 */
export function ShellLayout({
  current,
  crumb,
  userMenu,
  myWorkCount,
  actions,
  settingsPages,
  children,
}: ShellLayoutProps) {
  const navigate = useNavigate();
  const go = (href: string) => void navigate(href);
  return (
    <div className="flex min-h-screen flex-col">
      <ShellTopBar crumb={crumb} homeHref={MY_WORK_PATH} userMenu={userMenu} onNavigate={go} mainId="main">
        {actions}
      </ShellTopBar>
      <div className="flex flex-1">
        <AppNav
          current={current}
          hrefs={APP_NAV_HREFS}
          myWorkCount={myWorkCount}
          settingsPages={settingsPages}
          onNavigate={go}
        />
        <main id="main" tabIndex={-1} className="min-w-0 flex-1 px-3 py-5 sm:px-6">
          {children}
        </main>
      </div>
    </div>
  );
}
