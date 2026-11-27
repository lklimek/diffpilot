import { createContext } from "react";

/** The settings URL that opened the guide, including any repository selection. */
export const SettingsHelpOrigin = createContext<string | undefined>(undefined);

export function settingsGuideHref(from?: string) {
  return from ? `/settings/help?${new URLSearchParams({ from })}` : "/settings/help";
}
