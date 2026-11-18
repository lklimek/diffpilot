import { useRef } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../../api/client";
import type { AnalysisEstimate, ResolvePrResponse } from "../../api/generated/types";
import { useCreateAnalysis, useMe, useResolvePr, useResumeAnalysis } from "../../api/queries";
import { AnalyzeHelp } from "../../components/analyze-help/AnalyzeHelp";
import { Banner } from "../../components/banner/Banner";
import { busyTip, buttonClass } from "../../components/button/button";
import { PAUSE_REASON, pausedText } from "../../components/progress/paused";
import { PrCheckCard, type PrSize } from "../../components/pr-check/PrCheckCard";
import { PrUrlForm } from "../../components/pr-form/PrUrlForm";
import { SizeWarning } from "../../components/pr-form/SizeWarning";
import { COMMENTING_UNAVAILABLE_WHY } from "../../components/comment/unavailable";
import { SignInCard } from "../../components/sign-in/SignInCard";
import { StatusMessage } from "../../components/status/StatusMessage";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { TopBar } from "../../components/top-bar/TopBar";
import { LANGUAGE_NAME, LanguageTag } from "../../components/language-tag/LanguageTag";
import { analysisHref, homeOutcome, reviewLabel } from "./outcome";

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

// Size facts and the planning estimate of the lookup (server fields not yet in the generated types).
type Sized = ResolvePrResponse & {
  size?: { changed_lines: number; changed_files: number };
  planning_estimate?: AnalysisEstimate | null;
};
// A size warning already states the size; the facts line is for every other PR.
const sizeOf = (resolved: Sized): PrSize | undefined =>
  resolved.size && !resolved.size_warning
    ? { ...resolved.size, estimate: resolved.planning_estimate ?? null }
    : undefined;

