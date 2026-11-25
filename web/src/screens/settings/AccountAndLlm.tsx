import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAssessmentSettings, useCodexAccount } from "../../api/assessment-settings";
import { fetchMonthlyTokenUsage } from "../../api/provisional/monthlyTokenUsage";
import { fetchRankingUsage, resetRankingUsage } from "../../api/provisional/rankingUsage";
import {
  useAccessTokenActions,
  useAccessTokens,
  useDashboardSettings,
  useMe,
  useSetAskEffort,
  useSetJudgeEffort,
  useSetLanguage,
  useSetLlm,
  useSettings,
} from "../../api/queries";
import { useNow } from "../../app/now";
import { AccessTokensCard } from "../../components/dashboard/settings-access-tokens/AccessTokensCard";
import { messageOf } from "../../components/inspector/action";
import { MemcanStatusLine } from "../../components/section/MemcanStatusLine";
import { AccountCard } from "../../components/settings/AccountCard";
import { CodexAccountCard } from "../../components/settings/CodexAccountCard";
import { LanguageCard } from "../../components/settings/LanguageCard";
import { LlmSelectionCard, type LlmChoiceChanges } from "../../components/settings/LlmSelectionCard";
import { Section } from "../../components/settings/Section";
import { UsageLimitsSection } from "../../components/settings/UsageLimitsSection";
import { StatusMessage } from "../../components/status/StatusMessage";
import { LLM_USAGE_SECTION } from "../shell/paths";
import { useShellSlots } from "../shell/ShellRoute";

export function AccountPage() {
  const user = useMe().data?.user;
  const { signOut } = useShellSlots();
  const now = new Date(useNow());
  const codex = useCodexAccount(now.getTime());
  const tokens = useAccessTokens();
  const tokenActions = useAccessTokenActions();
  const codexError = codex.link.error ?? codex.unlink.error ?? codex.status.error;
  return (
    <>
      {user && (
        <AccountCard
          login={user.login}
          busy={signOut.pending}
          error={signOut.everywhere ? signOut.error : undefined}
          onSignOutEverywhere={() => signOut.leave(true)}
        />
      )}
      <CodexAccountCard
        status={codex.status.data}
        challenge={codex.challenge}
        now={now}
        busy={codex.link.isPending || codex.unlink.isPending}
        error={codexError ? messageOf(codexError) : undefined}
        onLink={() => {
          codex.unlink.reset();
          codex.link.mutate();
        }}
        onUnlink={() => codex.unlink.mutate()}
      />
      {tokens.data ? (
        <AccessTokensCard
          tokens={tokens.data}
          onCreate={tokenActions.create}
          onRevoke={tokenActions.revoke}
        />
      ) : (
        <Section title="Access tokens">
          <StatusMessage kind={tokens.isError ? "error" : "loading"}>
            {tokens.isError ? messageOf(tokens.error) : "Loading your access tokens…"}
          </StatusMessage>
        </Section>
      )}
    </>
  );
}

const rankingKey = ["settings", "ranking-usage-v14"] as const;
export function LlmPage({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) {
  const client = useQueryClient();
  const settings = useSettings();
  const dashboard = useDashboardSettings();
  const assessment = useAssessmentSettings();
  const now = new Date(useNow());
  const codex = useCodexAccount(now.getTime());
  const ranking = useQuery({ queryKey: rankingKey, queryFn: ({ signal }) => fetchRankingUsage(signal) });
  const month = useQuery({
    queryKey: ["monthly-token-usage-v14"],
    queryFn: ({ signal }) => fetchMonthlyTokenUsage(signal),
  });
  const setLlm = useSetLlm();
  const setAskEffort = useSetAskEffort();
  const setJudgeEffort = useSetJudgeEffort();
  const setLanguage = useSetLanguage();
  const save = async ({ model, ask, ranking }: LlmChoiceChanges) => {
    // Each mutation returns the full selection and updates the same cache.
    if (model) await setLlm.mutateAsync(model);
    if (ask) await setAskEffort.mutateAsync(ask);
    if (ranking) await setJudgeEffort.mutateAsync(ranking);
  };
  return (
    <>
      <div id={LLM_USAGE_SECTION} className="scroll-mt-14">
        <UsageLimitsSection
          ranking={ranking.data}
          rankingError={ranking.isError ? messageOf(ranking.error) : undefined}
          rankingZone={dashboard.data?.settings.time_zone ?? undefined}
          onReset={async () => {
            const usage = await resetRankingUsage();
            await client.cancelQueries({ queryKey: rankingKey });
            client.setQueryData(rankingKey, usage);
          }}
          quota={assessment.data?.quota}
          quotaError={assessment.isError ? messageOf(assessment.error) : undefined}
          tokens={settings.data && (settings.data.llm_usage ?? null)}
          tokensError={!settings.data && settings.isError ? messageOf(settings.error) : undefined}
          month={month.data === undefined ? undefined : (month.data?.this_month ?? null)}
          monthError={month.isError ? messageOf(month.error) : undefined}
          codex={codex.status.data}
          now={now}
          lastCall={settings.data?.llm_status}
        />
      </div>
      {settings.data?.llm_selection && (
        <LlmSelectionCard
          selection={settings.data.llm_selection}
          onSave={save}
          onDirtyChange={onDirtyChange}
        />
      )}
      {settings.data && (
        <LanguageCard
          language={settings.data.language}
          busy={setLanguage.isPending}
          onChange={(language) => setLanguage.mutate(language)}
          error={setLanguage.isError ? messageOf(setLanguage.error) : undefined}
        />
      )}
    </>
  );
}

export function MemcanPage() {
  const settings = useSettings();
  return (
    <Section title="MemCan status">
      {!settings.data ? (
        <StatusMessage kind={settings.isError ? "error" : "loading"}>
          {settings.isError ? messageOf(settings.error) : "Loading MemCan status…"}
        </StatusMessage>
      ) : (
        <p>
          <MemcanStatusLine status={settings.data.memcan_status} />
        </p>
      )}
      <p>diffpilot uses MemCan to read TODOs and project profiles.</p>
    </Section>
  );
}
