import type { ReactNode } from "react";
import { Tooltip } from "../tooltip/Tooltip";
import { MenuButton } from "./MenuButton";

const CONTROL =
  "inline-flex min-h-7 items-center justify-center border border-border bg-surface text-text hover:border-accent";

interface SplitButtonProps {
  label: string;
  icon?: ReactNode;
  moreLabel?: string;
  description?: string;
  disabledReason?: string;
  onAction: () => void;
  children: ReactNode;
}

/** A primary action with a separate menu for related actions. An icon replaces the primary label on phones. */
export function SplitButton({
  label,
  icon,
  moreLabel = `More ${label.toLowerCase()} options`,
  description,
  disabledReason,
  onAction,
  children,
}: SplitButtonProps) {
  const tip = [description, disabledReason].filter(Boolean).join(" ") || label;
  return (
    <div className="inline-flex" onClick={(event) => event.stopPropagation()}>
      <Tooltip tip={tip} className="inline-flex">
        <button
          type="button"
          aria-label={label}
          aria-disabled={Boolean(disabledReason) || undefined}
          onClick={() => !disabledReason && onAction()}
          className={`${CONTROL} w-full cursor-pointer gap-1.5 rounded-l-md px-2 text-xs aria-disabled:cursor-not-allowed aria-disabled:text-muted`}
        >
          {icon && <span aria-hidden>{icon}</span>}
          <span className={icon ? "hidden sm:inline" : undefined}>{label}</span>
        </button>
      </Tooltip>
      <MenuButton
        label={moreLabel}
        trigger={<span aria-hidden>▾</span>}
        className={`${CONTROL} cursor-pointer rounded-r-md border-l-0 px-1.5`}
      >
        {children}
      </MenuButton>
    </div>
  );
}
