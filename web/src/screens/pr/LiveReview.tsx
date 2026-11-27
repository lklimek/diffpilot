import type { ReactNode } from "react";
import { errorText } from "../../api/client";
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
import { analysisActivity, useTabActivity } from "../../app/tabStatus";
import { useProgressNotifications } from "../../app/workNotifications";
import { LanguageTag } from "../../components/language-tag/LanguageTag";
import { AnalysisProgress } from "../../components/progress/AnalysisProgress";
import { StatusMessage } from "../../components/status/StatusMessage";

import { analysisPath } from "./paths";

/** Live analysis progress and controls on the PR page. */
export function LiveReview({
  analysisId,
  cancelledAction,
}: {
  analysisId: AnalysisId;
  cancelledAction?: ReactNode;
}) {
  const live = useAnalysisLive(analysisId);
  const cancel = useCancelAnalysis();
  const retry = useRetryAnalysis();
  const pause = usePauseAnalysis();
  const resume = useResumeAnalysis();
  const work = useFlowWork(analysisId);
  const summary = live.summary;
  useTabActivity("analysis", summary ? analysisActivity(summary.state) : null);
  useProgressNotifications(summary);
  const now = useNow();

  const failure = cancel.error ?? retry.error ?? pause.error ?? resume.error ?? work.background.error;
  if (live.error !== undefined) return <StatusMessage kind="error">{errorText(live.error)}</StatusMessage>;
  if (!summary) return <StatusMessage kind="loading">Connecting…</StatusMessage>;
  if (summary.state.state === "cancelled") {
    return (
      <>
        <p id="pr-analysis-cancelled" role="status">
          Analysis cancelled. Its partial results were discarded.
        </p>
        {cancelledAction}
      </>
    );
  }
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
        reviewHref={analysisPath(summary.pr, summary.id)}
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
        onCancel={() => cancel.mutate(analysisId)}
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
