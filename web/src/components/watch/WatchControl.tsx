import type { WatchedPr } from "../../api/generated/types";
import { BUSY_NOTE } from "../button/button";
import { autoPassText, WATCH_STATE } from "../comments/PrHeader";
import { clockText } from "../format/time";
import { MenuItem } from "../menu/MenuButton";
import { Tooltip } from "../tooltip/Tooltip";

const OFFER = "New comments would be read and assessed automatically; uses LLM tokens.";
const WATCH_TIP =
  "diffpilot checks this pull request for new comments and reviews, assesses them with its LLM and notifies you. Uses GitHub requests and LLM tokens.";

type WatchControlProps = {
  busy?: boolean;
  disabledReason?: string;
  onChange: (watching: boolean) => void;
} & (
  | { variant: "menuitem"; watching: boolean }
  | {
      variant: "button" | "state";
      watch: Pick<WatchedPr, "since" | "state" | "last_auto_pass"> | null;
      now: Date;
    }
);

/** Watch comments from a menu, the PR page or the Comments header. */
export function WatchControl(props: WatchControlProps) {
  const { variant, busy, disabledReason, onChange } = props;
  const reason = disabledReason || (busy ? BUSY_NOTE : undefined);
  if (variant === "menuitem") {
    return (
      <MenuItem
        label="Watch comments"
        selection={{ kind: "checkbox", checked: props.watching }}
        description={OFFER}
        disabledReason={reason}
        onSelect={() => onChange(!props.watching)}
      />
    );
  }

  const { watch, now } = props;
  const watching = watch !== null;
  const latestPass = watch ? autoPassText(watch, now) : null;
  const state = watch && watch.state !== "active" ? WATCH_STATE[watch.state] : null;
  const note = watching
    ? [state ?? (variant === "button" ? `Watching since ${clockText(watch.since, now)}` : null), latestPass]
        .filter(Boolean)
        .join(" · ")
    : variant === "state"
      ? OFFER
      : null;
  const summary = note && <span className="text-xs text-muted">{note}</span>;
  return (
    <div className="flex flex-wrap items-center gap-2">
      {variant === "state" && summary}
      <Tooltip tip={reason ? `${WATCH_TIP} ${reason}` : WATCH_TIP}>
        <button
          type="button"
          aria-pressed={watching}
          aria-disabled={Boolean(reason) || undefined}
          onClick={() => !reason && onChange(!watching)}
          className={`inline-flex min-h-6 cursor-pointer items-center gap-1.5 rounded-full border px-2.5 text-xs aria-disabled:cursor-not-allowed ${
            watching
              ? "border-border bg-surface-2 font-semibold text-text"
              : "border-border bg-surface text-text"
          }`}
        >
          <span aria-hidden>{watching ? "◉" : "○"}</span>
          {watching ? "Watching" : variant === "state" ? "Watch this PR" : "Watch"}
        </button>
      </Tooltip>
      {variant === "button" && summary}
    </div>
  );
}
