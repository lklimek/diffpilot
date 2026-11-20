import { useId, type ReactNode } from "react";
import type {
  CrateChanges,
  DependencyCrate,
  DependencyRepository,
  DependencyView,
  HeadLine,
} from "../../api/generated/types";
import { CheckBadge } from "../badge/Badge";
import { GithubMark } from "../github/GithubMark";
import { Tooltip } from "../tooltip/Tooltip";
import { CrateFacts } from "./CrateFacts";
import { CrateNotes } from "./CrateNotes";
import { inApp, LineLinkTo } from "./LineLinkTo";
import { count, crateKey, crateOrigins, isOpen, repositoryKey, type DepsLinks } from "./model";

interface DependencyTreeProps {
  view: DependencyView;
  /** Levels the reader opened or closed, by key (`repositoryKey`, `crateKey`); the rest use their default. */
  open: Readonly<Record<string, boolean>>;
  onToggle: (key: string, open: boolean) => void;
  links: DepsLinks;
}

/** A level's disclosure button and, when open, its content. */
function Level({
  label,
  aside,
  open,
  onToggle,
  children,
}: {
  label: ReactNode;
  /** Chips and links beside the button (outside it: no control inside a control). */
  aside?: ReactNode;
  open: boolean;
  onToggle: (open: boolean) => void;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <li className="grid gap-1.5">
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          data-disclosure
          aria-expanded={open}
          aria-controls={open ? id : undefined}
          onClick={() => onToggle(!open)}
          className="min-h-6 cursor-pointer rounded px-1 text-left text-text before:mr-1.5 before:inline-block before:w-3 before:text-muted before:content-['▸'_/_''] hover:bg-accent-bg aria-expanded:before:content-['▾'_/_'']"
        >
          {label}
        </button>
        {aside}
      </div>
      {open && (
        <div id={id} className="ml-5 grid gap-2 border-l border-border pl-3">
          {children}
        </div>
      )}
    </li>
  );
}

const YOURS_TIP =
  "Its owner is this pull request's owner or is listed under Controlled organisations in Settings.";

function repositoryLabel(repo: DependencyRepository): ReactNode {
  const name = repo.repo ? `${repo.repo.owner}/${repo.repo.name}` : (repo.location ?? "");
  const suffix = {
    this: " · this repository",
    github: "",
    other_git: " — repository not looked up",
    registry: " — repository not looked up",
    path: " — path not read",
    unknown: " — source not determined",
  }[repo.kind];
  return (
    <>
      <span className="font-semibold [overflow-wrap:anywhere]">{name || "unknown source"}</span>
      {suffix} · {count(repo.crates.length, "crate")}
    </>
  );
}

function RepositoryAside({ repo }: { repo: DependencyRepository }) {
  return (
    <>
      {repo.controlled && (
        <Tooltip tip={YOURS_TIP}>
          <span
            tabIndex={0}
            className="cursor-help rounded border border-accent px-1.5 text-[11px] leading-[18px] font-semibold text-accent"
          >
            yours
          </span>
        </Tooltip>
      )}
      {repo.repo && (
        <Tooltip tip="Open the repository on GitHub (new tab)">
          <a
            href={`https://github.com/${repo.repo.owner}/${repo.repo.name}`}
            target="_blank"
            rel="noopener noreferrer"
            aria-label={`${repo.repo.owner}/${repo.repo.name} on GitHub (opens in a new tab)`}
            className="inline-flex size-6 items-center justify-center rounded border border-border"
          >
            <GithubMark />
          </a>
        </Tooltip>
      )}
    </>
  );
}

function crateLabel(crate: DependencyCrate): ReactNode {
  const parts = [
    crate.changes &&
      `${count(crate.changes.changed_lines, "line")} changed in ${count(crate.changes.files, "file")}`,
    crate.used_by.length > 0 && `used by ${crate.used_by.join(", ")}`,
    crate.facts_about > 0 && `${count(crate.facts_about, "conversion")} added for its types`,
  ].filter(Boolean);
  return (
    <>
      <span className="font-mono font-semibold">{crate.name}</span>
      {crate.version && ` ${crate.version}`}
      {parts.map((p) => ` · ${p}`)}
    </>
  );
}

const SOURCE_TIP = {
  "Cargo.lock": "Deterministic: read from the Cargo.lock record of the head commit; cargo was not run.",
  "Cargo.toml": "Deterministic: read from the manifest declaration at the head commit; cargo was not run.",
};

