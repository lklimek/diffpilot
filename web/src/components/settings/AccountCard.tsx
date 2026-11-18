import { busyTip, buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";
import { SettingsCard } from "./Card";
import { HelpTip } from "./HelpTip";

interface Props {
  /** Signed-in GitHub login. */
  login: string;
  /** Sign-out is running. */
  busy: boolean;
  /** Why the last sign-out failed. */
  error?: string;
  onSignOut: () => void;
}

/** The signed-in account and Sign out. */
export function AccountCard({ login, busy, error, onSignOut }: Props) {
  return (
    <SettingsCard title="Account" help={<HelpTip topic="sign_out" />}>
      <p>Signed in as {login}</p>
      <p>
        <Tooltip
          tip={busyTip(
            "Ends this session in this browser (other browsers stay signed in) and removes your unsent comment drafts in this tab. Your GitHub pending review stays on GitHub.",
            busy,
          )}
        >
          <button
            type="button"
            aria-disabled={busy || undefined}
            onClick={() => !busy && onSignOut()}
            className={buttonClass("plain")}
          >
            {busy ? "Signing out…" : "Sign out"}
          </button>
        </Tooltip>
      </p>
      {error && <p role="alert">Not signed out: {error}</p>}
    </SettingsCard>
  );
}
