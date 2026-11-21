import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from "react";
import { SpeechBubble } from "../comment/LineComments";
import { CHANGE_LOOK, flagBar, REMARK_LOOK } from "../code-view/changeLook";
import { FLAG_LOOK } from "../flag/flags";
import { SHORTCUTS } from "./shortcuts";

interface HelpDialogProps {
  /** Whether the dialog is shown. */
  open: boolean;
  /** The view's key map (the Flows view's by default). */
  shortcuts?: typeof SHORTCUTS;
  /** Called on Close, Escape or a click outside. */
  onClose: () => void;
}

const SWATCH = "mr-1.5 inline-block h-3.5 w-4 align-middle";

function Swatch({ look }: { look: string }) {
  return <i aria-hidden className={`${SWATCH} ${look}`} />;
}

function Bubble({ look }: { look: string }) {
  return (
    <span aria-hidden className={`mr-1 inline-flex align-middle ${look}`}>
      <SpeechBubble />
    </span>
  );
}

const bar = (band: keyof typeof REMARK_LOOK) => `border-l-4 ${REMARK_LOOK[band].bar}`;

// Exactly what the code view draws; change colours and bars come from the definitions it uses.
const LEGEND: ReactNode[] = [
  <>
    <Swatch look={CHANGE_LOOK.added.fill} />
    <b className={CHANGE_LOOK.added.signLook}>+</b> added line: green, as in the classic diff (deterministic,
    from the diff)
  </>,
  <>
    <Swatch look={CHANGE_LOOK.removed.fill} />
    <b className={CHANGE_LOOK.removed.signLook}>−</b> removed line: red, as in the classic diff
    (deterministic, from the diff), shown at the deletion marker
  </>,
  <>
    Bar beside lines: an LLM remark on them, labelled once at its top; its colour and pattern are the
    LLM&apos;s rating, not checked. <Swatch look={bar("high")} />
    HIGH: solid bar · <Swatch look={bar("medium")} />
    MEDIUM: dashed bar · <Swatch look={bar("low")} />
    LOW or affected: dotted bar
  </>,
  <>
    <Swatch look={`border-l-4 ${REMARK_LOOK.none.bar}`} />
    LLM remark without a risk rating: thin neutral line
  </>,
  <>
    <Swatch look={`border-l-4 ${flagBar("red", "high")}`} />
    <b className={FLAG_LOOK.red.text}>{FLAG_LOOK.red.label}</b> ·{" "}
    <Swatch look={`border-l-4 ${flagBar("yellow", "high")}`} />
    <b className={FLAG_LOOK.yellow.text}>{FLAG_LOOK.yellow.label}</b>: a bar with an LLM flag takes the
    flag&apos;s colour (grey for a flag stored without a level, &quot;{FLAG_LOOK.unknown.label}&quot;) and
    keeps the pattern of the step&apos;s rating (the thin line on an unrated step); its label names the flag.
    A flag is the LLM&apos;s statement of a risk scenario, not checked
  </>,
  <>A changed line without a bar has no LLM remark. An unchanged line inside a bar is unchanged.</>,
  <>
    <Swatch look="border-l-[6px] border-double border-mv-bd" />
    moved, text identical: double bar · <b>≠</b> line differs from the moved original · moved lines keep the
    change fill, as in the classic diff
  </>,
  <>
    <Swatch look="border-l-4 border-dotted border-mv-bd bg-[image:repeating-linear-gradient(135deg,transparent_0_3px,rgb(120_130_150/0.3)_3px_4px)]" />
    outside the diff, unchanged
  </>,
  <>
    <b className="text-llm">◆</b> LLM remark: opens its text
  </>,
  <>
    <Bubble look="text-accent" /> your pending comment (accent bubble) · <Bubble look="text-thread" /> GitHub
    review thread (slate bubble)
  </>,
  <>
    <span className="mr-1.5 rounded border border-border px-1 text-[11px]">
      N comment/import lines hidden · Show
    </span>
    comment and import lines folded in place; Show opens them there, the classic diff always has them
  </>,
  <>
    <span className="mr-1.5 font-mono text-[11px] font-semibold">
      <span className={REMARK_LOOK.high.text}>
        ▲ HIGH · <span className="font-normal">5 remarks</span>
      </span>
    </span>
    pills in the list and above the code: the LLM&apos;s rating of a step (not checked) and the number of its
    LLM remarks, two separate facts; its tooltip counts the flags among them; a flow shows its rating once
    (its label: the highest step rating) and a pill with all its remarks; remarks on an unrated step: a
    neutral pill; no pill means neither, not that the code is safe
  </>,
  <>
    <Swatch look="outline-2 -outline-offset-2 outline-med-bd" />
    fading frame: the line an evidence link jumped to
  </>,
];

const FOCUSABLE = 'button, [href], textarea, input, [tabindex]:not([tabindex="-1"])';

/** The `?` help: keyboard shortcuts and the legend of how code is drawn (brief §4 rule 4). */
export function HelpDialog({ open, onClose, shortcuts = SHORTCUTS }: HelpDialogProps) {
  const titleId = useId();
  const legendId = useId();
  const close = useRef<HTMLButtonElement>(null);
  const dialog = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    close.current?.focus();
    return () => previous?.focus();
  }, [open]);

  if (!open) return null;

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
    } else if (event.key === "Tab") {
      const items = [...(dialog.current?.querySelectorAll<HTMLElement>(FOCUSABLE) ?? [])];
      const first = items[0];
      const last = items.at(-1);
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last?.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first?.focus();
      }
    }
  };

  return (
    <div className="fixed inset-0 z-50 grid place-items-center p-4">
      <div
        data-testid="help-backdrop"
        aria-hidden
        className="absolute inset-0 bg-black/40"
        onClick={onClose}
      />
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onKeyDown}
        className="relative max-h-full w-full max-w-xl overflow-auto rounded-lg border border-border bg-surface px-5 py-4 text-sm text-text shadow-tip"
      >
        <h2 id={titleId} className="mb-2 text-lg font-bold">
          Keyboard shortcuts
        </h2>
        <table aria-labelledby={titleId} className="w-full border-collapse">
          <tbody>
            {shortcuts.map(({ label, text }) => (
              <tr key={label} className="border-b border-border last:border-0">
                <td className="py-1 pr-3 whitespace-nowrap">
                  <kbd className="rounded border border-b-2 border-border px-1 font-mono text-xs">
                    {label}
                  </kbd>
                </td>
                <td className="py-1">{text}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <h3 id={legendId} className="mt-4 mb-1 font-bold">
          How code is drawn
        </h3>
        <ul aria-labelledby={legendId} className="m-0 grid list-none gap-1 p-0 text-[12.5px]">
          {LEGEND.map((item, i) => (
            <li key={i}>{item}</li>
          ))}
        </ul>
        <p className="mt-3 text-xs text-muted">Active on the review screen.</p>
        <button
          ref={close}
          type="button"
          onClick={onClose}
          className="mt-3 cursor-pointer rounded-md border border-accent bg-accent px-3 py-1.5 font-semibold text-on-accent"
        >
          Close
        </button>
      </div>
    </div>
  );
}
