import { useId, useLayoutEffect, useRef, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { buttonClass } from "../button/button";
import { navigateOnClick, type Navigate } from "../dashboard/common/navigate";
import { useAnchoredPanel } from "../popover/useAnchoredPanel";
import { Tooltip, useTipPrecedence } from "../tooltip/Tooltip";

const ITEM = '[role="menuitem"], [role="menuitemradio"], [role="menuitemcheckbox"]';

interface MenuButtonProps {
  label: string;
  /** Visible trigger content, when different from its accessible name. */
  trigger?: ReactNode;
  tip?: string;
  className?: string;
  children: ReactNode;
  triggerRef?: RefObject<HTMLButtonElement | null>;
}

/** A menu of actions, single choices or toggles. Put MenuItems or separators directly inside. */
export function MenuButton({
  label,
  trigger,
  tip = label,
  className = buttonClass("plain", true),
  children,
  triggerRef,
}: MenuButtonProps) {
  const id = useId();
  const { root, anchor, panel, open, setOpen, closeAndFocus, dismiss } = useAnchoredPanel<HTMLDivElement>({
    align: "end",
    anchorRef: triggerRef,
  });
  const initialFocus = useRef<"first" | "last">("first");
  const items = () => Array.from(panel.current?.querySelectorAll<HTMLElement>(ITEM) ?? []);
  useTipPrecedence(open);

  useLayoutEffect(() => {
    if (!open || !panel.current) return;
    const entries = Array.from(panel.current.querySelectorAll<HTMLElement>(ITEM));
    const first = entries.at(initialFocus.current === "first" ? 0 : -1) ?? panel.current;
    first.focus();
  }, [open, panel]);

  const onMenuKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Tab" && event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      closeAndFocus();
      return;
    }
    const entries = items();
    if (!entries.length) return;
    const index = entries.indexOf(document.activeElement as HTMLElement);
    const last = entries.length - 1;
    let next: number;
    switch (event.key) {
      case "Home":
        next = 0;
        break;
      case "End":
        next = last;
        break;
      case "ArrowDown":
        next = index === last ? 0 : index + 1;
        break;
      case "ArrowUp":
        next = index <= 0 ? last : index - 1;
        break;
      case " ":
        if (event.target instanceof HTMLAnchorElement) {
          event.preventDefault();
          event.target.click();
        }
        return;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
    entries[next]?.focus();
  };

  return (
    <div ref={root} className="inline-flex" {...dismiss} onClick={(event) => event.stopPropagation()}>
      <Tooltip tip={tip} className="inline-flex">
        <button
          ref={anchor}
          id={`${id}-trigger`}
          type="button"
          aria-label={label}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-controls={open ? id : undefined}
          className={`min-h-6 min-w-6 ${className}`}
          onClick={() => {
            initialFocus.current = "first";
            setOpen(!open);
          }}
          onKeyDown={(event) => {
            if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
            event.preventDefault();
            event.stopPropagation();
            initialFocus.current = event.key === "ArrowUp" ? "last" : "first";
            setOpen(true);
          }}
        >
          {trigger ?? label}
        </button>
      </Tooltip>
      {open && (
        <div
          ref={panel}
          id={id}
          role="menu"
          aria-labelledby={`${id}-trigger`}
          tabIndex={-1}
          className="fixed z-50 w-64 max-w-[calc(100vw-16px)] overflow-y-auto overscroll-contain rounded-lg border border-border bg-surface p-1 text-left text-sm whitespace-normal text-text shadow-tip [overflow-wrap:anywhere]"
          onKeyDown={onMenuKey}
          onClick={(event) => {
            const item = (event.target as Element).closest(ITEM);
            if (item && item.getAttribute("aria-disabled") !== "true") closeAndFocus();
          }}
        >
          {children}
        </div>
      )}
    </div>
  );
}

interface MenuItemProps {
  label: string;
  icon?: ReactNode;
  description?: string;
  disabledReason?: string;
  selection?: { kind: "checkbox" | "radio"; checked: boolean };
  href?: string;
  /** External links open in a new tab, announced in the item's name. */
  external?: boolean;
  /** Routes plain clicks in the app while preserving native modified clicks. */
  onNavigate?: Navigate;
  onSelect?: () => void;
}

/** A focusable action, link or controlled selection inside MenuButton. */
export function MenuItem({
  label,
  icon,
  description,
  disabledReason,
  selection,
  href,
  external = false,
  onNavigate,
  onSelect,
}: MenuItemProps) {
  const id = useId();
  const detail = [description, disabledReason].filter(Boolean).join(" ");
  const disabled = Boolean(disabledReason);
  const newTab = href && external && !disabled;
  const props = {
    role: selection ? (`menuitem${selection.kind}` as const) : "menuitem",
    tabIndex: -1,
    "aria-label": newTab ? `${label}, new tab` : undefined,
    "aria-labelledby": newTab ? undefined : `${id}-label`,
    "aria-describedby": detail ? `${id}-detail` : undefined,
    "aria-checked": selection?.checked,
    "aria-disabled": disabled || undefined,
    className:
      "flex min-h-6 w-full cursor-pointer items-baseline gap-2 rounded px-2.5 py-1.5 text-left no-underline hover:bg-accent-bg focus-visible:bg-accent-bg aria-disabled:cursor-not-allowed aria-disabled:text-muted",
  };
  const content = (
    <>
      {(selection || icon) && (
        <span aria-hidden className="w-4 flex-none text-center text-accent">
          {selection ? (selection.checked ? "✓" : null) : icon}
        </span>
      )}
      <span>
        <span id={`${id}-label`}>{label}</span>
        {detail && (
          <small id={`${id}-detail`} className="block text-xs text-muted">
            {detail}
          </small>
        )}
      </span>
    </>
  );
  return href && !disabled ? (
    <a
      {...props}
      href={href}
      target={external ? "_blank" : undefined}
      rel={external ? "noopener noreferrer" : undefined}
      onClick={(event) => {
        onSelect?.();
        if (!external) navigateOnClick(href, onNavigate)(event);
      }}
    >
      {content}
    </a>
  ) : (
    <button {...props} type="button" onClick={() => !disabled && onSelect?.()}>
      {content}
    </button>
  );
}
