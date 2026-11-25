import { useQuery } from "@tanstack/react-query";
import { useCallback, useState, type ReactNode } from "react";
import { useSearchParams } from "react-router";
import { ApiError } from "../../api/client";
import { routes } from "../../api/generated/routes";
import type { RepoRef } from "../../api/generated/types";
import { fetchGitHubWebhook } from "../../api/provisional/githubWebhook";
import { fetchRepositoryInstallations } from "../../api/provisional/repositoryInstallations";
import {
  useDashboardSettings,
  useRepoSettings,
  useSaveDashboardSettings,
  useSetControlledOwners,
  useSettings,
} from "../../api/queries";
import { DashboardSettingsCard } from "../../components/dashboard/settings-dashboard/DashboardSettingsCard";
import { messageOf } from "../../components/inspector/action";
import { AppInstallationsCard } from "../../components/settings/AppInstallationsCard";
import { ControlledOwnersCard } from "../../components/settings/ControlledOwnersCard";
import { RepoSettingsCard } from "../../components/settings/RepoSettingsCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { useReportDirty } from "./useReportDirty";

function DashboardSettingsStatus({ error }: { error: unknown }) {
  return (
    <StatusMessage kind={error ? "error" : "loading"}>
      {error ? messageOf(error) : "Loading the dashboard settings…"}
    </StatusMessage>
  );
}

export function GitHubSyncPage({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) {
  const dashboard = useDashboardSettings();
  const save = useSaveDashboardSettings();
  const settings = useSettings();
  const owners = useSetControlledOwners();
  const webhook = useQuery({
    queryKey: ["github-webhook-v14"],
    queryFn: ({ signal }) => fetchGitHubWebhook(signal),
  });
  const [dirty, setDirty] = useState({ dashboard: false, owners: false });
  const dashboardDirty = useCallback(
    (value: boolean) => setDirty((current) => ({ ...current, dashboard: value })),
    [],
  );
  const ownersDirty = useCallback(
    (value: boolean) => setDirty((current) => ({ ...current, owners: value })),
    [],
  );
  useReportDirty(dirty.dashboard || dirty.owners, onDirtyChange);
  const app = settings.data ? (
    <AppInstallationsCard
      installations={settings.data.app_installations}
      signInHref={routes.authLogin.path}
      webhookUrl={webhook.isSuccess ? (webhook.data?.webhook_url ?? null) : undefined}
      webhookError={webhook.isError ? messageOf(webhook.error) : undefined}
    />
  ) : (
    <StatusMessage kind={settings.isError ? "error" : "loading"}>
      {settings.isError ? messageOf(settings.error) : "Loading GitHub App settings…"}
    </StatusMessage>
  );
  return (
    <>
      {dashboard.data ? (
        <DashboardSettingsCard
          view={dashboard.data}
          page="github-sync"
          onSave={save}
          onDirtyChange={dashboardDirty}
        >
          {app}
        </DashboardSettingsCard>
      ) : (
        <>
          <DashboardSettingsStatus error={dashboard.error} />
          {app}
        </>
      )}
      {settings.data?.controlled_owners && (
        <ControlledOwnersCard
          owners={settings.data.controlled_owners}
          busy={owners.isPending}
          saved={owners.isSuccess}
          error={owners.isError ? messageOf(owners.error) : undefined}
          onSave={(value) => owners.mutate(value)}
          onDirtyChange={ownersDirty}
        />
      )}
    </>
  );
}

const repoName = ({ owner, name }: RepoRef) => `${owner}/${name}`;

function RepositoryDetails({ repo }: { repo: RepoRef }) {
  const settings = useRepoSettings(repo);
  if (settings.data) return <RepoSettingsCard settings={settings.data} />;
  const error = settings.error;
  if (error instanceof ApiError && error.code === "analysis_not_finished")
    return <StatusMessage kind="empty">{error.message}</StatusMessage>;
  if (error instanceof ApiError && error.code === "not_found")
    return (
      <StatusMessage kind="empty">
        Not known yet. Analysis rules appear after the first analysis.
      </StatusMessage>
    );
  return (
    <StatusMessage kind={settings.isError ? "error" : "loading"}>
      {settings.isError ? messageOf(error) : "Loading the repository settings…"}
    </StatusMessage>
  );
}

export function RepositoriesPage({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) {
  const dashboard = useDashboardSettings();
  const save = useSaveDashboardSettings();
  const settings = useSettings();
  const catalogue = useQuery({
    queryKey: ["repository-installations-v14"],
    queryFn: ({ signal }) => fetchRepositoryInstallations(signal),
  });
  const [params, setParams] = useSearchParams();
  const expanded = params.get("repo");
  const installationStatusKnown = catalogue.data != null || settings.data?.app_installations?.length === 0;
  const installations: { name: string; live: boolean | null }[] =
    catalogue.data?.repositories.map(({ repo, live }) => ({ name: repoName(repo), live })) ??
    settings.data?.repos.map((repo) => ({
      name: repoName(repo),
      live: settings.data?.app_installations?.some(
        (app) => app.all_repositories && app.account.toLowerCase() === repo.owner.toLowerCase(),
      )
        ? true
        : installationStatusKnown
          ? false
          : null,
    })) ??
    [];
  const [owner, name, extra] = expanded?.split("/") ?? [];
  const details: ReactNode =
    owner && name && extra === undefined ? <RepositoryDetails repo={{ owner, name }} /> : null;
  return (
    <>
      {catalogue.isError && <StatusMessage kind="error">{messageOf(catalogue.error)}</StatusMessage>}
      {dashboard.data ? (
        <DashboardSettingsCard
          view={dashboard.data}
          page="repositories"
          onSave={save}
          onDirtyChange={onDirtyChange}
          repositoryOptions={{
            installations,
            installationStatusKnown,
            expanded,
            details,
            onExpand: (repo) =>
              setParams((previous) => {
                const next = new URLSearchParams(previous);
                if (repo) next.set("repo", repo);
                else next.delete("repo");
                return next;
              }),
          }}
        />
      ) : (
        <DashboardSettingsStatus error={dashboard.error} />
      )}
    </>
  );
}
