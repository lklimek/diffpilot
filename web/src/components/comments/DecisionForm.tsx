import { useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import type { CommentItem, DecideRequest, DecisionBatchRow } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { Kbd } from "../help/Kbd";
import { Tooltip } from "../tooltip/Tooltip";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import { useCardKeys } from "./cardKeys";
import {
  CHOICE_LABEL,
  choicesFor,
  decideBlocked,
  gateOf,
  missingInput,
  needOf,
  plannedReply,
  previewFor,
  recommendedValue,
  requestOf,
  resolveLocked,
  type Choice,
  type EffectsPreviewState,
} from "./decisions";
import { itemId } from "./parts";
import { DecideNote, PlannedEffects, useDraft } from "./PlannedEffects";

/** Hint under "Your comment" (plan §3.2 item 7). */
export const COMMENT_HINT =
  "Saved with the decision and sent to the agent that executes it. Not posted to GitHub: the reply text below is what gets posted.";

const INPUT = "w-full rounded-md border border-muted bg-surface px-2 py-1 text-text";

const NEED_LABEL: Record<NonNullable<ReturnType<typeof needOf>>, string> = {
  reason: "Reason (posted as the public reply)",
  pr: "Existing pull request",
  title: "Title of the new follow-up PR (optional)",
  upstream: 'Upstream issue or PR URL, or "create"',
  todo: "TODO title",
};

interface DecisionFormProps {
  item: CommentItem;
  /** Submits the decision. */
  onDecide: (request: DecideRequest) => void;
  /** Opens the shared Ask chat about this item; absent: Ask is not available (`askUnavailable` says why). */
  onAsk?: () => void;
  askUnavailable?: string;
  onSkip: () => void;
  busy: boolean;
  /** The server's refusal of the last submit. */
  error: string | null;
  storageKey: string;
  /** Extra actions after Skip (none by default). */
  extra?: ReactNode;
  /** The server's plan of the draft (RC16): "What happens next" and the reply text that gets posted. */
  preview: EffectsPreviewState;
  /** The draft to plan whenever it changes; null while an input is missing. */
  onDraft: (rows: DecisionBatchRow[] | null) => void;
}

/**
 * "Your decision": options with the recommended one first "(Recommended)" (RQ7), the input the choice needs,
 * "Resolve this thread" (ticked, neutral; locked for D1 and D8), "Your comment", what happens next with the reply
 * preview (both as the server plans them), Decide / Ask / Skip. Keys: 1–8 choose, Enter decide, a Ask, s Skip.
 * When the item changes while it is open, untouched input follows it; typed input is kept until the reader
 * confirms it against the changed item.
 */
export function DecisionForm(props: DecisionFormProps) {
  const { item, onDecide, onAsk, askUnavailable, onSkip, busy, error, storageKey, extra } = props;
  const id = useId();
  const choices = choicesFor(item);
  const recommended = item.assessment?.recommendation.kind ?? null;
  const [choice, setChoice] = useState<Choice>(choices[0]!);
  const [value, setValue] = useState(() => recommendedValue(item));
  const [resolve, setResolve] = useState(true);
  const [comment, setComment] = useState("");
  const [reply, setReply] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // The revision the input was made against, and whether the reader typed or chose anything.
  const [seen, setSeen] = useState(item.revision);
  const [touched, setTouched] = useState(false);
  if (item.revision !== seen && !touched) {
    setSeen(item.revision);
    setChoice(choices[0]!);
    setValue(recommendedValue(item));
  }
  const changed = item.revision !== seen;
  const touch = () => {
    setTouched(true);
    setProblem(null);
  };
  const valueRef = useRef<HTMLInputElement>(null);
  const radios = useRef<(HTMLInputElement | null)[]>([]);
  const thread = item.kind === "thread";
  const gate = gateOf(item);
  const locked = resolveLocked(choice);
  const need = needOf(choice);
  const outOfScope = choice === "fix_here" && item.assessment?.necessity.verdict === "out";
  const edited = reply?.trim() ? reply : null;
  const requestFor = (revision: number, text: string | null, note: string): DecideRequest => {
    const request = requestOf({ ...item, revision }, { choice, value, resolve, comment: note }, text);
    return outOfScope ? { ...request, override: { type: "necessity_gate" } } : request;
  };
  // The server plans the reply that gets posted (its default, or the edited one); the comment changes no effect.
  const missing = missingInput({ choice, value });
  const draft = missing ? null : [{ item_no: item.no, ...requestFor(item.revision, edited, "") }];
  useDraft(draft, props.onDraft);
  const blocked = decideBlocked(props.preview, draft);
  const effects = previewFor(props.preview, item.no);
  const planned = effects ? plannedReply(effects) : null;
  const shownReply = edited ?? planned;
  // The reply shown is of the previous draft until the server planned this one.
  const updating = props.preview.state === "ready" && props.preview.updating && edited === null;

  const pick = (next: Choice) => {
    touch();
    setChoice(next);
    setReply(null);
    setValue(
      next === recommended
        ? recommendedValue(item)
        : next === "follow_up_pr"
          ? `Follow-up: ${item.title}`
          : "",
    );
  };

  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    if (busy) return;
    if (missing) {
      setProblem(missing);
      valueRef.current?.focus();
      return;
    }
    if (changed) {
      setProblem(`${itemId(item)} changed since you started: check it, then choose "Keep my input".`);
      return;
    }
    if (blocked) return;
    onDecide(requestFor(seen, edited, comment));
  };

  useCardKeys({
    options: choices.length,
    choose: (index) => {
      pick(choices[index]!);
      radios.current[index]?.focus();
    },
    decide: submit,
    ask: onAsk,
    skip: onSkip,
  });

  const resolveNote = locked
    ? choice === "accept"
      ? 'Always resolved: choosing "Accept despite the gate" is your decision to resolve.'
      : "Always resolved: this decision resolves the thread."
    : gate
      ? `Gate item (${gate}): submitting with this ticked is your decision to resolve it. Untick to reply and leave the thread open.`
      : "Untick to reply and leave the thread open.";

  return (
    <form noValidate onSubmit={submit} aria-label={`Decision on ${itemId(item)}`}>
      <CollapsibleSection title="Your decision" storageKey={`${storageKey}:decision`}>
        <fieldset className="m-0 rounded-md border border-border px-3 py-2">
          <legend className="sr-only">Decision on {itemId(item)}</legend>
          <div role="radiogroup" aria-label={`Decision on ${itemId(item)}`} className="grid gap-0.5">
            {choices.map((c, i) => (
              <label
                key={c}
                className={`flex min-h-6 cursor-pointer items-start gap-2 py-0.5 ${c === "accept" ? "mt-1 border-t border-dashed border-border pt-1.5" : ""}`}
              >
                <input
                  ref={(el) => {
                    radios.current[i] = el;
                  }}
                  type="radio"
                  name={`${id}-choice`}
                  value={c}
                  checked={choice === c}
                  onChange={() => pick(c)}
                  className="mt-1"
                />
                <span aria-hidden="true" className="min-w-3 font-mono text-muted">
                  {i + 1}
                </span>
                <span>
                  {CHOICE_LABEL[c]}
                  {c === recommended && " "}
                  {c === recommended && (
                    <span className="ml-1 rounded border border-ok px-1 text-[11px] font-bold text-ok">
                      (Recommended)
                    </span>
                  )}
                  {c === "accept" && (
                    <span className="text-xs text-muted">
                      {" "}
                      — reject with a gate override; reason required and posted as the reply; the thread is
                      always resolved
                    </span>
                  )}
                </span>
              </label>
            ))}
          </div>
          {need && (
            <div className="mt-2">
              <label htmlFor={`${id}-value`} className="mb-0.5 block text-xs font-semibold">
                {NEED_LABEL[need]}
              </label>
              <input
                ref={valueRef}
                id={`${id}-value`}
                type="text"
                value={value}
                aria-invalid={problem !== null || undefined}
                aria-describedby={need === "title" ? `${id}-value-d` : undefined}
                inputMode={need === "pr" ? "numeric" : undefined}
                onChange={(e) => {
                  touch();
                  setValue(e.target.value);
                }}
                className={`${INPUT} ${need === "pr" ? "max-w-40" : ""}`}
              />
              {need === "title" && (
                <p id={`${id}-value-d`} className="mt-0.5 text-xs text-muted">
                  The agent creates the PR; no link is needed, it does not exist yet.
                </p>
              )}
            </div>
          )}
          {outOfScope && (
            <p className="mt-1.5 text-xs text-muted">
              Recorded with a scope override: the LLM says this is out of scope for this pull request.
            </p>
          )}
          {thread ? (
            <div className="mt-2.5 rounded-md border border-border bg-surface-2 px-2.5 py-1.5">
              <label className="flex min-h-6 cursor-pointer items-center gap-2">
                <input
                  type="checkbox"
                  checked={locked || resolve}
                  disabled={locked}
                  aria-describedby={`${id}-resolve-d`}
                  onChange={(e) => {
                    touch();
                    setResolve(e.target.checked);
                  }}
                />
                Resolve this thread
              </label>
              <p id={`${id}-resolve-d`} className="m-0 text-xs text-muted">
                {resolveNote}
              </p>
            </div>
          ) : (
            <p className="mt-2 text-xs text-muted">
              No thread to resolve: the reply goes into one PR-level comment per review.
            </p>
          )}
          <label htmlFor={`${id}-comment`} className="mt-2 mb-0.5 block text-xs font-semibold">
            Your comment (optional)
          </label>
          <textarea
            id={`${id}-comment`}
            rows={2}
            value={comment}
            aria-describedby={`${id}-comment-d`}
            placeholder="e.g. keep the test small; touch only retry.rs"
            onChange={(e) => {
              touch();
              setComment(e.target.value);
            }}
            className={INPUT}
          />
          <p id={`${id}-comment-d`} className="mt-0.5 text-xs text-muted">
            {COMMENT_HINT}
          </p>
        </fieldset>
      </CollapsibleSection>
      <CollapsibleSection title="What happens next" storageKey={`${storageKey}:effects`}>
        <PlannedEffects
          preview={props.preview}
          nos={[item.no]}
          idle={
            missing ? "Fill in what this choice needs; then its effects show here." : "Planning what happens…"
          }
        >
          {!locked && thread && !resolve && <li>Thread stays open (not ticked).</li>}
          {choice === "accept" && gate && <li>Recorded as Reject with a gate override ({gate}).</li>}
          {comment.trim() && (
            <li>The executing agent receives your comment with the decision; it is not posted.</li>
          )}
        </PlannedEffects>
        {shownReply !== null && (
          <>
            <label htmlFor={`${id}-reply`} className="mt-2 mb-0.5 block text-xs font-semibold">
              Reply text (public; posted by the external agent that claims this decision)
            </label>
            <textarea
              id={`${id}-reply`}
              rows={2}
              value={reply ?? shownReply}
              onChange={(e) => {
                touch();
                setReply(e.target.value);
              }}
              className={INPUT}
            />
            <div
              role="group"
              aria-label="Preview of the reply on GitHub"
              aria-busy={updating || undefined}
              className={`mt-2 rounded-md border border-border bg-surface-2 px-2.5 py-2 text-[13px] ${updating ? "opacity-60" : ""}`}
            >
              <div className="mb-1 flex items-center gap-1.5 text-xs">
                <span className="rounded-full border border-border px-2">agent&apos;s GitHub account</span>
                <span className="text-muted">preview on GitHub</span>
              </div>
              <p className="m-0 whitespace-pre-wrap">{shownReply.trim()}</p>
            </div>
          </>
        )}
      </CollapsibleSection>
      {changed && (
        <div role="status" className="mt-1.5 flex flex-wrap items-center gap-2 text-sm text-med">
          {itemId(item)} changed since you started (a new read or assessment). Your input is kept: check the
          comment above, then
          <Tooltip tip="Keeps what you chose and typed for the changed comment; Decide sends it.">
            <button
              type="button"
              onClick={() => setSeen(item.revision)}
              className={buttonClass("plain", true)}
            >
              Keep my input
            </button>
          </Tooltip>
        </div>
      )}
      {(problem ?? error) && (
        <p role="alert" className="mt-1.5 text-sm text-high">
          {problem ?? error}
        </p>
      )}
      <DecideNote id={`${id}-blocked`} reason={blocked} />
      <div className="mt-2.5 flex flex-wrap gap-2">
        <Tooltip tip="Stores the decision and opens the next question (Enter).">
          <button
            type="submit"
            aria-disabled={busy || blocked !== null || undefined}
            aria-describedby={blocked ? `${id}-blocked` : undefined}
            className={buttonClass("primary")}
          >
            Decide <Kbd decorative>Enter</Kbd>
          </button>
        </Tooltip>
        <Tooltip
          tip={
            onAsk
              ? "Opens your Ask chat about this comment (a). Nothing is sent until you press Ask."
              : (askUnavailable ?? "Ask is not available here.")
          }
        >
          <button
            type="button"
            aria-disabled={!onAsk || undefined}
            onClick={() => onAsk?.()}
            className={buttonClass("plain")}
          >
            Ask about this comment <Kbd decorative>a</Kbd>
          </button>
        </Tooltip>
        <Tooltip tip="Keeps this question undecided and opens the next one (s).">
          <button type="button" onClick={onSkip} className={buttonClass("plain")}>
            Skip for now <Kbd decorative>s</Kbd>
          </button>
        </Tooltip>
        {extra}
      </div>
    </form>
  );
}
