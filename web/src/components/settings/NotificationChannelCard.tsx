import { Tooltip } from "../tooltip/Tooltip";
import { useId, useState, type FormEvent } from "react";
import type {
  ChannelKind,
  ChannelTestResult,
  NotificationChannel,
  PutChannelRequest,
  SecretChange,
  UserEventKind,
} from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { clockText } from "../format/time";
import { SettingsCard } from "./Card";

const EVENTS: [UserEventKind, string][] = [
  ["questions_ready", "Questions ready on a watched PR"],
  ["round_executed", "All decisions of a round executed"],
];
const EVENT_WORD: Record<UserEventKind, string> = {
  questions_ready: "questions ready",
  round_executed: "round executed",
  test: "test",
};
const INPUT = "w-full rounded-md border border-muted bg-surface px-2 py-1 text-text";

interface NotificationChannelCardProps {
  kind: ChannelKind;
  /** The stored channel; null when not set up. */
  channel: NotificationChannel | null;
  onSave: (body: PutChannelRequest) => void;
  onRemove: () => void;
  onTest: () => void;
  /** The last test's answer, shown until the next. */
  testResult: ChannelTestResult | null;
  busy: boolean;
  error: string | null;
  /** The save was refused because the stored secret would go to another destination: `error` shows at the secret. */
  secretRequired?: boolean;
  now: Date;
}

/** Delivery state: last delivery, or failing since with the error (US22, US23). */
function Delivery({ channel, now }: { channel: NotificationChannel; now: Date }) {
  const { last, last_error, failing_since } = channel.delivery;
  if (failing_since)
    return (
      <span>
        <span className="rounded-full border border-high-bd bg-high-bg px-2 text-xs font-semibold text-high">
          Failing since {clockText(failing_since, now)}
        </span>{" "}
        {last_error}
      </span>
    );
  if (!last) return <span className="text-muted">No delivery yet.</span>;
  return (
    <span>
      Last delivery: {last.event ? EVENT_WORD[last.event] : "push"},{" "}
      {last.outcome === "delivered" ? "delivered" : "failed"} {clockText(last.at, now)}
    </span>
  );
}

