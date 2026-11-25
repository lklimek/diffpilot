import { Tooltip } from "../tooltip/Tooltip";
import { Section } from "./Section";
import { HelpTip } from "./HelpTip";
import { buttonClass } from "../button/button";

/** Notification support of this browser, as `notifySupport` reports it. */
export type NotificationSupport = "unsupported" | "insecure" | "denied" | "default" | "granted";

const MESSAGES = {
  unsupported: "This browser cannot show notifications.",
  insecure: "Notifications need a secure connection: open diffpilot over https (or on localhost).",
  denied:
    "Blocked by the browser for this site. To allow: open the site information (the icon left of the address), set Notifications to Allow, then reload this page.",
};

interface NotificationsCardProps {
  support: NotificationSupport;
  enabled: boolean;
  onChange: (enabled: boolean) => void;
  onCheck?: () => void;
}

/** Per-browser opt-in to notifications when work finishes. */
export function NotificationsCard({ support, enabled, onChange, onCheck }: NotificationsCardProps) {
  const available = support === "default" || support === "granted";
  return (
    <Section heading="h3" title="Browser" help={<HelpTip topic="notify_when_done" />}>
      <p className="text-xs text-muted">
        A browser notification when an Ask answer arrives, a flow is analysed, or an analysis is ready,
        finishes or fails, while a diffpilot tab is open but you are looking elsewhere. Only in this browser.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <p role="status">
          {available
            ? support === "granted" &&
              (enabled ? "✓ Allowed in this browser" : "Allowed by the browser, off for diffpilot")
            : MESSAGES[support]}
        </p>
        {available && (
          <Tooltip tip="Changes diffpilot notifications for this browser.">
            <button type="button" className={buttonClass("plain")} onClick={() => onChange(!enabled)}>
              {support === "default" ? "Enable browser notifications" : enabled ? "Turn off" : "Turn on"}
            </button>
          </Tooltip>
        )}
        {support === "denied" && (
          <Tooltip tip="Reads the current permission from this browser.">
            <button type="button" className={buttonClass("plain")} onClick={onCheck}>
              Check again
            </button>
          </Tooltip>
        )}
      </div>
    </Section>
  );
}
