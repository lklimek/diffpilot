import { useEffect, useId, useRef, useState } from "react";
import type { Gate, GateMark } from "../../api/generated/types";
import { busyTip, buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";

/** The blocker gates (`severity`) with what each one means. */
export const GATES: readonly [Gate, string][] = [
  ["G-FUNDS", "loss or misdirection of funds"],
  ["G-SECRET", "secret exposure"],
  ["G-CRYPTO", "cryptographic weakness"],
  ["G-DATA", "data loss or corruption"],
];

interface GateMenuProps {
  /** The item's gates: those the assessment names stay; the owner's own marks can be cleared. */
  gates: readonly GateMark[];
  /** `C<no>` of the item. */
  itemLabel: string;
  /** A mark is being sent. */
  busy: boolean;
  /** The last mark that failed, in plain words. */
  error: string | null;
  onMark: (gate: Gate, marked: boolean) => void;
}

/**
 * "Mark gate ▾" (RC15): the owner marks or clears a blocker gate on a comment item. A gated item needs its own
 * decision (no group or "decide the rest"). Gates the LLM assessment names are shown checked and cannot be cleared.
 */
export function GateMenu({ gates, itemLabel, busy, error, onMark }: GateMenuProps) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const button = useRef<HTMLButtonElement>(null);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const outside = (event: Event) => {
      if (!box.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);
  const source = (gate: Gate) => gates.find((g) => g.gate === gate)?.source ?? null;
  const owned = gates.filter((g) => g.source === "owner").length;
  return (
    <div
      ref={box}
      className="relative inline-block"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !open) return;
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        button.current?.focus();
      }}
    >
      <Tooltip tip="Marks or clears a blocker gate on this comment. A gated comment needs its own decision.">
        <button
          ref={button}
          type="button"
          aria-expanded={open}
          aria-controls={`${id}-gates`}
          onClick={() => setOpen((o) => !o)}
          className={buttonClass("plain", true)}
        >
          Mark gate{owned > 0 && ` (${owned})`} <span aria-hidden>▾</span>
        </button>
      </Tooltip>
      {open && (
        <fieldset
          id={`${id}-gates`}
          className="absolute top-full left-0 z-40 m-0 mt-1 grid w-[min(22rem,90vw)] gap-1 rounded-md border border-border bg-surface p-2 text-[12.5px] whitespace-normal shadow-tip"
        >
          <legend className="sr-only">Gates of {itemLabel}</legend>
          {GATES.map(([gate, meaning]) => {
            const from = source(gate);
            const fixed = from === "assessment";
            return (
              <label key={gate} className="flex min-h-6 cursor-pointer items-start gap-2">
                <Tooltip
                  tip={
                    fixed
                      ? "The LLM assessment names this gate; it stays."
                      : busyTip(from ? "Clears your mark." : "Marks this gate.", busy)
                  }
                >
                  <input
                    type="checkbox"
                    checked={from !== null}
                    aria-disabled={fixed || busy || undefined}
                    onChange={() => !fixed && !busy && onMark(gate, from === null)}
                    className="mt-0.5"
                  />
                </Tooltip>
                <span>
                  <b>{gate}</b> {meaning}
                  {fixed && <span className="text-muted"> · named by the LLM assessment</span>}
                  {from === "owner" && <span className="text-muted"> · your mark</span>}
                </span>
              </label>
            );
          })}
          {error && (
            <p role="alert" className="m-0 text-high">
              {error}
            </p>
          )}
        </fieldset>
      )}
    </div>
  );
}
