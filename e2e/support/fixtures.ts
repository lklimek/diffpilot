// The `test` of every spec: each spec file gets its own stack (started with its first test, stopped when the
// worker moves on to the next file or ends); `baseURL` points at it.
import { test as base } from "@playwright/test";
import { startStack, type Stack } from "./stack";

/** The stack of the spec file the worker is running. */
class StackPool {
  private file: string | null = null;
  private running: { stack: Stack; stop: () => Promise<void> } | null = null;

  async forFile(file: string): Promise<Stack> {
    if (this.file !== file || !this.running) {
      await this.stop();
      this.running = await startStack();
      this.file = file;
    }
    return this.running.stack;
  }

  async stop(): Promise<void> {
    const running = this.running;
    this.running = null;
    this.file = null;
    await running?.stop();
  }
}

export const test = base.extend<{ stack: Stack }, { stacks: StackPool }>({
  stacks: [
    async ({}, use) => {
      const pool = new StackPool();
      await use(pool);
      await pool.stop();
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
