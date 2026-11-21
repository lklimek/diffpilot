import { describe, expect, it } from "vitest";
import { githubLineUrl, sha256Hex, siteDiffUrl, stepDiffUrl } from "./links";

const PR = {
  repo: { owner: "dashpay", name: "platform" },
  number: 4794,
  base_sha: "b".repeat(40),
  head_sha: "a".repeat(40),
};
const VECTOR_PATH = "packages/rs-dpp/src/data_contract/config/vectors/contract_config_vectors.json";
const VECTOR_HASH = "6fbfc4d3478f613586057ca272e72484fa71926c11aa9a1644fbf2c8033851d2";
const CHANGES = "https://github.com/dashpay/platform/pull/4794/changes";

describe("sha256Hex", () => {
  it.each([
    ["", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"],
    ["abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"],
    [
      "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
      "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
    ],
    ["a".repeat(1_000_000), "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"],
    // GitHub's own anchor of this file in dashpay/platform#4794.
    [VECTOR_PATH, VECTOR_HASH],
    ["docs/my file.md", "69b43ff48800054a19c8d2666102b2442384b3ca6e361b52c81e28b88e0b6a69"],
    ["src/zażółć/ünï 文件.rs", "ad1f09c17ded532ea32be2974c7ad983b78180f53de4342268aeb9a472a1e9df"],
  ])("hashes the UTF-8 bytes of %j", (text, hex) => {
    expect(sha256Hex(text)).toBe(hex);
  });

  it("hashes lengths around the 55/56/64-byte padding boundaries like the reference", async () => {
    const { createHash } = await import("node:crypto");
    for (const n of [55, 56, 63, 64, 65, 119, 120]) {
      const text = "x".repeat(n);
      expect(sha256Hex(text)).toBe(createHash("sha256").update(text).digest("hex"));
    }
  });
});

describe("githubLineUrl", () => {
  it("links a head line in the diff to the PR's changes view, right side", () => {
    const span = { path: VECTOR_PATH, side: "head" as const, start: 12, end: 12 };
    expect(githubLineUrl(PR, span, true)).toBe(`${CHANGES}#diff-${VECTOR_HASH}R12`);
  });

  it("links a base range in the diff to the left side", () => {
    const span = { path: VECTOR_PATH, side: "base" as const, start: 3, end: 5 };
    expect(githubLineUrl(PR, span, true)).toBe(`${CHANGES}#diff-${VECTOR_HASH}L3-L5`);
  });

  it("links a head range in the diff as R<a>-R<b>", () => {
    const span = { path: VECTOR_PATH, side: "head" as const, start: 12, end: 20 };
    expect(githubLineUrl(PR, span, true)).toBe(`${CHANGES}#diff-${VECTOR_HASH}R12-R20`);
  });

  it("falls back to the file at the head commit outside the diff", () => {
    const span = { path: "src/a.rs", side: "head" as const, start: 12, end: 20 };
    expect(githubLineUrl(PR, span, false)).toBe(
      `https://github.com/dashpay/platform/blob/${PR.head_sha}/src/a.rs#L12-L20`,
    );
  });

  it("falls back to the file at the base commit for a base line", () => {
    const span = { path: "src/a.rs", side: "base" as const, start: 7, end: 7 };
    expect(githubLineUrl(PR, span, false)).toBe(
      `https://github.com/dashpay/platform/blob/${PR.base_sha}/src/a.rs#L7`,
    );
  });

  it("encodes path segments in the file link and hashes the raw path in the diff anchor", () => {
    const span = { path: "src/zażółć/ünï 文件.rs", side: "head" as const, start: 1, end: 1 };
    expect(githubLineUrl(PR, span, false)).toBe(
      `https://github.com/dashpay/platform/blob/${PR.head_sha}/src/za%C5%BC%C3%B3%C5%82%C4%87/%C3%BCn%C3%AF%20%E6%96%87%E4%BB%B6.rs#L1`,
    );
    expect(githubLineUrl(PR, span, true)).toBe(
      `${CHANGES}#diff-ad1f09c17ded532ea32be2974c7ad983b78180f53de4342268aeb9a472a1e9dfR1`,
    );
  });
});

describe("stepDiffUrl", () => {
  const facts = {
    file: "src/a.rs",
    side: "head" as const,
    range: { start_line: 10, end_line: 40 },
    changed_lines: [
      { start_line: 14, end_line: 15 },
      { start_line: 22, end_line: 25 },
    ],
  };
  const hash = sha256Hex("src/a.rs");

  it("anchors the span of the step's changed lines in the PR diff", () => {
    expect(stepDiffUrl(PR, facts)).toBe(`${CHANGES}#diff-${hash}R14-R25`);
  });

  it("uses the left side for a base step", () => {
    expect(stepDiffUrl(PR, { ...facts, side: "base", changed_lines: [{ start_line: 3, end_line: 3 }] })).toBe(
      `${CHANGES}#diff-${hash}L3`,
    );
  });

  it("is null for a step without changed lines", () => {
    expect(stepDiffUrl(PR, { ...facts, changed_lines: [] })).toBeNull();
  });
});

describe("siteDiffUrl", () => {
  const step = {
    file: "src/a.rs",
    side: "head" as const,
    changed_lines: [
      { start_line: 89, end_line: 89 },
      { start_line: 91, end_line: 94 },
      { start_line: 96, end_line: 96 },
    ],
  };
  const site = { file: "src/a.rs", side: "head" as const, range: { start_line: 90, end_line: 95 } };
  const hash = sha256Hex("src/a.rs");

  it("anchors the site's lines that are changed lines of its step", () => {
    expect(siteDiffUrl(PR, site, step)).toBe(`${CHANGES}#diff-${hash}R91-R94`);
  });

  it("is null without the step, outside its changed lines, or on another file or side", () => {
    expect(siteDiffUrl(PR, site, undefined)).toBeNull();
    expect(siteDiffUrl(PR, { ...site, range: { start_line: 97, end_line: 99 } }, step)).toBeNull();
    expect(siteDiffUrl(PR, { ...site, file: "src/b.rs" }, step)).toBeNull();
    expect(siteDiffUrl(PR, { ...site, side: "base" }, step)).toBeNull();
  });
});
