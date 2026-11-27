import type {
  DependencyCrate,
  DependencyRepository,
  DependencyView,
  FlowId,
  LineSide,
} from "../../api/generated/types";

/** Lines of a file at one side of the analysed pull request. */
export interface LineTarget {
  file: string;
  side: LineSide;
  line: number;
  end?: number;
}

/** Where a line opens: the Files view (in-app) or GitHub at the analysed commit (new tab). */
export interface LineLink {
  href: string;
  external: boolean;
}

/** Links of the Dependencies view; the screen decides where they lead. */
export interface DepsLinks {
  line: (target: LineTarget) => LineLink;
  /** In-app link to a flow in the Flows view. */
  flow: (id: FlowId) => string;
  /** In-app link to Settings. */
  settings: string;
  crate?: (key: string) => string;
  /** Follows an in-app link (a plain click). */
  onNavigate: (href: string) => void;
}

/** Stable key of a repository level (open state); `index` tells apart sources with neither name nor location. */
export function repositoryKey(repo: DependencyRepository, index: number): string {
  if (repo.kind === "this") return "this";
  const name = repo.repo ? `${repo.repo.owner}/${repo.repo.name}` : repo.location;
  return `${repo.kind}:${name ?? `#${index}`}`;
}

/** Stable key of a crate level (open state): one per package version of its source. */
export const crateKey = (repoKey: string, crate: DependencyCrate): string =>
  `${repoKey}/${crate.name}@${crate.version ?? ""}`;

export const unchangedCrate = (crate: DependencyCrate) => crate.changes === null && crate.facts_about === 0;

/** A dependency deep link identifies one crate including its source and version. */
export const crateHash = (key: string) => `#crate=${encodeURIComponent(key)}`;
export const crateFromHash = (hash: string) => new URLSearchParams(hash.slice(1)).get("crate");

/** Open unless toggled: this repository and changed crates start open, everything else closed. */
export const isOpen = (open: Readonly<Record<string, boolean>>, key: string, byDefault: boolean) =>
  open[key] ?? byDefault;

/** Whether any crate of the map is changed by the pull request. */
export const hasChangedCrate = (view: DependencyView) =>
  view.repositories.some((r) => r.crates.some((c) => c.changes !== null));

/** `n thing` or `n things`. */
export const count = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** Where a crate of the map comes from: its repository's name and whether it is yours. */
export interface CrateOrigin {
  repository: string;
  yours: boolean;
}

/** Origins of the map's crates by package name (the first listing wins). */
export function crateOrigins(view: DependencyView): Record<string, CrateOrigin> {
  const origins: Record<string, CrateOrigin> = {};
  for (const repo of view.repositories) {
    const repository =
      repo.kind === "this"
        ? "this repository"
        : repo.repo
          ? `${repo.repo.owner}/${repo.repo.name}`
          : (repo.location ?? "an unknown source");
    for (const crate of repo.crates)
      origins[crate.name] ??= { repository, yours: repo.controlled && repo.kind !== "this" };
  }
  return origins;
}
