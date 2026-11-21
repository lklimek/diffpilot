import type { PluginStatus } from "../../../api/generated/types";
import { OpenExternal } from "../common/OpenExternal";

const TEXT: Record<Exclude<PluginStatus["state"], "current">, string> = {
  waiting: "not read yet",
  stale: "unavailable or out of date",
};

/**
 * One attributed line per configured dashboard plug-in whose statements are not applied now (ADR-022): not read
 * yet, or unavailable or out of date; nothing for a current one. `what` names what goes without its statements.
 */
export function PluginStatusNote({ plugins, what }: { plugins: PluginStatus[] | undefined; what: string }) {
  const off = (plugins ?? []).filter(
    (plugin): plugin is PluginStatus & { state: keyof typeof TEXT } => plugin.state !== "current",
  );
  if (off.length === 0) return null;
  return (
    <>
      {off.map((plugin) => (
        <p key={plugin.plugin} data-plugin-status={plugin.state} className="m-0 mt-1.5 text-xs text-muted">
          <span className="font-semibold">{plugin.source}:</span> {TEXT[plugin.state]}; {what} without its
          statements. <OpenExternal href={plugin.url} label={`${plugin.source} home page`} />
        </p>
      ))}
    </>
  );
}
