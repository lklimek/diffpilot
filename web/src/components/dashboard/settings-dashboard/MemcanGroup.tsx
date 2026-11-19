import { useId } from "react";
import { HelpTip } from "../../settings/HelpTip";
import { ListEditor } from "./ListEditor";
import type { FieldErrors } from "./validation";

interface MemcanGroupProps {
  includeTodos: boolean;
  /** MemCan is configured on this server; without it the switch is off and says why. */
  configured: boolean;
  /** Short names of the repository rows: always read, not editable here. */
  repoProjects: string[];
  extraProjects: string[];
  errors: FieldErrors;
  /** Why a project name cannot be added, or null. */
  checkProject: (value: string, items: string[]) => string | null;
  onChange: (change: { include_todos?: boolean; memcan_extra_projects?: string[] }) => void;
}

/** MemCan TODOs on the dashboard (plan §F.6 G12, G17): the switch and the projects read. */
export function MemcanGroup({
  includeTodos,
  configured,
  repoProjects,
  extraProjects,
  errors,
  checkProject,
  onChange,
}: MemcanGroupProps) {
  const id = useId();
  return (
    <div className="grid content-start gap-3">
      <div className="grid gap-1">
        <span className="flex items-center gap-2">
          <label className="flex items-center gap-2 text-sm">
            <input
              type="checkbox"
              checked={configured && includeTodos}
              disabled={!configured}
              aria-describedby={configured ? undefined : `${id}-off`}
              className="size-4"
              onChange={(e) => onChange({ include_todos: e.target.checked })}
            />
            Show my MemCan TODOs
          </label>
          <HelpTip topic="include_todos" />
        </span>
        {!configured && (
          <p id={`${id}-off`} className="text-xs text-muted">
            MemCan is not configured on this server, so there are no TODOs to show.
          </p>
        )}
      </div>
      <div className="grid gap-1.5">
        <span id={`${id}-projects`} className="text-sm font-semibold">
          MemCan projects
        </span>
        {repoProjects.length > 0 ? (
          <ul aria-labelledby={`${id}-projects`} className="m-0 flex list-none flex-wrap gap-1.5 p-0">
            {repoProjects.map((project) => (
              <li
                key={project}
                className="rounded border border-border px-1.5 py-0.5 font-mono text-[12.5px] text-muted"
              >
                {project}
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-xs text-muted">No repository rows yet.</p>
        )}
        <p className="text-xs text-muted">
          The short names of your repositories are always read; add other project names below.
        </p>
      </div>
      <ListEditor
        label="Extra MemCan projects"
        help={<HelpTip topic="memcan_extra_projects" />}
        addLabel="Add a MemCan project"
        items={extraProjects}
        empty="None."
        placeholder="project-name"
        check={checkProject}
        error={errors.memcan_extra_projects}
        onChange={(memcan_extra_projects) => onChange({ memcan_extra_projects })}
      />
    </div>
  );
}
