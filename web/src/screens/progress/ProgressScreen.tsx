import { useEffect } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { ApiError } from "../../api/client";
import {
  useAnalysisLive,
  useCancelAnalysis,
  useMe,
  useRetryAnalysis,
  usePauseAnalysis,
  useResumeAnalysis,
} from "../../api/queries";
import { useNow } from "../../app/now";
import { AnalysisProgress } from "../../components/progress/AnalysisProgress";
import { StatusMessage } from "../../components/status/StatusMessage";
import { TopBar } from "../../components/top-bar/TopBar";
import type { HomeState } from "../home/HomeScreen";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** Analysis progress (brief §4.2), live from the event stream; cancelled returns to Home. */
export function ProgressScreen() {
  const { analysisId = "" } = useParams();
  const me = useMe();
  const live = useAnalysisLive(analysisId);
  const cancel = useCancelAnalysis();
  const retry = useRetryAnalysis();
  const pause = usePauseAnalysis();
  const resume = useResumeAnalysis();
  const navigate = useNavigate();
  const summary = live.summary;
  const now = useNow();

  const cancelledPrUrl = summary?.state.state === "cancelled" ? summary.pr.html_url : null;
  useEffect(() => {
    if (cancelledPrUrl === null) return;
    const state: HomeState = { cancelledPrUrl };
    void navigate("/", { replace: true, state });
  }, [cancelledPrUrl, navigate]);

  const failure = cancel.error ?? retry.error ?? pause.error ?? resume.error;
  return (
    <div className="flex min-h-screen flex-col">
      <TopBar pr={summary?.pr} login={me.data?.user?.login ?? null} />
      <main id="main" className="flex flex-1 flex-col items-center justify-center gap-4 px-8 py-6">
        <h1 className="sr-only">Analysis progress</h1>
        {!summary && live.error === undefined && <StatusMessage kind="loading">Connecting…</StatusMessage>}
        {live.error !== undefined && (
          <>
            <StatusMessage kind="error">{errorText(live.error)}</StatusMessage>
            <Link to="/">Back to Home</Link>
          </>
        )}
        {summary && (
          <AnalysisProgress
            reviewReady={summary.review_ready}
            state={summary.state}
            progress={summary.progress}
            usage={summary.usage}
            reviewHref={`/analyses/${summary.id}`}
            busy={cancel.isPending || retry.isPending || pause.isPending || resume.isPending}
            onPause={() => pause.mutate(analysisId)}
            onResume={() => resume.mutate(analysisId)}
            onCancel={() =>
              cancel.mutate(analysisId, {
                onSuccess: (cancelled) => {
                  const state: HomeState = { cancelledPrUrl: cancelled.pr.html_url };
                  void navigate("/", { replace: true, state });
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
        )}
        {failure && <StatusMessage kind="error">{errorText(failure)}</StatusMessage>}
      </main>
    </div>
  );
}
