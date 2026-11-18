import { describe, expect, it } from "vitest";
import type { DashboardSettings, RepositoryRule } from "../../../api/generated/types";
import { checkEntry, fromDraft, toDraft, validateSettings } from "./validation";

const DEFAULTS: DashboardSettings = {
  linked_logins: [],
  repositories: [],
  enumerate_owners: [],
  lookback_days: 7,
  ready_label: "ready-for-human",
  skip_authors: ["dependabot", "dependabot[bot]"],
  include_todos: true,
  memcan_extra_projects: [],
  schedule_enabled: true,
  schedule_time: "06:00",
  time_zone: "Europe/Warsaw",
};

const row = (repo: string, extra: Partial<RepositoryRule> = {}): RepositoryRule => {
  const [owner = "", name = ""] = repo.split("/");
  return {
    repo: { owner, name },
    alias: null,
    watch_issues: false,
    ownership_policy: null,
    branch_patterns: [],
    ...extra,
  };
};

const errors = (settings: Partial<DashboardSettings>) =>
  validateSettings({ ...DEFAULTS, ...settings }, "alice");

describe("dashboard settings validation (plan §F.6)", () => {
  it("accepts the defaults and a fully filled valid form", () => {
    expect(errors({})).toEqual({});
    expect(
      errors({
        linked_logins: ["alice-ci", "Corp_EMU"],
        repositories: [
          row("acme/platform", {
            alias: "PLAT",
            ownership_policy: { repo: { owner: "acme", name: "policies" }, path: "policies/platform.json" },
            branch_patterns: ["release/*", "vX.Y-dev", "main"],
          }),
          row("acme/.github"),
        ],
        enumerate_owners: ["acme"],
        lookback_days: 30,
        skip_authors: ["renovate[bot]"],
        memcan_extra_projects: ["release-notes", "a.b_c"],
        schedule_time: "23:45",
      }),
    ).toEqual({});
  });

  it("G2 linked logins: syntax, at most 5, unique, not the own login", () => {
    expect(errors({ linked_logins: ["bad login"] })).toHaveProperty("linked_logins");
    expect(errors({ linked_logins: ["a", "b", "c", "d", "e", "f"] }).linked_logins).toMatch(/at most 5/);
    expect(errors({ linked_logins: ["bob", "Bob"] }).linked_logins).toMatch(/twice/);
    expect(errors({ linked_logins: ["Alice"] }).linked_logins).toMatch(/your own login/);
  });

  it("G3 repositories: owner/name syntax, unique, at most 50 rows", () => {
    expect(errors({ repositories: [row("acme")] })).toHaveProperty(["repositories.0.repo"]);
    expect(errors({ repositories: [row("acme/..")] })).toHaveProperty(["repositories.0.repo"]);
    expect(errors({ repositories: [row("acme/a b")] })).toHaveProperty(["repositories.0.repo"]);
    expect(errors({ repositories: [row("acme/x"), row("ACME/X")] })).toHaveProperty(["repositories.1.repo"]);
    const many = Array.from({ length: 51 }, (_, i) => row(`acme/r${i}`));
    expect(errors({ repositories: many }).repositories).toMatch(/at most 50/);
  });

  it("G4 alias: pattern, unique across rows and the other rows' fallback names", () => {
    expect(errors({ repositories: [row("acme/x", { alias: "plat" })] })).toHaveProperty([
      "repositories.0.alias",
    ]);
    expect(errors({ repositories: [row("acme/x", { alias: "-X" })] })).toHaveProperty([
      "repositories.0.alias",
    ]);
    expect(errors({ repositories: [row("acme/x", { alias: "ABCDEFGHIJKLM" })] })).toHaveProperty([
      "repositories.0.alias",
    ]);
    expect(errors({ repositories: [row("acme/x", { alias: "ABCDEFGHIJKL" })] })).toEqual({});
    expect(
      errors({ repositories: [row("acme/x", { alias: "P" }), row("acme/y", { alias: "P" })] }),
    ).toHaveProperty(["repositories.1.alias"]);
    // Row 1 without an alias is shown as "core"; another row may not take that name.
    expect(errors({ repositories: [row("acme/x", { alias: "CORE" }), row("acme/core")] })).toHaveProperty([
      "repositories.0.alias",
    ]);
  });

  it("G6 ownership policy: both or neither, owner/name, relative .json path without ..", () => {
    const policy = (owner: string, name: string, path: string) =>
      errors({ repositories: [row("acme/x", { ownership_policy: { repo: { owner, name }, path } })] });
    expect(policy("acme", "p", "")["repositories.0.policy"]).toMatch(/both .* or neither/);
    expect(policy("", "", "a.json")["repositories.0.policy"]).toMatch(/both .* or neither/);
    expect(policy("acme", "", "a.json")).toHaveProperty(["repositories.0.policy"]);
    expect(policy("acme", "p", "/abs.json")).toHaveProperty(["repositories.0.policy"]);
    expect(policy("acme", "p", "a/../b.json")).toHaveProperty(["repositories.0.policy"]);
    expect(policy("acme", "p", "a.yaml")).toHaveProperty(["repositories.0.policy"]);
    expect(policy("acme", "p", "dir/a.json")).toEqual({});
  });

  it("G7 release branches: at most 10 patterns of the allowed characters", () => {
    const patterns = (branch_patterns: string[]) =>
      errors({ repositories: [row("acme/x", { branch_patterns })] });
    expect(patterns(["release/*", "vX.Y", "hotfix/1.2_a-b"])).toEqual({});
    expect(patterns(["rel ease"])).toHaveProperty(["repositories.0.branch_patterns"]);
    expect(patterns(["main?"])).toHaveProperty(["repositories.0.branch_patterns"]);
    expect(patterns(Array.from({ length: 11 }, (_, i) => `b${i}`))["repositories.0.branch_patterns"]).toMatch(
      /at most 10/,
    );
  });

  it("G8 owners to enumerate: login syntax, at most 5", () => {
    expect(errors({ enumerate_owners: ["a/b"] })).toHaveProperty("enumerate_owners");
    expect(errors({ enumerate_owners: ["a", "b", "c", "d", "e", "f"] }).enumerate_owners).toMatch(
      /at most 5/,
    );
  });

  it("G9–G11 rules: look-back 1–30 whole days, label 1–50 characters, bot logins at most 20", () => {
    expect(errors({ lookback_days: 0 })).toHaveProperty("lookback_days");
    expect(errors({ lookback_days: 31 })).toHaveProperty("lookback_days");
    expect(errors({ lookback_days: Number.NaN })).toHaveProperty("lookback_days");
    expect(errors({ lookback_days: 1 })).toEqual({});
    expect(errors({ ready_label: " " })).toHaveProperty("ready_label");
    expect(errors({ ready_label: "x".repeat(51) })).toHaveProperty("ready_label");
    expect(errors({ ready_label: "x".repeat(50) })).toEqual({});
    expect(errors({ skip_authors: ["bot[robot]"] })).toHaveProperty("skip_authors");
    expect(errors({ skip_authors: Array.from({ length: 21 }, (_, i) => `b${i}`) }).skip_authors).toMatch(
      /at most 20/,
    );
  });

  it("G17 MemCan projects: name syntax, unique, at most 10", () => {
    expect(errors({ memcan_extra_projects: ["has space"] })).toHaveProperty("memcan_extra_projects");
    expect(errors({ memcan_extra_projects: ["x".repeat(101)] })).toHaveProperty("memcan_extra_projects");
    expect(errors({ memcan_extra_projects: ["a", "a"] }).memcan_extra_projects).toMatch(/twice/);
    expect(
      errors({ memcan_extra_projects: Array.from({ length: 11 }, (_, i) => `p${i}`) }).memcan_extra_projects,
    ).toMatch(/at most 10/);
  });

  it("G15 time of day: HH:MM in 15-minute steps", () => {
    expect(errors({ schedule_time: "06:10" })).toHaveProperty("schedule_time");
    expect(errors({ schedule_time: "24:00" })).toHaveProperty("schedule_time");
    expect(errors({ schedule_time: "6:00" })).toHaveProperty("schedule_time");
  });

  it("every message names its field", () => {
    const all = errors({
      linked_logins: ["x y"],
      repositories: [row("bad")],
      lookback_days: 0,
      ready_label: "",
      schedule_time: "x",
    });
    expect(Object.values(all)).toEqual([
      expect.stringMatching(/^Linked logins/),
      expect.stringMatching(/^Repository 1/),
      expect.stringMatching(/^Look-back days/),
      expect.stringMatching(/^"Ready for a human" label/),
      expect.stringMatching(/^Time of day/),
    ]);
  });
});

