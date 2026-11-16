// Global setup: starts deploy/dev/stack.sh, publishes its URL and state dir to the workers through the
// environment, and returns the teardown that stops the whole stack.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

const LAUNCHER = fileURLToPath(new URL("../deploy/dev/stack.sh", import.meta.url));
// Long enough for a first build when no prebuilt binaries are given.
const START_TIMEOUT_MS = Number(process.env.SR_E2E_START_TIMEOUT_MS ?? 15 * 60_000);
const STOP_TIMEOUT_MS = 20_000;

export default async function globalSetup(): Promise<() => Promise<void>> {
  const args = (process.env.SR_E2E_STACK_ARGS ?? "").split(/\s+/).filter(Boolean);
  // Own process group: the launcher and its build (npm, cargo) are stopped together; the components run in
  // groups of their own, which the launcher stops.
  const stack = spawn(LAUNCHER, args, { detached: true, stdio: ["ignore", "pipe", "inherit"] });
  const group = stack.pid;
  if (group === undefined) throw new Error("the dev stack launcher could not be started");
  const exited = new Promise<void>((resolve) => stack.once("exit", () => resolve()));
  const signalGroup = (signal: NodeJS.Signals | 0) => {
    try {
      process.kill(-group, signal);
      return true;
    } catch {
      return false; // the group is gone
    }
  };
  process.once("exit", () => signalGroup("SIGTERM"));

  const ready = new Promise<Record<string, string>>((resolve, reject) => {
    const values: Record<string, string> = {};
    const timer = setTimeout(
      () => reject(new Error("the dev stack did not start in time")),
      START_TIMEOUT_MS,
    );
    createInterface({ input: stack.stdout }).on("line", (line) => {
      const match = /^(SR_DEV_URL|SR_DEV_STATE_DIR)=(.+)$/.exec(line);
      if (match) values[match[1]] = match[2];
      if (values.SR_DEV_URL && values.SR_DEV_STATE_DIR) {
        clearTimeout(timer);
        resolve(values);
      }
    });
    stack.once("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`the dev stack launcher exited with ${code} before it was ready`));
    });
  });

  const stop = async () => {
    signalGroup("SIGTERM");
    await Promise.race([exited, delay(STOP_TIMEOUT_MS)]);
    // Build processes may outlive the launcher (bash runs its TERM trap only after the foreground command).
    if (signalGroup(0)) signalGroup("SIGKILL");
    for (let i = 0; i < 50 && signalGroup(0); i++) await delay(100);
  };

  try {
    const values = await ready;
    process.env.SR_E2E_BASE_URL = values.SR_DEV_URL;
    process.env.SR_E2E_STATE_DIR = values.SR_DEV_STATE_DIR;
  } catch (error) {
    await stop();
    throw error;
  }
  return stop;
}
