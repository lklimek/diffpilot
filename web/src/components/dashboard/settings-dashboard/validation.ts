// Client mirror of the dashboard settings rules of plan §F.6; the server stays authoritative (422 names the field).
import type { DashboardSettings, RepoRef } from "../../../api/generated/types";

/** Built-in release and long-lived branch patterns (plan §C.2 K9), used when a row lists none. */
export const BUILT_IN_BRANCHES = ["main", "master", "release/*", "hotfix/*", "vX.Y", "vX.Y-dev"];

/** Messages by field key (`linked_logins`, `repositories.0.alias`, …), in form order. */
export type FieldErrors = Record<string, string>;

/** Lists edited one entry at a time. */
export type ListField = "linked_logins" | "enumerate_owners" | "skip_authors" | "memcan_extra_projects";

const LOGIN = /^[A-Za-z0-9_-]{1,39}$/;
const REPO_NAME = /^[A-Za-z0-9._-]{1,100}$/;
const ALIAS = /^[A-Z0-9][A-Z0-9-]{0,11}$/;
const BRANCH = /^[A-Za-z0-9._/*-]+$/;
const TIME = /^([01][0-9]|2[0-3]):(00|15|30|45)$/;

const LISTS: Record<ListField, { label: string; max: number; valid: RegExp; rule: string }> = {
  linked_logins: {
    label: "Linked logins",
    max: 5,
    valid: LOGIN,
    rule: "is not a GitHub login (letters, digits, - or _, up to 39)",
  },
  enumerate_owners: {
    label: "Owners to list",
    max: 5,
    valid: LOGIN,
    rule: "is not a GitHub login (letters, digits, - or _, up to 39)",
  },
  skip_authors: {
    label: "Bot authors to skip",
    max: 20,
    valid: /^[A-Za-z0-9_-]{1,39}(\[bot\])?$/,
    rule: "is not a GitHub login (optionally ending [bot])",
  },
  memcan_extra_projects: {
    label: "Extra MemCan projects",
    max: 10,
    valid: /^[A-Za-z0-9._-]{1,100}$/,
    rule: "is not a project name (letters, digits, . _ or -, up to 100)",
  },
};

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Why `value` cannot join `list` (syntax, duplicate, own login, limit), or null. */
export function checkEntry(field: ListField, value: string, list: string[], ownLogin: string): string | null {
  const { label, max, valid, rule } = LISTS[field];
  if (!valid.test(value)) return `${label}: "${value}" ${rule}.`;
  // Logins are case-insensitive on GitHub; project names are compared as written.
  const equal =
    field === "memcan_extra_projects" ? (a: string) => a === value : (a: string) => same(a, value);
  if (list.some(equal)) return `${label}: "${value}" is already listed.`;
  if (field === "linked_logins" && same(value, ownLogin)) {
    return `${label}: "${value}" is your own login, which is always scanned.`;
  }
  if (list.length >= max) return `${label}: at most ${max}.`;
  return null;
}

/** Why a whole list breaks its rules, or null. */
function checkList(field: ListField, list: string[], ownLogin: string): string | null {
  const { label, max } = LISTS[field];
  if (list.length > max) return `${label}: at most ${max}.`;
  for (const [index, value] of list.entries()) {
    const problem = checkEntry(field, value, list.slice(0, index), ownLogin);
    if (problem) return problem.replace("is already listed", "is listed twice");
  }
  return null;
}

const validRepo = (repo: RepoRef) =>
  LOGIN.test(repo.owner) && REPO_NAME.test(repo.name) && repo.name !== "." && repo.name !== "..";

const repoText = (repo: RepoRef) => `${repo.owner}/${repo.name}`;

const validPolicyPath = (path: string) =>
  path.endsWith(".json") && !path.startsWith("/") && !path.split("/").includes("..");

/** Every rule of plan §F.6 the client can check; empty when the settings may be saved. */
export function validateSettings(settings: DashboardSettings, ownLogin: string): FieldErrors {
  const errors: FieldErrors = {};
  const put = (field: string, message: string | null) => {
    if (message && !(field in errors)) errors[field] = message;
  };
  put("linked_logins", checkList("linked_logins", settings.linked_logins, ownLogin));
  const rows = settings.repositories;
  if (rows.length > 50) put("repositories", "Repositories: at most 50.");
  const prefix = (index: number) => rows[index]!.alias ?? rows[index]!.repo.name;
  rows.forEach((row, i) => {
    const label = `Repository ${i + 1}`;
    if (!validRepo(row.repo)) put(`repositories.${i}.repo`, `${label}: use owner/name.`);
    else if (rows.slice(0, i).some((r) => same(repoText(r.repo), repoText(row.repo)))) {
      put(`repositories.${i}.repo`, `${label}: ${repoText(row.repo)} is listed twice.`);
    }
    if (row.alias !== null) {
      if (!ALIAS.test(row.alias)) {
        put(
          `repositories.${i}.alias`,
          `${label}: the short name uses capital letters, digits and -, starts with a letter or digit, up to 12.`,
        );
      } else if (rows.some((_, j) => j !== i && same(prefix(j), row.alias!))) {
        put(`repositories.${i}.alias`, `${label}: the short name ${row.alias} is used by another row.`);
      }
    }
    const policy = row.ownership_policy;
    if (policy) {
      const repoGiven = policy.repo.owner !== "" || policy.repo.name !== "";
      if (!repoGiven || policy.path === "") {
        put(
          `repositories.${i}.policy`,
          `${label}: give both the policy repository and its path, or neither.`,
        );
      } else if (!validRepo(policy.repo)) {
        put(`repositories.${i}.policy`, `${label}: the policy repository uses owner/name.`);
      } else if (!validPolicyPath(policy.path)) {
        put(
          `repositories.${i}.policy`,
          `${label}: the policy path is relative, ends with .json and has no "..".`,
        );
      }
    }
    if (row.branch_patterns.length > 10) {
      put(`repositories.${i}.branch_patterns`, `${label}: at most 10 release branch patterns.`);
    } else {
      const bad = row.branch_patterns.find((p) => !BRANCH.test(p));
      if (bad !== undefined) {
        put(
          `repositories.${i}.branch_patterns`,
          `${label}: "${bad}" may use only letters, digits, . _ / - and *.`,
        );
      }
    }
  });
  put("enumerate_owners", checkList("enumerate_owners", settings.enumerate_owners, ownLogin));
  const days = settings.lookback_days;
  if (!Number.isInteger(days) || days < 1 || days > 30) {
    put("lookback_days", "Look-back days: use a whole number from 1 to 30.");
  }
  const label = settings.ready_label.trim();
  if (label === "" || [...label].length > 50) {
    put("ready_label", '"Ready for a human" label: use 1 to 50 characters.');
  }
  put("skip_authors", checkList("skip_authors", settings.skip_authors, ownLogin));
  put("memcan_extra_projects", checkList("memcan_extra_projects", settings.memcan_extra_projects, ownLogin));
  if (!TIME.test(settings.schedule_time)) {
    put("schedule_time", "Time of day: use HH:MM in steps of 15 minutes.");
  }
  return errors;
}

/** A repository row as edited: text fields, split on save. */
export interface RepositoryDraft {
  repo: string;
  alias: string;
  watch_issues: boolean;
  policy_repo: string;
  policy_path: string;
  /** Patterns separated by spaces or commas; empty = the built-ins. */
  branch_patterns: string;
}

/** The settings form as edited. */
export interface SettingsDraft extends Omit<
  DashboardSettings,
  "repositories" | "lookback_days" | "time_zone"
> {
  repositories: RepositoryDraft[];
  lookback_days: string;
  /** Empty = no time zone stored yet. */
  time_zone: string;
}

/** An empty repository row. */
export const EMPTY_ROW: RepositoryDraft = {
  repo: "",
  alias: "",
  watch_issues: false,
  policy_repo: "",
  policy_path: "",
  branch_patterns: "",
};

/** The form of stored settings. */
export function toDraft(settings: DashboardSettings): SettingsDraft {
  return {
    ...settings,
    repositories: settings.repositories.map((row) => ({
      repo: repoText(row.repo),
      alias: row.alias ?? "",
      watch_issues: row.watch_issues,
      policy_repo: row.ownership_policy ? repoText(row.ownership_policy.repo) : "",
      policy_path: row.ownership_policy?.path ?? "",
      branch_patterns: row.branch_patterns.join(", "),
    })),
    lookback_days: String(settings.lookback_days),
    time_zone: settings.time_zone ?? "",
  };
}

/** `owner/name` split at the first slash; anything else keeps an empty part so validation names it. */
function parseRepo(text: string): RepoRef {
  const slash = text.indexOf("/");
  return slash < 0 ? { owner: text, name: "" } : { owner: text.slice(0, slash), name: text.slice(slash + 1) };
}

/** The wire settings of a form; validate the result with {@link validateSettings}. */
export function fromDraft(draft: SettingsDraft): DashboardSettings {
  const days = draft.lookback_days.trim();
  return {
    ...draft,
    repositories: draft.repositories.map((row) => {
      const policyRepo = row.policy_repo.trim();
      const policyPath = row.policy_path.trim();
      const alias = row.alias.trim();
      return {
        repo: parseRepo(row.repo.trim()),
        alias: alias === "" ? null : alias,
        watch_issues: row.watch_issues,
        ownership_policy:
          policyRepo === "" && policyPath === ""
            ? null
            : { repo: policyRepo === "" ? { owner: "", name: "" } : parseRepo(policyRepo), path: policyPath },
        branch_patterns: row.branch_patterns.split(/[\s,]+/).filter((p) => p !== ""),
      };
    }),
    lookback_days: /^[0-9]+$/.test(days) ? Number(days) : Number.NaN,
    time_zone: draft.time_zone === "" ? null : draft.time_zone,
  };
}
