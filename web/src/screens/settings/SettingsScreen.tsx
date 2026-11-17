import { useState } from "react";
import { ApiError } from "../../api/client";
import { routes } from "../../api/generated/routes";
import type { RepoRef } from "../../api/generated/types";
import { useMe, useRepoSettings, useSetLanguage, useSettings } from "../../api/queries";
import { AppInstallationsCard } from "../../components/settings/AppInstallationsCard";
import { LanguageCard } from "../../components/settings/LanguageCard";
import { LlmStatusCard } from "../../components/settings/LlmStatusCard";
import { RepoList } from "../../components/settings/RepoList";
import { RepoSettingsCard } from "../../components/settings/RepoSettingsCard";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { TopBar } from "../../components/top-bar/TopBar";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Reload the page.";

function RepoSection({ repos }: { repos: RepoRef[] }) {
  const [chosen, setChosen] = useState<RepoRef | undefined>(undefined);
  const selected = chosen ?? repos[0];
  const settings = useRepoSettings(selected);
  if (!selected) {
    return (
      <StatusMessage kind="empty">
        No pull request has been analysed yet, so no repository settings exist.
      </StatusMessage>
    );
  }
  return (
    <div className="grid content-start gap-2">
      {repos.length > 1 && <RepoList repos={repos} selected={selected} onSelect={setChosen} />}
      {settings.isPending && <StatusMessage kind="loading">Loading the repository settings…</StatusMessage>}
      {settings.isError &&
        (settings.error instanceof ApiError && settings.error.code === "analysis_not_finished" ? (
          // Not known yet is an expected state before the first analysis, not a failure.
          <StatusMessage kind="empty">{settings.error.message}</StatusMessage>
        ) : (
          <StatusMessage kind="error">{errorText(settings.error)}</StatusMessage>
        ))}
      {settings.data && <RepoSettingsCard settings={settings.data} />}
    </div>
  );
}

function SettingsContent() {
  const settings = useSettings();
  const setLanguage = useSetLanguage();
  if (settings.isPending) return <StatusMessage kind="loading">Loading the settings…</StatusMessage>;
  if (settings.isError) return <StatusMessage kind="error">{errorText(settings.error)}</StatusMessage>;
  const { llm_status, language, app_installations, repos } = settings.data;
  return (
    <>
      <div className="grid gap-4 [grid-template-columns:repeat(auto-fit,minmax(420px,1fr))]">
        <LlmStatusCard status={llm_status} />
        <LanguageCard
          language={language}
          busy={setLanguage.isPending}
          onChange={(l) => setLanguage.mutate(l)}
        />
        <AppInstallationsCard installations={app_installations} signInHref={routes.authLogin.path} />
        <RepoSection repos={repos} />
      </div>
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
        {user && <SettingsContent />}
      </main>
    </div>
  );
}
