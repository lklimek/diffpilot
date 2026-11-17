import type { RepoRef } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { Tooltip } from "../tooltip/Tooltip";

interface RepoListProps {
  /** Repositories with settings. */
  repos: RepoRef[];
  /** The one shown. */
  selected: RepoRef;
  /** Called with the chosen repository. */
  onSelect: (repo: RepoRef) => void;
}

const name = (repo: RepoRef) => `${repo.owner}/${repo.name}`;

/** Choice of the repository whose read-only settings are shown. */
export function RepoList({ repos, selected, onSelect }: RepoListProps) {
  return (
    <ul aria-label="Repositories" className="flex flex-wrap gap-2">
      {repos.map((repo) => (
        <li key={name(repo)}>
          <Tooltip tip="Shows the read-only settings of this repository.">
            <button
              type="button"
              aria-pressed={name(repo) === name(selected)}
              onClick={() => onSelect(repo)}
              className={`${buttonClass("plain", true)} font-mono aria-pressed:border-accent aria-pressed:bg-accent-bg`}
            >
              {name(repo)}
            </button>
          </Tooltip>
        </li>
      ))}
    </ul>
  );
}
