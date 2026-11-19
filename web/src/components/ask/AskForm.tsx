import { useEffect, useId, useRef, useState, type ReactNode, type Ref } from "react";
import type { AskAnchor, CommentTarget, StepId } from "../../api/generated/types";
import { busyTip } from "../button/button";
import { assignRef, growField, useAction } from "../inspector/action";
import type { SectionControl } from "../inspector/sectionState";
import { Tooltip } from "../tooltip/Tooltip";

/** What a question is about: its anchor, a label ("Step 5 · Retry allowed?") and a noun ("this step"). */
export interface AskSubject {
  anchor: AskAnchor;
  label: string;
  /** "this step", "these lines", "this hunk", "this file": the field's placeholder and the button's tip. */
  noun: string;
}

/** The subject of a step (the Flows view): its whole range. */
export const stepSubject = (id: StepId, label: string): AskSubject => ({
  anchor: { kind: "step", step_id: id, lines: null },
  label,
  noun: "this step",
});

// Draft and outcome belong to one subject: another anchor starts afresh.
const subjectKey = (anchor: AskAnchor) => JSON.stringify(anchor);

/** Props of the Ask field in the inspector dock. */
export interface AskFormProps {
  /** What is asked about; null when there is nothing to ask about (the Flows view outside a step). */
  subject: AskSubject | null;
  /** Sends the question; a rejection keeps the text and shows its message. */
  onAsk: (anchor: AskAnchor, text: string) => Promise<unknown> | void;
  /** The text field, for the `a` shortcut. */
  inputRef?: Ref<HTMLTextAreaElement>;
  /** The conversation (e.g. `AskThread`), shown above the field, which stays at the bottom. */
  answers?: ReactNode;
  /** The Inspector's section state: with it, Ask collapses like the other sections (key `ask`). */
  sections?: SectionControl;
  /** Questions asked about the subject, shown in the header (also collapsed). */
  count?: number;
  /** A problem with the answers (e.g. the lost answer stream), announced above them; opens a collapsed Ask. */
  notice?: string | null;
  /** Text to put in at the cursor ("Ask about this line"): opens Ask and focuses the field; nothing is sent. */
  insert?: { text: string } | null;
  /** `insert` went in; the next one may follow. */
  onInserted?: () => void;
}

/** A code line or range as the Ask field takes it: `path:line` or `path:start-end`, old code said. */
export function lineReference(target: CommentTarget): string {
  const lines = target.start_line === null ? `${target.line}` : `${target.start_line}-${target.line}`;
  return `${target.path}:${lines}${target.side === "base" ? " (old code)" : ""}`;
}

/** One-line question field that grows while typing; Enter sends, Shift+Enter adds a line. */
export function AskForm({
  subject,
  onAsk,
  inputRef,
  answers,
  sections,
  count,
  notice,
  insert,
  onInserted,
}: AskFormProps) {
  if (subject === null) {
    return (
      <section aria-label="Ask">
        <h3 className="text-[10.5px] font-bold tracking-wide text-muted uppercase">Ask</h3>
        <p className="text-[12.5px] text-muted">Ask works on a step. Open a step to ask the LLM about it.</p>
      </section>
    );
  }
  // Draft, error and pending state belong to one subject: another starts afresh, an old outcome is dropped.
  return (
    <AskDraft
      key={subjectKey(subject.anchor)}
      subject={subject}
      onAsk={onAsk}
      inputRef={inputRef}
      answers={answers}
      sections={sections}
      count={count}
      notice={notice}
      insert={insert}
      onInserted={onInserted}
    />
  );
}

