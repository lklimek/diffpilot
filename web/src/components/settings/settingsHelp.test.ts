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

  it("excluded authors, known bots and linked logins say exactly how each list works", () => {
    expect(SETTINGS_HELP.skip_authors.title).toBe("Excluded authors");
    expect(SETTINGS_HELP.skip_authors.what).toBe(
      "Banned authors. Pull requests and issues written by these accounts are never shown in Needs you — also when a review is requested from you or they are assigned to you. Being listed does not make an account a bot.",
    );
    expect(SETTINGS_HELP.skip_authors.empty).toBe("dependabot, dependabot[bot].");
    expect(SETTINGS_HELP.skip_authors.example).toBe("Add renovate[bot] to hide Renovate's pull requests.");
    expect(SETTINGS_HELP.known_bots.title).toBe("Known bots");
    expect(SETTINGS_HELP.known_bots.what).toBe(
      "Accounts treated as bots: their review requests and comments do not count as a person waiting for you, they are not listed among reviewers you wait on, and their reviews count as bot reviews. GitHub Apps (…[bot]) are always bots and need not be listed. Nothing else is built in.",
    );
    expect(SETTINGS_HELP.known_bots.empty).toContain("thepastaclaw, Claudius-Maginificent and DCG-Claude");
    expect(SETTINGS_HELP.linked_logins.example).toBe(
      "A bot account that opens pull requests you take care of (e.g. Claudius-Maginificent): put it here so its pull requests count as yours, and keep it in Known bots so its reviews count as a bot's. Do not put it into Excluded authors.",
    );
  });

  it("Sign out everywhere is on the page and says what it ends", () => {
    const help = SETTINGS_HELP.sign_out_everywhere;
    expect(help.onPage).toBeUndefined();
    expect(help.when).toContain("Sign out everywhere");
    expect(help.what).toContain("revokes all your personal access tokens");
    expect(help.what).toContain("GitHub sign-in");
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
