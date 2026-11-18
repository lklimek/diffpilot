import { Tooltip } from "../tooltip/Tooltip";

/** "Use as review comment" on LLM text: opens the comment composer with it as an editable draft. */
export function UseAsCommentButton({ onClick, what }: { onClick: () => void; what: string }) {
  return (
    <Tooltip
      tip={`Opens the comment composer with ${what} as a draft, marked LLM-suggested; edit it, then add it to your pending review yourself. Nothing is posted by this button.`}
      className="justify-self-start"
    >
      <button
        type="button"
        onClick={onClick}
        className="min-h-6 cursor-pointer rounded-md border border-border bg-surface px-2 text-[12px] text-text hover:border-accent"
      >
        Use as review comment
      </button>
    </Tooltip>
  );
}
