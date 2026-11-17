// Global setup: resolves the binaries every spec's stack runs (support/stack.ts). Prebuilt binaries are taken from
// SR_DEV_*_BIN when all four are set; otherwise the web UI and the binaries are built once here (bounded by
// SR_E2E_BUILD_TIMEOUT_MS) and published to the workers through the environment. Nothing is built against an
// already-running stack (SR_E2E_BASE_URL).
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("..", import.meta.url));
/** Bound for the whole build (web UI and binaries). */
const BUILD_TIMEOUT_MS = Number(process.env.SR_E2E_BUILD_TIMEOUT_MS ?? 30 * 60_000);
/** Environment variable → cargo target name. */
const BINARIES: [string, string][] = [
  ["SR_DEV_SERVER_BIN", "smart-review-server"],
  ["SR_DEV_RUNNER_BIN", "smart-review-runner"],
  ["SR_DEV_FAKE_GITHUB_BIN", "fake-github"],
  ["SR_DEV_FAKE_MEMCAN_BIN", "fake-memcan"],
];

/** Runs `command` in a process group of its own; past `deadline` the whole group (compilers too) is killed. */
function run(command: string, args: string[], deadline: number): Promise<string> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: REPO, detached: true, stdio: ["ignore", "pipe", "inherit"] });
    const chunks: Buffer[] = [];
    child.stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
    const timer = setTimeout(
      () => {
        try {
          if (child.pid !== undefined) process.kill(-child.pid, "SIGKILL");
        } catch {
          // already gone
        }
        reject(new Error(`${command} ${args.join(" ")} did not finish within the build timeout`));
      },
      Math.max(0, deadline - Date.now()),
    );
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      if (code === 0) resolve(Buffer.concat(chunks).toString("utf8"));
      else reject(new Error(`${command} ${args.join(" ")} failed (${code})`));
    });
  });
}

export default async function globalSetup(): Promise<void> {
  if (process.env.SR_E2E_BASE_URL) return;
  if (BINARIES.every(([variable]) => process.env[variable])) return;
  console.error("building the web UI and the binaries (set SR_DEV_*_BIN to skip)");
  const deadline = Date.now() + BUILD_TIMEOUT_MS;
  if (!existsSync(`${REPO}/web/node_modules`)) await run("npm", ["--prefix", "web", "ci"], deadline);
  await run("npm", ["--prefix", "web", "run", "build"], deadline);
  const packages = "-p server --features server/embed-ui -p runner --features runner/fake-provider";
  const messages = await run(
    "cargo",
    [
      "build",
      "--message-format=json-render-diagnostics",
      ...`${packages} -p fake-github -p fake-memcan`.split(" "),
    ],
    deadline,
  );
  const executables = new Map<string, string>();
  for (const line of messages.split("\n")) {
    try {
      const message = JSON.parse(line) as { reason?: string; target?: { name: string }; executable?: string };
      if (message.reason === "compiler-artifact" && message.target && message.executable)
        executables.set(message.target.name, message.executable);
    } catch {
      // not a cargo message
    }
  }
  for (const [variable, target] of BINARIES) {
    const executable = executables.get(target);
    if (!executable) throw new Error(`cargo built no executable ${target}`);
    process.env[variable] = executable;
  }
}
