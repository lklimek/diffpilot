// @vitest-environment node
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import dashboardSchema from "../../../../schemas/api/dashboard_settings_view.schema.json";
import settingsSchema from "../../../../schemas/api/settings.schema.json";
import { expectTrustWording } from "../../__tests__/wording";
import {
  CONTRACT_HELP,
  HELP_SECTIONS,
  HELP_TOPICS,
  SETTINGS_HELP,
  helpLines,
  settingsHelpMarkdown,
  type HelpTopic,
} from "./settingsHelp";

const DOC = fileURLToPath(new URL("../../../../docs/settings.md", import.meta.url));

type Schema = {
  properties: Record<string, unknown>;
  $defs?: Record<string, { properties: Record<string, unknown> }>;
};
const fields = (schema: Schema, def?: string) =>
  Object.keys(def ? schema.$defs![def]!.properties : schema.properties);

describe("settings help (owner request B3)", () => {
  it.each([
    ["settings", fields(settingsSchema as unknown as Schema)],
    ["llmSelection", fields(settingsSchema as unknown as Schema, "LlmSelection")],
    ["dashboard", fields(dashboardSchema as unknown as Schema, "DashboardSettings")],
    ["repository", fields(dashboardSchema as unknown as Schema, "RepositoryRule")],
  ] as const)("every field of the %s contract has a help entry", (group, names) => {
    expect(names.length).toBeGreaterThan(0);
    const map = CONTRACT_HELP[group] as Record<string, HelpTopic | undefined>;
    for (const name of names) {
      const topic = map[name];
      expect(topic, `${group}.${name} has no help`).toBeDefined();
      expect(SETTINGS_HELP[topic!], `${group}.${name}`).toBeDefined();
    }
  });

  it("each entry says what, when, empty or default and gives an example, in the trust wording", () => {
    for (const topic of HELP_TOPICS) {
      const help = SETTINGS_HELP[topic];
      for (const [label, text] of helpLines(help)) expect(text.trim(), `${topic} ${label}`).not.toBe("");
      expectTrustWording(
        [help.title, ...helpLines(help).map(([, t]) => t), ...(help.details ?? [])].join("\n"),
      );
    }
    expect(new Set(HELP_TOPICS.map((t) => SETTINGS_HELP[t].section))).toEqual(new Set(HELP_SECTIONS));
  });

  it("the ownership policy points at stale_prs_are_bad with the worked examples", () => {
    const text = [
      SETTINGS_HELP.ownership_policy.what,
      ...(SETTINGS_HELP.ownership_policy.details ?? []),
    ].join(" ");
    expect(text).toContain("https://github.com/dashpay/stale_prs_are_bad");
    for (const path of ["policies/platform.json", "policies/tenderdash.json", "policies/dash-evo-tool.json"])
      expect(text).toContain(path);
  });

  it("release branches explain X.Y, * and that a list replaces the built-ins", () => {
    const help = SETTINGS_HELP.branch_patterns;
    expect(help.empty).toContain("main, master, release/*, hotfix/*, vX.Y, vX.Y-dev");
    expect(help.empty).toContain("replaces the built-ins");
    expect(help.example).toContain("vX.Y-dev matches v1.8-dev and v5.1-dev");
  });

  it("docs/settings.md is the guide generated from this map", () => {
    const expected = settingsHelpMarkdown();
    if (process.env.UPDATE_GENERATED) writeFileSync(DOC, expected);
    expect(readFileSync(DOC, "utf8")).toBe(expected);
  });
});
