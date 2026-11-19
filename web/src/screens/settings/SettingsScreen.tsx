import { MemcanStatusLine } from "../../components/section/MemcanStatusLine";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Link, useBlocker, useNavigate, useSearchParams } from "react-router";
import { LlmSelectionCard } from "../../components/settings/LlmSelectionCard";
import { ApiError } from "../../api/client";
import { routes } from "../../api/generated/routes";
import type { RepoRef } from "../../api/generated/types";
import {
  useAccessTokenActions,
  useAccessTokens,
  useDashboardSettings,
  useMe,
  useRepoSettings,
  useSaveDashboardSettings,
  useSetAskEffort,
  useSetJudgeEffort,
  useSetLanguage,
  useSettings,
  useSetLlm,
  useSignOut,
} from "../../api/queries";
import { AccessTokensCard } from "../../components/dashboard/settings-access-tokens/AccessTokensCard";
import { DashboardSettingsCard } from "../../components/dashboard/settings-dashboard/DashboardSettingsCard";
import { UnsavedChangesPrompt } from "../../components/dashboard/settings-dashboard/UnsavedChangesPrompt";
import { JudgeEffortSelect } from "../../components/dashboard/settings-judge-effort/JudgeEffortSelect";
import { AskEffortSelect } from "../../components/settings/AskEffortSelect";
import { AccountCard } from "../../components/settings/AccountCard";
import { AppInstallationsCard } from "../../components/settings/AppInstallationsCard";
import { LanguageCard } from "../../components/settings/LanguageCard";
import { LlmStatusCard } from "../../components/settings/LlmStatusCard";
import { SettingsCard } from "../../components/settings/Card";
import { HelpTip } from "../../components/settings/HelpTip";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { SETTINGS_HELP_PATH } from "../shell/paths";
import { RepoList } from "../../components/settings/RepoList";
import { RepoSettingsCard } from "../../components/settings/RepoSettingsCard";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { clearCommentDrafts } from "../tour/drafts";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Reload the page.";

/** Why a sign-out failed; Sign out everywhere on an ended session revoked nothing and says how to retry. */
const signOutError = (error: unknown, everywhere: boolean) =>
  everywhere && error instanceof ApiError && error.status === 401
    ? "your session had already ended, so nothing was revoked. Other sessions and personal access tokens still work; sign in and choose Sign out everywhere again."
    : errorText(error);

