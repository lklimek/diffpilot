import { useEffect } from "react";
import { Link, useLocation } from "react-router";
import { SettingsHelpGuide } from "../../components/settings/SettingsHelpGuide";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { SETTINGS_PATH } from "../shell/paths";

/** `/settings/help`: the settings guide, built in (works without the server); `#<option>` scrolls to an option. */
export function SettingsHelpScreen() {
  const { hash } = useLocation();
  useEffect(() => {
    if (hash) document.getElementById(decodeURIComponent(hash.slice(1)))?.scrollIntoView();
  }, [hash]);
  return (
    <div className="grid content-start gap-4">
      <div className="flex flex-wrap items-baseline gap-4">
        <h1 className="text-[22px] font-bold">Settings guide</h1>
        <Tooltip tip="Back to the Settings page.">
          <Link to={SETTINGS_PATH} className="text-sm text-accent underline">
            Back to Settings
          </Link>
        </Tooltip>
      </div>
      <p className="max-w-[100ch] text-sm">
        Every option on the Settings page: what it is, when smart-review uses it, what happens when it is
        empty, its limits and an example.
      </p>
      <SettingsHelpGuide />
    </div>
  );
}
