import type { ReactNode } from "react";
import { useNavigate } from "react-router";
import { AppNav, type AppSection } from "../../components/app-shell/AppNav";
import { ShellTopBar } from "../../components/app-shell/ShellTopBar";
import { MY_WORK_PATH, REVIEW_START_PATH, SETTINGS_PATH } from "./paths";

const HREFS: Record<AppSection, string> = {
  "my-work": MY_WORK_PATH,
  review: REVIEW_START_PATH,
  settings: SETTINGS_PATH,
};

interface ShellLayoutProps {
  /** The section shown. */
  current: AppSection;
  /** Page name after the brand. */
  crumb: string;
  /** Signed-in GitHub login; null when signed out. */
  login: string | null;
  /** Ranked items of the latest snapshot; no count when null. */
  myWorkCount: number | null;
  /** Opens the page's keyboard help. */
  onHelp?: () => void;
  /** Page actions in the top bar (the refresh control). */
  actions?: ReactNode;
  children: ReactNode;
}

/**
 * The navigation shell (plan §F.2): slim top bar, left navigation and the main region of `/`, `/review` and
 * `/settings` (`ShellRoute`).
 */
export function ShellLayout({
  current,
  crumb,
  login,
  myWorkCount,
  onHelp,
  actions,
  children,
}: ShellLayoutProps) {
  const navigate = useNavigate();
  const go = (href: string) => void navigate(href);
  return (
    <div className="flex min-h-screen flex-col">
      <ShellTopBar
        crumb={crumb}
        homeHref={MY_WORK_PATH}
        settingsHref={SETTINGS_PATH}
        login={login}
        onHelp={onHelp}
        onNavigate={go}
        mainId="main"
      >
        {actions}
      </ShellTopBar>
      <div className="flex flex-1">
        <AppNav current={current} hrefs={HREFS} myWorkCount={myWorkCount} onNavigate={go} />
        <main id="main" tabIndex={-1} className="min-w-0 flex-1 px-6 py-5">
          {children}
        </main>
      </div>
    </div>
  );
}
