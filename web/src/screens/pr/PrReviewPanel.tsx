import { useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router";
import { ApiError } from "../../api/client";
import type { AnalysisId, AnalysisSummary, LlmLanguage, PrRef } from "../../api/generated/types";
import { prCheckKey, useCreateAnalysis, useResolvePr } from "../../api/queries";
import { useTabActivity } from "../../app/tabStatus";
import { Banner } from "../../components/banner/Banner";
import { busyTip, buttonClass } from "../../components/button/button";
import { COMMENTING_UNAVAILABLE_WHY } from "../../components/comment/unavailable";
import { prGithubUrl } from "../../components/github/links";
import { LANGUAGE_OPTION, LanguageTag } from "../../components/language-tag/LanguageTag";
import { PrCheckCard } from "../../components/pr-check/PrCheckCard";
import { SizeWarning } from "../../components/pr-form/SizeWarning";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { GITHUB_SYNC_SETTINGS_PATH } from "../shell/paths";
import { LiveReview } from "./LiveReview";
import { analysisHref, openTip, reviewOutcome } from "./outcome";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

export function PrReviewPanel({
  pr,
  language,
  progressId,
  previous,
}: {
  pr: PrRef;
  language?: LlmLanguage;
  progressId?: AnalysisId;
  previous: AnalysisSummary | null;
}) {
  const url = prGithubUrl(pr);
  const check = useResolvePr(url);
  const start = useCreateAnalysis();
  const client = useQueryClient();
  const result = check.data;
  const outcome = result && reviewOutcome(result);
  useTabActivity("pr-check", check.isPending ? "busy" : check.isError ? "attention" : null);
  if (!result) {
    if (check.isPending) return <StatusMessage kind="loading">Checking the pull request…</StatusMessage>;
    return (
      <>
        <StatusMessage kind="error">Could not check the pull request. {errorText(check.error)}</StatusMessage>
        <Tooltip tip="Checks the pull request again.">
          <button type="button" className={buttonClass("plain", true)} onClick={() => void check.refetch()}>
            Try again
          </button>
        </Tooltip>
        {previous && <LiveReview analysisId={previous.id} />}
      </>
    );
  }

  const moved = outcome?.kind === "head_moved" ? outcome.analysis : undefined;
  const existing = result.analysis;
  const liveId = moved
    ? progressId === moved.id
      ? progressId
      : undefined
    : outcome?.kind === "in_progress" || outcome?.kind === "failed"
      ? outcome.analysis.id
      : progressId;
  const differentLanguage =
    existing && existing.state.state !== "cancelled" && language && existing.language !== language
      ? language
      : null;
  const warned = Boolean(result.size_warning) || !result.commenting_available;
  const analyze = () => {
    if (start.isPending) return;
    void client.cancelQueries({ queryKey: prCheckKey(url) });
    start.mutate(
      { pr, head_sha: result.pr.head_sha },
      {
        onError: (error) => {
          if (error instanceof ApiError && error.code === "pr_head_changed") void check.refetch();
        },
        onSuccess: (analysis) => {
          client.setQueryData(prCheckKey(url), { ...result, analysis, head_moved: false });
          void client.invalidateQueries({ queryKey: ["my-analyses"] });
          void client.invalidateQueries({ queryKey: ["pr-history"] });
          void client.invalidateQueries({ queryKey: ["pr-page", pr.repo.owner, pr.repo.name, pr.number] });
        },
      },
    );
  };

  const startButton = (describedBy: string, label: string, primary: boolean) => (
    <Tooltip
      tip={busyTip(
        `${label}. This starts a fresh analysis of the current pull request head.`,
        start.isPending,
      )}
    >
      <button
        type="button"
        aria-disabled={start.isPending || undefined}
        aria-describedby={describedBy}
        onClick={analyze}
        className={buttonClass(primary ? "primary" : "plain")}
      >
        {start.isPending ? (primary ? "Starting…" : "Analyse again") : label}
      </button>
    </Tooltip>
  );
  const newLabel = moved
    ? `Start analysis of the new head${warned ? " anyway" : ""}`
    : warned
      ? "Start analysis anyway"
      : "Start analysis";

  return (
    <PrCheckCard
      size={result.size_warning ? undefined : (result.size ?? undefined)}
      estimate={result.planning_estimate ?? undefined}
      actions={(describedBy) => (
        <>
          {(moved || (!liveId && outcome?.kind === "analyze")) && startButton(describedBy, newLabel, true)}
          {differentLanguage &&
            !moved &&
            startButton(describedBy, `Analyse again in ${LANGUAGE_OPTION[differentLanguage]}`, false)}
        </>
      )}
    >
      {result.size_warning && <SizeWarning warning={result.size_warning} />}
      {result.commenting_available ? (
        <p className="text-sm text-muted">Commenting available through the GitHub App.</p>
      ) : (
        <Banner
          tone="warn"
          title={`Commenting is not available for ${pr.repo.owner}/${pr.repo.name} right now.`}
        >
          {COMMENTING_UNAVAILABLE_WHY} The analysis still runs; comments cannot be written into your pending
          review while this lasts. The installations are listed in{" "}
          <Link to={GITHUB_SYNC_SETTINGS_PATH}>GitHub sync</Link>.
        </Banner>
      )}
      {moved && (
        <Banner
          tone="warn"
          title="The PR has moved."
          actions={
            <Tooltip tip="Opens the analysis of the older head.">
              <Link to={analysisHref(moved)} className={buttonClass("plain", true)}>
                Open the old analysis
              </Link>
            </Tooltip>
          }
        >
          Analysis of <code>{moved.pr.head_sha.slice(0, 7)}</code>; the PR is now at{" "}
          <code>{result.pr.head_sha.slice(0, 7)}</code>. Your ticks stay with the old analysis.
        </Banner>
      )}
      {liveId && (
        <LiveReview
          analysisId={liveId}
          cancelledAction={
            !moved ? (
              <div className="w-fit">{startButton("pr-analysis-cancelled", newLabel, true)}</div>
            ) : undefined
          }
        />
      )}
      {!liveId && outcome?.kind === "resume" && (
        <Banner
          tone="info"
          title="Analysis found."
          actions={
            <Tooltip tip={openTip(outcome.analysis)}>
              <Link to={analysisHref(outcome.analysis)} className={buttonClass("primary", true)}>
                Open review <span aria-hidden="true">→</span>
              </Link>
            </Tooltip>
          }
        >
          <LanguageTag language={outcome.analysis.language} />{" "}
          {outcome.analysis.has_my_state === true
            ? "This PR head is already analysed. Your ticks, answers and last viewed block are kept."
            : "This PR head is already analysed."}
        </Banner>
      )}
      {!liveId && outcome?.kind === "analyze" && (
        <p className="text-sm">
          {existing?.state.state === "cancelled" ? "Analysis cancelled." : "No analysis yet."}
        </p>
      )}
      {start.isError && (
        <StatusMessage kind="error">Cannot start the analysis. {errorText(start.error)}</StatusMessage>
      )}
    </PrCheckCard>
  );
}