/** Settings → Notifications: one push channel (ntfy or webhook) with its events, Send test and delivery state. */
export function NotificationChannelCard(props: NotificationChannelCardProps) {
  const { kind, channel, now } = props;
  const id = useId();
  const ntfy = kind === "ntfy";
  const [url, setUrl] = useState(channel?.url ?? (ntfy ? "https://ntfy.sh" : ""));
  const [topic, setTopic] = useState(channel?.topic ?? "");
  const [secret, setSecret] = useState<SecretChange>({ action: "keep" });
  const [events, setEvents] = useState<UserEventKind[]>(
    channel?.events ?? ["questions_ready", "round_executed"],
  );
  const [enabled, setEnabled] = useState(channel?.enabled ?? true);
  const secretName = ntfy ? "Access token (optional)" : "Authorization header (optional)";
  const secretError = props.secretRequired && props.error ? props.error : null;
  const secretDescription = `${id}-secret-d${secretError ? ` ${id}-secret-e` : ""}`;
  const save = (event: FormEvent) => {
    event.preventDefault();
    if (props.busy) return;
    props.onSave({
      url: url.trim(),
      topic: ntfy ? topic.trim() || null : null,
      secret: secret.action === "replace" && secret.value === "" ? { action: "keep" } : secret,
      events,
      enabled,
    });
    setSecret({ action: "keep" });
  };
  return (
    <SettingsCard title={ntfy ? "ntfy" : "Webhook (e.g. Home Assistant)"}>
      <form noValidate onSubmit={save} className="grid gap-2 text-sm">
        <label className="flex min-h-6 items-center gap-2">
          <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.target.checked)} />
          On
        </label>
        <div>
          <label htmlFor={`${id}-url`} className="mb-0.5 block text-xs font-semibold">
            {ntfy ? "Server URL" : "URL"}
          </label>
          <input
            id={`${id}-url`}
            type="url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            className={INPUT}
          />
        </div>
        {ntfy && (
          <div>
            <label htmlFor={`${id}-topic`} className="mb-0.5 block text-xs font-semibold">
              Topic
            </label>
            <input
              id={`${id}-topic`}
              type="text"
              value={topic}
              onChange={(e) => setTopic(e.target.value)}
              className={INPUT}
            />
          </div>
        )}
        <div>
          <label htmlFor={`${id}-secret`} className="mb-0.5 block text-xs font-semibold">
            {secretName}
          </label>
          {secret.action === "replace" ? (
            <div className="flex flex-wrap items-center gap-2">
              <input
                id={`${id}-secret`}
                type="password"
                autoComplete="off"
                value={secret.value}
                aria-describedby={secretDescription}
                onChange={(e) => setSecret({ action: "replace", value: e.target.value })}
                className={`${INPUT} max-w-80`}
              />
              <Tooltip tip="Keeps the stored secret as it is.">
                <button
                  type="button"
                  onClick={() => setSecret({ action: "keep" })}
                  className={buttonClass("plain", true)}
                >
                  Cancel
                </button>
              </Tooltip>
            </div>
          ) : (
            <div className="flex flex-wrap items-center gap-2">
              <input
                id={`${id}-secret`}
                type="text"
                readOnly
                value={
                  secret.action === "remove"
                    ? "removed when you save"
                    : channel?.secret_last4
                      ? `•••• ${channel.secret_last4}`
                      : "not set"
                }
                aria-describedby={secretDescription}
                className={`${INPUT} max-w-56`}
              />
              {secret.action === "keep" && (
                <Tooltip tip="Type a new secret; the stored one stays until you save.">
                  <button
                    type="button"
                    onClick={() => setSecret({ action: "replace", value: "" })}
                    className={buttonClass("plain", true)}
                  >
                    {channel?.secret_last4 ? "Replace" : "Set"}
                  </button>
                </Tooltip>
              )}
              {secret.action === "keep" && channel?.secret_last4 && (
                <Tooltip tip="Removes the stored secret when you save the channel.">
                  <button
                    type="button"
                    onClick={() => setSecret({ action: "remove" })}
                    className={buttonClass("plain", true)}
                  >
                    Remove secret
                  </button>
                </Tooltip>
              )}
              {secret.action === "remove" && (
                <Tooltip tip="Keeps the stored secret.">
                  <button
                    type="button"
                    onClick={() => setSecret({ action: "keep" })}
                    className={buttonClass("plain", true)}
                  >
                    Keep it
                  </button>
                </Tooltip>
              )}
            </div>
          )}
          <p id={`${id}-secret-d`} className="mt-0.5 text-xs text-muted">
            Stored encrypted on the server; only the last 4 characters are shown.
          </p>
          {secretError && (
            <p id={`${id}-secret-e`} role="alert" className="m-0 mt-0.5 text-sm text-high">
              {secretError}
            </p>
          )}
        </div>
        <fieldset className="m-0 rounded-md border border-border px-3 py-1.5">
          <legend className="px-1 font-semibold">Send</legend>
          {EVENTS.map(([event, label]) => (
            <label key={event} className="flex min-h-6 items-center gap-2">
              <input
                type="checkbox"
                checked={events.includes(event)}
                onChange={(e) =>
                  setEvents(e.target.checked ? [...events, event] : events.filter((x) => x !== event))
                }
              />
              {label}
            </label>
          ))}
        </fieldset>
        {props.error && !secretError && (
          <p role="alert" className="m-0 text-high">
            {props.error}
          </p>
        )}
        <div className="flex flex-wrap items-center gap-2">
          <Tooltip tip="Saves this channel; an untouched secret stays as stored.">
            <button
              type="submit"
              aria-disabled={props.busy || undefined}
              className={buttonClass("primary", true)}
            >
              Save channel
            </button>
          </Tooltip>
          {channel && (
            <>
              <Tooltip tip="Sends one test message now (one attempt, no retry).">
                <button
                  type="button"
                  aria-disabled={props.busy || undefined}
                  onClick={() => !props.busy && props.onTest()}
                  className={buttonClass("plain", true)}
                >
                  Send test
                </button>
              </Tooltip>
              <Tooltip tip="Removes this channel and its stored secret.">
                <button
                  type="button"
                  aria-disabled={props.busy || undefined}
                  onClick={() => !props.busy && props.onRemove()}
                  className={buttonClass("danger", true)}
                >
                  Remove channel
                </button>
              </Tooltip>
            </>
          )}
        </div>
        {channel && (
          <p role="status" className="m-0 text-xs">
            {props.testResult
              ? props.testResult.outcome === "delivered"
                ? `Test delivered${props.testResult.http_status ? ` · HTTP ${props.testResult.http_status}` : ""}.`
                : `Test failed: ${props.testResult.error ?? "no answer"}; one attempt, no retry.`
              : null}{" "}
            <Delivery channel={channel} now={now} />
          </p>
        )}
      </form>
    </SettingsCard>
  );
}
