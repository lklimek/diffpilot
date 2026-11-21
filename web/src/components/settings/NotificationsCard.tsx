import { SettingsCard } from "./Card";
import { HelpTip } from "./HelpTip";

/** Notification support of this browser, as `notifySupport` reports it. */
export type NotificationSupport = "unsupported" | "insecure" | "denied" | "default" | "granted";

/** Why the browser cannot notify, for the states where it cannot. */
const BLOCKED: Partial<Record<NotificationSupport, string>> = {
  unsupported: "This browser cannot show notifications.",
  insecure: "Notifications need a secure connection: open diffpilot over https (or on localhost).",
  denied:
    "Notifications are blocked for this site. Allow them in the browser's site settings, then reload this page.",
};

interface NotificationsCardProps {
  support: NotificationSupport;
  /** The opt-in is stored in this browser and allowed by it. */
  enabled: boolean;
  /** Called with the new choice; turning it on asks the browser's permission. */
  onChange: (enabled: boolean) => void;
}

/** Per-browser opt-in to notifications when work finishes. */
export function NotificationsCard({ support, enabled, onChange }: NotificationsCardProps) {
  const blocked = BLOCKED[support];
  return (
    <SettingsCard title="Notifications" help={<HelpTip topic="notify_when_done" />}>
      {blocked ? (
        <p>{blocked}</p>
      ) : (
        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={enabled}
            className="size-4"
            onChange={(e) => onChange(e.target.checked)}
          />
          Notify me when work finishes
        </label>
      )}
      <p className="text-xs text-muted">
        A browser notification when an Ask answer arrives, a flow is analysed, or an analysis is ready,
        finishes or fails, while a diffpilot tab is open but you are looking elsewhere. Only in this browser.
      </p>
    </SettingsCard>
  );
}
