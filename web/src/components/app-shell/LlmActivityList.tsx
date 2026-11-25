import { useId, useState } from "react";
import type { LlmCallOutcome } from "../../api/generated/types";
import type { LlmActivity, LlmJobKind, QueuedLlmJob, RunningLlmJob } from "../../api/provisional/llmActivity";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { clockText } from "../format/time";

const PREVIEW_COUNT = 5;

const KIND: Record<LlmJobKind, string> = {
  analysis: "Analysis",
  flow_analysis: "Flow analysis",
  ranking: "Ranking",
  rejudge: "Re-judge",
  comment_assessment: "Comment assessment",
  ask: "Ask",
};

const APP_ORIGIN = "https://diffpilot.invalid";

function subjectHref(href: string): string | null {
  if (!href.startsWith("/") || !URL.canParse(href, APP_ORIGIN)) return null;
  const url = new URL(href, APP_ORIGIN);
  if (url.origin !== APP_ORIGIN || url.pathname.startsWith("//")) return null;
  return `${url.pathname}${url.search}${url.hash}`;
}

function Jobs({
  title,
  jobs,
  now,
  onNavigate,
}: {
  title: string;
  jobs: (RunningLlmJob | QueuedLlmJob)[];
  now: Date;
  onNavigate?: Navigate;
}) {
  const id = useId();
  const [expanded, setExpanded] = useState(false);
  if (!jobs.length) return null;
  const shown = expanded ? jobs : jobs.slice(0, PREVIEW_COUNT);
  return (
    <div className="grid gap-1">
      <h4 id={`${id}-heading`} className="text-xs font-semibold">
        {title} ({jobs.length})
      </h4>
      <ul id={id} aria-labelledby={`${id}-heading`} className="m-0 divide-y divide-border p-0">
        {shown.map((job) => {
          const running = "since" in job;
          const time = running ? job.since : job.queued_at;
          const subject = job.owner === "self" ? job.subject : null;
          const href = subject ? subjectHref(subject.href) : null;
          return (
            <li key={job.id} className="flex items-baseline gap-2 py-1">
              <span aria-hidden className={running ? "text-low" : "text-muted"}>
                {running ? "◔" : "◌"}
              </span>
              <span className="min-w-0 flex-1">
                {job.owner === "other" ? "1 job of another user" : KIND[job.kind]}
                {subject && (
                  <>
                    {" · "}
                    {href ? (
                      <a
                        href={href}
                        onClick={navigateOnClick(href, onNavigate)}
                        className="inline-flex min-h-6 items-center text-accent underline"
                      >
                        {subject.label}
                      </a>
                    ) : (
                      subject.label
                    )}
                  </>
                )}
                {job.owner === "self" && !running && job.waits_for && (
                  <small className="block text-xs text-muted">{job.waits_for}</small>
                )}
              </span>
              <span className="shrink-0 text-xs text-muted tabular-nums">
                {running ? "since" : "queued"} {clockText(time, now)}
              </span>
            </li>
          );
        })}
      </ul>
      {jobs.length > PREVIEW_COUNT && (
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={id}
          onClick={() => setExpanded(!expanded)}
          className="min-h-6 w-fit cursor-pointer text-xs text-accent underline"
        >
          {expanded ? "Show less" : `Show all (${jobs.length})`}
        </button>
      )}
    </div>
  );
}

interface LlmActivityListProps {
  activity: LlmActivity | null | undefined;
  pausedUntil: string | null;
  lastCall: LlmCallOutcome | null;
  now: Date;
  usageHref: string;
  onNavigate?: Navigate;
}

/** Current LLM jobs and the most recent call, using only server-observed facts. */
export function LlmActivityList({
  activity,
  pausedUntil,
  lastCall,
  now,
  usageHref,
  onNavigate,
}: LlmActivityListProps) {
  const id = useId();
  const failed = lastCall?.status.state === "not_working";
  const count = activity ? activity.running.length + activity.queued.length : 0;
  return (
    <section aria-labelledby={id} className="grid gap-2 border-y border-border px-2.5 py-2">
      <div>
        <h3 id={id} className="font-semibold">
          LLM activity
        </h3>
        {pausedUntil ? (
          <p className="text-med">Paused at the usage limit until {clockText(pausedUntil, now)}.</p>
        ) : failed ? (
          <p className="text-high">LLM not working</p>
        ) : activity && count > 0 ? (
          <p className="text-muted">
            {activity.running.length} running · {activity.queued.length} queued
          </p>
        ) : activity ? (
          <p className="text-muted">Idle</p>
        ) : null}
        {activity === undefined && <p className="text-muted">Loading job activity…</p>}
        {activity === null && <p className="text-muted">Job activity unavailable.</p>}
      </div>
      {activity && (
        <>
          <Jobs title="Running now" jobs={activity.running} now={now} onNavigate={onNavigate} />
          <Jobs title="Queued" jobs={activity.queued} now={now} onNavigate={onNavigate} />
        </>
      )}
      <div className="text-xs text-muted">
        {lastCall ? (
          <p>
            Last LLM call {clockText(lastCall.at, now)} · {failed ? "failed" : "succeeded"}.
          </p>
        ) : (
          <p>Last LLM call outcome unknown.</p>
        )}
        {lastCall?.status.state === "not_working" && <p>{lastCall.status.message}</p>}
        <a
          href={usageHref}
          onClick={navigateOnClick(usageHref, onNavigate)}
          className="inline-flex min-h-6 items-center text-accent underline"
        >
          Usage and limits
        </a>
      </div>
    </section>
  );
}
