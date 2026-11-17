import type { AppInstallation } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";
import { SettingsCard } from "./Card";

/** GitHub App installation status (brief §4.4); `null` = not checked, never shown as "none installed". */
export function AppInstallationsCard({ installations }: { installations: AppInstallation[] | null }) {
  return (
    <SettingsCard title="GitHub App">
      {installations === null ? (
        <p>GitHub App installations: not checked — GitHub could not be reached.</p>
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
            Manage on GitHub ↗
          </a>
        </Tooltip>
      </p>
    </SettingsCard>
  );
}
