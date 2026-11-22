import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { buttonClass, busyTip } from "../../button/button";
import { Tooltip } from "../../tooltip/Tooltip";

/** Snooze lengths offered as one click, in days. */
export const SNOOZE_DAYS = [1, 3, 7, 30] as const;
/** Most days a typed snooze may last (the server's `MAX_SNOOZE_DAYS`). */
export const MAX_SNOOZE_DAYS = 365;

/** Whole days 1–365 of a typed value, or null. */
export function snoozeDays(text: string): number | null {
  const trimmed = text.trim();
  if (!/^\d+$/.test(trimmed)) return null;
  const days = Number(trimmed);
  return days >= 1 && days <= MAX_SNOOZE_DAYS ? days : null;
}

const daysText = (days: number) => `${days} ${days === 1 ? "day" : "days"}`;

interface SnoozeMenuProps {
  /** Display id of the item (`PLAT-1203`); names the controls. */
  itemId: string;
  /** Days chosen (1–365). */
  onSnooze: (days: number) => void;
  /** A snooze of this item is being saved: the choices do nothing. */
  pending?: boolean;
}

/**
 * A row's menu of more actions: snooze the item for 1, 3, 7 or 30 days, or a typed number of days (owner SN1).
 * A disclosure: Escape or a click outside closes it, a choice closes it and returns the focus to its button.
 */
export function SnoozeMenu({ itemId, onSnooze, pending = false }: SnoozeMenuProps) {
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState("");
  const [invalid, setInvalid] = useState(false);
  const wrapper = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const panelId = useId();
  const inputId = useId();
  const errorId = useId();

  const close = (refocus: boolean) => {
    setOpen(false);
    setInvalid(false);
    if (refocus) trigger.current?.focus();
  };

  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (!wrapper.current?.contains(event.target as Node)) close(false);
    };
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, [open]);

  const choose = (days: number) => {
    if (pending) return;
    onSnooze(days);
    setTyped("");
    close(true);
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const days = snoozeDays(typed);
    if (days === null) {
      setInvalid(true);
      return;
    }
    choose(days);
  };

  return (
    <div
      ref={wrapper}
      className="relative"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !open) return;
        // The list's own keys (j/k, Enter) must not see it.
        event.preventDefault();
        event.stopPropagation();
        close(true);
      }}
    >
      <Tooltip tip="More actions: snooze this item">
        <button
          ref={trigger}
          type="button"
          aria-label={`More actions for ${itemId}`}
          aria-expanded={open}
          aria-controls={open ? panelId : undefined}
          onClick={() => (open ? close(false) : setOpen(true))}
          className="inline-flex h-7 w-7 cursor-pointer items-center justify-center rounded-md border border-transparent text-muted hover:border-border hover:text-text aria-expanded:border-accent aria-expanded:text-accent"
        >
          <span aria-hidden="true">⋯</span>
        </button>
      </Tooltip>
      {open && (
        <div
          id={panelId}
          role="group"
          aria-label={`Snooze ${itemId}`}
          className="absolute top-full right-0 z-30 mt-1 grid w-64 gap-2 rounded-lg border border-border bg-surface p-2.5 text-xs shadow-tip"
        >
          <p className="m-0 font-semibold text-text">Snooze for</p>
          <div className="flex flex-wrap gap-1.5">
            {SNOOZE_DAYS.map((days) => (
              <Tooltip key={days} tip={busyTip(`Leaves your list for ${daysText(days)}.`, pending)}>
                <button
                  type="button"
                  aria-disabled={pending || undefined}
                  onClick={() => choose(days)}
                  className={buttonClass("plain", true)}
                >
                  {daysText(days)}
                </button>
              </Tooltip>
            ))}
          </div>
          <form onSubmit={submit} noValidate className="flex items-center gap-1.5">
            <label htmlFor={inputId} className="text-muted">
              Days (1–{MAX_SNOOZE_DAYS})
            </label>
            <input
              id={inputId}
              type="number"
              inputMode="numeric"
              min={1}
              max={MAX_SNOOZE_DAYS}
              step={1}
              value={typed}
              aria-invalid={invalid || undefined}
              aria-describedby={invalid ? errorId : undefined}
              onChange={(event) => {
                setTyped(event.target.value);
                setInvalid(false);
              }}
              className="w-16 rounded border border-border bg-surface px-1.5 py-0.5 text-text aria-invalid:border-high-bd"
            />
            <Tooltip tip={busyTip("Leaves your list for the typed number of days.", pending)}>
              <button
                type="submit"
                aria-disabled={pending || undefined}
                className={buttonClass("plain", true)}
              >
                Snooze
              </button>
            </Tooltip>
          </form>
          {invalid && (
            <p id={errorId} role="alert" className="m-0 text-high">
              Type a whole number of days from 1 to {MAX_SNOOZE_DAYS}.
            </p>
          )}
          <p className="m-0 text-muted">
            It leaves your list until then, or earlier when you get a new review request or mention on it.
            Excluded lists it with Wake now.
          </p>
        </div>
      )}
    </div>
  );
}
