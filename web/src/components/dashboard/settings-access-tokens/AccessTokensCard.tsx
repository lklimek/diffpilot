import { useEffect, useRef, useState } from "react";
import type { AccessToken, CreateAccessToken, CreatedAccessToken } from "../../../api/generated/types";
import { busyTip, buttonClass } from "../../button/button";
import { dateTimeText } from "../../format/time";
import { useAction } from "../../inspector/action";
import { SettingsCard } from "../../settings/Card";
import { Tooltip } from "../../tooltip/Tooltip";
import { CreateTokenDialog } from "./CreateTokenDialog";
import { HelpTip } from "../../settings/HelpTip";

/** Most live tokens per user (`CreateAccessToken::MAX_LIVE_PER_USER`). */
const MAX_LIVE = 10;

const COLUMNS: [string, string][] = [
  ["Name", "The name you gave the token."],
  ["Token", "Start of the token (srp_ and its id); the secret part is never shown again."],
  ["Scope", "What the token may do: read = every read route, as you."],
  ["Created", "When the token was created."],
  ["Last used", "Last request the token was accepted for, recorded at most once a minute."],
  ["Expires", "The token stops working at this time."],
];

const CELL = "border-b border-border px-2 py-1.5 text-left align-top";
const HEAD = `${CELL} text-[11px] tracking-[.4px] whitespace-nowrap text-muted uppercase`;

interface AccessTokensCardProps {
  /** The user's tokens that are not revoked, newest first. */
  tokens: AccessToken[];
  /** Creates a token; resolves with its secret, which only the create dialog shows. */
  onCreate: (request: CreateAccessToken) => Promise<CreatedAccessToken>;
  onRevoke: (id: string) => Promise<unknown>;
  /** Current time, for the "expired" mark. */
  now?: Date;
}

