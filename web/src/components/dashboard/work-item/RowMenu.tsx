import { useRef, useState } from "react";
import type { WorkItem } from "../../../api/generated/types";
import { BUSY_NOTE } from "../../button/button";
import { GithubIcon } from "../../github/GithubMark";
import { isGithubUrl } from "../../github/links";
import { MenuButton, MenuItem } from "../../menu/MenuButton";
import { useOutsidePress } from "../../popover/useOutsidePress";
import { WatchControl } from "../../watch/WatchControl";
import { httpUrl } from "../common/OpenExternal";
import { SnoozePanel } from "../snooze-menu/SnoozePanel";

export interface RowActions {
  onWatch?: (watching: boolean) => void;
  watched?: boolean;
  watching?: boolean;
  onSnooze?: (days: number) => void;
  snoozing?: boolean;
}

export function RowMenu({
  item,
  onWatch,
  watched = false,
  watching,
  onSnooze,
  snoozing,
}: RowActions & { item: WorkItem }) {
  const [snooze, setSnooze] = useState(false);
  const wrapper = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const url = httpUrl(item.url);
  const github = url && isGithubUrl(url);
  const canWatch = item.kind === "pr" && onWatch;
  const close = () => {
    setSnooze(false);
    trigger.current?.focus();
  };
  useOutsidePress(snooze, wrapper, () => setSnooze(false));
  if (!canWatch && !onSnooze && !github) return null;

  return (
    <div
      ref={wrapper}
      className="relative"
      onKeyDown={(event) => {
        if (!snooze || event.key !== "Escape" || event.defaultPrevented) return;
        event.preventDefault();
        event.stopPropagation();
        close();
      }}
    >
      <MenuButton
        label={`More actions for ${item.id}`}
        trigger={<span aria-hidden>⋯</span>}
        triggerRef={trigger}
        className="inline-flex size-7 cursor-pointer items-center justify-center rounded-md border border-transparent text-muted hover:border-border hover:text-text aria-expanded:border-accent"
      >
        {canWatch && (
          <WatchControl
            variant="menuitem"
            busy={watching}
            watching={watched}
            onChange={onWatch}
            disabledReason={item.private ? "Watch reads comments from public repositories." : undefined}
          />
        )}
        {onSnooze && (
          <MenuItem
            label="Snooze…"
            description="1, 3, 7, 30 days or a custom length."
            disabledReason={snoozing ? BUSY_NOTE : undefined}
            onSelect={() => setSnooze(true)}
          />
        )}
        {github && (
          <MenuItem
            label="Open on GitHub"
            description="Opens in a new tab."
            href={url}
            newTab
            icon={<GithubIcon />}
          />
        )}
      </MenuButton>
      {snooze && onSnooze && (
        <SnoozePanel itemId={item.id} onSnooze={onSnooze} onClose={close} pending={snoozing} />
      )}
    </div>
  );
}
