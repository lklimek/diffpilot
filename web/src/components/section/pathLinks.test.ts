import { describe, expect, it } from "vitest";
import { pathMatcher } from "./pathLinks";

const paths = ["Cargo.toml", "packages/sdk/src/rpc/client.rs", "src/generated/client.rs", "src/lib.rs"];
const match = pathMatcher(paths, { owner: "acme", name: "node" });

describe("analysis file mentions", () => {
  it.each([
    ["Cargo.toml", "Cargo.toml", undefined, undefined],
    ["./src/lib.rs:12", "src/lib.rs", 12, undefined],
    ["acme/node/src/lib.rs:12-30", "src/lib.rs", 12, 30],
    ["node/src/lib.rs#L12", "src/lib.rs", 12, undefined],
    ["rpc/client.rs", "packages/sdk/src/rpc/client.rs", undefined, undefined],
  ])("resolves %s", (mention, path, line, end) => {
    expect(match(mention)).toEqual({ path, line, end });
  });

  it.each([
    "client.rs",
    "unknown/src/lib.rs",
    "/src/lib.rs",
    "../src/lib.rs",
    "https://example.org/acme/node/src/lib.rs",
    "src/lib.rs:0",
    "src/lib.rs:12-2",
    "src/lib.rs#L99999999999999999999",
  ])("leaves %s alone", (mention) => expect(match(mention)).toBeUndefined());

  it("prefers an exact path and rejects an ambiguous suffix", () => {
    const repo = { owner: "acme", name: "node" };
    const a = "one/src/lib.rs";
    const b = "two/src/lib.rs";
    expect(pathMatcher([a, b, "src/lib.rs"], repo)("src/lib.rs")?.path).toBe("src/lib.rs");
    expect(pathMatcher([a, b], repo)("src/lib.rs")).toBeUndefined();
  });
});
