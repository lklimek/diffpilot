import { expect, it } from "vitest";
import { parsePrRef, prPath } from "./paths";

it.each([
  "https://github.com/acme/repo/issues/42",
  "https://github.com.evil.example/acme/repo/pull/42",
  "https://github.com@evil.example/acme/repo/pull/42",
  "https://user@github.com/acme/repo/pull/42",
  "https://github.com/acme/repo/pull/0",
  "https://github.com/acme/repo/pull/1e3",
  "https://github.com/acme/repo/pull/9007199254740992",
  "not a link",
  "https://github.com:8443/acme/repo/pull/42",
  "https://github.com/acme/repo/pull/4294967296",
  "acme/..#42",
])("rejects %s as a PR destination", (input) => {
  expect(parsePrRef(input)).toBeNull();
});

it.each([
  "https://github.com/acme/repo/pull/42",
  "http://github.com/acme/repo/pull/42",
  "https://github.com/acme/repo/pull/42/commits",
  "https://www.github.com/acme/repo/pull/42/files?diff=split#L7",
  "github.com/acme/repo/pull/42",
  " acme/repo#42 ",
])("maps %s to the same internal PR path", (input) => {
  const pr = parsePrRef(input);
  expect(pr).toEqual({ repo: { owner: "acme", name: "repo" }, number: 42 });
  expect(prPath(pr!)).toBe("/pr/acme/repo/42");
});
