import { RuleLabel } from "../quiet/RuleLabel";
import type {
  NotAnalysedCategory,
  ProfileSource,
  ProjectProfileStatus,
  RepoSettings,
} from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { SettingsCard } from "./Card";

const SECTION: Record<NotAnalysedCategory, string> = {
  tests: "Tests",
  docs_comments: "Docs & comments",
  generated_mechanical: "Mechanical & generated",
};
const SOURCE: Record<ProfileSource, string> = {
  repository_file: ".claudius/project-profile.md on the base ref",
  memcan: "MemCan (type: project-profile)",
};
const COLUMNS = [
  ["Rule", "The path pattern, line expression or detector description."],
  ["Kind", "The detector used: path glob, line regex, lexer, generated-file header or binary content."],
  ["Section", "The Not analysed section that matching material goes to."],
] as const;

function profileText({ sources, memcan }: ProjectProfileStatus): string {
  if (memcan === "not_checked") {
    const file = sources.includes("repository_file")
      ? SOURCE.repository_file
      : "No .claudius/project-profile.md on the base ref";
    return `${file}. Project profile in MemCan: not checked.`;
  }
  if (sources.length === 0) {
    return "No project profile: neither .claudius/project-profile.md on the base ref nor a MemCan profile. Analyses run without it.";
  }
  const names = sources.map((s) => SOURCE[s]).join(" and ");
  return sources.length > 1 ? `${names}; the repository file wins on conflict.` : `${names}.`;
}

/** Read-only settings of one repository: built-in Not analysed rules, related repositories, profile source. */
export function RepoSettingsCard({ settings }: { settings: RepoSettings }) {
  const { repo, rules, related_repos, project_profile } = settings;
  return (
    <SettingsCard
      title={
        <>
          Repository <code>{`${repo.owner}/${repo.name}`}</code>{" "}
          <span className="text-xs font-normal text-muted">read-only</span>
        </>
      }
    >
      <h3 className="text-sm font-semibold">Not analysed: built-in rules</h3>
      {rules.length === 0 ? (
        <p>No built-in rules apply to this repository.</p>
      ) : (
        <details>
          <summary className="cursor-pointer text-sm">Show the {rules.length} built-in rules</summary>
          <div className="max-h-80 overflow-auto">
            <table className="w-full table-fixed border-collapse text-xs">
              <thead>
                <tr>
                  {COLUMNS.map(([name, tip]) => (
                    <th
                      key={name}
                      className="border-b border-border px-2.5 py-1.5 text-left text-[11px] tracking-[.4px] text-muted uppercase"
                    >
                      <Tooltip tip={tip}>
                        <span tabIndex={0} className="cursor-help">
                          {name}
                        </span>
                      </Tooltip>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rules.map((rule) => (
                  <tr key={`${rule.kind}:${rule.pattern}:${rule.category}`}>
                    <td className="border-b border-border px-2.5 py-2 font-mono [overflow-wrap:anywhere]">
                      {rule.pattern}
                    </td>
                    <td className="border-b border-border px-2.5 py-2">
                      <RuleLabel kind={rule.kind} />
                    </td>
                    <td className="border-b border-border px-2.5 py-2">{SECTION[rule.category]}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </details>
      )}
      <h3 className="mt-2 text-sm font-semibold">Related repositories</h3>
      {related_repos.length === 0 ? (
        <p className="text-xs">
          None detected (same organisation, from Cargo.toml and Cargo.lock on the base ref).
        </p>
      ) : (
        <ul className="flex flex-wrap gap-1.5">
          {related_repos.map((r) => (
            <li
              key={`${r.owner}/${r.name}`}
              className="rounded border border-border bg-surface-2 px-2 font-mono text-xs"
            >
              {`${r.owner}/${r.name}`}
            </li>
          ))}
        </ul>
      )}
      <h3 className="mt-2 text-sm font-semibold">Project profile source</h3>
      <p className="text-xs">{profileText(project_profile)}</p>
    </SettingsCard>
  );
}
