import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../../api/client";
import type { ResolvePrResponse } from "../../api/generated/types";
import { useCreateAnalysis, useMe, useResolvePr } from "../../api/queries";
import { AnalyzeHelp } from "../../components/analyze-help/AnalyzeHelp";
import { Banner } from "../../components/banner/Banner";
import { busyTip, buttonClass } from "../../components/button/button";
import { PAUSE_REASON, pausedText } from "../../components/progress/paused";
import { PrUrlForm } from "../../components/pr-form/PrUrlForm";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { TopBar } from "../../components/top-bar/TopBar";
import { analysisHref, homeOutcome, startsAtOnce } from "./outcome";

/** Router state of Home after Cancel analysis on the progress screen. */
export interface HomeState {
  /** PR URL of the cancelled analysis (fills the field). */
  cancelledPrUrl?: string;
}

// `/?login_error=<LoginError>` from the OAuth callback; unknown values get the generic text, never echoed.
const LOGIN_ERRORS: Record<string, [string, string]> = {
  login_not_allowed: [
    "This GitHub login is not allowed.",
    "Only the GitHub logins this smart-review instance allows can sign in. Ask its owner to add yours.",
  ],
  oauth_failed: ["Sign-in with GitHub did not complete.", "GitHub did not confirm the sign-in. Try again."],
  state_mismatch: ["The sign-in expired.", "It took too long or was started in another tab. Sign in again."],
};
const LOGIN_FAILED: [string, string] = ["Sign-in failed.", "Try again."];

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";
const short = (sha: string) => <code>{sha.slice(0, 7)}</code>;
const SMALL_PRIMARY = buttonClass("primary", true);

