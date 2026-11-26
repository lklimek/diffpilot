import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useAssessmentSettings, useCodexAccount } from "../../api/assessment-settings";
import { api, errorText } from "../../api/client";
import {
  useAccessTokenActions,
  useAccessTokens,
  rankingUsageQuery,
  useDashboardSettings,
  useMe,
  useSetAskEffort,
  useSetJudgeEffort,
  useSetLlmJobSlots,
  useSetLanguage,
  useSetLlm,
  useSettings,
} from "../../api/queries";
import { useNow } from "../../app/now";
import { AccessTokensCard } from "../../components/dashboard/settings-access-tokens/AccessTokensCard";
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
        error={codexError ? errorText(codexError) : undefined}
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
            {tokens.isError ? errorText(tokens.error) : "Loading your access tokens…"}
          </StatusMessage>
        </Section>
      )}
    </>
  );
}

export function LlmPage({ onDirtyChange }: { onDirtyChange: (dirty: boolean) => void }) {
  const client = useQueryClient();
  const settings = useSettings();
  const dashboard = useDashboardSettings();
  const assessment = useAssessmentSettings();
  const now = new Date(useNow());
  const codex = useCodexAccount(now.getTime());
  const ranking = useQuery(rankingUsageQuery);
  const setLlm = useSetLlm();
  const setAskEffort = useSetAskEffort();
  const setJudgeEffort = useSetJudgeEffort();
  const setLanguage = useSetLanguage();
  const setSlots = useSetLlmJobSlots();
  const sessionScope = settings.sessionScope;
  const save = async (sessionScope: number, { model, ask, ranking, slots }: LlmChoiceChanges) => {
    if (model) await setLlm.mutateAsync({ value: model, sessionScope });
    if (ask) await setAskEffort.mutateAsync({ value: ask, sessionScope });
    if (ranking) await setJudgeEffort.mutateAsync({ value: ranking, sessionScope });
    if (slots !== undefined) await setSlots.mutateAsync({ value: slots, sessionScope });
  };
  return (
    <>
      <div id={LLM_USAGE_SECTION} className="scroll-mt-14">
        <UsageLimitsSection
          ranking={ranking.data}
          rankingError={ranking.isError ? errorText(ranking.error) : undefined}
          rankingZone={dashboard.data?.settings.time_zone ?? undefined}
          onReset={async () => {
            const usage = await api.resetRankingUsage();
            await client.cancelQueries({ queryKey: rankingUsageQuery.queryKey });
            client.setQueryData(rankingUsageQuery.queryKey, usage);
          }}
          quota={assessment.data?.quota}
          quotaError={assessment.isError ? errorText(assessment.error) : undefined}
          tokens={settings.data && (settings.data.llm_usage ?? null)}
          tokensError={!settings.data && settings.isError ? errorText(settings.error) : undefined}
          codex={codex.status.data}
          now={now}
          lastCall={settings.data?.llm_status}
        />
      </div>
      {settings.data?.llm_selection && sessionScope !== undefined && (
        <LlmSelectionCard
          key={sessionScope}
          selection={settings.data.llm_selection}
          jobSlots={settings.data.llm_job_slots}
          onSave={(changes) => save(sessionScope, changes)}
          onDirtyChange={onDirtyChange}
        />
      )}
      {settings.data && (
        <LanguageCard
          language={settings.data.language}
          busy={setLanguage.isPending}
          onChange={(language) => setLanguage.mutate(language)}
          error={setLanguage.isError ? errorText(setLanguage.error) : undefined}
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
          {settings.isError ? errorText(settings.error) : "Loading MemCan status…"}
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
