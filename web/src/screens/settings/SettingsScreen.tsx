import { errorText } from "../../api/client";
import { useEffect, useState } from "react";
import { Link, Navigate, useBlocker, useLocation } from "react-router";
import { useMe } from "../../api/queries";
import { UnsavedChangesPrompt } from "../../components/dashboard/settings-dashboard/UnsavedChangesPrompt";
import { SettingsHelpOrigin, settingsGuideHref } from "../../components/settings/helpOrigin";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import {
  ACCOUNT_SETTINGS_PATH,
  LLM_USAGE_PATH,
  REPOSITORY_SETTINGS_PATH,
  REVIEW_LOOP_SETTINGS_PATH,
  SETTINGS_PAGES,
  type SettingsPage,
} from "../shell/paths";
import { useShellSlots } from "../shell/ShellRoute";
import { AccountPage, LlmPage, MemcanPage } from "./AccountAndLlm";
import { GitHubSyncPage, RepositoriesPage } from "./GitHubSettings";
import { ReviewLoopPage } from "./ReviewLoopPage";

/** Preserves old deep links, mock scenario parameters and unrelated anchors. */
export function SettingsRedirect() {
  const { search, hash } = useLocation();
  const pathname = new URLSearchParams(search).has("repo")
    ? REPOSITORY_SETTINGS_PATH
    : hash === "#notifications"
      ? REVIEW_LOOP_SETTINGS_PATH
      : hash === "#llm-usage"
        ? LLM_USAGE_PATH
        : ACCOUNT_SETTINGS_PATH;
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
  const { search, hash } = useLocation();
  const current = SETTINGS_PAGES.find(({ id }) => id === page)!;
  const origin = current.href + search + hash;
  return (
    <div className="@container/settings grid w-full max-w-[1280px] content-start gap-4">
      <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
        <h1 className="text-[22px] font-bold">{current.label}</h1>
        <Link to={settingsGuideHref(origin)} className="text-sm text-accent underline">
          Settings help
        </Link>
      </div>
      {blocker.state === "blocked" && (
        <UnsavedChangesPrompt onStay={blocker.reset} onLeave={blocker.proceed} />
      )}
      {me.isPending && <StatusMessage kind="loading">Loading…</StatusMessage>}
      {me.isError && <StatusMessage kind="error">{errorText(me.error)}</StatusMessage>}
      {me.isSuccess && !user && <SignInCard loginHref="/auth/github/login" />}
      {user && (
        <SettingsHelpOrigin value={origin}>
          <Page onDirtyChange={setDirty} />
        </SettingsHelpOrigin>
      )}
    </div>
  );
}
