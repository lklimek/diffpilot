import { Kbd } from "../../help/Kbd";
import { Tooltip } from "../../tooltip/Tooltip";

/** Props of the Ask chat's launcher. */
export interface AskLauncherProps {
  /** Answers (or failures) the reader has not seen. */
  unread: number;
  /** An answer is being written. */
  busy: boolean;
  onOpen: () => void;
}

/**
 * Bottom-right "Ask" while the chat is hidden (proposal AC1): the count of new answers and a spinner while an answer
 * is being written. Its name is its visible text plus the hidden detail (WCAG 2.5.3).
 */
export function AskLauncher({ unread, busy, onOpen }: AskLauncherProps) {
  return (
    <Tooltip
      className="fixed right-4 bottom-4 z-40"
      tip="Your private Ask chat about this PR (a). It stays open while you move between steps, flows and files."
    >
      <button
        type="button"
        data-ask-launcher
        onClick={onOpen}
        className="inline-flex cursor-pointer items-center gap-2 rounded-full border border-accent bg-accent px-3.5 py-1.5 text-[14px] font-semibold text-on-accent shadow-lg hover:brightness-110"
      >
        {busy && (
          <span
            aria-hidden
            className="inline-block size-3 animate-spin rounded-full border-2 border-on-accent border-t-transparent motion-reduce:animate-none"
          />
        )}
        <span>Ask</span>
        {unread > 0 && " "}
        {unread > 0 && <b className="rounded-full bg-surface px-1.5 text-[12px] text-text">{unread} new</b>}
        {unread > 0 && " "}
        <span className="sr-only">
          {unread > 0 ? `answer${unread > 1 ? "s" : ""}` : ""}: open your private chat about this PR
          {busy ? ", an answer is being written" : ""}
        </span>
        <span className="text-[12px] font-normal [&_kbd]:border-on-accent">
          <Kbd decorative>a</Kbd>
        </span>
      </button>
    </Tooltip>
  );
}
