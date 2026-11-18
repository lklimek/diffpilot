// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { MeResponse, ReviewState, Tour } from "../api/generated/types";
import { FIXTURE_STATES, mergePatch, seedFor } from "./fixtures";

describe("mergePatch (RFC 7396)", () => {
  it("merges objects recursively, replaces arrays and deletes on null", () => {
    expect(mergePatch({ a: { b: 1, c: [1, 2] }, d: 1 }, { a: { c: [3], e: "x" }, d: null })).toEqual({
      a: { b: 1, c: [3], e: "x" },
    });
  });

  it("replaces the target when the patch is not an object", () => {
    expect(mergePatch({ a: 1 }, [1])).toEqual([1]);
    expect(mergePatch([1], { a: 1 })).toEqual({ a: 1 });
  });

  it("does not modify its inputs", () => {
    const target = { a: { b: 1 } };
    mergePatch(target, { a: { b: 2 } });
    expect(target).toEqual({ a: { b: 1 } });
  });
});

describe("fixture seed", () => {
  it("serves the default seed of every slice endpoint", () => {
    expect((seedFor("default", "me") as MeResponse).user?.login).toBe("alice");
    const tour = seedFor("default", "tour") as Tour;
    expect(tour.flows).toHaveLength(5);
    expect(seedFor("default", "lines/head/src/rpc/client.rs")).toMatchObject({ path: "src/rpc/client.rs" });
  });

  it("applies the state patches", () => {
    const profile = (seedFor("noprofile", "tour") as Tour).pr_context.facts.project_profile.sources;
    expect(profile).toEqual([]);
    const noq = seedFor("noq", "tour") as Tour;
    const annotations = noq.flows.flatMap((f) => f.steps.flatMap((s) => s.llm.annotations));
    expect(annotations.some((a) => a.type === "flag")).toBe(false);
    expect((seedFor("resume", "review-state") as ReviewState).last_viewed).toEqual({
      block: "step",
      step_id: "f1.5",
    });
    // Other parts of the seed survive a patch.
    expect((seedFor("noprofile", "tour") as Tour).flows).toHaveLength(5);
  });

  it("knows the six fixture states and rejects others", () => {
    expect(FIXTURE_STATES).toEqual(["default", "wrong", "missed", "noq", "noprofile", "resume"]);
    expect(() => seedFor("bogus" as never, "me")).toThrow(/bogus/);
  });

  it("returns undefined for an unknown seed path", () => {
    expect(seedFor("default", "lines/head/nope.rs")).toBeUndefined();
  });
});