/** Home (brief §4.1): sign-in, PR URL, resume, head moved, errors. */
export function HomeScreen() {
  const me = useMe();
  const resolve = useResolvePr();
  const create = useCreateAnalysis();
  const resume = useResumeAnalysis();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const { cancelledPrUrl } = (useLocation().state ?? {}) as HomeState;
  // `/?pr=<url>` (a refused comment's way back to analysing): fills the field, starts nothing.
  const givenPrUrl = params.get("pr") ?? undefined;
  const user = me.data?.user ?? null;
  const loginErrorCode = params.get("login_error");
  const loginError = loginErrorCode === null ? null : (LOGIN_ERRORS[loginErrorCode] ?? LOGIN_FAILED);

  const analyze = (resolved: ResolvePrResponse) =>
    create.mutate(
      { pr: { repo: resolved.pr.repo, number: resolved.pr.number }, head_sha: resolved.pr.head_sha },
      { onSuccess: (analysis) => void navigate(`/analyses/${analysis.id}/progress`) },
    );
  // Check only looks the PR up; nothing starts without Start analysis in the result card.
  const submit = (url: string) => {
    create.reset();
    resume.reset();
    resolve.mutate(url);
  };
  const field = useRef<HTMLInputElement>(null);
  // The card belongs to the checked URL: an edit of the field clears it, so Start never takes another PR.
  // While a start or resume is on its way the card stays: resetting it would drop its navigation.
  const edited = (url: string) => {
    if (create.isPending || resume.isPending) return;
    if (!resolve.isIdle && url !== resolve.variables) {
      resolve.reset();
      create.reset();
      resume.reset();
    }
  };
  const clear = () => {
    resolve.reset();
    create.reset();
    resume.reset();
    field.current?.focus();
  };

  const resolved = resolve.data;
  const outcome = resolved && homeOutcome(resolved);
  const busy = resolve.isPending || create.isPending || resume.isPending;
  // With a size warning or without commenting the start is confirmed against that warning.
  const warned = resolved !== undefined && (resolved.size_warning !== null || !resolved.commenting_available);
  const existing =
    outcome && (outcome.kind === "resume" || outcome.kind === "in_progress" || outcome.kind === "failed")
      ? outcome.analysis
      : null;
  // An existing analysis in another language than the user's setting is offered first; a new one is secondary.
  const otherLanguage = existing && user && existing.language !== user.language ? user.language : null;
  const again = resolved && otherLanguage && (
    <Tooltip
      tip={busyTip(
        `Starts a new analysis of this head with LLM texts in ${LANGUAGE_NAME[otherLanguage]}, your language setting. The ${LANGUAGE_NAME[existing!.language]} one stays.`,
        busy,
      )}
    >
      <button
        type="button"
        aria-disabled={busy || undefined}
        onClick={() => !busy && analyze(resolved)}
        className={buttonClass("plain")}
      >
        {create.isPending ? "Starting…" : `Analyse again in ${LANGUAGE_NAME[otherLanguage]}`}
      </button>
    </Tooltip>
  );
  const start = (describedBy: string) =>
    resolved &&
    (outcome?.kind === "analyze" || outcome?.kind === "head_moved") && (
      <Tooltip
        tip={busyTip(
          outcome.kind === "head_moved"
            ? "Starts a fresh analysis of the current head; ticks are not carried over."
            : "Starts the analysis of this pull request: the plan and the PR context first.",
          busy,
        )}
      >
        <button
          type="button"
          data-primary
          aria-describedby={describedBy}
          aria-disabled={busy || undefined}
          onClick={() => !busy && analyze(resolved)}
          className={buttonClass("primary")}
        >
          {create.isPending ? "Starting…" : warned ? "Start analysis anyway" : "Start analysis"}
        </button>
      </Tooltip>
    );

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
                <PrUrlForm
                  initialUrl={cancelledPrUrl ?? givenPrUrl}
                  busy={busy}
                  checking={resolve.isPending}
                  inputRef={field}
                  onSubmit={submit}
                  onChange={edited}
                />
                {cancelledPrUrl && resolve.isIdle && (
                  <Banner tone="info" title="Analysis cancelled. Check the PR to start again.">
                    Its partial results were discarded.
                  </Banner>
                )}
                <div aria-live="polite" className="grid gap-3">
                  {resolve.isError && (
                    <PrCheckCard pr={null} onCancel={clear}>
                      <Banner tone="error" title="Cannot open this pull request.">
                        {errorText(resolve.error)}
                      </Banner>
                    </PrCheckCard>
                  )}
                  {resolved && (
                    <PrCheckCard
                      pr={resolved.pr}
                      size={sizeOf(resolved)}
                      onCancel={clear}
                      actions={(describedBy) => (
                        <>
                          {start(describedBy)}
                          {again}
                        </>
                      )}
                    >
                      {resolved.size_warning && <SizeWarning warning={resolved.size_warning} />}
                      {resolved && !resolved.commenting_available && (
                        <Banner
                          tone="warn"
                          title={`Commenting is not available for ${resolved.pr.repo.owner}/${resolved.pr.repo.name} right now.`}
                        >
                          {COMMENTING_UNAVAILABLE_WHY} The analysis still runs; comments cannot be written
                          into your pending review while this lasts. The installations are listed in{" "}
                          <Link to="/settings">Settings</Link>.
                        </Banner>
                      )}
                      {outcome?.kind === "resume" && outcome.analysis.state.state === "finished" && (
                        <Banner
                          tone="info"
                          title="Analysis found."
                          actions={
                            <Tooltip
                              tip={
                                outcome.analysis.has_my_state === true
                                  ? "Returns to your review of this analysis where you left it."
                                  : "Opens the review of this analysis."
                              }
                            >
                              <Link
                                to={analysisHref(outcome.analysis)}
                                data-primary
                                className={SMALL_PRIMARY}
                              >
                                {reviewLabel(outcome.analysis)}
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
                      {outcome?.kind === "in_progress" && (
                        <Banner
                          tone="info"
                          title="An analysis of this PR is in progress."
                          actions={
                            <>
                              {outcome.analysis.state.state === "paused" &&
                                outcome.analysis.state.reason === "user" && (
                                  <Tooltip
                                    tip={busyTip("Continues this analysis with its saved results.", busy)}
                                  >
                                    <button
                                      type="button"
                                      aria-disabled={busy || undefined}
                                      className={SMALL_PRIMARY}
                                      onClick={() =>
                                        !busy &&
                                        resume.mutate(outcome.analysis.id, {
                                          onSuccess: (a) => void navigate(analysisHref(a)),
                                        })
                                      }
                                    >
                                      Resume
                                    </button>
                                  </Tooltip>
                                )}
                              <Tooltip
                                tip={
                                  outcome.analysis.review_ready
                                    ? "Opens accepted results while the remaining flows are analysed."
                                    : "Opens the progress of the running analysis."
                                }
                              >
                                <Link
                                  to={analysisHref(outcome.analysis)}
                                  data-primary
                                  className={SMALL_PRIMARY}
                                >
                                  {outcome.analysis.review_ready
                                    ? reviewLabel(outcome.analysis)
                                    : "Show progress"}
                                </Link>
                              </Tooltip>
                            </>
                          }
                        >
                          <LanguageTag language={outcome.analysis.language} />{" "}
                          {outcome.analysis.state.state === "paused" &&
                            `${pausedText(outcome.analysis.state)}${outcome.analysis.state.reason === "user" ? "" : `. ${PAUSE_REASON}`} `}
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
                              <Link
                                to={analysisHref(outcome.analysis)}
                                data-primary
                                className={SMALL_PRIMARY}
                              >
                                {outcome.analysis.state.state === "failed" &&
                                outcome.analysis.state.reason === "configuration"
                                  ? "Show details"
                                  : "Show details and retry"}
                              </Link>
                            </Tooltip>
                          }
                        >
                          <LanguageTag language={outcome.analysis.language} /> {outcome.message}
                        </Banner>
                      )}
                      {outcome?.kind === "head_moved" && resolved && (
                        <Banner
                          tone="warn"
                          title="The PR has moved."
                          actions={
                            <Tooltip tip="Opens the analysis of the older head.">
                              <Link
                                to={analysisHref(outcome.analysis)}
                                className={buttonClass("plain", true)}
                              >
                                Open the old analysis
                              </Link>
                            </Tooltip>
                          }
                        >
                          <LanguageTag language={outcome.analysis.language} /> This analysis is for{" "}
                          {short(outcome.analysis.pr.head_sha)}; the PR is now at{" "}
                          {short(resolved.pr.head_sha)}. Start analysis starts a fresh analysis; ticks are not
                          carried over.
                        </Banner>
                      )}
                      {resume.isError && (
                        <StatusMessage kind="error">{errorText(resume.error)}</StatusMessage>
                      )}
                      {create.isError && (
                        <Banner tone="error" title="Cannot start the analysis.">
                          {errorText(create.error)}
                        </Banner>
                      )}
                    </PrCheckCard>
                  )}
                </div>
              </>
            )}
          </div>
          {user && <AnalyzeHelp settingsHref="/settings" />}
        </div>
      </main>
    </div>
  );
}
