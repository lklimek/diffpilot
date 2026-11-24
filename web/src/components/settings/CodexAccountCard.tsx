import { Tooltip } from "../tooltip/Tooltip";
import { useState } from "react";
import type { CredentialStatus, DeviceLink } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { SettingsCard } from "./Card";

function deviceUrl(value: string): string | undefined {
  try {
    const url = new URL(value);
    if (url.username || url.password) return;
    if (
      url.protocol === "https:" ||
      (url.protocol === "http:" && ["127.0.0.1", "localhost"].includes(url.hostname))
    )
      return url.href;
  } catch {
    /* An invalid provider URL stays text. */
  }
}

/** Plan-only account controls; device tokens never enter the page. */
export function CodexAccountCard({
  status,
  challenge,
  now,
  busy,
  error,
  onLink,
  onUnlink,
}: {
  status?: CredentialStatus;
  challenge?: DeviceLink;
  now: Date;
  busy: boolean;
  error?: string;
  onLink: () => void;
  onUnlink: () => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const expired = challenge !== undefined && Date.parse(challenge.expires_at) <= now.getTime();
  const href = challenge && !expired ? deviceUrl(challenge.verification_url) : undefined;
  const linking = status?.state === "linking" || (challenge !== undefined && !expired);
  const linkBlocked = busy || !status || (linking && !expired);
  return (
    <SettingsCard title="Codex account">
      <p>Use your ChatGPT plan for jobs you start.</p>
      <p className="rounded border border-border bg-surface-2 p-2">
        Linking may log your Codex out on other machines. You may need to sign in there again. The scope and
        likelihood are unknown; a separate link does not guarantee session isolation.
      </p>
      {!status && !error && <p role="status">Loading your Codex account…</p>}
      {status?.state === "linked" && (
        <p>
          Linked: {status.label ?? "Account label unavailable"} · Plan: {status.plan ?? "not supplied"}
        </p>
      )}
      {status?.state === "relink_needed" && (
        <p>
          {status.relink_reason === "quarantined"
            ? "Link paused because a credential exchange outcome is uncertain. Relink to continue; the exchange will not be repeated."
            : "Your Codex account needs linking before jobs can run."}
        </p>
      )}
      {status?.paused_until && <p>Plan usage paused until {status.paused_until}.</p>}
      {/* TODO(W2): expose D13 link date, plan usage/resets, per-user tokens and live containers in the safe status contract. */}
      {linking && <p role="status">Waiting for you to finish linking…</p>}
      {challenge && (
        <div className="grid gap-1" aria-label="Device linking">
          {expired ? (
            <p role="status">The device code expired. Start linking again.</p>
          ) : (
            <>
              <p>
                Device code: <code className="select-all font-bold">{challenge.user_code}</code>
              </p>
              <p>Expires at {challenge.expires_at}.</p>
              {href ? (
                <a className="text-accent underline" href={href} target="_blank" rel="noopener noreferrer">
                  Open the device linking page<span className="sr-only"> (opens in a new tab)</span>
                </a>
              ) : (
                <p>The device linking URL could not be opened. Try linking again.</p>
              )}
            </>
          )}
        </div>
      )}
      {error && <p role="alert">{error}</p>}
      <div className="flex flex-wrap gap-2">
        <Tooltip
          tip={
            linkBlocked
              ? "Wait for the current operation or device code to finish or expire."
              : "Starts device linking. Linking may end Codex sessions on other machines."
          }
        >
          <button
            type="button"
            aria-disabled={linkBlocked || undefined}
            className={buttonClass("plain", true)}
            onClick={() => !linkBlocked && onLink()}
          >
            {status?.state === "linked" || status?.relink_reason === "quarantined"
              ? "Relink Codex account"
              : "Link Codex account"}
          </button>
        </Tooltip>
        {status && (
          <Tooltip
            tip={
              busy
                ? "An account operation is in progress. Wait for it to finish."
                : "Shows a confirmation before revoking the linked account."
            }
          >
            <button
              type="button"
              aria-disabled={busy || undefined}
              className={buttonClass("plain", true)}
              onClick={() => !busy && setConfirm(true)}
            >
              Unlink Codex account
            </button>
          </Tooltip>
        )}
      </div>
      {confirm && (
        <div className="grid gap-2">
          <p>
            Unlink revokes this account's access and refresh token. Running turns will end with “Codex account
            unlinked”.
          </p>
          <div className="flex gap-2">
            <Tooltip
              tip={
                busy
                  ? "An account operation is in progress. Wait for it to finish."
                  : "Revokes this linked account and ends its running turns."
              }
            >
              <button
                type="button"
                aria-disabled={busy || undefined}
                className={buttonClass("plain", true)}
                onClick={() => {
                  if (!busy) {
                    setConfirm(false);
                    onUnlink();
                  }
                }}
              >
                Confirm unlink
              </button>
            </Tooltip>
            <Tooltip
              tip={
                busy
                  ? "An account operation is in progress. Wait for it to finish."
                  : "Closes this confirmation without unlinking the account."
              }
            >
              <button
                type="button"
                aria-disabled={busy || undefined}
                className={buttonClass("plain", true)}
                onClick={() => !busy && setConfirm(false)}
              >
                Keep linked
              </button>
            </Tooltip>
          </div>
        </div>
      )}
    </SettingsCard>
  );
}
