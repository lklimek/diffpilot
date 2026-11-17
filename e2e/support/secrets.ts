// Leak check of a local stack's component logs, run for every stack when it stops (support/fixtures.ts): no
// generated secret, no line of the App private key, no App JWT or installation token fake GitHub issued, no
// fixture credential, no session secret a test registered, and nothing shaped like the server's random tokens
// (job tokens, session tokens: 32 random bytes in base64url). Job tokens are checked by shape only: the server keeps
// just their hash and clears it when the job ends.
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import type { Stack } from "./stack";

/** The secret values in the launcher's stack.env; the rest (URLs, paths, ids) is not secret. */
const SECRET_KEYS = ["SR_SECRET_KEY", "SR_RUNNER_SECRET", "SR_GITHUB_CLIENT_SECRET", "SR_MEMCAN_TOKEN"];
/** Credentials of the fake GitHub recordings; minted installation tokens extend the recorded one. */
const FIXTURE_SECRETS: [string, string][] = [
  ["OAuth code", "fixture-oauth-code"],
  ["user access token", "fixture-user-token-alice"],
  ["user refresh token", "fixture-refresh-token-alice"],
  ["installation token", "fixture-installation-token-482"],
];
const LOGS = ["server.log", "runner.log", "fake-github.log", "fake-memcan.log"];
/** 43 base64url characters on their own: the shape of a random 32-byte token, whatever characters it drew. */
const RANDOM_TOKEN = /(?<![A-Za-z0-9_-])[A-Za-z0-9_-]{43}(?![A-Za-z0-9_-])/g;
/**
 * Known non-secrets of that shape, excluded explicitly: hex digests (hex only; a random token is all hex with
 * probability (22/64)^43). Other long ids in the logs cannot match the shape: commit SHAs (40 hex), UUIDs (36,
 * with hyphens), ULIDs (26), SHA-256 digests (64), paths and module names (`/`, `.`, `:`).
 */
const NOT_SECRETS: RegExp[] = [/^[0-9a-fA-F]+$/];

/** Runs of `text` shaped like a random 32-byte token in base64url, known non-secrets excluded. */
export function randomTokens(text: string): string[] {
  return [...text.matchAll(RANDOM_TOKEN)]
    .map((match) => match[0])
    .filter((run) => !NOT_SECRETS.some((known) => known.test(run)));
}

/** What fake GitHub issued (`GET /_fake/app-credentials`). */
export interface GithubCredentials {
  jwts: string[];
  installation_tokens: string[];
  revoked: string[];
}

/** The secrets of a local stack known before its logs are read. */
export function stackSecrets(stateDir: string, github: GithubCredentials): [string, string][] {
  const env = readFileSync(join(stateDir, "stack.env"), "utf8")
    .split("\n")
    .map((line): [string, string] => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)])
    .filter(([key]) => SECRET_KEYS.includes(key));
  const missing = SECRET_KEYS.filter((key) => !env.some(([found]) => found === key));
  if (missing.length > 0) throw new Error(`stack.env lacks ${missing.join(", ")}`);
  const pem = readFileSync(join(stateDir, "github-app-key.pem"), "utf8")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length >= 16 && !line.startsWith("-----"))
    .map((line, n): [string, string] => [`App private key line ${n + 1}`, line]);
  if (pem.length === 0) throw new Error("the App private key has no body lines");
  return [
    ...env,
    ...pem,
    ...FIXTURE_SECRETS,
    ...github.jwts.flatMap((jwt, n): [string, string][] => [
      [`App JWT ${n + 1}`, jwt],
      [`App JWT ${n + 1} signature`, jwt.split(".")[2] ?? jwt],
    ]),
    ...github.installation_tokens.map((token, n): [string, string] => [`installation token ${n + 1}`, token]),
  ];
}

/** Every leak in the stack's logs, as `log: what`; empty when clean. */
export function logLeaks(stack: Stack, secrets: [string, string][]): string[] {
  const logDir = join(stack.stateDir ?? "", "logs");
  const logs = readdirSync(logDir);
  const leaks = LOGS.filter((log) => !logs.includes(log)).map((log) => `${log}: missing`);
  for (const log of logs) {
    const text = readFileSync(join(logDir, log), "utf8");
    if (text.length === 0) leaks.push(`${log}: empty`);
    for (const [name, value] of [...secrets, ...stack.secrets]) {
      if (value.length >= 8 && text.includes(value)) leaks.push(`${log}: the value of ${name}`);
    }
    for (const token of randomTokens(text)) {
      leaks.push(`${log}: a random token (job or session token shape) "${token.slice(0, 6)}…"`);
    }
  }
  return leaks;
}
