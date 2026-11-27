import { Tooltip } from "../tooltip/Tooltip";

/** Opens the private Ask composer with the remark as context. */
export function AskRemarkButton({ onAsk }: { onAsk: () => void }) {
  return (
    <Tooltip tip="Opens your private Ask draft with the remark and its lines. You choose what to send.">
      <button
        type="button"
        onClick={onAsk}
        className="min-h-6 cursor-pointer border-0 bg-transparent p-0 text-left text-accent underline"
      >
        Ask about this remark
      </button>
    </Tooltip>
  );
}
