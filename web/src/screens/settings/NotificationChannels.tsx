import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../../api/client";
import type { ChannelKind, ChannelTestResult } from "../../api/generated/types";
import { useChannelActions, useNotificationSettings } from "../../api/review-loop";
import { useNow } from "../../app/now";
import { NotificationChannelCard } from "../../components/settings/NotificationChannelCard";
import { StatusMessage } from "../../components/status/StatusMessage";

const errorText = (error: unknown) =>
  error instanceof ApiError ? error.message : "Something went wrong. Try again.";

/** Settings → Notifications push channels (V7, RN1–RN3): ntfy and a generic webhook. */
export function NotificationChannels({
  onDirtyChange,
}: {
  onDirtyChange: (kind: ChannelKind, dirty: boolean) => void;
}) {
  const ntfyDirty = useCallback((dirty: boolean) => onDirtyChange("ntfy", dirty), [onDirtyChange]);
  const webhookDirty = useCallback((dirty: boolean) => onDirtyChange("webhook", dirty), [onDirtyChange]);
  const settings = useNotificationSettings();
  const { save, remove, test } = useChannelActions();
  const [tested, setTested] = useState<{ kind: ChannelKind; result: ChannelTestResult } | null>(null);
  // The latest action: only its outcome is shown.
  const [acting, setActing] = useState<{ kind: ChannelKind; action: "save" | "remove" | "test" } | null>(
    null,
  );
  const now = new Date(useNow());
  const loaded = settings.data !== undefined;
  useEffect(() => {
    if (loaded && window.location.hash === "#notifications")
      document.getElementById("notifications")?.scrollIntoView?.();
  }, [loaded]);
  if (settings.isError) return <StatusMessage kind="error">{errorText(settings.error)}</StatusMessage>;
  if (!settings.data) return <StatusMessage kind="loading">Loading the notification channels…</StatusMessage>;
  const busy = save.isPending || remove.isPending || test.isPending;
  const failure = (kind: ChannelKind) =>
    acting?.kind === kind ? { save, remove, test }[acting.action].error : null;
  return (
    <>
      {(["ntfy", "webhook"] as const).map((kind) => {
        const channel = settings.data.channels.find((c) => c.kind === kind) ?? null;
        const error = failure(kind);
        return (
          <NotificationChannelCard
            key={`${kind}:${channel?.url ?? ""}:${channel?.secret_last4 ?? ""}`}
            kind={kind}
            onDirtyChange={kind === "ntfy" ? ntfyDirty : webhookDirty}
            channel={channel}
            busy={busy}
            error={error ? errorText(error) : null}
            secretRequired={error instanceof ApiError && error.code === "secret_required"}
            testResult={tested?.kind === kind ? tested.result : null}
            now={now}
            onSave={(body) => {
              setActing({ kind, action: "save" });
              save.mutate({ kind, body });
            }}
            onRemove={() => {
              setActing({ kind, action: "remove" });
              remove.mutate(kind);
            }}
            onTest={() => {
              setActing({ kind, action: "test" });
              test.mutate(kind, { onSuccess: (result) => setTested({ kind, result }) });
            }}
          />
        );
      })}
      <p className="m-0 text-xs text-muted">
        Only http and https addresses. Redirects are not followed. Each attempt waits up to 10 seconds; 3
        attempts, then the channel shows as failing here. Messages carry a short text and a link to the PR,
        never comment text.
      </p>
    </>
  );
}
