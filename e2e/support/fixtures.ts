// The `test` of every spec: each spec file gets its own stack (started with its first test, stopped when the
// worker moves on to the next file or ends); `baseURL` points at it. A local stack's logs are checked for leaked
// secrets when it stops (support/secrets.ts); a leak fails the run as an error of the worker's teardown.
import { test as base } from "@playwright/test";
import { logLeaks, stackSecrets } from "./secrets";
import { startStack, type Stack } from "./stack";

/** The stack of the spec file the worker is running. */
class StackPool {
  private file: string | null = null;
  private running: { stack: Stack; stop: () => Promise<void> } | null = null;
  /** Leak reports of stacks stopped on a change of file; raised when the worker ends. */
  readonly failures: string[] = [];

  async forFile(file: string): Promise<Stack> {
    if (this.file !== file || !this.running) {
      await this.stop().catch((error: Error) => this.failures.push(error.message));
      this.running = await startStack();
      this.file = file;
    }
    return this.running.stack;
  }

  async stop(): Promise<void> {
    const running = this.running;
    const file = this.file;
    this.running = null;
    this.file = null;
    if (!running) return;
    const { stack } = running;
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

export const test = base.extend<{ stack: Stack }, { stacks: StackPool }>({
  stacks: [
    async ({}, use) => {
      const pool = new StackPool();
      await use(pool);
      await pool.stop().catch((error: Error) => pool.failures.push(error.message));
      if (pool.failures.length > 0) throw new Error(pool.failures.join("\n\n"));
    },
    { scope: "worker" },
  ],
  stack: async ({ stacks }, use, testInfo) => {
    await use(await stacks.forFile(testInfo.file));
  },
  baseURL: async ({ stack }, use) => {
    await use(stack.url);
  },
});

export { expect } from "@playwright/test";
