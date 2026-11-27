import { useEffect } from "react";
import { Link, useLocation } from "react-router";
import { SettingsHelpGuide } from "../../components/settings/SettingsHelpGuide";
import { Tooltip } from "../../components/tooltip/Tooltip";
import { ACCOUNT_SETTINGS_PATH, MY_WORK_PATH, SETTINGS_PAGES } from "../shell/paths";

/** `/settings/help`: the settings guide, built in (works without the server); `#<option>` scrolls to an option. */
export function SettingsHelpScreen() {
  const { hash, search } = useLocation();
  const from = new URLSearchParams(search).get("from");
  const fromPath = from?.split(/[?#]/, 1)[0];
  const fromMyWork = fromPath === MY_WORK_PATH;
  const back =
    from && (fromMyWork || SETTINGS_PAGES.some(({ href }) => href === fromPath))
      ? from
      : ACCOUNT_SETTINGS_PATH;
  const backLabel = fromMyWork ? "Back to My work" : "Back to Settings";
  useEffect(() => {
    if (hash) document.getElementById(decodeURIComponent(hash.slice(1)))?.scrollIntoView();
  }, [hash]);
  return (
    <div className="grid content-start gap-4">
      <div className="flex flex-wrap items-baseline gap-4">
        <h1 className="text-[22px] font-bold">Settings guide</h1>
        <Tooltip tip={fromMyWork ? "Returns to My work." : "Returns to the Settings page."}>
          <Link to={back} className="text-sm text-accent underline">
            {backLabel}
          </Link>
        </Tooltip>
      </div>
      <p className="max-w-[100ch] text-sm">
        Find what each setting does, when diffpilot uses it, and what happens when it is empty.
      </p>
      <SettingsHelpGuide />
    </div>
  );
}
