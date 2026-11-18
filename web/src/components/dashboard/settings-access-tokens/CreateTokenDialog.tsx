import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import type { CreateAccessToken, CreatedAccessToken } from "../../../api/generated/types";
import { busyTip, buttonClass } from "../../button/button";
import { useAction } from "../../inspector/action";
import { Tooltip } from "../../tooltip/Tooltip";

/** Lifetimes offered, in days (`CreateAccessToken::EXPIRY_DAYS`); 90 is preselected. */
const EXPIRY_DAYS = [30, 90, 365];
const DEFAULT_EXPIRY_DAYS = 90;
const FIELD = "min-w-0 rounded border border-border bg-surface px-2 py-1 text-sm";

interface CreateTokenDialogProps {
  /** Names already in use (unique per user). */
  names: string[];
  onCreate: (request: CreateAccessToken) => Promise<CreatedAccessToken>;
  /** Called on Cancel, Done or Escape; the secret is gone with the dialog. */
  onClose: () => void;
}

/** Why a token name cannot be used, or null (1–60 characters, unique). */
export function nameProblem(name: string, names: string[]): string | null {
  const length = [...name].length;
  if (length < 1 || length > 60) return "Name: use 1 to 60 characters.";
  if (names.includes(name)) return `Name: you already have a token called "${name}".`;
  return null;
}

/**
 * Modal dialog that creates a read-only token and shows its secret once, with a copy button. The secret lives
 * only in this dialog's state: never in browser storage or logs, and it is dropped when the dialog closes.
 */
export function CreateTokenDialog({ names, onCreate, onClose }: CreateTokenDialogProps) {
  const id = useId();
  const dialog = useRef<HTMLDivElement>(null);
  const [name, setName] = useState("");
  const [days, setDays] = useState(DEFAULT_EXPIRY_DAYS);
  const [problem, setProblem] = useState<string | null>(null);
  const [secret, setSecret] = useState<string | null>(null);
  const [copied, setCopied] = useState<"copied" | "failed" | null>(null);
  const action = useAction();

  useEffect(() => {
    dialog.current?.querySelector<HTMLElement>("input, select, button")?.focus();
  }, [secret]);

  const create = async () => {
    if (action.busy) return;
    const trimmed = name.trim();
    const refused = nameProblem(trimmed, names);
    setProblem(refused);
    if (refused) return;
    await action.run(async () => {
      const created = await onCreate({ name: trimmed, scopes: ["read"], expires_in_days: days });
      setSecret(created.token);
    });
  };
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(secret ?? "");
      setCopied("copied");
    } catch {
      setCopied("failed");
    }
  };
  // While the request runs the dialog stays: closing it then would hide a secret the server already made.
  const close = () => {
    if (action.busy) return;
    setSecret(null);
    onClose();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = [
      ...(dialog.current?.querySelectorAll<HTMLElement>("input, select, button") ?? []),
    ].filter((el) => !el.hasAttribute("disabled"));
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  };
  const message = problem ?? action.error;

  return (
    <div className="fixed inset-0 z-40 grid place-items-center p-4">
      <div aria-hidden className="absolute inset-0 bg-black/40" />
      <div
        ref={dialog}
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${id}-title`}
        onKeyDown={onKeyDown}
        className="relative grid max-h-full w-full max-w-lg gap-3 overflow-auto rounded-lg border border-border bg-surface px-5 py-4 text-sm text-text shadow-tip"
      >
        {secret === null ? (
          <>
            <h2 id={`${id}-title`} className="text-lg font-bold">
              New access token
            </h2>
            <div className="grid gap-1">
              <label htmlFor={`${id}-name`} className="font-semibold">
                Name
              </label>
              <input
                id={`${id}-name`}
                value={name}
                maxLength={60}
                autoComplete="off"
                placeholder="dispatcher agent"
                aria-invalid={problem ? true : undefined}
                aria-describedby={message ? `${id}-error` : undefined}
                className={FIELD}
                onChange={(e) => {
                  setName(e.target.value);
                  setProblem(null);
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void create();
                }}
              />
            </div>
            <div className="grid gap-1">
              <label htmlFor={`${id}-expiry`} className="font-semibold">
                Expires after
              </label>
              <select
                id={`${id}-expiry`}
                value={days}
                className={`${FIELD} w-fit`}
                onChange={(e) => setDays(Number(e.target.value))}
              >
                {EXPIRY_DAYS.map((d) => (
                  <option key={d} value={d}>
                    {d} days
                  </option>
                ))}
              </select>
            </div>
            <p>
              Scope: <b>read</b>. The token reads what you can read here and cannot change anything.
            </p>
            {message && (
              <p id={`${id}-error`} role="alert" className="text-high">
                Not created: {message}
              </p>
            )}
            <div className="flex gap-2">
              <Tooltip tip={busyTip("Creates the token and shows it once.", action.busy)}>
                <button
                  type="button"
                  aria-disabled={action.busy || undefined}
                  className={buttonClass("primary")}
                  onClick={() => void create()}
                >
                  {action.busy ? "Creating…" : "Create token"}
                </button>
              </Tooltip>
              <Tooltip tip={busyTip("Closes this dialog without creating a token.", action.busy)}>
                <button
                  type="button"
                  aria-disabled={action.busy || undefined}
                  className={buttonClass("plain")}
                  onClick={close}
                >
                  Cancel
                </button>
              </Tooltip>
            </div>
          </>
        ) : (
          <>
            <h2 id={`${id}-title`} className="text-lg font-bold">
              Token created
            </h2>
            <p
              id={`${id}-once`}
              className="rounded-md border border-l-4 border-med-bd bg-med-bg px-3 py-2 text-med"
            >
              <b>Shown only now.</b> Store it now; it cannot be shown again. If you lose it, revoke it and
              create a new one.
            </p>
            <label htmlFor={`${id}-secret`} className="font-semibold">
              Your new token
            </label>
            <input
              id={`${id}-secret`}
              readOnly
              value={secret}
              autoComplete="off"
              spellCheck={false}
              aria-describedby={`${id}-once`}
              className={`${FIELD} w-full font-mono`}
              onFocus={(e) => e.target.select()}
            />
            <div className="flex gap-2">
              <Tooltip tip="Copies the token to the clipboard.">
                <button type="button" className={buttonClass("primary")} onClick={() => void copy()}>
                  Copy
                </button>
              </Tooltip>
              <Tooltip tip="Closes this dialog; the token is not shown again.">
                <button type="button" className={buttonClass("plain")} onClick={close}>
                  Done
                </button>
              </Tooltip>
            </div>
            {copied === "copied" && <p role="status">Copied to the clipboard.</p>}
            {copied === "failed" && (
              <p role="alert" className="text-high">
                Copying did not work in this browser. Select the token and copy it yourself.
              </p>
            )}
          </>
        )}
      </div>
    </div>
  );
}
