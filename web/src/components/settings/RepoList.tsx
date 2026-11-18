import { useId, useState } from "react";
import type { RepoRef } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";

const name = (repo: RepoRef | null) => (repo ? `${repo.owner}/${repo.name}` : "");

/** Searchable repository choice; input order is most recently analysed first. */
export function RepoList({
  repos,
  selected,
  onSelect,
}: {
  repos: RepoRef[];
  /** The chosen repository; null when none is (e.g. the URL named an unknown one). */
  selected: RepoRef | null;
  onSelect: (repo: RepoRef) => void;
}) {
  const id = useId();
  const [text, setText] = useState(name(selected));
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const query = text === name(selected) ? "" : text.toLowerCase();
  const filtered = repos.filter((repo) => name(repo).toLowerCase().includes(query));
  const choose = (repo: RepoRef) => {
    setText(name(repo));
    setOpen(false);
    setActive(-1);
    onSelect(repo);
  };
  return (
    <div className="relative min-w-0">
      <label htmlFor={id} className="text-sm">
        Repository
      </label>
      <Tooltip tip="Type an owner or repository name to filter. Use arrow keys and Enter to choose.">
        <input
          id={id}
          role="combobox"
          aria-autocomplete="list"
          aria-expanded={open}
          aria-controls={`${id}-list`}
          aria-activedescendant={open && active >= 0 ? `${id}-${active}` : undefined}
          autoComplete="off"
          value={text}
          className="w-full min-w-0 rounded border border-border bg-surface px-2 py-1 font-mono text-sm"
          onFocus={() => setOpen(true)}
          onBlur={() => {
            setOpen(false);
            setText(name(selected));
          }}
          onChange={(e) => {
            setText(e.target.value);
            setOpen(true);
            setActive(-1);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              setOpen(true);
              setActive((i) =>
                filtered.length === 0
                  ? -1
                  : Math.max(0, Math.min(filtered.length - 1, i + (e.key === "ArrowDown" ? 1 : -1))),
              );
            }
            if (e.key === "Enter" && open && filtered[active]) {
              e.preventDefault();
              choose(filtered[active]);
            }
            if (e.key === "Escape") {
              setOpen(false);
              setText(name(selected));
              setActive(-1);
            }
          }}
        />
      </Tooltip>
      {open && (
        <ul
          id={`${id}-list`}
          role="listbox"
          aria-label="Repositories"
          className="absolute z-20 mt-1 max-h-64 w-full overflow-y-auto rounded border border-border bg-surface shadow-lg"
        >
          {filtered.map((repo, i) => (
            <li
              id={`${id}-${i}`}
              role="option"
              aria-selected={i === active}
              key={name(repo)}
              className={`cursor-pointer px-2 py-1 font-mono text-sm [overflow-wrap:anywhere] ${i === active ? "bg-accent-bg" : ""}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => choose(repo)}
            >
              {name(repo)}
            </li>
          ))}
          {filtered.length === 0 && <li className="p-2 text-sm">No matching repositories.</li>}
        </ul>
      )}
    </div>
  );
}
