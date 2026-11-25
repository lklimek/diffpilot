import type { WorkItem, FactTag } from "../../../api/generated/types";

const FIELDS = ["label", "size", "author", "repo"] as const;
export type FilterField = (typeof FIELDS)[number];
export type FilterValue = { field: FilterField; key: string; label: string };
export type Filter = Pick<FilterValue, "field" | "key"> & { excluded: boolean };
export const filterId = (filter: Pick<FilterValue, "field" | "key">) => `${filter.field}:${filter.key}`;

export function decodeFilters(value: string | null): Filter[] {
  const filters = new Map<string, Filter>();
  for (const token of value?.split(",") ?? []) {
    const match = /^(-?)(label|size|author|repo):(.+)$/.exec(token);
    if (!match) continue;
    try {
      const filter = {
        field: match[2] as FilterField,
        key: decodeURIComponent(match[3]!),
        excluded: match[1] === "-",
      };
      filters.set(filterId(filter), filter);
    } catch {
      // A malformed URI component is ignored; the other chips remain active.
    }
  }
  return [...filters.values()];
}

export const encodeFilters = (filters: readonly Filter[]) =>
  filters
    .map(
      ({ field, key, excluded }) =>
        `${excluded ? "-" : ""}${field}:${encodeURIComponent(key).replaceAll("%2F", "/")}`,
    )
    .join(",");

export function tagFilter(tag: FactTag): FilterValue {
  if (tag.filter) return { field: "label", ...tag.filter };
  const label = tag.text
    .replace(/^(\d+ )(?=unresolved threads?$)/i, "")
    .replace(/^unresolved thread$/i, "unresolved threads")
    .replace(/^(not ready): .+$/i, "$1")
    .replace(/^(review requested)(?: \d+ [dhm]| today)$/i, "$1");
  return { field: "label", key: label.toLowerCase().replaceAll(/\s+/g, "_"), label };
}

export const repoFilter = (item: WorkItem): FilterValue | null => {
  if (!item.repo) return null;
  const label = `${item.repo.owner}/${item.repo.name}`;
  return { field: "repo", key: label.toLowerCase(), label };
};

export const authorFilter = (author: string): FilterValue => ({
  field: "author",
  key: author.toLowerCase(),
  label: `@${author}`,
});

export const sizeFilter = (size: string): FilterValue => ({ field: "size", key: size, label: size });

export function itemFilters(item: WorkItem, tags: readonly FilterValue[]): FilterValue[] {
  const values = [...tags];
  const repo = repoFilter(item);
  if (repo) values.push(repo);
  if (item.author) values.push(authorFilter(item.author));
  if (item.change_size) values.push(sizeFilter(item.change_size.size));
  return values;
}

export function matchesFilters(
  item: WorkItem,
  filters: readonly Filter[],
  tags: readonly FilterValue[],
): boolean {
  if (!filters.length || item.score.tier === "p0") return true;
  const ids = new Set(itemFilters(item, tags).map(filterId));
  return (
    !filters.some((filter) => filter.excluded && ids.has(filterId(filter))) &&
    FIELDS.every((field) => {
      const included = filters.filter((filter) => filter.field === field && !filter.excluded);
      return !included.length || included.some((filter) => ids.has(filterId(filter)));
    })
  );
}

export function addFilter(filters: readonly Filter[], filter: FilterValue, excluded: boolean): Filter[] {
  const { field, key } = filter;
  const next = { field, key, excluded };
  return filters.some((value) => filterId(value) === filterId(filter))
    ? filters.map((value) => (filterId(value) === filterId(filter) ? next : value))
    : [...filters, next];
}

export function filterLabel(filter: Filter): string {
  if (filter.field === "author") return `@${filter.key}`;
  if (filter.key.startsWith("ci_")) return `CI ${filter.key.slice(3).replaceAll("_", " ")}`;
  return filter.field === "label" ? filter.key.replaceAll("_", " ") : filter.key;
}
