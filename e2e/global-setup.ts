// Global setup: resolves the binaries every spec's stack runs (support/stack.ts). Prebuilt binaries are taken from
// SR_DEV_*_BIN when all four are set; otherwise the web UI and the binaries are built once here and published to
// the workers through the environment. Nothing is built against an already-running stack (SR_E2E_BASE_URL).
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("..", import.meta.url));
/** Environment variable → cargo target name. */
const BINARIES: [string, string][] = [
  ["SR_DEV_SERVER_BIN", "smart-review-server"],
  ["SR_DEV_RUNNER_BIN", "smart-review-runner"],
  ["SR_DEV_FAKE_GITHUB_BIN", "fake-github"],
  ["SR_DEV_FAKE_MEMCAN_BIN", "fake-memcan"],
];

function run(command: string, args: string[]): string {
  const result = spawnSync(command, args, {
    cwd: REPO,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed (${result.status})`);
  return result.stdout;
}

export default function globalSetup(): void {
  if (process.env.SR_E2E_BASE_URL) return;
  if (BINARIES.every(([variable]) => process.env[variable])) return;
  console.error("building the web UI and the binaries (set SR_DEV_*_BIN to skip)");
  if (!existsSync(`${REPO}/web/node_modules`)) run("npm", ["--prefix", "web", "ci"]);
  run("npm", ["--prefix", "web", "run", "build"]);
  const packages = "-p server --features server/embed-ui -p runner --features runner/fake-provider";
  const messages = run("cargo", [
    "build",
    "--message-format=json-render-diagnostics",
    ...`${packages} -p fake-github -p fake-memcan`.split(" "),
  ]);
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
