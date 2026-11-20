import { useId } from "react";
import type { DependencyLimit, DependencyView } from "../../api/generated/types";
import { Tooltip } from "../tooltip/Tooltip";
import { inApp } from "./LineLinkTo";
import { count, hasChangedCrate, type DepsLinks } from "./model";

/** The sentence of a repository without Cargo (brief 2026-11-20: Not available). */
export const NOT_RUST =
  "Dependencies: not available — this view reads Cargo (Rust) workspaces only; nothing was checked.";

function limitText(limit: DependencyLimit): string {
  switch (limit.kind) {
    case "manifests_cut":
      return `Only ${limit.read} of ${limit.total} Cargo.toml files were read (the first in path order); crates of the others are missing.`;
    case "manifest_unreadable":
      return `${limit.path} was not read: too large or does not parse.`;
    case "lock_unreadable":
      return `${limit.path} was not read: too large or does not parse.`;
    case "no_lock_file":
      return `No Cargo.lock in the workspace ${limit.workspace_dir === "" ? "at the repository root" : limit.workspace_dir}: versions are not resolved; sources come from the manifests.`;
    case "source_unread":
      return `${limit.path} was not read: its conversions are not checked.`;
    case "facts_cut":
      return `Only the first ${limit.kept} conversion facts in file order are kept.`;
  }
}

/** What the map covers, before the tree: not available, its limits, files outside crates, owners, notes. */
export function DependencyNotices({
  view,
  prOwner,
  links,
}: {
  view: DependencyView;
  /** Owner of the pull request's repository (from the analysis): always yours, even with an empty map. */
  prOwner: string;
  links: DepsLinks;
}) {
  const id = useId();
  if (view.status === "not_rust") return <p className="m-0">{NOT_RUST}</p>;
  const others = view.controlled_owners.filter((o) => o.toLowerCase() !== prOwner.toLowerCase());
  const outside = view.files_outside_crates;
  // Absence is claimed only for a complete map: with any limit or no crate listed it was not established.
  const cut = view.limits.length > 0 || view.repositories.length === 0;
  const settings = (
    <Tooltip tip="Opens Settings, where the installation's controlled organisations are listed.">
      <a href={links.settings} onClick={(event) => inApp(event, () => links.onNavigate(links.settings))}>
        Controlled organisations
      </a>
    </Tooltip>
  );
  return (
    <div className="grid gap-2">
      {view.limits.length > 0 && (
        <section
          aria-labelledby={id}
          className="rounded-md border border-l-4 border-med-bd bg-med-bg px-3.5 py-2.5"
        >
          <h2 id={id} className="text-sm font-semibold">
            This map is incomplete
          </h2>
          <ul className="m-0 grid list-disc gap-0.5 pl-5">
            {view.limits.map((limit, index) => (
              <li key={index} className="[overflow-wrap:anywhere]">
                {limitText(limit)}
              </li>
            ))}
          </ul>
        </section>
      )}
      {!hasChangedCrate(view) ? (
        cut ? (
          <p className="m-0">
            No crate that was read is changed by this pull request; {count(outside, "changed file")}{" "}
            {outside === 1 ? "is" : "are"} in no read crate and {outside === 1 ? "is" : "are"} not checked.
          </p>
        ) : (
          <p className="m-0">
            No Rust crate is changed by this pull request; {count(outside, "changed file")} outside crates{" "}
            {outside === 1 ? "is" : "are"} not checked.
          </p>
        )
      ) : (
        outside > 0 && (
          <p className="m-0">
            {count(outside, "changed file is", "changed files are")} outside any Cargo crate (not checked).
          </p>
        )
      )}
      <p className="m-0">
        Yours: {prOwner} (this pull request&apos;s owner)
        {others.length > 0
          ? `, ${others.join(", ")}.`
          : ". No other organisation is listed in Settings."}{" "}
        {settings}
      </p>
      {view.notes === "pending" && (
        <p className="m-0 text-muted">
          LLM notes are not written yet: they come with the plan of this analysis.
        </p>
      )}
      {view.notes === "not_produced" && (
        <p className="m-0 text-muted">LLM notes were not produced for this analysis.</p>
      )}
    </div>
  );
}
