import { useId, useState } from "react";
import type { Feedback, FeedbackRequest, FeedbackTarget, UserFeedbackValue } from "../../api/generated/types";
import { growField, useAction } from "../inspector/action";
import { Tooltip } from "../tooltip/Tooltip";

/** Props of the feedback buttons in the inspector dock. */
export interface FeedbackButtonsProps {
  /** What the feedback is about (the step, or the selected annotation); null = no step open. */
  target: FeedbackTarget | null;
  /** Short name of the target for labels, e.g. "step 5" or "this annotation". */
  subject: string;
  /** Stored feedback of the analysis (`GET …/feedback`); entries on `target` show as pressed. */
  feedback: readonly Feedback[];
  /** Stores a value with the optional note (POST …/feedback); a rejection shows its message. */
  onSubmit: (request: FeedbackRequest) => Promise<unknown> | void;
}

const VALUES: [UserFeedbackValue, string][] = [
  ["correct", "The remark or rating is right."],
  ["wrong", "The remark is factually wrong."],
  ["irrelevant", "True, but not worth a reviewer's attention."],
  ["overrated", "The risk level is too high. Also saved as a severity calibration example."],
  ["underrated", "The risk level is too low. Also saved as a severity calibration example."],
];

function sameTarget(a: FeedbackTarget, b: FeedbackTarget): boolean {
  if (a.target === "step" && b.target === "step") return a.step_id === b.step_id;
  if (a.target === "annotation" && b.target === "annotation") return a.annotation_id === b.annotation_id;
  if (a.target === "line" && b.target === "line")
    return a.path === b.path && a.side === b.side && a.line === b.line;
  return false;
}

const BUTTON =
  "cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold text-text hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent-bg disabled:cursor-wait disabled:opacity-60";

/** correct / wrong / irrelevant / overrated / underrated with an optional note (brief §3 Feedback). */
export function FeedbackButtons({ target, subject, feedback, onSubmit }: FeedbackButtonsProps) {
  const id = useId();
  const heading = (
    <h3 id={`${id}-h`} className="text-[10.5px] font-bold tracking-wide text-muted uppercase">
      Feedback
    </h3>
  );
  if (target === null) {
    return (
      <section aria-labelledby={`${id}-h`}>
        {heading}
        <p className="text-[12.5px] text-muted">
          Feedback applies to a step or an annotation. Open a step to give feedback on the LLM analysis.
        </p>
      </section>
    );
  }
  // Note draft, error and confirmation belong to one target: another target starts afresh, an old outcome is dropped.
  return (
    <section aria-labelledby={`${id}-h`}>
      {heading}
      <FeedbackForm
        key={JSON.stringify(target)}
        target={target}
        subject={subject}
        feedback={feedback}
        onSubmit={onSubmit}
      />
    </section>
  );
}

function FeedbackForm({
  target,
  subject,
  feedback,
  onSubmit,
}: FeedbackButtonsProps & { target: FeedbackTarget }) {
  const action = useAction();
  const [draft, setDraft] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const mine = feedback.filter((f) => sameTarget(f.target, target));
  const chosen = mine.filter(
    (f): f is Feedback & { value: UserFeedbackValue } => f.value !== "missed_by_llm",
  );
  const newestFirst = [...chosen].sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at));
  const latest = newestFirst[0];
  const storedNote = newestFirst.find((f) => f.note)?.note ?? "";
  const note = draft ?? storedNote;
  const submit = async (value: UserFeedbackValue) => {
    setSaved(null);
    const request: FeedbackRequest = { target, value, note: note.trim() || null };
    if (await action.run(() => onSubmit(request))) setSaved(value);
  };

  return (
    <>
      <div role="group" aria-label={`Feedback on ${subject}`} className="flex flex-wrap gap-1.5">
        {VALUES.map(([value, tip]) => (
          <Tooltip key={value} tip={tip}>
            <button
              type="button"
              aria-pressed={chosen.some((f) => f.value === value)}
              disabled={action.busy}
              onClick={() => void submit(value)}
              className={BUTTON}
            >
              {value}
            </button>
          </Tooltip>
        ))}
      </div>
      {mine.some((f) => f.value === "missed_by_llm") && (
        <p className="mt-1 text-[12px] text-muted">
          Recorded automatically: missed by the LLM (you commented on a changed line that has no LLM remark).
        </p>
      )}
      <div className="mt-1.5 flex items-start gap-2">
        <textarea
          rows={1}
          value={note}
          readOnly={action.busy}
          aria-label="Optional note"
          placeholder="Optional note (why?)"
          onChange={(event) => {
            setDraft(event.target.value);
            growField(event.target);
          }}
          className="min-h-8 flex-1 resize-none rounded-md border border-border bg-surface px-2 py-1 font-sans"
        />
        {latest && note.trim() !== storedNote && (
          <button
            type="button"
            disabled={action.busy}
            onClick={() => void submit(latest.value)}
            className={BUTTON}
          >
            Save note
          </button>
        )}
      </div>
      {action.error && (
        <p role="alert" className="mt-1 text-[12px] text-high">
          Feedback not saved: {action.error}
        </p>
      )}
      {saved && (
        <p role="status" className="mt-1 text-[12px] text-muted">
          Feedback saved: {saved}.
        </p>
      )}
    </>
  );
}
