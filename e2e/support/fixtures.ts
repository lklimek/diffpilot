// The `test` of every spec: each spec file gets its own stack (started with its first test, stopped when the
// worker moves on to the next file or ends); `baseURL` points at it. A local stack's logs are checked for leaked
// secrets when it stops (support/secrets.ts); a leak fails the run as an error of the worker's teardown.
import { test as base } from "@playwright/test";
import { logLeaks, stackSecrets } from "./secrets";
import { startStack, type Stack, type StackOptions } from "./stack";

/**
 * Whether the running stack (started for `running.file` with `running.options`) serves a test of `file` asking for
 * `options`: yes for the same file, no for another file (or none running). A file's tests share its stack, so a
 * test asking for other options than that stack's fails instead of running on it.
 */
export function reusesStack(
  running: { file: string; options: StackOptions } | null,
  file: string,
  options: StackOptions,
): boolean {
  if (running?.file !== file) return false;
  const had = running.options.reviewLoop ?? false;
  const wants = options.reviewLoop ?? false;
  if (had !== wants)
    throw new Error(
      `${file}: its tests share one stack, started with reviewLoop: ${had}; this test asks for reviewLoop: ${wants}. Set test.use({ reviewLoop }) for the whole file.`,
    );
  return true;
}

/** The stack of the spec file the worker is running. */
class StackPool {
  private running: {
    file: string;
    options: StackOptions;
    stack: Stack;
    stop: () => Promise<void>;
  } | null = null;
  /** Leak reports of stacks stopped on a change of file; raised when the worker ends. */
  readonly failures: string[] = [];

  async forFile(file: string, options: StackOptions): Promise<Stack> {
    if (!this.running || !reusesStack(this.running, file, options)) {
      await this.stop().catch((error: Error) => this.failures.push(error.message));
      this.running = { file, options, ...(await startStack(options)) };
    }
    return this.running.stack;
  }

  async stop(): Promise<void> {
    const running = this.running;
    this.running = null;
    if (!running) return;
    const { stack, file } = running;
    const issued = stack.stateDir ? await stack.githubCredentials?.() : undefined;
    await running.stop();
    if (!stack.stateDir || !issued) return;
    const leaks = logLeaks(stack, stackSecrets(stack.stateDir, issued));
    if (leaks.length > 0) {
      throw new Error(
        `secrets in the logs of the stack of ${file} (${stack.stateDir}):\n${leaks.join("\n")}`,
      );
    }
  }
}

export const test = base.extend<{ stack: Stack; reviewLoop: boolean }, { stacks: StackPool }>({
  /** `test.use({ reviewLoop: true })`: the file's stack serves the review-loop reads (`StackOptions`). */
  reviewLoop: [false, { option: true }],
  stacks: [
    async ({}, use) => {
      const pool = new StackPool();
      await use(pool);
      await pool.stop().catch((error: Error) => pool.failures.push(error.message));
      if (pool.failures.length > 0) throw new Error(pool.failures.join("\n\n"));
    },
    { scope: "worker" },
  ],
  stack: async ({ stacks, reviewLoop }, use, testInfo) => {
    await use(await stacks.forFile(testInfo.file, { reviewLoop }));
  },
  baseURL: async ({ stack }, use) => {
    await use(stack.url);
  },
});

export { expect } from "@playwright/test";
