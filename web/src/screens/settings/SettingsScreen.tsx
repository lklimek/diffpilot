import { MemcanStatusLine } from "../../components/section/MemcanStatusLine";
import { useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useNavigate, useSearchParams } from "react-router";
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
  useSetJudgeEffort,
  useSetLanguage,
  useSettings,
  useSetLlm,
  useSignOut,
} from "../../api/queries";
import { AccessTokensCard } from "../../components/dashboard/settings-access-tokens/AccessTokensCard";
import { DashboardSettingsCard } from "../../components/dashboard/settings-dashboard/DashboardSettingsCard";
import { JudgeEffortSelect } from "../../components/dashboard/settings-judge-effort/JudgeEffortSelect";
import { AccountCard } from "../../components/settings/AccountCard";
import { AppInstallationsCard } from "../../components/settings/AppInstallationsCard";
import { LanguageCard } from "../../components/settings/LanguageCard";
import { LlmStatusCard } from "../../components/settings/LlmStatusCard";
import { SettingsCard } from "../../components/settings/Card";
import { RepoList } from "../../components/settings/RepoList";
import { RepoSettingsCard } from "../../components/settings/RepoSettingsCard";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { TopBar } from "../../components/top-bar/TopBar";
import { clearCommentDrafts } from "../tour/drafts";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Reload the page.";

/** The server has no such route yet (dashboard dark launch, plan §H.2.2): its block stays hidden. */
const notDeployed = (query: { isError: boolean; error: unknown }) =>
  query.isError && query.error instanceof ApiError && query.error.status === 404;

/** Dashboard settings (plan §F.6) and access tokens (ADR-019), each in its own card. */
function DashboardSection() {
  const settings = useDashboardSettings();
  const save = useSaveDashboardSettings();
  const tokens = useAccessTokens();
  const tokenActions = useAccessTokenActions();
  return (
    <div className="grid w-full max-w-[1800px] items-start gap-4">
      {settings.data ? (
        <DashboardSettingsCard view={settings.data} onSave={save} />
      ) : (
        !notDeployed(settings) && (
          <SettingsCard title="Dashboard">
            {settings.isPending && (
              <StatusMessage kind="loading">Loading the dashboard settings…</StatusMessage>
            )}
            {settings.isError && <StatusMessage kind="error">{errorText(settings.error)}</StatusMessage>}
          </SettingsCard>
        )
      )}
      <div className="max-w-[1100px]">
        {tokens.data ? (
          <AccessTokensCard
            tokens={tokens.data}
            onCreate={tokenActions.create}
            onRevoke={tokenActions.revoke}
          />
        ) : (
          !notDeployed(tokens) && (
            <SettingsCard title="Access tokens">
              {tokens.isPending && <StatusMessage kind="loading">Loading your access tokens…</StatusMessage>}
              {tokens.isError && <StatusMessage kind="error">{errorText(tokens.error)}</StatusMessage>}
            </SettingsCard>
          )
        )}
      </div>
    </div>
  );
}

function RepoSection({ repos, userId }: { repos: RepoRef[]; userId: string }) {
  const [params, setParams] = useSearchParams();
  const key = `smart-review:settings:repo:${userId}`;
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

function SettingsContent({ userId }: { userId: string }) {
  const setLlm = useSetLlm();
  const setJudgeEffort = useSetJudgeEffort();
  const dashboard = useDashboardSettings();
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
              key={settings.data.llm_selection.changed_at ?? "default"}
              selection={settings.data.llm_selection}
              busy={setLlm.isPending}
              saved={setLlm.isSuccess}
              error={setLlm.isError ? errorText(setLlm.error) : undefined}
              onSave={(choice) => setLlm.mutate(choice)}
            >
              {/* Shown with the dashboard settings, so not before the server has them (dark launch). */}
              {dashboard.isSuccess && (
                <JudgeEffortSelect
                  effort={settings.data.llm_selection.judge_effort}
                  efforts={settings.data.llm_selection.efforts}
                  busy={setJudgeEffort.isPending}
                  saved={setJudgeEffort.isSuccess}
                  error={setJudgeEffort.isError ? errorText(setJudgeEffort.error) : undefined}
                  onChange={(effort) => setJudgeEffort.mutate(effort)}
                />
              )}
            </LlmSelectionCard>
          )}
          <LlmStatusCard status={llm_status} usage={settings.data.llm_usage} />
          <SettingsCard title="Memory">
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
      <DashboardSection />
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
  // Sign-out clears this login's local drafts and every cached answer, then shows the signed-out Home.
  const signedOut = (login: string) => {
    clearCommentDrafts(login);
    client.clear();
    void navigate("/", { replace: true });
  };
  const leave = (login: string) =>
    signOut.mutate(undefined, {
      onSuccess: () => signedOut(login),
      // 401: the session had already ended, so the user is signed out all the same.
      onError: (error) => {
        if (error instanceof ApiError && error.status === 401) signedOut(login);
      },
    });
  return (
    <div className="flex min-h-screen flex-col">
      <TopBar login={user?.login ?? null} />
      <main id="main" className="grid flex-1 content-start gap-4 px-8 py-5">
        <h1 className="text-[22px] font-bold">Settings</h1>
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
              error={signOut.isError ? errorText(signOut.error) : undefined}
              onSignOut={() => leave(user.login)}
            />
          </div>
        )}
        {user && <SettingsContent userId={user.login} />}
      </main>
    </div>
  );
}
