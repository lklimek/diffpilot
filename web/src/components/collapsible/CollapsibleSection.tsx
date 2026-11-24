import { useId, useState, type ReactNode } from "react";

/** localStorage key prefix of the open / collapsed sections (plan §3.9). */
export const COLLAPSIBLE_STORAGE_PREFIX = "diffpilot.sections.";

const read = (key: string, defaultOpen: boolean): boolean => {
  try {
    const stored = window.localStorage.getItem(key);
    return stored === null ? defaultOpen : stored !== "0";
  } catch {
    return defaultOpen;
  }
};

const write = (key: string, open: boolean) => {
  try {
    window.localStorage.setItem(key, open ? "1" : "0");
  } catch {
    // Storage unavailable: the state lives for this page only.
  }
};

interface CollapsibleSectionProps {
  /** Heading text; also the region's name. */
  title: ReactNode;
  /** Where the state is remembered: view, user and section, e.g. `alice:comments-question:code`. */
  storageKey: string;
  /** Heading level of a page section (2) or a card section (3). */
  level?: 2 | 3;
  /** Tags or actions beside the heading (outside the button). */
  aside?: ReactNode;
  className?: string;
  /** Initial state when no reader preference exists. */
  defaultOpen?: boolean;
  children: ReactNode;
}

/**
 * A section whose heading is a disclosure button with a chevron (`aria-expanded`, `aria-controls`; Enter and Space
 * as any button); open by default, its state remembered per view and user (plan §3.9).
 */
export function CollapsibleSection({
  title,
  storageKey,
  level = 3,
  aside,
  className = "",
  defaultOpen = true,
  children,
}: CollapsibleSectionProps) {
  const id = useId();
  const key = `${COLLAPSIBLE_STORAGE_PREFIX}${storageKey}`;
  const [open, setOpen] = useState(() => read(key, defaultOpen));
  const Heading = level === 2 ? "h2" : "h3";
  const look =
    level === 2 ? "text-base font-semibold" : "text-xs font-bold tracking-wide text-muted uppercase";
  return (
    <section aria-labelledby={`${id}-h`} data-collapsible={storageKey} className={`min-w-0 ${className}`}>
      <div className="mt-3 mb-1 flex flex-wrap items-center gap-2">
        <Heading id={`${id}-h`} className={`m-0 ${look}`}>
          <button
            type="button"
            aria-expanded={open}
            aria-controls={`${id}-b`}
            onClick={() => {
              setOpen(!open);
              write(key, !open);
            }}
            className="inline-flex min-h-6 cursor-pointer items-center gap-1.5 rounded border-0 bg-transparent p-0 text-left font-[inherit] text-inherit [text-transform:inherit] [letter-spacing:inherit]"
          >
            <span
              aria-hidden="true"
              className={`inline-block text-[10px] text-muted transition-transform ${open ? "" : "-rotate-90"}`}
            >
              ▼
            </span>
            {title}
          </button>
        </Heading>
        {aside}
      </div>
      <div id={`${id}-b`} hidden={!open}>
        {children}
      </div>
    </section>
  );
}
