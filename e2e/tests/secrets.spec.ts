// Self-test of the leak check's token shape (support/secrets.ts), no stack: any 43-character base64url run is a
// random 32-byte token whatever characters it drew; known non-secrets are not. A whole personal access token is
// found, its `srp_<id>` prefix alone is not.
import { accessTokens, randomTokens } from "../support/secrets";
import { expect, test } from "@playwright/test";

const line = (value: string) => `2026-11-17T09:02:44Z INFO server::jobs: claimed token=${value} job=7`;

const tokens: [string, string][] = [
  ["mixed", "rZY0VUc5DjoSp62_hHKPMNeL_dZ6A4y22nE0fI2fuHI"],
  ["without a digit", "rZYqVUcxDjoSpw_hHKPMNeL_dZyAzyhnEfIfuHIabcd"],
  ["without lower case", "RZY0VUC5DJOSP62_HHKPMNEL_DZ6A4Y22NE0FI2FUHI"],
  ["without upper case", "rzy0vuc5djosp62_hhkpmnel_dz6a4y22ne0fi2fuhi"],
  ["letters only", "rZYqVUcxDjoSpwahHKPMNeLbdZyAzyhnEfIfuHIabcd"],
  ["with leading dashes", "--we45L-C82nE1qmqMQaNeQbFiEcSCRHG-Z7LYPQIzs"],
];
for (const [name, token] of tokens) {
  test(`a 43-character base64url token ${name} is found`, () => {
    expect(token).toHaveLength(43);
    expect(randomTokens(line(token))).toEqual([token]);
  });
}

const notSecrets: [string, string][] = [
  ["a commit SHA", "f05b0c07049436cfb8b66a12715f45c9ba9c77d6"],
  ["a UUID", "0b5f6a7e-6f43-4f7e-9a51-3c1d2e4f5a6b"],
  ["a ULID", "01JD4Z8K9M2N3P4Q5R6S7T8V9W"],
  ["a SHA-256 digest", "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"],
  ["43 hex characters", "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0"],
  ["a 44-character run", "rZY0VUc5DjoSp62_hHKPMNeL_dZ6A4y22nE0fI2fuHIx"],
  ["a 42-character run", "rZY0VUc5DjoSp62_hHKPMNeL_dZ6A4y22nE0fI2fuH"],
  ["a module path", "server::jobs::claim::sweep_expired_job_tokens_now"],
];
for (const [name, value] of notSecrets) {
  test(`${name} is not a token`, () => {
    expect(randomTokens(line(value))).toEqual([]);
  });
}

// Synthetic: the id and the first 43-character sample above.
const ACCESS_TOKEN = `srp_Ab3dE6gH_${tokens[0]![1]}`;

test("a personal access token is found, also inside a longer run", () => {
  expect(accessTokens(line(ACCESS_TOKEN))).toEqual([ACCESS_TOKEN]);
  expect(accessTokens(`Authorization: Bearer ${ACCESS_TOKEN}`)).toEqual([ACCESS_TOKEN]);
  // The 43-character secret is not on its own there, so the random-token shape alone would miss it.
  expect(randomTokens(line(ACCESS_TOKEN))).toEqual([]);
});

test("a token's prefix alone is not a token", () => {
  expect(accessTokens(line("srp_Ab3dE6gH…"))).toEqual([]);
  expect(accessTokens("token srp_Ab3dE6gH revoked")).toEqual([]);
});
