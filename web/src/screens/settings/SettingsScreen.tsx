import { useEffect, useState } from "react";
import { Link, Navigate, useBlocker, useLocation } from "react-router";
import { useMe } from "../../api/queries";
import { UnsavedChangesPrompt } from "../../components/dashboard/settings-dashboard/UnsavedChangesPrompt";
import { messageOf } from "../../components/inspector/action";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { LLM_USAGE_PATH, SETTINGS_HELP_PATH, SETTINGS_PAGES, type SettingsPage } from "../shell/paths";
import { useShellSlots } from "../shell/ShellRoute";
import { AccountPage, LlmPage, MemcanPage } from "./AccountAndLlm";
import { GitHubSyncPage, RepositoriesPage } from "./GitHubSettings";
import { ReviewLoopPage } from "./ReviewLoopPage";

/** Preserves old deep links, mock scenario parameters and unrelated anchors. */
export function SettingsRedirect() {
  const { search, hash } = useLocation();
  const pathname = new URLSearchParams(search).has("repo")
    ? "/settings/repositories"
    : hash === "#notifications"
      ? "/settings/review-loop"
      : hash === "#llm-usage"
        ? LLM_USAGE_PATH
        : "/settings/account";
  return <Navigate to={{ pathname, search, hash }} replace />;
}

const PAGES = {
  account: AccountPage,
  llm: LlmPage,
  "github-sync": GitHubSyncPage,
  repositories: RepositoriesPage,
  "review-loop": ReviewLoopPage,
  memcan: MemcanPage,
} satisfies Record<SettingsPage, React.ComponentType<{ onDirtyChange: (dirty: boolean) => void }>>;

/** Settings page frame and the leave guard shared by editable sections. */
export function SettingsScreen({ page }: { page: SettingsPage }) {
  const me = useMe();
  const user = me.data?.user;
  const { signOut } = useShellSlots();
  const [dirty, setDirty] = useState(false);
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirty && !signOut.leaving.current && currentLocation.pathname !== nextLocation.pathname,
  );
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);
  const Page = PAGES[page];
  return (
    <div className="@container/settings grid w-full max-w-[1280px] content-start gap-4">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <h1 className="text-[22px] font-bold">{SETTINGS_PAGES.find(({ id }) => id === page)?.label}</h1>
        <Link to={SETTINGS_HELP_PATH} className="text-sm text-accent underline">
          Settings help
        </Link>
      </div>
      {blocker.state === "blocked" && (
        <UnsavedChangesPrompt onStay={blocker.reset} onLeave={blocker.proceed} />
      )}
      {me.isPending && <StatusMessage kind="loading">Loading…</StatusMessage>}
      {me.isError && <StatusMessage kind="error">{messageOf(me.error)}</StatusMessage>}
      {me.isSuccess && !user && <SignInCard loginHref="/auth/github/login" />}
      {user && <Page onDirtyChange={setDirty} />}
    </div>
  );
}
