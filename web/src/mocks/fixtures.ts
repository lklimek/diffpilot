// Fixture PR acme/node-client#482 (fixtures/pr-482/README.md): the default API seed plus the RFC 7396 state
// patches, one document `{"<seed path without .json>": response}` per state.
import type { LineSide, LinesResponse } from "../api/generated/types";
const SEED_DIR = "../../../fixtures/pr-482/api-seed/default/";
const STATES_DIR = "../../../fixtures/pr-482/api-states/";

const seedFiles = import.meta.glob<unknown>("../../../fixtures/pr-482/api-seed/default/**/*.json", {
  eager: true,
  import: "default",
});
const stateFiles = import.meta.glob<unknown>("../../../fixtures/pr-482/api-states/*.json", {
  eager: true,
  import: "default",
});

/** The fixture states of plan §12.4 / http-api.md. */
export const FIXTURE_STATES = ["default", "wrong", "missed", "noq", "noprofile", "resume"] as const;

/** One fixture state. */
export type FixtureState = (typeof FIXTURE_STATES)[number];

type Json = unknown;
const isObject = (value: Json): value is Record<string, Json> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** RFC 7396 JSON merge patch; returns a new value. */
export function mergePatch(target: Json, patch: Json): Json {
  if (!isObject(patch)) return patch;
  const result: Record<string, Json> = isObject(target) ? { ...target } : {};
  for (const [key, value] of Object.entries(patch)) {
    if (value === null) delete result[key];
    else result[key] = mergePatch(result[key], value);
  }
  return result;
}

const seed: Record<string, Json> = Object.fromEntries(
  Object.entries(seedFiles).map(([file, json]) => [file.slice(SEED_DIR.length, -".json".length), json]),
);
const documents = new Map<FixtureState, Record<string, Json>>();

function documentFor(state: FixtureState): Record<string, Json> {
  if (!FIXTURE_STATES.includes(state)) throw new Error(`unknown fixture state ${String(state)}`);
  let document = documents.get(state);
  if (!document) {
    const patch = state === "default" ? {} : stateFiles[`${STATES_DIR}${state}.json`];
    document = mergePatch(seed, patch) as Record<string, Json>;
    documents.set(state, document);
  }
  return document;
}

/** The response stored at `path` (e.g. `tour`, `lines/head/src/lib.rs`) in a state; undefined if none. */
export function seedFor(state: FixtureState, path: string): Json {
  return structuredClone(documentFor(state)[path]);
}

let current: FixtureState = "default";

/** Selects the fixture state the handlers answer from (runtime switch). */
export function setFixtureState(state: FixtureState): void {
  documentFor(state);
  current = state;
}

/** The selected fixture state. */
export function fixtureState(): FixtureState {
  return current;
}

/** The response at `path` in the selected state. */
export function seedNow(path: string): Json {
  return seedFor(current, path);
}

/** Lines `from..=to` (clamped) of a seeded file with the gutter facts inside the range; undefined if unknown. */
export function linesFor(
  state: FixtureState,
  path: string,
  side: LineSide,
  from: number,
  to: number,
): LinesResponse | undefined {
  const file = seedFor(state, `lines/${side}/${path}`) as LinesResponse | undefined;
  if (!file) return undefined;
  const end = Math.min(to, file.total_lines);
  const inRange = (line: number) => line >= from && line <= end;
  return {
    ...file,
    from,
    to: end,
    lines: file.lines.slice(from - 1, end),
    changed: file.changed.filter(inRange),
    deleted: file.deleted.filter((d) => d.after_line >= from - 1 && d.after_line <= end),
    moved: file.moved
      .filter((m) => m.range.start_line <= end && m.range.end_line >= from)
      .map((m) => ({ ...m, differing: m.differing.filter(inRange) })),
  };
}
