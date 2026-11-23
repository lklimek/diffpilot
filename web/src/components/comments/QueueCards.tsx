// The other question kinds of V1 (plan §2.4, §2.6): group, "Decide the rest", PR-level questions, assessment
// running and the empty queue.
import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import type {
  AssessmentProgress,
  CommentItem,
  DecideRest,
  DecisionBatchRow,
  DecideRequest,
  GithubRead,
  LeftOutItem,
} from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { Banner } from "../banner/Banner";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { clockText } from "../format/time";
import { Kbd } from "../help/Kbd";
import { ProvenanceTag } from "../section/ProvenanceTag";
import { Tooltip } from "../tooltip/Tooltip";
import { CollapsibleSection } from "../collapsible/CollapsibleSection";
import { useCardKeys } from "./cardKeys";
import { COMMENT_HINT } from "./DecisionForm";
import {
  CHOICE_LABEL,
  decideBlocked,
  EFFECT_WORD,
  missingInput,
  previewFor,
  recommendedValue,
  requestOf,
  resolveLocked,
  type Choice,
  type EffectsPreviewState,
} from "./decisions";
import { GateChip, itemId, sourceText } from "./parts";
import { DecideNote, PlannedEffects, useDraft } from "./PlannedEffects";

const INPUT = "w-full rounded-md border border-muted bg-surface px-2 py-1 text-text";

/** A row of a batch from an item's recommendation. */
function recommendedRow(
  item: CommentItem,
  resolve: boolean,
  comment: string,
  value?: string,
): DecisionBatchRow {
  const kind = item.assessment!.recommendation.kind;
  return {
    item_no: item.no,
    ...requestOf(item, { choice: kind, value: value ?? recommendedValue(item), resolve, comment }, null),
  };
}

/** The comment on GitHub, in a new tab. */
const OpenItem = ({ item }: { item: CommentItem }) => (
  <OpenExternal href={item.url} label={`Open ${itemId(item)} on GitHub`} />
);

const Heading = ({ id, text }: { id: string; text: string }) => {
  const ref = useRef<HTMLHeadingElement>(null);
  useEffect(() => ref.current?.focus(), [id]);
  return (
    <h2 id={id} ref={ref} tabIndex={-1} className="mt-1 mb-2 text-lg font-semibold">
      {text}
    </h2>
  );
};

interface GroupCardProps {
  /** Two or more non-gate items with the same recommendation. */
  items: CommentItem[];
  position: { index: number; total: number };
  onDecide: (rows: DecisionBatchRow[]) => void;
  /** Takes the item out of the group: it is asked on its own. */
  onDiscussIndividually: (no: number) => void;
  onSkip: () => void;
  busy: boolean;
  error: string | null;
  storageKey: string;
  /** The server's plan of the group's rows (RC16). */
  preview: EffectsPreviewState;
  /** The rows to plan whenever they change; null while an input is missing. */
  onDraft: (rows: DecisionBatchRow[] | null) => void;
}

/**
 * A group question (US5): one decision for every row, "Discuss individually" per row; never gate items. Keys: Enter
 * decide, s Skip, i the first "Discuss individually".
 */