/** Home (brief §4.1): sign-in, PR URL, resume, head moved, errors. */
export function HomeScreen() {
  const me = useMe();
  const resolve = useResolvePr();
  const create = useCreateAnalysis();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const { cancelledPrUrl } = (useLocation().state ?? {}) as HomeState;
  const user = me.data?.user ?? null;
  const loginErrorCode = params.get("login_error");
  const loginError = loginErrorCode === null ? null : (LOGIN_ERRORS[loginErrorCode] ?? LOGIN_FAILED);

  const analyze = (resolved: ResolvePrResponse) =>
    create.mutate(
      { pr: { repo: resolved.pr.repo, number: resolved.pr.number }, head_sha: resolved.pr.head_sha },
      { onSuccess: (analysis) => void navigate(`/analyses/${analysis.id}/progress`) },
    );
  const submit = (url: string) => {
    create.reset();
    resolve.mutate(url, {
      onSuccess: (resolved) => {
        if (startsAtOnce(resolved)) analyze(resolved);
      },
    });
  };

  const resolved = resolve.data;
  const outcome = resolved && homeOutcome(resolved);
  const busy = resolve.isPending || create.isPending;

  return (
    <div className="flex min-h-screen flex-col">
      <TopBar login={user?.login ?? null} />
      <main id="main" className="flex-1 px-8 py-5">
        <div className="grid items-start gap-6 min-[1100px]:grid-cols-[minmax(0,2fr)_minmax(300px,1fr)]">
          <div className="grid content-start gap-3">
            {loginError && (
              <Banner tone="error" title={loginError[0]}>
                {loginError[1]}
              </Banner>
            )}
            {me.isPending && <StatusMessage kind="loading">Loading…</StatusMessage>}
            {me.isError && <StatusMessage kind="error">{errorText(me.error)}</StatusMessage>}
            {me.isSuccess && !user && <SignInCard loginHref="/auth/github/login" />}
            {user && (
              <>
                <h1 className="text-[22px] font-bold">Which pull request?</h1>
                <PrUrlForm initialUrl={cancelledPrUrl} busy={busy} onSubmit={submit} />
                {cancelledPrUrl && resolve.isIdle && (
                  <Banner tone="info" title="Analysis cancelled — analyze again.">
                    Its partial results were discarded.
                  </Banner>
                )}
                {resolve.isError && (
                  <Banner tone="error" title="Cannot open this pull request.">
                    {errorText(resolve.error)}
                  </Banner>
                )}
                {create.isError && (
                  <Banner tone="error" title="Cannot start the analysis.">
                    {errorText(create.error)}
                  </Banner>
                )}
                {resolved && !resolved.commenting_available && (
                  <Banner
                    tone="warn"
                    title={`The GitHub App is not installed on ${resolved.pr.repo.owner}/${resolved.pr.repo.name}.`}
                    actions={
                      outcome?.kind === "analyze" && (
                        <Tooltip tip={busyTip("Starts the analysis without the GitHub App.", busy)}>
                          <button
                            type="button"
                            aria-disabled={busy || undefined}
                            onClick={() => !busy && analyze(resolved)}
                            className={SMALL_PRIMARY}
                          >
                            Analyze without it
                          </button>
                        </Tooltip>
                      )
                    }
                  >
                    The analysis works without it, but related work is not checked and comments cannot be
                    written into your pending review. The installations are listed in{" "}
                    <Link to="/settings">Settings</Link>.
                  </Banner>
                )}
                {outcome?.kind === "resume" && outcome.analysis.state.state === "finished" && (
                  <Banner
                    tone="info"
                    title="Analysis found."
                    actions={
                      <Tooltip tip="Opens this analysis; any saved progress is restored.">
                        <Link to={analysisHref(outcome.analysis)} className={SMALL_PRIMARY}>
                          Open review
                        </Link>
                      </Tooltip>
                    }
                  >
                    {outcome.analysis.has_my_state === true
                      ? "This PR head is already analysed. Your ticks, answers and last viewed block are kept."
                      : "This PR head is already analysed. Open review."}
                  </Banner>
                )}
                {outcome?.kind === "in_progress" && (
                  <Banner
                    tone="info"
                    title="An analysis of this PR is in progress."
                    actions={
                      <Tooltip tip="Opens the progress of the running analysis.">
                        <Link to={analysisHref(outcome.analysis)} className={SMALL_PRIMARY}>
                          Show progress
                        </Link>
                      </Tooltip>
                    }
                  >
                    {outcome.analysis.state.state === "paused" &&
                      `${pausedText(outcome.analysis.state)}. ${PAUSE_REASON} `}
                    Nothing new is started.
                  </Banner>
                )}
                {outcome?.kind === "failed" && (
                  <Banner
                    tone="error"
                    title="The last analysis of this PR failed."
                    actions={
                      <Tooltip
                        tip={
                          outcome.analysis.state.state === "failed" &&
                          outcome.analysis.state.reason === "configuration"
                            ? "Opens the configuration problem that needs administrator action."
                            : "Opens the failed analysis, where you can retry it."
                        }
                      >
                        <Link to={analysisHref(outcome.analysis)} className={SMALL_PRIMARY}>
                          {outcome.analysis.state.state === "failed" &&
                          outcome.analysis.state.reason === "configuration"
                            ? "Show details"
                            : "Show details and retry"}
                        </Link>
                      </Tooltip>
                    }
                  >
                    {outcome.message}
                  </Banner>
                )}
                {outcome?.kind === "head_moved" && resolved && (
                  <Banner
                    tone="warn"
                    title="The PR has moved."
                    actions={
                      <>
                        <Tooltip
                          tip={busyTip(
                            "Starts a fresh analysis of the current head; ticks are not carried over.",
                            busy,
                          )}
                        >
                          <button
                            type="button"
                            aria-disabled={busy || undefined}
                            onClick={() => !busy && analyze(resolved)}
                            className={SMALL_PRIMARY}
                          >
                            Re-analyze
                          </button>
                        </Tooltip>
                        <Tooltip tip="Opens the analysis of the older head.">
                          <Link to={analysisHref(outcome.analysis)} className={buttonClass("plain", true)}>
                            Open the old analysis
                          </Link>
                        </Tooltip>
                      </>
                    }
                  >
                    This analysis is for {short(outcome.analysis.pr.head_sha)}; the PR is now at{" "}
                    {short(resolved.pr.head_sha)}. Re-analyzing starts a fresh analysis; ticks are not carried
                    over.
                  </Banner>
                )}
              </>
            )}
          </div>
          {user && <AnalyzeHelp settingsHref="/settings" />}
        </div>
      </main>
    </div>
  );
}