function AskDraft({
  subject: about,
  onAsk,
  inputRef,
  answers,
  sections,
  count,
  notice,
  insert,
  onInserted,
}: AskFormProps & { subject: AskSubject }) {
  const open = sections ? sections.isOpen("ask") : true;
  const subject = about.label;
  const id = useId();
  const field = useRef<HTMLTextAreaElement | null>(null);
  const [text, setText] = useState("");
  const [empty, setEmpty] = useState(false);
  const action = useAction();

  const send = async () => {
    const question = text.trim();
    if (!question) {
      setEmpty(true);
      field.current?.focus();
      return;
    }
    setEmpty(false);
    if (await action.run(() => onAsk(about.anchor, question))) {
      // Only the sent text goes: a line reference inserted meanwhile (appended, see below) stays.
      setText((now) => (now.startsWith(text) ? now.slice(text.length).trimStart() : now));
      requestAnimationFrame(() => growField(field.current));
    }
  };

  const error = empty
    ? "Write a question first."
    : action.error && `The question was not sent: ${action.error}`;
  // A failure is never left behind a collapsed header: Ask opens and the alert is inserted (so announced) then.
  const alerting = Boolean(action.error) || Boolean(notice);
  const reveal = sections?.reveal;
  useEffect(() => {
    if (alerting && !open) reveal?.("ask");
  }, [alerting, open, reveal]);

  // Inserted text goes in at the cursor (the field keeps it while unfocused), spaced from its neighbours; while a
  // question is being sent, after it, so clearing the sent text keeps it.
  const inserted = useRef<{ text: string } | null>(null);
  const caret = useRef<{ at: number; value: string } | null>(null);
  useEffect(() => {
    if (!insert || inserted.current === insert) return;
    inserted.current = insert;
    const element = field.current;
    const from = action.busy ? text.length : (element?.selectionStart ?? text.length);
    const before = text.slice(0, from);
    const after = text.slice(action.busy ? from : (element?.selectionEnd ?? from));
    const piece = `${before && !/\s$/.test(before) ? " " : ""}${insert.text}${/^\s/.test(after) ? "" : " "}`;
    caret.current = { at: before.length + piece.length, value: before + piece + after };
    setText(caret.current.value);
    setEmpty(false);
    if (!open) reveal?.("ask");
    onInserted?.();
  }, [insert, text, open, reveal, onInserted, action.busy]);
  useEffect(() => {
    const element = field.current;
    // Once the new text is rendered and Ask is open.
    if (!caret.current || !open || !element || element.value !== caret.current.value) return;
    element.focus();
    element.setSelectionRange(caret.current.at, caret.current.at);
    caret.current = null;
    growField(element);
  });
  return (
    // `data-dock-grow`: in a resized Inspector dock, Ask takes the room; only its conversation gives room back.
    <section aria-labelledby={`${id}-h`} data-dock-grow className="flex flex-1 flex-col">
      <h3 id={`${id}-h`} className="m-0 shrink-0 text-[10.5px] font-bold tracking-wide text-muted uppercase">
        {sections ? (
          <button
            type="button"
            aria-expanded={open}
            aria-controls={`${id}-b`}
            onClick={() => sections.toggle("ask")}
            className={`flex min-h-6 w-full cursor-pointer items-baseline gap-1.5 rounded py-0.5 text-left font-bold uppercase before:inline-block before:w-3 before:text-muted ${open ? "before:content-['▾']" : "before:content-['▸']"}`}
          >
            <AskCaption subject={subject} count={count} noun={about.noun} />
          </button>
        ) : (
          <AskCaption subject={subject} count={count} noun={about.noun} />
        )}
      </h3>
      <div id={`${id}-b`} hidden={!open} className="flex flex-1 flex-col">
        {answers && <div className="mt-1 flex flex-1 flex-col">{answers}</div>}
        <form
          className="mt-1 flex shrink-0 items-start gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            void send();
          }}
        >
          <textarea
            ref={(el) => {
              field.current = el;
              assignRef(inputRef, el);
            }}
            rows={1}
            value={text}
            readOnly={action.busy}
            aria-label={`Ask the LLM about ${subject}`}
            aria-invalid={empty || undefined}
            aria-describedby={error ? `${id}-e` : undefined}
            placeholder={`Ask about ${about.noun}…`}
            onChange={(event) => {
              setText(event.target.value);
              setEmpty(false);
              growField(event.target);
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" && !event.shiftKey) {
                event.preventDefault();
                void send();
              }
            }}
            className="min-h-8 flex-1 resize-none rounded-md border border-border bg-surface px-2 py-1 font-sans"
          />
          <Tooltip
            tip={busyTip(
              `Asks the LLM about ${about.noun} (a). The answer arrives whole, above the field.`,
              action.busy,
            )}
          >
            <button
              type="submit"
              aria-disabled={action.busy || undefined}
              className="cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold aria-disabled:cursor-wait aria-disabled:opacity-60"
            >
              {action.busy ? "Sending…" : "Ask"}
            </button>
          </Tooltip>
        </form>
        {error && open && (
          <p id={`${id}-e`} role="alert" className="mt-1 text-[12px] text-high">
            {error}
          </p>
        )}
        {notice && open && (
          <p role="alert" className="mt-1 text-[12px] text-high">
            {notice}
          </p>
        )}
      </div>
    </section>
  );
}

function AskCaption({ subject, count, noun }: { subject: string; count?: number; noun: string }) {
  return (
    <span>
      Ask{" "}
      <span className="text-[12px] font-normal tracking-normal normal-case">
        about {subject}; one conversation per analysis, newest at the bottom
        {count !== undefined &&
          count > 0 &&
          ` · ${count} ${count === 1 ? "question" : "questions"} on ${noun}`}
      </span>
    </span>
  );
}
