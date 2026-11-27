import { useEffect, useId, useRef, useState } from "react";
import { busyTip, buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";
import { SettingsCard } from "./Card";
import { HelpTip } from "./HelpTip";

interface Props {
  /** Signed-in GitHub login. */
  login: string;
  /** A sign-out is running. */
  busy: boolean;
  /** Why the last sign-out failed. */
  error?: string;
  /** Called once the user confirmed Sign out everywhere. */
  onSignOutEverywhere: () => void;
}

const EVERYWHERE_TEXT =
  "This ends every session of yours in every browser, revokes all your personal access tokens and deletes the GitHub sign-in this server keeps for the nightly scan; the nightly scan then waits until you sign in again. Nothing changes on GitHub.";

/** The signed-in account, Sign out everywhere (asked first). */
export function AccountCard({ login, busy, error, onSignOutEverywhere }: Props) {
  const [asking, setAsking] = useState(false);
  const everywhere = useRef<HTMLButtonElement>(null);
  return (
    <SettingsCard title="GitHub sign-in" help={<HelpTip topic="sign_out" />}>
      <p>Signed in as {login}</p>
      <div className="flex flex-wrap items-center gap-2">
        <Tooltip
          tip={busyTip(
            "Asks first, then ends every session of yours, revokes all your personal access tokens and deletes the GitHub sign-in kept for the nightly scan.",
            busy,
          )}
        >
          <button
            ref={everywhere}
            type="button"
            aria-disabled={busy || undefined}
            aria-expanded={asking}
            onClick={() => !busy && setAsking(true)}
            className={buttonClass("plain")}
          >
            Sign out everywhere
          </button>
        </Tooltip>
        <HelpTip topic="sign_out_everywhere" />
      </div>
      {asking && (
        <EverywherePrompt
          onCancel={() => {
            setAsking(false);
            everywhere.current?.focus();
          }}
          onConfirm={() => {
            setAsking(false);
            onSignOutEverywhere();
          }}
        />
      )}
      {error && <p role="alert">Not signed out: {error}</p>}
    </SettingsCard>
  );
}

/** The confirmation of Sign out everywhere; Cancel takes the focus and Escape cancels. */
function EverywherePrompt({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  const id = useId();
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => cancel.current?.focus(), []);
  return (
    <div
      role="alertdialog"
      aria-label="Sign out everywhere?"
      aria-describedby={id}
      onKeyDown={(event) => {
        if (event.key === "Escape") onCancel();
      }}
      className="grid gap-2 rounded-md border border-l-4 border-med-bd bg-med-bg px-3.5 py-2.5 text-text"
    >
      <p id={id}>{EVERYWHERE_TEXT}</p>
      <div className="flex gap-2">
        <Tooltip tip="Keeps every session and token as they are.">
          <button ref={cancel} type="button" className={buttonClass("primary")} onClick={onCancel}>
            Cancel
          </button>
        </Tooltip>
        <Tooltip tip="Signs you out everywhere now.">
          <button type="button" className={buttonClass("danger")} onClick={onConfirm}>
            Yes, sign out everywhere
          </button>
        </Tooltip>
      </div>
    </div>
  );
}
