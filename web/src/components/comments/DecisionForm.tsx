import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import type { CommentItem, DecideRequest } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { Kbd } from "../help/Kbd";
import { Tooltip } from "../tooltip/Tooltip";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import {
  CHOICE_LABEL,
  choicesFor,
  executorName,
  gateOf,
  missingInput,
  needOf,
  plannedEffects,
  recommendedValue,
  replyText,
  requestOf,
  resolveLocked,
  type Choice,
} from "./decisions";
import { itemId } from "./parts";

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

/** A field that takes typed keys (radios and checkboxes leave 1–8, Enter, a and s to the card). */
export const isTypingField = (target: EventTarget | null) =>
  target instanceof HTMLElement &&
  (/^(TEXTAREA|SELECT)$/.test(target.tagName) ||
    target.isContentEditable ||
    (target instanceof HTMLInputElement && !["radio", "checkbox"].includes(target.type)));

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
}

/**
 * "Your decision": options with the recommended one first "(Recommended)" (RQ7), the input the choice needs,
 * "Resolve this thread" (ticked, neutral; locked for D1 and D8), "Your comment", what happens next with the reply
 * preview, Decide / Ask / Skip. Keys: 1–8 choose, Enter decide, a Ask, s Skip.
 */
export function DecisionForm({
  item,
  onDecide,
  onAsk,
  askUnavailable,
  onSkip,
  busy,
  error,
  storageKey,
  extra,
}: DecisionFormProps) {
  const id = useId();
  const choices = choicesFor(item);
  const recommended = item.assessment?.recommendation.kind ?? null;
  const [choice, setChoice] = useState<Choice>(choices[0]!);
  const [value, setValue] = useState(() => recommendedValue(item));
  const [resolve, setResolve] = useState(true);
  const [comment, setComment] = useState("");
  const [reply, setReply] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const valueRef = useRef<HTMLInputElement>(null);
  const radios = useRef<(HTMLInputElement | null)[]>([]);
  const thread = item.kind === "thread";
  const gate = gateOf(item);
  const locked = resolveLocked(choice);
  const need = needOf(choice);
  const template = replyText(choice, value);
  const effects = plannedEffects(choice, resolve, thread);
  const outOfScope = choice === "fix_here" && item.assessment?.necessity.verdict === "out";

  const pick = (next: Choice) => {
    setChoice(next);
    setProblem(null);
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
    const missing = missingInput({ choice, value });
    if (missing) {
      setProblem(missing);
      valueRef.current?.focus();
      return;
    }
    const request = requestOf(item, { choice, value, resolve, comment }, reply);
    onDecide(outOfScope ? { ...request, override: { type: "necessity_gate" } } : request);
  };

  // Keys of the card while focus is not in a field (plan §3.8).
  const latest = useRef({ choices, pick, submit, onAsk, onSkip });
  useEffect(() => {
    latest.current = { choices, pick, submit, onAsk, onSkip };
  });
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (
        event.defaultPrevented ||
        event.ctrlKey ||
        event.metaKey ||
        event.altKey ||
        isTypingField(event.target)
      )
        return;
      if ((event.target as HTMLElement | null)?.closest?.("[data-ask-chat], [role=dialog]")) return;
      const { choices: list, pick: choose, submit: decide, onAsk: ask, onSkip: skip } = latest.current;
      const n = Number(event.key);
      if (Number.isInteger(n) && n >= 1 && n <= Math.min(8, list.length)) {
        event.preventDefault();
        choose(list[n - 1]!);
        radios.current[n - 1]?.focus();
      } else if (event.key === "Enter" && !(event.target instanceof HTMLButtonElement)) {
        event.preventDefault();
        decide();
      } else if (event.key === "a" && ask) {
        event.preventDefault();
        ask();
      } else if (event.key === "s") {
        event.preventDefault();
        skip();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

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
                  setValue(e.target.value);
                  setProblem(null);
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
                  onChange={(e) => setResolve(e.target.checked)}
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
            onChange={(e) => setComment(e.target.value)}
            className={INPUT}
          />
          <p id={`${id}-comment-d`} className="mt-0.5 text-xs text-muted">
            {COMMENT_HINT}
          </p>
        </fieldset>
      </CollapsibleSection>
      <CollapsibleSection title="What happens next" storageKey={`${storageKey}:effects`}>
        <ol className="m-0 grid gap-1 pl-5 text-[13px]">
          {effects.map((effect, i) => (
            <li key={i}>
              <span className="mr-1.5 rounded bg-accent-bg px-1 text-[11px] font-bold text-accent">
                {executorName(effect.executor)}
              </span>
              {effect.text}
            </li>
          ))}
          {!locked && thread && !resolve && <li>Thread stays open (not ticked).</li>}
          {choice === "accept" && gate && <li>Recorded as Reject with a gate override ({gate}).</li>}
          {comment.trim() && (
            <li>The executing agent receives your comment with the decision; it is not posted.</li>
          )}
        </ol>
        {template !== null && (
          <>
            <label htmlFor={`${id}-reply`} className="mt-2 mb-0.5 block text-xs font-semibold">
              Reply text (public; posted by the external agent that claims this decision)
            </label>
            <textarea
              id={`${id}-reply`}
              rows={2}
              value={reply ?? template}
              onChange={(e) => setReply(e.target.value)}
              className={INPUT}
            />
            <div
              role="group"
              aria-label="Preview of the reply on GitHub"
              className="mt-2 rounded-md border border-border bg-surface-2 px-2.5 py-2 text-[13px]"
            >
              <div className="mb-1 flex items-center gap-1.5 text-xs">
                <span className="rounded-full border border-border px-2">agent&apos;s GitHub account</span>
                <span className="text-muted">preview on GitHub</span>
              </div>
              <p className="m-0 whitespace-pre-wrap">{reply ?? template}</p>
            </div>
          </>
        )}
      </CollapsibleSection>
      {(problem ?? error) && (
        <p role="alert" className="mt-1.5 text-sm text-high">
          {problem ?? error}
        </p>
      )}
      <div className="mt-2.5 flex flex-wrap gap-2">
        <Tooltip tip="Stores the decision and opens the next question (Enter).">
          <button type="submit" aria-disabled={busy || undefined} className={buttonClass("primary")}>
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