describe("checkEntry: one value about to join a list", () => {
  it("refuses a bad, repeated or own value and one over the limit", () => {
    expect(checkEntry("linked_logins", "bob", [], "alice")).toBeNull();
    expect(checkEntry("linked_logins", "x y", [], "alice")).toMatch(/not a GitHub login/);
    expect(checkEntry("linked_logins", "BOB", ["bob"], "alice")).toMatch(/already/);
    expect(checkEntry("linked_logins", "alice", [], "alice")).toMatch(/your own login/);
    expect(checkEntry("linked_logins", "f", ["a", "b", "c", "d", "e"], "alice")).toMatch(/at most 5/);
    expect(checkEntry("skip_authors", "renovate[bot]", [], "alice")).toBeNull();
    expect(checkEntry("memcan_extra_projects", "notes.v2", [], "alice")).toBeNull();
  });
});

describe("form draft", () => {
  it("round-trips the settings and turns text fields into the wire shape", () => {
    const settings: DashboardSettings = {
      ...DEFAULTS,
      repositories: [
        row("acme/platform", {
          alias: "PLAT",
          ownership_policy: { repo: { owner: "acme", name: "policies" }, path: "p.json" },
          branch_patterns: ["release/*", "vX.Y"],
        }),
        row("acme/docs"),
      ],
    };
    expect(fromDraft(toDraft(settings))).toEqual(settings);
    const draft = toDraft(settings);
    draft.repositories[1] = {
      ...draft.repositories[1]!,
      repo: " acme/web ",
      alias: " ",
      branch_patterns: "main,  release/* hotfix/*",
    };
    draft.lookback_days = "x";
    draft.time_zone = "";
    const back = fromDraft(draft);
    expect(back.repositories[1]).toEqual(
      row("acme/web", { branch_patterns: ["main", "release/*", "hotfix/*"] }),
    );
    expect(back.lookback_days).toBeNaN();
    expect(back.time_zone).toBeNull();
  });

  it("keeps a half-filled policy so validation can name it", () => {
    const draft = toDraft({ ...DEFAULTS, repositories: [row("acme/x")] });
    draft.repositories[0]!.policy_path = "a.json";
    expect(fromDraft(draft).repositories[0]!.ownership_policy).toEqual({
      repo: { owner: "", name: "" },
      path: "a.json",
    });
  });
});