/** Personal access tokens (ADR-019): read-only API access for the user's agents; list, create, revoke. */
export function AccessTokensCard({ tokens, onCreate, onRevoke, now = new Date() }: AccessTokensCardProps) {
  const [creating, setCreating] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);
  const opener = useRef<HTMLButtonElement>(null);
  const keep = useRef<HTMLButtonElement>(null);
  const revokeButtons = useRef(new Map<string, HTMLButtonElement>());
  // Where focus goes when the confirmation closes: the row's Revoke (Keep) or New token (revoked).
  const returnFocus = useRef<{ to: "new" } | { to: "row"; id: string } | null>(null);
  useEffect(() => {
    if (confirming !== null) {
      keep.current?.focus();
      return;
    }
    const target = returnFocus.current;
    returnFocus.current = null;
    if (target === null) return;
    const row = target.to === "row" ? revokeButtons.current.get(target.id) : undefined;
    (row ?? opener.current)?.focus();
  }, [confirming]);
  const revoke = useAction();
  const expired = (token: AccessToken) => new Date(token.expires_at) <= now;
  const live = tokens.filter((t) => !expired(t)).length;
  const full = live >= MAX_LIVE;
  return (
    <SettingsCard title="Access tokens" help={<HelpTip topic="access_tokens" />}>
      <p>
        A personal access token lets your agents read over HTTP what you can read here: My work, reviews and
        their questions. Tokens are <b>read-only</b>: they cannot change anything, start a refresh or an
        analysis, post comments, or list and create tokens.
      </p>
      <p className="text-xs text-muted">
        Send it as <code className="font-mono">Authorization: Bearer srp_…</code>. The agent API is described
        in <code className="font-mono">docs/contracts/agent-api.md</code> of diffpilot. Signing out everywhere
        revokes every token.
      </p>
      {tokens.length === 0 ? (
        <p className="text-muted">No tokens yet.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-sm">
            <caption className="sr-only">Your access tokens</caption>
            <thead>
              <tr>
                {COLUMNS.map(([name, tip]) => (
                  <th key={name} scope="col" className={HEAD}>
                    <Tooltip tip={tip}>
                      <span tabIndex={0} className="cursor-help">
                        {name}
                      </span>
                    </Tooltip>
                  </th>
                ))}
                <th scope="col" className={HEAD}>
                  <span className="sr-only">Revoke</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {tokens.map((token) => (
                <tr key={token.id}>
                  <td className={`${CELL} [overflow-wrap:anywhere]`}>{token.name}</td>
                  <td className={`${CELL} font-mono whitespace-nowrap`}>{token.prefix}…</td>
                  <td className={CELL}>{token.scopes.join(", ")}</td>
                  <td className={`${CELL} whitespace-nowrap`}>{dateTimeText(token.created_at)}</td>
                  <td className={`${CELL} whitespace-nowrap`}>
                    {token.last_used_at ? dateTimeText(token.last_used_at) : "never"}
                  </td>
                  <td className={`${CELL} whitespace-nowrap`}>
                    {dateTimeText(token.expires_at)}
                    {expired(token) && (
                      <Tooltip tip="This token stopped working when it expired. Revoke it to remove it from the list.">
                        <span tabIndex={0} className="ml-1.5 cursor-help font-semibold text-high">
                          expired
                        </span>
                      </Tooltip>
                    )}
                  </td>
                  <td className={CELL}>
                    {confirming === token.id ? (
                      <div
                        role="group"
                        aria-label={`Confirm revoking ${token.name}`}
                        className="grid gap-1.5"
                      >
                        <span>Agents using it lose access at once.</span>
                        <span className="flex gap-1.5">
                          <Tooltip
                            tip={busyTip(`Revokes ${token.name} now; this cannot be undone.`, revoke.busy)}
                          >
                            <button
                              type="button"
                              aria-disabled={revoke.busy || undefined}
                              className={buttonClass("danger", true)}
                              onClick={() =>
                                void revoke.run(async () => {
                                  await onRevoke(token.id);
                                  returnFocus.current = { to: "new" };
                                  setConfirming(null);
                                })
                              }
                            >
                              Revoke token
                            </button>
                          </Tooltip>
                          <Tooltip tip="Keeps the token and closes this confirmation.">
                            <button
                              ref={keep}
                              type="button"
                              aria-disabled={revoke.busy || undefined}
                              className={buttonClass("plain", true)}
                              onClick={() => {
                                if (revoke.busy) return;
                                returnFocus.current = { to: "row", id: token.id };
                                setConfirming(null);
                              }}
                            >
                              Keep
                            </button>
                          </Tooltip>
                        </span>
                      </div>
                    ) : (
                      <Tooltip tip={`Asks for confirmation, then revokes ${token.name}.`}>
                        <button
                          ref={(button) => {
                            if (button) revokeButtons.current.set(token.id, button);
                            else revokeButtons.current.delete(token.id);
                          }}
                          type="button"
                          aria-label={`Revoke ${token.name}`}
                          className={buttonClass("plain", true)}
                          onClick={() => {
                            revoke.setError(null);
                            setConfirming(token.id);
                          }}
                        >
                          Revoke
                        </button>
                      </Tooltip>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {revoke.error && (
        <p role="alert" className="text-high">
          Not revoked: {revoke.error}
        </p>
      )}
      <div className="flex flex-wrap items-center gap-3">
        <Tooltip
          tip={
            full
              ? `You have ${MAX_LIVE} tokens. Revoke one to create another.`
              : "Opens a dialog to name the token and choose when it expires."
          }
        >
          <button
            ref={opener}
            type="button"
            aria-disabled={full || undefined}
            className={buttonClass("primary")}
            onClick={() => !full && setCreating(true)}
          >
            New token
          </button>
        </Tooltip>
        <span className="text-xs text-muted">
          {live} of {MAX_LIVE} tokens in use.
        </span>
      </div>
      {creating && (
        <CreateTokenDialog
          names={tokens.map((t) => t.name)}
          onCreate={onCreate}
          onClose={() => {
            setCreating(false);
            opener.current?.focus();
          }}
        />
      )}
    </SettingsCard>
  );
}
