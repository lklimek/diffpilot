// One stack per spec file: deploy/dev/stack.sh with the prebuilt binaries of the global setup, on free localhost
// ports with state under /data/tmp, or an already-running stack given by URL (SR_E2E_* variables, README).
import { execFileSync, spawn } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import type { GithubCredentials } from "./secrets";

const LAUNCHER = fileURLToPath(new URL("../../deploy/dev/stack.sh", import.meta.url));
const REVIEW_LOOP_RECORDINGS = fileURLToPath(new URL("../../fixtures/review-loop/github", import.meta.url));
const START_TIMEOUT_MS = Number(process.env.SR_E2E_START_TIMEOUT_MS ?? 120_000);
const STOP_TIMEOUT_MS = 20_000;

/** One rule of the fake provider's script (crates/runner/src/provider/fake.rs). */
export interface ScriptRule {
  id: string;
  job: "plan" | "pr_context" | "analyse_flow" | "ask" | "judge_work" | "verify_comments";
  flow?: number;
  prompt_contains?: string;
  times?: number;
  action:
    | { replay: { step_delay_ms: number } }
    | { usage_limit: { resets_in_secs: number | null } }
    | { fail: { message: string } }
    | { answer: { output: unknown } };
}

/** A running stack and what the tests may reach of it. */
export interface Stack {
  /** Public URL of the server. */
  url: string;
  /** Local state dir (logs, stack.env, database); null for a stack started elsewhere. */
  stateDir: string | null;
  /** Fake GitHub base URL (test controls `/_fake/*`); null for a stack started elsewhere. */
  github: string | null;
  /** Fake MemCan base URL and bearer token, when reachable. */
  memcan: { url: string; token: string } | null;
  /** Replaces the fake provider's script; null when the runner's files are out of reach. */
  script: ((rules: ScriptRule[]) => void) | null;
  /**
   * Pushes a new commit (same tree) onto the PR head in the clone base and moves fake GitHub's PR head to it;
   * returns its id. Null when the clone base is out of reach.
   */
  moveHead: (() => Promise<string>) | null;
  /** What fake GitHub's App endpoints issued so far; null when out of reach. */
  githubCredentials: (() => Promise<GithubCredentials>) | null;
  /** Secrets a test saw (session cookie, CSRF token): the leak check at stop looks for them in the logs. */
  secrets: [string, string][];
}

function envFile(stateDir: string): Record<string, string> {
  return Object.fromEntries(
    readFileSync(join(stateDir, "stack.env"), "utf8")
      .split("\n")
      .filter((line) => line.includes("="))
      .map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
  );
}

function localStack(url: string, stateDir: string): Stack {
  const env = envFile(stateDir);
  const memcan = env.SR_MEMCAN_URL ? new URL(env.SR_MEMCAN_URL).origin : null;
  return {
    url,
    stateDir,
    github: env.SR_GITHUB_API_URL ?? null,
    memcan: memcan ? { url: memcan, token: env.SR_MEMCAN_TOKEN } : null,
    script: env.SR_RUNNER_FAKE_SCRIPT
      ? (rules) => writeFileSync(env.SR_RUNNER_FAKE_SCRIPT, JSON.stringify({ rules }))
      : null,
    moveHead: env.SR_GITHUB_CLONE_URL?.startsWith("file://")
      ? () =>
          moveHead(
            `${env.SR_GITHUB_CLONE_URL.slice("file://".length)}/acme/node-client.git`,
            env.SR_GITHUB_API_URL,
          )
      : null,
    githubCredentials: async () => {
      const response = await fetch(`${env.SR_GITHUB_API_URL}/_fake/app-credentials`);
      if (!response.ok) throw new Error(`fake GitHub credentials: ${response.status}`);
      return (await response.json()) as GithubCredentials;
    },
    secrets: [],
  };
}

async function moveHead(bare: string, github: string): Promise<string> {
  const git = (...args: string[]) =>
    execFileSync("git", ["--git-dir", bare, ...args], {
      encoding: "utf8",
      env: {
        PATH: process.env.PATH,
        GIT_AUTHOR_NAME: "e2e",
        GIT_AUTHOR_EMAIL: "e2e@example.invalid",
        GIT_COMMITTER_NAME: "e2e",
        GIT_COMMITTER_EMAIL: "e2e@example.invalid",
      },
    }).trim();
  const head = git("rev-parse", "refs/pull/482/head");
  const moved = git("commit-tree", `${head}^{tree}`, "-p", head, "-m", "e2e: the author pushed again");
  git("update-ref", "refs/pull/482/head", moved);
  git("update-ref", "refs/heads/feat/retry-timeouts", moved);
  const response = await fetch(`${github}/_fake/head`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ sha: moved }),
  });
  if (response.status !== 204) throw new Error(`fake GitHub did not move the head: ${response.status}`);
  return moved;
}

