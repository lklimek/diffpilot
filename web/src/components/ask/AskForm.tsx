import { useId, useRef, useState, type Ref } from "react";
import type { StepId } from "../../api/generated/types";
import { assignRef, growField, useAction } from "../inspector/action";
import { Tooltip } from "../tooltip/Tooltip";

/** Props of the Ask field in the inspector dock. */
export interface AskFormProps {
  /** The step asked about (`label` e.g. "Step 5 · Retry allowed?"); null when no step is open (Ask needs one). */
  step: { id: StepId; label: string } | null;
  /** Sends the question (POST …/steps/{step}/questions); a rejection keeps the text and shows its message. */
  onAsk: (step: StepId, text: string) => Promise<unknown> | void;
  /** The text field, for the `a` shortcut. */
  inputRef?: Ref<HTMLTextAreaElement>;
}

/** One-line question field that grows while typing; Enter sends, Shift+Enter adds a line. */
export function AskForm({ step, onAsk, inputRef }: AskFormProps) {
  if (step === null) {
    return (
      <section aria-label="Ask">
        <h3 className="text-[10.5px] font-bold tracking-wide text-muted uppercase">Ask</h3>
        <p className="text-[12.5px] text-muted">Ask works on a step. Open a step to ask the LLM about it.</p>
      </section>
    );
  }
  // Draft, error and pending state belong to one step: another step starts afresh, an old outcome is dropped.
  return <AskDraft key={step.id} step={step} onAsk={onAsk} inputRef={inputRef} />;
}

function AskDraft({ step, onAsk, inputRef }: AskFormProps & { step: { id: StepId; label: string } }) {
  const subject = step.label;
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
    if (await action.run(() => onAsk(step.id, question))) {
      setText("");
      requestAnimationFrame(() => growField(field.current));
    }
  };

  const error = empty
    ? "Write a question first."
    : action.error && `The question was not sent: ${action.error}`;
  return (
    <section aria-labelledby={`${id}-h`}>
      <h3 id={`${id}-h`} className="text-[10.5px] font-bold tracking-wide text-muted uppercase">
        Ask{" "}
        <span className="text-[12px] font-normal tracking-normal normal-case">
          about {subject}; answers stay pinned above
        </span>
      </h3>
      <form
        className="mt-1 flex items-start gap-2"
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
          placeholder="Ask about this step…"
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
        <Tooltip tip="Asks the LLM about this step (a). The answer arrives whole and stays pinned to the step.">
          <button
            type="submit"
            disabled={action.busy}
            className="cursor-pointer rounded-md border border-border bg-surface px-2.5 py-1 text-[12.5px] font-semibold disabled:cursor-wait disabled:opacity-60"
          >
            {action.busy ? "Sending…" : "Ask"}
          </button>
        </Tooltip>
      </form>
      {error && (
        <p id={`${id}-e`} role="alert" className="mt-1 text-[12px] text-high">
          {error}
        </p>
      )}
    </section>
  );
}
