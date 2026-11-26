import { useState } from "react";
import { writeClipboard } from "../dashboard/common/copyList";
import { GithubIcon } from "../github/GithubMark";
import type { AppInstallation, GithubWebhook } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { dateTimeText } from "../format/time";
import { GithubMark } from "../github/GithubMark";
import { Tooltip } from "../tooltip/Tooltip";
import { SettingsCard } from "./Card";
import { HelpTip } from "./HelpTip";

/** GitHub App installation status (brief §4.4); `null` = not checked (cause unknown), never "none installed". */
export function AppInstallationsCard({
  installations,
  signInHref,
  webhook,
  webhookError,
}: {
  installations: AppInstallation[] | null;
  /** The sign-in route, offered when the installations were not checked. */
  signInHref: string;
  webhook?: GithubWebhook | null;
  webhookError?: string;
}) {
  const webhookUrl = webhook?.webhook_url;
  const [copied, setCopied] = useState<boolean>();
  return (
    <SettingsCard title="GitHub App" help={<HelpTip topic="app_installations" />}>
      {installations === null ? (
        <p>
          GitHub App installations: not checked. Signing in again may help.{" "}
          <a href={signInHref} className={buttonClass("plain")}>
            <GithubIcon /> Sign in with GitHub
          </a>
        </p>
      ) : installations.length === 0 ? (
        <p>
          The App is not installed on any account. Analyses still run, but related work is not checked and
          comments cannot be written into your pending review.
        </p>
      ) : (
        <ul className="list-disc pl-5">
          {installations.map((installation) => (
            <li key={installation.account}>
              Installed on <strong>{installation.account}</strong>:{" "}
              {installation.all_repositories ? "all repositories" : "selected repositories"}
            </li>
          ))}
        </ul>
      )}
      <p>
        <Tooltip tip="Opens your GitHub App installations on GitHub (new tab).">
          <a
            href="https://github.com/settings/installations"
            target="_blank"
            rel="noopener noreferrer"
            className={buttonClass("plain")}
          >
            Manage on GitHub
            <GithubMark />
          </a>
        </Tooltip>
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <span>Webhook URL to set in the GitHub App:</span>
        {webhookUrl ? (
          <>
            <code className="select-all [overflow-wrap:anywhere]">{webhookUrl}</code>
            <Tooltip tip="Copies the GitHub App webhook URL.">
              <button
                type="button"
                className={buttonClass("plain")}
                onClick={async () => setCopied(await writeClipboard({ text: webhookUrl }))}
              >
                Copy
              </button>
            </Tooltip>
            {copied !== undefined && (
              <span role="status">{copied ? "Copied" : "Copy failed. Select the URL to copy it."}</span>
            )}
          </>
        ) : webhookError ? (
          <span role="alert">{webhookError}</span>
        ) : (
          <span role="status">{webhook === undefined ? "Loading…" : "Unavailable."}</span>
        )}
      </div>
      {webhook && (
        <ul className="list-disc pl-5">
          <li>Webhook {webhook.configured ? "configured" : "not configured"}</li>
          <li>
            Last GitHub delivery:{" "}
            {webhook.last_delivery_at ? dateTimeText(webhook.last_delivery_at) : "none recorded"}
          </li>
          <li>Dropped deliveries since last restart, up to 14 days: {webhook.dropped_deliveries}</li>
          <li>Connections refused since last restart: {webhook.refused_connections}</li>
        </ul>
      )}
    </SettingsCard>
  );
}