/** The stack named by SR_E2E_BASE_URL (and optionally SR_E2E_FAKE_MEMCAN_URL, SR_E2E_MEMCAN_TOKEN). */
function externalStack(url: string): Stack {
  const memcan = process.env.SR_E2E_FAKE_MEMCAN_URL;
  const token = process.env.SR_E2E_MEMCAN_TOKEN;
  return {
    url,
    stateDir: null,
    github: null,
    memcan: memcan && token ? { url: memcan, token } : null,
    script: null,
    moveHead: null,
    githubCredentials: null,
    secrets: [],
  };
}

/** What a spec file needs of its stack. */
export interface StackOptions {
  /**
   * Fake GitHub serves the review-loop reads of PR 482 (`SR_DEV_REVIEW_LOOP_RECORDINGS`, fixtures/review-loop/github).
   * Off by default: each read that finds new comments queues an assessment job, an LLM call the other journeys do
   * not expect.
   */
  reviewLoop?: boolean;
}

/** Starts a stack (or names the external one) and returns it with the function that stops it. */
export async function startStack(
  options: StackOptions = {},
): Promise<{ stack: Stack; stop: () => Promise<void> }> {
  const external = process.env.SR_E2E_BASE_URL;
  if (external) return { stack: externalStack(external), stop: async () => {} };

  const args = (process.env.SR_E2E_STACK_ARGS ?? "").split(/\s+/).filter(Boolean);
  const env = {
    ...process.env,
    SR_DEV_REVIEW_LOOP_RECORDINGS: options.reviewLoop ? REVIEW_LOOP_RECORDINGS : "",
    SR_DEV_ASSESSMENT_METHOD: "prompt_v4",
  };
  // Own process group: the launcher stops its components (each in a group of its own) on SIGTERM.
  const launcher = spawn(LAUNCHER, args, { detached: true, env, stdio: ["ignore", "pipe", "inherit"] });
  const group = launcher.pid;
  if (group === undefined) throw new Error("the dev stack launcher could not be started");
  const exited = new Promise<void>((resolve) => launcher.once("exit", () => resolve()));
  const signalGroup = (signal: NodeJS.Signals | 0) => {
    try {
      process.kill(-group, signal);
      return true;
    } catch {
      return false; // the group is gone
    }
  };
  // The guard outlives a worker killed by SIGKILL or the OOM killer: its stdin is a pipe from this process, so the
  // worker's death ends `cat`, and the guard stops the launcher's group (the launcher stops the components).
  const guard = spawn(
    "bash",
    ["-c", 'cat > /dev/null; kill -TERM -- "-$1" 2> /dev/null; exit 0', "stack-guard", String(group)],
    { detached: true, stdio: ["pipe", "ignore", "ignore"] },
  );
  guard.stdin?.on("error", () => {});
  guard.unref();
  (guard.stdin as unknown as { unref?: () => void } | null)?.unref?.();
  const onExit = () => signalGroup("SIGTERM");
  process.once("exit", onExit);
  const stop = async () => {
    process.off("exit", onExit);
    guard.kill("SIGKILL"); // stopped here; a late guard must not signal a reused group id
    signalGroup("SIGTERM");
    await Promise.race([exited, delay(STOP_TIMEOUT_MS)]);
    if (signalGroup(0)) signalGroup("SIGKILL");
    for (let i = 0; i < 50 && signalGroup(0); i++) await delay(100);
  };

  const ready = new Promise<Record<string, string>>((resolve, reject) => {
    const values: Record<string, string> = {};
    const timer = setTimeout(
      () => reject(new Error("the dev stack did not start in time")),
      START_TIMEOUT_MS,
    );
    createInterface({ input: launcher.stdout }).on("line", (line) => {
      const match = /^(SR_DEV_URL|SR_DEV_STATE_DIR)=(.+)$/.exec(line);
      if (match) values[match[1]] = match[2];
      if (values.SR_DEV_URL && values.SR_DEV_STATE_DIR) {
        clearTimeout(timer);
        resolve(values);
      }
    });
    launcher.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`the dev stack launcher exited with ${code} before it was ready`));
    });
  });
  try {
    const values = await ready;
    return { stack: localStack(values.SR_DEV_URL, values.SR_DEV_STATE_DIR), stop };
  } catch (error) {
    await stop();
    throw error;
  }
}
