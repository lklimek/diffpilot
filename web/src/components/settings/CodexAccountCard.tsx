import { Tooltip } from "../tooltip/Tooltip";
import { useEffect, useRef, useState } from "react";
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

function planPercent(value: number): string {
  return `${value}%`; // wording-check: allow provider-observed plan usage (D13)
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
  const unlinkButton = useRef<HTMLButtonElement>(null);
  const linkButton = useRef<HTMLButtonElement>(null);
  const restoreAfterUnlink = useRef(false);
  const keepButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (confirm) keepButton.current?.focus();
  }, [confirm]);
  useEffect(() => {
    if (restoreAfterUnlink.current && status?.state !== "linked" && !busy) {
      linkButton.current?.focus();
      restoreAfterUnlink.current = false;
    }
  }, [status?.state, busy]);
  const closeConfirmation = () => {
    setConfirm(false);
    (unlinkButton.current ?? linkButton.current)?.focus();
  };
  const expired = challenge !== undefined && Date.parse(challenge.expires_at) <= now.getTime();
  const href = challenge && !expired ? deviceUrl(challenge.verification_url) : undefined;
  const linking = challenge ? !expired : status?.state === "linking";
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
          Linked: {status.label ?? "not reported yet"} · Plan: {status.plan ?? "not reported yet"}
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
      {status?.state === "linked" && (
        <div className="grid gap-1">
          <p>Linked at: {status.telemetry.linked_at ?? "not reported yet"}.</p>
          {(["primary", "secondary"] as const).map((window) => {
            const usage = status.telemetry[window];
            const resetPassed = usage?.resets_at != null && Date.parse(usage.resets_at) <= now.getTime();
            return (
              <p key={window} data-codex-plan-usage>
                {window === "primary" ? "Primary" : "Secondary"} plan usage:{" "}
                {resetPassed ? (
                  <>reset at {usage.resets_at}; no newer observation</>
                ) : (
                  <>
                    {usage?.used_percent == null ? "not reported yet" : planPercent(usage.used_percent)};
                    resets at {usage?.resets_at ?? "not reported yet"}
                  </>
                )}
                .
              </p>
            );
          })}
          <p>
            Tokens today (UTC):{" "}
            {status.telemetry.tokens_today == null
              ? "not reported yet"
              : status.telemetry.tokens_today.input_tokens + status.telemetry.tokens_today.output_tokens}
            .
          </p>
          <p>
            Tokens this month (UTC):{" "}
            {status.telemetry.tokens_month == null
              ? "not reported yet"
              : status.telemetry.tokens_month.input_tokens + status.telemetry.tokens_month.output_tokens}
            .
          </p>
          <p>
            Live containers (claimed or running):{" "}
            {status.telemetry.live_containers?.length ?? "not reported yet"}.
          </p>
        </div>
      )}
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
            !status
              ? "Account status is unavailable; linking is possible once it loads."
              : linkBlocked
                ? "Wait for the current operation or device code to finish or expire."
                : "Starts device linking. Linking may end Codex sessions on other machines."
          }
        >
          <button
            ref={linkButton}
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
        {status?.state === "linked" && (
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
              ref={unlinkButton}
              onClick={() => !busy && setConfirm(true)}
            >
              Unlink Codex account
            </button>
          </Tooltip>
        )}
      </div>
      {confirm && (
        <div className="grid gap-2" role="group" aria-label="Confirm account unlink">
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
                    restoreAfterUnlink.current = true;
                    closeConfirmation();
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
                ref={keepButton}
                onClick={() => !busy && closeConfirmation()}
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