function Source({ at, links }: { at: HeadLine; links: DepsLinks }) {
  const file = at.file === "Cargo.lock" || at.file.endsWith("/Cargo.lock") ? "Cargo.lock" : "Cargo.toml";
  return (
    <span className="flex flex-wrap items-baseline gap-1.5">
      Source <CheckBadge text={file} tip={SOURCE_TIP[file]} />
      <LineLinkTo target={{ ...at, side: "head" }} links={links} />
    </span>
  );
}

function Changes({
  changes,
  view,
  links,
}: {
  changes: CrateChanges;
  view: DependencyView;
  links: DepsLinks;
}) {
  return (
    <>
      <p className="m-0 flex flex-wrap items-baseline gap-x-1.5">
        {changes.flows.length === 0 ? (
          "No flow has lines in it."
        ) : (
          <>
            In flows:
            {changes.flows.map((flow, index) => {
              const href = links.flow(flow.id);
              return (
                <span key={flow.id}>
                  <Tooltip tip="Opens this flow in the Flows view.">
                    <a href={href} onClick={(event) => inApp(event, () => links.onNavigate(href))}>
                      {flow.title}
                    </a>
                  </Tooltip>
                  {index < changes.flows.length - 1 && ","}
                </span>
              );
            })}
          </>
        )}
      </p>
      <section className="grid gap-1">
        <h3 className="text-sm font-semibold">Conversions added for other crates&apos; types</h3>
        <CrateFacts changes={changes} origins={crateOrigins(view)} links={links} />
      </section>
      <section className="grid gap-1">
        <h3 className="text-sm font-semibold">LLM notes on what changed</h3>
        <CrateNotes changes={changes} notes={view.notes} links={links} />
      </section>
      {changes.other_dependencies > 0 && (
        <p className="m-0 text-muted">
          {count(changes.other_dependencies, "other direct dependency is", "other direct dependencies are")}{" "}
          not listed (neither in this repository nor controlled).
        </p>
      )}
    </>
  );
}

function CrateDetails({
  crate,
  view,
  links,
}: {
  crate: DependencyCrate;
  view: DependencyView;
  links: DepsLinks;
}) {
  return (
    <>
      {(crate.manifest || crate.source_at || crate.revision) && (
        <p className="m-0 flex flex-wrap items-baseline gap-x-3 text-[12.5px]">
          {crate.manifest && (
            <span>
              Manifest{" "}
              <LineLinkTo target={{ file: crate.manifest, side: "head", line: 1 }} links={links}>
                {crate.manifest}
              </LineLinkTo>
            </span>
          )}
          {crate.source_at && <Source at={crate.source_at} links={links} />}
          {crate.revision && (
            <Tooltip tip={`Locked git commit: ${crate.revision}`}>
              <span tabIndex={0} className="cursor-help font-mono">
                revision {crate.revision.slice(0, 7)}
              </span>
            </Tooltip>
          )}
        </p>
      )}
      {crate.changes ? (
        <Changes changes={crate.changes} view={view} links={links} />
      ) : (
        <p className="m-0 text-muted">Not changed by this pull request.</p>
      )}
    </>
  );
}

/** Repositories → crates → per changed crate its facts and LLM notes; levels open on click. */
export function DependencyTree({ view, open, onToggle, links }: DependencyTreeProps) {
  return (
    <ul aria-label="Repositories" className="m-0 grid list-none gap-2 p-0">
      {view.repositories.map((repo) => {
        const key = repositoryKey(repo);
        return (
          <Level
            key={key}
            label={repositoryLabel(repo)}
            aside={<RepositoryAside repo={repo} />}
            open={isOpen(open, key, repo.kind === "this")}
            onToggle={(value) => onToggle(key, value)}
          >
            <ul
              aria-label={`Crates of ${repo.repo ? `${repo.repo.owner}/${repo.repo.name}` : (repo.location ?? "")}`}
              className="m-0 grid list-none gap-1.5 p-0"
            >
              {repo.crates.map((crate) => {
                const ck = crateKey(repo, crate);
                return (
                  <Level
                    key={ck}
                    label={crateLabel(crate)}
                    open={isOpen(open, ck, crate.changes !== null)}
                    onToggle={(value) => onToggle(ck, value)}
                  >
                    <CrateDetails crate={crate} view={view} links={links} />
                  </Level>
                );
              })}
            </ul>
          </Level>
        );
      })}
    </ul>
  );
}
