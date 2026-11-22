import { useEffect } from "react";
import { useNavigate } from "react-router";
import { ApiError } from "../../api/client";
import type { AnalysisId } from "../../api/generated/types";
import {
  useAnalysisLive,
  useCancelAnalysis,
  useFlowWork,
  usePauseAnalysis,
  useResumeAnalysis,
  useRetryAnalysis,
} from "../../api/queries";
import { useNow } from "../../app/now";
import { analysisActivity, usePrTabSubject, useTabActivity } from "../../app/tabStatus";
import { useProgressNotifications } from "../../app/workNotifications";
import { LanguageTag } from "../../components/language-tag/LanguageTag";
import { AnalysisProgress } from "../../components/progress/AnalysisProgress";
import { StatusMessage } from "../../components/status/StatusMessage";
import type { HomeState } from "../home/HomeScreen";
import { REVIEW_START_PATH } from "../shell/paths";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** The live progress of the PR's analysis (the old progress page's content); cancelled returns to Review a PR. */
export function LiveReview({ analysisId }: { analysisId: AnalysisId }) {
  const live = useAnalysisLive(analysisId);
  const cancel = useCancelAnalysis();
  const retry = useRetryAnalysis();
  const pause = usePauseAnalysis();
  const resume = useResumeAnalysis();
  const work = useFlowWork(analysisId);
  const navigate = useNavigate();
  const summary = live.summary;
  useTabActivity("analysis", summary ? analysisActivity(summary.state) : null);
  usePrTabSubject(summary?.pr);
  useProgressNotifications(summary);
  const now = useNow();

  const cancelledPrUrl = summary?.state.state === "cancelled" ? summary.pr.html_url : null;
  useEffect(() => {
    if (cancelledPrUrl === null) return;
    const state: HomeState = { cancelledPrUrl };
    void navigate(REVIEW_START_PATH, { replace: true, state });
  }, [cancelledPrUrl, navigate]);

  const failure = cancel.error ?? retry.error ?? pause.error ?? resume.error ?? work.background.error;
  if (live.error !== undefined) return <StatusMessage kind="error">{errorText(live.error)}</StatusMessage>;
  if (!summary) return <StatusMessage kind="loading">Connecting…</StatusMessage>;
  return (
    <div className="grid gap-3">
      <p className="m-0 flex items-center gap-2 text-sm">
        <LanguageTag language={summary.language} /> Analysis at <code>{summary.pr.head_sha.slice(0, 7)}</code>
      </p>
      <AnalysisProgress
        reviewReady={summary.review_ready}
        reviewStarted={summary.has_my_state === true}
        state={summary.state}
        progress={summary.progress}
        usage={summary.usage}
        reviewHref={`/analyses/${summary.id}`}
        busy={
          cancel.isPending ||
          retry.isPending ||
          pause.isPending ||
          resume.isPending ||
          work.background.isPending
        }
        backgroundAnalysis={summary.background_analysis}
        onBackground={(enabled) => work.background.mutate(enabled)}
        onPause={() => pause.mutate(analysisId)}
        onResume={() => resume.mutate(analysisId)}
        onCancel={() =>
          cancel.mutate(analysisId, {
            onSuccess: (cancelled) => {
              const state: HomeState = { cancelledPrUrl: cancelled.pr.html_url };
              void navigate(REVIEW_START_PATH, { replace: true, state });
            },
          })
        }
        onRetry={() => retry.mutate(analysisId)}
        activity={
          live.lastSignalAt !== undefined
            ? {
                now,
                clockOffset: live.clockOffset ?? null,
                lastSignalAt: live.lastSignalAt,
                connected: !live.reconnecting,
              }
            : undefined
        }
      />
      {failure && <StatusMessage kind="error">{errorText(failure)}</StatusMessage>}
    </div>
  );
}
