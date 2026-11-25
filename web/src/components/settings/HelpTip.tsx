import { useContext } from "react";
import { SettingsHelpOrigin } from "./helpOrigin";
import { Tooltip } from "../tooltip/Tooltip";
import { SETTINGS_HELP, helpHref, helpLines, type HelpTopic } from "./settingsHelp";

/** The tip's lines: what, when used, example (the guide has the rest). */
const TIP_LINES = new Set(["What", "When used", "Example"]);

/** `?` next to a Settings option: hover or focus shows what it is, when it is used and an example; a click opens
 * the guide at the option in a new tab, so unsaved settings stay. */
export function HelpTip({ topic }: { topic: HelpTopic }) {
  const help = SETTINGS_HELP[topic];
  const from = useContext(SettingsHelpOrigin);
  const tip = (
    <span className="grid gap-1">
      {helpLines(help)
        .filter(([label]) => TIP_LINES.has(label))
        .map(([label, text]) => (
          <span key={label} className="block">
            <b>{label}:</b> {text}
          </span>
        ))}
      <span className="block text-muted">Click for the full guide (new tab).</span>
    </span>
  );
  return (
    <Tooltip tip={tip}>
      <a
        href={helpHref(topic, from)}
        target="_blank"
        rel="noopener"
        className="inline-flex size-6 flex-none items-center justify-center rounded-full border border-border text-xs font-bold text-muted no-underline hover:border-accent hover:text-text"
      >
        {/* Named by text, not aria-label, so label lookups of the option do not find the help link. */}
        <span aria-hidden>?</span>
        <span className="sr-only">Help: {help.title}</span>
      </a>
    </Tooltip>
  );
}
