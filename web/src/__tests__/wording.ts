// Trust and wording rules of brief §1 / plan §12.4, asserted on rendered text.
import { expect } from "vitest";
import type { Tour } from "../api/generated/types";
import { FIXTURE_STATES, seedFor, type FixtureState } from "../mocks/fixtures";

const BANNED = [
  /\bno\s+risks?\b/i,
  /\bnothing\s+to\s+review\b/i,
  /\bno\s+review\s+needed\b/i,
  /\bnoise\b/i,
  /\bverified\b/i,
  /\bwas\s+wrong\b/i,
  /\bconfidence\b/i,
  /\d\s?%/,
];

/** Fails when `text` contains a banned phrase or a percentage. */
export function expectTrustWording(text: string): void {
  for (const pattern of BANNED) expect(text, `banned wording ${pattern}`).not.toMatch(pattern);
}

/** The tour of every fixture state. */
export const toursByState: [FixtureState, Tour][] = FIXTURE_STATES.map((state) => [
  state,
  seedFor(state, "tour") as Tour,
]);