export function GroupCard({
  items,
  position,
  onDecide,
  onDiscussIndividually,
  onSkip,
  busy,
  error,
  storageKey,
  preview,
  onDraft,
}: GroupCardProps) {
  const id = useId();
  const first = items[0]!;
  const rec = first.assessment!.recommendation;
  const [value, setValue] = useState(() => recommendedValue(first));
  const [resolve, setResolve] = useState(true);
  const [comment, setComment] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const discuss = useRef<HTMLButtonElement>(null);
  const label = CHOICE_LABEL[rec.kind] ?? rec.kind;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "i" || /^(INPUT|TEXTAREA|SELECT)$/.test((event.target as HTMLElement).tagName))
        return;
      event.preventDefault();
      discuss.current?.focus();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);
  const missing = missingInput({ choice: rec.kind, value });
  const rows = (text: string) => items.map((item) => recommendedRow(item, resolve, text, value));
  const draft = missing ? null : rows("");
  useDraft(draft, onDraft);
  const blocked = decideBlocked(preview, draft);
  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    if (busy) return;
    if (missing) return setProblem(missing);
    if (blocked) return;
    onDecide(rows(comment));
  };
  useCardKeys({ decide: submit, skip: onSkip });
  return (
    <form
      noValidate
      onSubmit={submit}
      aria-labelledby={`${id}-h`}
      className="rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <p className="m-0 text-xs text-muted">
        Question {position.index} of {position.total} · items with the same recommendation
      </p>
      <Heading
        id={`${id}-h`}
        text={`Same recommendation: ${label} — ${rec.reason} (${items.length} items)`}
      />
      <table className="w-full border-collapse text-[13px]">
        <thead>
          <tr className="text-left text-[11px] text-muted uppercase">
            <th scope="col" className="border-b border-border px-2 py-1">
              ID
            </th>
            <th scope="col" className="border-b border-border px-2 py-1">
              Reviewer
            </th>
            <th scope="col" className="border-b border-border px-2 py-1">
              Comment
            </th>
            <th scope="col" className="border-b border-border px-2 py-1">
              LLM assessment <ProvenanceTag source="llm" focusable={false} />
            </th>
            <th scope="col" className="border-b border-border px-2 py-1">
              <span className="sr-only">Action</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {items.map((item, i) => (
            <tr key={item.id}>
              <td className="border-b border-border px-2 py-1.5 whitespace-nowrap">
                {itemId(item)} <OpenItem item={item} />
              </td>
              <td className="border-b border-border px-2 py-1.5">{sourceText(item)}</td>
              <td className="border-b border-border px-2 py-1.5">{item.title}</td>
              <td className="border-b border-border px-2 py-1.5">{item.assessment?.necessity.reason}</td>
              <td className="border-b border-border px-2 py-1.5">
                <Tooltip tip={`Takes ${itemId(item)} out of this group; it is asked on its own (i).`}>
                  <button
                    ref={i === 0 ? discuss : undefined}
                    type="button"
                    onClick={() => onDiscussIndividually(item.no)}
                    className={buttonClass("plain", true)}
                  >
                    Discuss individually
                  </button>
                </Tooltip>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <CollapsibleSection title="Your decision" storageKey={`${storageKey}:group-decision`}>
        <label htmlFor={`${id}-reason`} className="mb-0.5 block text-xs font-semibold">
          {rec.kind === "reject" ? "Reason for all (posted as each reply)" : "Reason or target for all"}
        </label>
        <input
          id={`${id}-reason`}
          type="text"
          value={value}
          onChange={(e) => (setValue(e.target.value), setProblem(null))}
          className={INPUT}
        />
        <label className="mt-2 flex min-h-6 items-center gap-2 rounded-md border border-border bg-surface-2 px-2.5">
          <input
            type="checkbox"
            checked={resolveLocked(rec.kind) || resolve}
            disabled={resolveLocked(rec.kind)}
            onChange={(e) => setResolve(e.target.checked)}
          />
          Resolve these threads
        </label>
        <label htmlFor={`${id}-comment`} className="mt-2 mb-0.5 block text-xs font-semibold">
          Your comment (optional)
        </label>
        <textarea
          id={`${id}-comment`}
          rows={2}
          value={comment}
          aria-describedby={`${id}-comment-d`}
          onChange={(e) => setComment(e.target.value)}
          className={INPUT}
        />
        <p id={`${id}-comment-d`} className="mt-0.5 text-xs text-muted">
          Saved with each decision and sent to the executing agent. Not posted to GitHub.
        </p>
      </CollapsibleSection>
      <CollapsibleSection title="What happens next, per item" storageKey={`${storageKey}:group-effects`}>
        <PlannedEffects
          preview={preview}
          nos={items.map((item) => item.no)}
          idle="Fill in the reason or target; then the effects show here."
        />
      </CollapsibleSection>
      <p className="mt-2 text-xs text-muted">
        Gate items are never grouped. Each item gets its own decision record.
      </p>
      {(problem ?? error) && (
        <p role="alert" className="text-sm text-high">
          {problem ?? error}
        </p>
      )}
      <DecideNote id={`${id}-blocked`} reason={blocked} preview={preview} />
      <div className="mt-2 flex flex-wrap gap-2">
        <Tooltip tip="Stores one decision per item of this group, each with its own record (Enter).">
          <button
            type="submit"
            aria-disabled={busy || blocked !== null || undefined}
            aria-describedby={blocked ? `${id}-blocked` : undefined}
            className={buttonClass("primary")}
          >
            {label} all {items.length} <Kbd decorative>Enter</Kbd>
          </button>
        </Tooltip>
        <Tooltip tip="Keeps this group undecided and opens the next question (s).">
          <button type="button" onClick={onSkip} className={buttonClass("plain")}>
            Skip for now <Kbd decorative>s</Kbd>
          </button>
        </Tooltip>
      </div>
    </form>
  );
}

interface DecideRestCardProps {
  /** The server's list (RC17): rows with their recommendation and planned effects, and the items left out. */
  rest: DecideRest;
  /** The PR's items by number (titles, links). */
  items: ReadonlyMap<number, CommentItem>;
  onConfirm: (rows: DecisionBatchRow[]) => void;
  onCancel: () => void;
  busy: boolean;
  error: string | null;
  /** The server's plan of the included rows as ticked now (RC16). */
  preview: EffectsPreviewState;
  /** The included rows to plan whenever they change. */
  onDraft: (rows: DecisionBatchRow[] | null) => void;
}

const LEFT_OUT: Record<LeftOutItem["reason"], string> = {
  gate: "gate item",
  asked_again: "asked again: its reason needs a look",
  not_assessed: "not assessed yet",
  incomplete: "the recommendation lacks its reason or target",
};

/** `C14 (gate item G-FUNDS)`, `C21 (asked again: …)`. */
function leftOutText(left: LeftOutItem, item: CommentItem | undefined): string {
  const gates = left.reason === "gate" && item ? ` ${item.gates.map((g) => g.gate).join(", ")}` : "";
  return `${item ? itemId(item) : `C${left.item_no}`} (${LEFT_OUT[left.reason] ?? left.reason}${gates})`;
}

/**
 * "Decide the rest per recommendation" (US7): the server's confirmation list (RC17); nothing is stored before
 * Apply. Every item the server leaves out is named with its reason.
 */
export function DecideRestCard({
  rest,
  items,
  onConfirm,
  onCancel,
  busy,
  error,
  preview,
  onDraft,
}: DecideRestCardProps) {
  const id = useId();
  const [excluded, setExcluded] = useState<ReadonlySet<number>>(new Set());
  const [unresolved, setUnresolved] = useState<ReadonlySet<number>>(new Set());
  const [comment, setComment] = useState("");
  const rowOf = ({ row }: DecideRest["rows"][number], text: string): DecisionBatchRow => ({
    ...row,
    resolve_ticked: resolveLocked(row.kind) || !unresolved.has(row.item_no),
    comment: text.trim() || null,
  });
  const chosen = rest.rows.filter((r) => !excluded.has(r.row.item_no));
  const draft = chosen.map((r) => rowOf(r, ""));
  useDraft(draft, onDraft);
  const blocked = chosen.length === 0 ? null : decideBlocked(preview, draft);
  const toggle = (set: ReadonlySet<number>, no: number) => {
    const next = new Set(set);
    if (!next.delete(no)) next.add(no);
    return next;
  };
  const byKind = new Map<string, number>();
  for (const r of chosen) {
    const label = CHOICE_LABEL[r.recommendation.kind as Choice];
    byKind.set(label, (byKind.get(label) ?? 0) + 1);
  }
  const leftOut = rest.left_out;
  const gateLeftOut = leftOut.some((l) => l.reason === "gate");
  return (
    <section aria-labelledby={`${id}-h`} className="rounded-lg border border-border bg-surface px-4 py-3.5">
      <Heading
        id={`${id}-h`}
        text={`Decide the rest per recommendation — confirm ${chosen.length} decisions`}
      />
      <p>
        Each row becomes its own decision, marked &quot;via decide the rest&quot;. Untick &quot;Include&quot;
        to keep a row as a question. Nothing is stored until you confirm.
      </p>
      <table className="w-full border-collapse text-[13px]">
        <caption className="sr-only">Decisions to confirm</caption>
        <thead>
          <tr className="text-left text-[11px] text-muted uppercase">
            {["Include", "ID", "Comment"].map((h) => (
              <th key={h} scope="col" className="border-b border-border px-2 py-1">
                {h}
              </th>
            ))}
            <th scope="col" className="border-b border-border px-2 py-1">
              Recommendation <ProvenanceTag source="llm" focusable={false} />
            </th>
            <th scope="col" className="border-b border-border px-2 py-1">
              Effects
            </th>
            <th scope="col" className="border-b border-border px-2 py-1">
              Resolve
            </th>
          </tr>
        </thead>
        <tbody>
          {rest.rows.map((r) => {
            const no = r.row.item_no;
            const item = items.get(no);
            const name = item ? itemId(item) : `C${no}`;
            const rec = r.recommendation;
            const effects = (!excluded.has(no) && previewFor(preview, no)) || r.effects;
            const locked = resolveLocked(r.row.kind);
            return (
              <tr key={no}>
                <td className="border-b border-border px-2 py-1">
                  <input
                    type="checkbox"
                    checked={!excluded.has(no)}
                    aria-label={`Include ${name}`}
                    onChange={() => setExcluded(toggle(excluded, no))}
                  />
                </td>
                <td className="border-b border-border px-2 py-1 whitespace-nowrap">
                  {name} {item && <OpenItem item={item} />}
                </td>
                <td className="border-b border-border px-2 py-1">{item?.title}</td>
                <td className="border-b border-border px-2 py-1">
                  {CHOICE_LABEL[rec.kind]}: {rec.reason}
                </td>
                <td className="border-b border-border px-2 py-1">
                  {effects.map((e) => EFFECT_WORD[e.kind] ?? e.kind).join(", ") || "—"}
                </td>
                <td className="border-b border-border px-2 py-1">
                  {item?.kind === "thread" ? (
                    <input
                      type="checkbox"
                      checked={locked || !unresolved.has(no)}
                      disabled={locked}
                      aria-label={`Resolve ${name}`}
                      onChange={() => setUnresolved(toggle(unresolved, no))}
                    />
                  ) : (
                    <span className="text-muted">no thread</span>
                  )}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      <label htmlFor={`${id}-comment`} className="mt-2 mb-0.5 block text-xs font-semibold">
        Your comment for all (optional)
      </label>
      <textarea
        id={`${id}-comment`}
        rows={2}
        value={comment}
        aria-describedby={`${id}-comment-d`}
        onChange={(e) => setComment(e.target.value)}
        className={INPUT}
      />
      <p id={`${id}-comment-d`} className="mt-0.5 text-xs text-muted">
        Saved with each decision and sent to the executing agent. Not posted to GitHub.
      </p>
      {leftOut.length > 0 && (
        <div className="mt-2.5">
          <Banner
            tone="warn"
            title={`Left out: ${leftOut.length} ${leftOut.length === 1 ? "item" : "items"}.`}
          >
            {leftOut.map((l) => leftOutText(l, items.get(l.item_no))).join(", ")} stay single questions
            {gateLeftOut ? "; gate items are never decided in bulk" : ""}.
          </Banner>
        </div>
      )}
      {preview.state === "error" && (
        <p role="alert" className="text-sm text-high">
          {preview.message}
        </p>
      )}
      <p className="mt-2 text-sm" aria-live="polite">
        {chosen.length} decisions: {[...byKind].map(([k, n]) => `${k} ${n}`).join(" · ") || "none"}.
      </p>
      {error && (
        <p role="alert" className="text-sm text-high">
          {error}
        </p>
      )}
      <DecideNote id={`${id}-blocked`} reason={blocked} preview={preview} />
      <div className="flex flex-wrap gap-2">
        <Tooltip tip="Stores one decision per included row, marked via decide the rest.">
          <button
            type="button"
            aria-disabled={busy || chosen.length === 0 || blocked !== null || undefined}
            aria-describedby={blocked ? `${id}-blocked` : undefined}
            onClick={() =>
              !busy && chosen.length > 0 && !blocked && onConfirm(chosen.map((r) => rowOf(r, comment)))
            }
            className={buttonClass("primary")}
          >
            Apply {chosen.length} decisions
          </button>
        </Tooltip>
        <Tooltip tip="Closes the list without storing anything.">
          <button type="button" onClick={onCancel} className={buttonClass("plain")}>
            Cancel
          </button>
        </Tooltip>
      </div>
    </section>
  );
}

interface PrQuestionCardProps {
  item: CommentItem;
  /** The PR on GitHub ("Merge on GitHub"). */
  prUrl: string;
  onDecide: (request: DecideRequest) => void;
  onSkip: () => void;
  busy: boolean;
  error: string | null;
  /** The server's plan of the chosen option (RC16): what happens next and the default dismissal message. */
  preview: EffectsPreviewState;
  /** The decision to plan whenever it changes. */
  onDraft: (rows: DecisionBatchRow[] | null) => void;
}

/**
 * PR-level questions (US11, US12): merge now with follow-ups / another round; dismiss stale reviews / keep them.
 * Keys: 1–2 choose, Enter decide, s Skip.
 */
export function PrQuestionCard({
  item,
  prUrl,
  onDecide,
  onSkip,
  busy,
  error,
  preview,
  onDraft,
}: PrQuestionCardProps) {
  const id = useId();
  const dismiss = item.stable_key.endsWith(":dismiss");
  const options: Choice[] = dismiss
    ? ["dismiss_stale_reviews", "keep_reviews"]
    : ["merge_with_follow_ups", "another_round"];
  const [choice, setChoice] = useState<Choice>(options[0]!);
  // The dismissal message as edited; null shows the server's default (posted when left as is).
  const [message, setMessage] = useState<string | null>(null);
  const [comment, setComment] = useState("");
  const radios = useRef<(HTMLInputElement | null)[]>([]);
  const requestFor = (text: string): DecideRequest => ({
    ...requestOf(item, { choice, value: "", resolve: false, comment: text }, null),
    // The server posts `reason` as the dismissal message (P3); `reply_text` is not read there.
    reason: choice === "dismiss_stale_reviews" ? message?.trim() || null : null,
  });
  const draft = [{ item_no: item.no, ...requestFor("") }];
  useDraft(draft, onDraft);
  const effects = previewFor(preview, item.no);
  const defaultMessage = effects?.find((e) => e.kind === "dismiss_review")?.params.text ?? "";
  // The plan the owner had in view for this draft: one that changes while the card is open (other items were read
  // or decided since) needs the owner's confirmation before Decide sends it.
  const draftKey = JSON.stringify(draft);
  const plan = decideBlocked(preview, draft) === null ? JSON.stringify(effects) : null;
  const [seenPlan, setSeenPlan] = useState<{ draft: string; plan: string } | null>(null);
  if (plan !== null && seenPlan?.draft !== draftKey) setSeenPlan({ draft: draftKey, plan });
  const rescoped = plan !== null && seenPlan?.draft === draftKey && seenPlan.plan !== plan;
  const blocked =
    decideBlocked(preview, draft) ??
    (rescoped ? 'Decide is available once you confirm the new plan ("Use the new plan").' : null);
  const submit = (event?: FormEvent) => {
    event?.preventDefault();
    if (busy || blocked) return;
    onDecide(requestFor(comment));
  };
  useCardKeys({
    options: options.length,
    choose: (index) => {
      setChoice(options[index]!);
      radios.current[index]?.focus();
    },
    decide: submit,
    skip: onSkip,
  });
  return (
    <form
      noValidate
      onSubmit={submit}
      aria-labelledby={`${id}-h`}
      className="rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <p className="m-0 text-xs text-muted">
        PR-level question{item.first_round !== null && ` · asked after bot round ${item.first_round} ended`}
      </p>
      <Heading id={`${id}-h`} text={item.title} />
      <p className="whitespace-pre-wrap">{item.body}</p>
      {dismiss && (
        <>
          <label htmlFor={`${id}-msg`} className="mb-0.5 block text-xs font-semibold">
            Dismissal message (posted)
          </label>
          <input
            id={`${id}-msg`}
            type="text"
            value={message ?? defaultMessage}
            // Left empty, the default message is posted.
            placeholder={defaultMessage}
            onChange={(e) => setMessage(e.target.value)}
            className={INPUT}
          />
        </>
      )}
      <CollapsibleSection title="Your decision" storageKey={`pr-question:${dismiss ? "dismiss" : "merge"}`}>
        <div role="radiogroup" aria-label={`Decision on ${itemId(item)}`} className="grid gap-1">
          {options.map((c, i) => (
            <label key={c} className="flex min-h-6 cursor-pointer items-start gap-2">
              <input
                ref={(el) => {
                  radios.current[i] = el;
                }}
                type="radio"
                name={`${id}-c`}
                checked={choice === c}
                onChange={() => setChoice(c)}
                className="mt-1"
              />
              <span aria-hidden="true" className="font-mono text-muted">
                {i + 1}
              </span>
              <span>
                {CHOICE_LABEL[c]}
                {c === "merge_with_follow_ups" && (
                  <span className="text-xs text-muted">
                    {" "}
                    — an agent creates one follow-up PR listing them, replies with its link and resolves each
                    thread; gate items stay in the queue for their own decision; you merge on GitHub
                  </span>
                )}
                {c === "another_round" && (
                  <span className="text-xs text-muted"> — the items are asked one by one</span>
                )}
                {c === "dismiss_stale_reviews" && (
                  <span className="text-xs text-muted"> — executed by an external agent</span>
                )}
              </span>
            </label>
          ))}
        </div>
        <label htmlFor={`${id}-comment`} className="mt-2 mb-0.5 block text-xs font-semibold">
          Your comment (optional)
        </label>
        <textarea
          id={`${id}-comment`}
          rows={2}
          value={comment}
          aria-describedby={`${id}-comment-d`}
          onChange={(e) => setComment(e.target.value)}
          className={INPUT}
        />
        <p id={`${id}-comment-d`} className="mt-0.5 text-xs text-muted">
          {COMMENT_HINT}
        </p>
      </CollapsibleSection>
      <CollapsibleSection
        title="What happens next"
        storageKey={`pr-question:${dismiss ? "dismiss" : "merge"}:effects`}
      >
        <PlannedEffects preview={preview} nos={[item.no]} several idle="Planning what happens…" />
        {rescoped && (
          <div role="status" className="mt-1.5 flex flex-wrap items-center gap-2 text-sm text-med">
            What this decision acts on changed since you looked (a new read or another decision): check the
            effects above, then
            <Tooltip tip="Takes the effects shown above as the plan this decision sends.">
              <button
                type="button"
                onClick={() => plan !== null && setSeenPlan({ draft: draftKey, plan })}
                className={buttonClass("plain", true)}
              >
                Use the new plan
              </button>
            </Tooltip>
          </div>
        )}
      </CollapsibleSection>
      <p className="text-xs text-muted">
        No recommendation: this is a PR-level choice. Merging stays your own action on GitHub.
      </p>
      {error && (
        <p role="alert" className="text-sm text-high">
          {error}
        </p>
      )}
      <DecideNote id={`${id}-blocked`} reason={blocked} preview={preview} />
      <div className="flex flex-wrap gap-2">
        <Tooltip tip="Stores the decision on this PR-level question (Enter).">
          <button
            type="submit"
            aria-disabled={busy || blocked !== null || undefined}
            aria-describedby={blocked ? `${id}-blocked` : undefined}
            className={buttonClass("primary")}
          >
            Decide <Kbd decorative>Enter</Kbd>
          </button>
        </Tooltip>
        {!dismiss && (
          <Tooltip tip="Opens the pull request on GitHub, where you merge it yourself (new tab).">
            <a href={prUrl} target="_blank" rel="noopener noreferrer" className={buttonClass("plain")}>
              Merge on GitHub<span className="sr-only"> (opens on GitHub in a new tab)</span>
            </a>
          </Tooltip>
        )}
        <Tooltip tip="Keeps this question undecided and opens the next one (s).">
          <button type="button" onClick={onSkip} className={buttonClass("plain")}>
            Skip for now <Kbd decorative>s</Kbd>
          </button>
        </Tooltip>
      </div>
    </form>
  );
}

/** "LLM assessment running: 4 of 9 comments assessed". */
export function AssessmentRunningCard({ progress }: { progress: AssessmentProgress }) {
  return (
    <section
      aria-label="LLM assessment running"
      className="rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <h2 className="mt-0 mb-1.5 text-lg font-semibold">LLM assessment running</h2>
      <p>
        diffpilot&apos;s LLM reads each comment against the PR
        {progress.head && (
          <>
            {" "}
            at <code>{progress.head.slice(0, 7)}</code>
          </>
        )}
        . Questions appear as soon as the job finishes.
      </p>
      <div
        role="progressbar"
        aria-label="Comments assessed"
        aria-valuemin={0}
        aria-valuemax={progress.total}
        aria-valuenow={progress.assessed}
        className="my-1.5 h-2 overflow-hidden rounded border border-border bg-surface-2"
      >
        <i
          className="block h-full origin-left bg-accent"
          style={{ transform: `scaleX(${progress.total ? progress.assessed / progress.total : 0})` }}
        />
      </div>
      <p className="text-sm">
        <b>
          {progress.assessed} of {progress.total} comments assessed
        </b>
      </p>
      <p className="text-xs text-muted">
        The LLM reads and does not change code. You can already decide items assessed earlier.
      </p>
    </section>
  );
}

/** The empty queue with what was read (plan §3.2 Empty). */
export function EmptyQueueCard({
  read,
  assessed,
  head,
  now,
}: {
  read: GithubRead;
  assessed: number;
  head: string;
  now: Date;
}) {
  return (
    <section
      aria-label="No undecided questions"
      className="rounded-lg border border-border bg-surface px-4 py-3.5"
    >
      <h2 className="mt-0 mb-1.5 text-lg font-semibold">No undecided questions</h2>
      <p>
        In what was read{read.read_at ? ` at ${clockText(read.read_at, now)}` : ""}: {read.threads} threads (
        {read.complete ? "all pages read" : "not every page read"}), {read.reviews} reviews and{" "}
        {read.comments} PR comments; {assessed} comments assessed at <code>{head.slice(0, 7)}</code>.
      </p>
      <p className="text-xs text-muted">
        An empty queue says nothing about defects the reviewers did not raise.
      </p>
    </section>
  );
}

/** Gate chips of an item (queue rows, tables). */
export const GateChips = ({ item }: { item: CommentItem }) => (
  <>
    {item.gates.map((g) => (
      <GateChip key={g.gate} gate={g.gate} />
    ))}
  </>
);