/** Dashboard settings (plan §F.6) and access tokens (ADR-019), each in its own card. */
function DashboardSection({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) {
  const settings = useDashboardSettings();
  const save = useSaveDashboardSettings();
  const tokens = useAccessTokens();
  const tokenActions = useAccessTokenActions();
  return (
    <div className="grid w-full max-w-[1800px] items-start gap-4">
      {settings.data ? (
        <DashboardSettingsCard view={settings.data} onSave={save} onDirtyChange={onDirtyChange} />
      ) : (
        <SettingsCard title="Dashboard">
          {settings.isPending && (
            <StatusMessage kind="loading">Loading the dashboard settings…</StatusMessage>
          )}
          {settings.isError && <StatusMessage kind="error">{errorText(settings.error)}</StatusMessage>}
        </SettingsCard>
      )}
      <div className="max-w-[1100px]">
        {tokens.data ? (
          <AccessTokensCard
            tokens={tokens.data}
            onCreate={tokenActions.create}
            onRevoke={tokenActions.revoke}
          />
        ) : (
          <SettingsCard title="Access tokens">
            {tokens.isPending && <StatusMessage kind="loading">Loading your access tokens…</StatusMessage>}
            {tokens.isError && <StatusMessage kind="error">{errorText(tokens.error)}</StatusMessage>}
          </SettingsCard>
        )}
      </div>
    </div>
  );
}

function RepoSection({ repos, userId }: { repos: RepoRef[]; userId: string }) {
  const [params, setParams] = useSearchParams();
  const key = `diffpilot:settings:repo:${userId}`;
  let remembered: string | null = null;
  try {
    remembered = localStorage.getItem(key);
  } catch {
    /* Browser storage can be unavailable. */
  }
  const asked = params.get("repo");
  const wanted = asked ?? remembered;
  // An unknown repository named in the URL is said, never replaced by another one; a stale memory is.
  const unknown = asked !== null && !repos.some((r) => `${r.owner}/${r.name}` === asked);
  const selected = unknown ? undefined : (repos.find((r) => `${r.owner}/${r.name}` === wanted) ?? repos[0]);
  const selectedName = selected && `${selected.owner}/${selected.name}`;
  useEffect(() => {
    if (!selectedName) return;
    try {
      localStorage.setItem(key, selectedName);
    } catch {
      /* URL still preserves the selection. */
    }
    if (params.get("repo") !== selectedName)
      setParams(
        (previous) => {
          const next = new URLSearchParams(previous);
          next.set("repo", selectedName);
          return next;
        },
        { replace: true },
      );
  }, [key, selectedName, params, setParams]);
  const setChosen = (repo: RepoRef) =>
    setParams((previous) => {
      const next = new URLSearchParams(previous);
      next.set("repo", `${repo.owner}/${repo.name}`);
      return next;
    });
  const settings = useRepoSettings(selected);
  if (unknown && repos.length > 0) {
    return (
      <div className="grid content-start gap-2">
        <RepoList key={asked} repos={repos} selected={null} onSelect={setChosen} />
        <StatusMessage kind="empty">Repository {asked} not found in this installation.</StatusMessage>
      </div>
    );
  }
  if (!selected) {
    return (
      <StatusMessage kind="empty">
        No pull request has been analysed yet, so no repository settings exist.
      </StatusMessage>
    );
  }
  return (
    <div className="grid content-start gap-2">
      {repos.length > 1 && (
        <RepoList key={selectedName} repos={repos} selected={selected} onSelect={setChosen} />
      )}
      {settings.data ? (
        <RepoSettingsCard settings={settings.data} />
      ) : (
        // Loading, not known yet and failures sit in the repository's card, where its facts would be.
        <SettingsCard
          help={<HelpTip topic="repository_settings" />}
          title={
            <>
              Repository <code>{`${selected.owner}/${selected.name}`}</code>
            </>
          }
        >
          {settings.isPending && (
            <StatusMessage kind="loading">Loading the repository settings…</StatusMessage>
          )}
          {settings.isError &&
            (settings.error instanceof ApiError && settings.error.code === "analysis_not_finished" ? (
              // Not known yet is an expected state before the first analysis, not a failure.
              <StatusMessage kind="empty">{settings.error.message}</StatusMessage>
            ) : (
              <StatusMessage kind="error">{errorText(settings.error)}</StatusMessage>
            ))}
        </SettingsCard>
      )}
    </div>
  );
}

function SettingsContent({
  userId,
  onDirtyChange,
}: {
  userId: string;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const setLlm = useSetLlm();
  const setJudgeEffort = useSetJudgeEffort();
  const setAskEffort = useSetAskEffort();
  const settings = useSettings();
  const setLanguage = useSetLanguage();
  if (settings.isPending) return <StatusMessage kind="loading">Loading the settings…</StatusMessage>;
  if (settings.isError) return <StatusMessage kind="error">{errorText(settings.error)}</StatusMessage>;
  const { llm_status, language, app_installations, repos } = settings.data;
  return (
    <>
      <div className="grid w-full max-w-[1800px] items-start gap-4 lg:grid-cols-2">
        <div className="grid min-w-0 items-start gap-4 min-[1600px]:grid-cols-2">
          {settings.data.llm_selection && (
            <LlmSelectionCard
              // The model choice only: saving the Ask or ranking effort must not reset an unsaved model edit.
              key={`${settings.data.llm_selection.current.model}/${settings.data.llm_selection.current.reasoning_effort}`}
              selection={settings.data.llm_selection}
              busy={setLlm.isPending}
              saved={setLlm.isSuccess}
              error={setLlm.isError ? errorText(setLlm.error) : undefined}
              onSave={(choice) => setLlm.mutate(choice)}
            >
              <AskEffortSelect
                effort={settings.data.llm_selection.ask_effort}
                efforts={settings.data.llm_selection.efforts}
                busy={setAskEffort.isPending}
                saved={setAskEffort.isSuccess}
                error={setAskEffort.isError ? errorText(setAskEffort.error) : undefined}
                onChange={(effort) => setAskEffort.mutate(effort)}
              />
              <JudgeEffortSelect
                effort={settings.data.llm_selection.judge_effort}
                efforts={settings.data.llm_selection.efforts}
                busy={setJudgeEffort.isPending}
                saved={setJudgeEffort.isSuccess}
                error={setJudgeEffort.isError ? errorText(setJudgeEffort.error) : undefined}
                onChange={(effort) => setJudgeEffort.mutate(effort)}
              />
            </LlmSelectionCard>
          )}
          <LlmStatusCard status={llm_status} usage={settings.data.llm_usage} />
          <SettingsCard title="Memory" help={<HelpTip topic="memcan_status" />}>
            <p>
              <MemcanStatusLine status={settings.data.memcan_status} />
            </p>
          </SettingsCard>
          <LanguageCard
            language={language}
            busy={setLanguage.isPending}
            onChange={(l) => setLanguage.mutate(l)}
          />
          <AppInstallationsCard installations={app_installations} signInHref={routes.authLogin.path} />
        </div>
        <RepoSection repos={repos} userId={userId} />
      </div>
      <DashboardSection onDirtyChange={onDirtyChange} />
      {setLanguage.isError && (
        <StatusMessage kind="error">The language was not saved: {errorText(setLanguage.error)}</StatusMessage>
      )}
    </>
  );
}

/** Settings (brief §4.4): LLM status of the last call, language, App installations, read-only repo settings. */
export function SettingsScreen() {
  const me = useMe();
  const user = me.data?.user ?? null;
  const signOut = useSignOut();
  const client = useQueryClient();
  const navigate = useNavigate();
  // Unsaved dashboard settings guard leaving Settings (another screen or unloading the page), not sign-out.
  const [dirty, setDirty] = useState(false);
  const signingOut = useRef(false);
  const blocker = useBlocker(
    ({ currentLocation, nextLocation }) =>
      dirty && !signingOut.current && currentLocation.pathname !== nextLocation.pathname,
  );
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);
  // Sign-out clears this login's local drafts and every cached answer, then shows the signed-out Home.
  const signedOut = (login: string) => {
    signingOut.current = true;
    clearCommentDrafts(login);
    client.clear();
    void navigate("/", { replace: true });
  };
  const leave = (login: string, everywhere: boolean) =>
    signOut.mutate(everywhere, {
      onSuccess: () => signedOut(login),
      // 401: the session had already ended. Sign out counts as done; Sign out everywhere revoked nothing.
      onError: (error) => {
        if (!everywhere && error instanceof ApiError && error.status === 401) signedOut(login);
      },
    });
  return (
    <div className="grid content-start gap-4">
      <div className="flex flex-wrap items-baseline gap-4">
        <h1 className="text-[22px] font-bold">Settings</h1>
        <Tooltip tip="Opens the guide to every option on this page: what it is, when it is used and an example.">
          <Link to={SETTINGS_HELP_PATH} className="text-sm text-accent underline">
            Settings help
          </Link>
        </Tooltip>
      </div>
      {blocker.state === "blocked" && (
        <div className="max-w-[900px]">
          <UnsavedChangesPrompt onStay={blocker.reset} onLeave={blocker.proceed} />
        </div>
      )}
      {me.isPending && <StatusMessage kind="loading">Loading…</StatusMessage>}
      {me.isError && <StatusMessage kind="error">{errorText(me.error)}</StatusMessage>}
      {me.isSuccess && !user && (
        <div className="max-w-[900px]">
          <SignInCard loginHref="/auth/github/login" />
        </div>
      )}
      {user && (
        <div className="max-w-[900px]">
          <AccountCard
            login={user.login}
            busy={signOut.isPending}
            error={signOut.isError ? signOutError(signOut.error, signOut.variables === true) : undefined}
            onSignOut={() => leave(user.login, false)}
            onSignOutEverywhere={() => leave(user.login, true)}
          />
        </div>
      )}
      {user && <SettingsContent userId={user.login} onDirtyChange={setDirty} />}
    </div>
  );
}
