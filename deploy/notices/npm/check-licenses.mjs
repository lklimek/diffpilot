#!/usr/bin/env node
// Licence gate of the web UI's production npm packages (deploy/notices/README.md): fails unless every package's
// `license` expression is satisfiable by the `accepted` list of deploy/notices/about.toml, the policy of the Rust
// notices. Fails closed: a missing, non-SPDX or unparsable licence fails, `WITH` exceptions are not accepted.
//
// Usage: node check-licenses.mjs PROJECT_DIR ABOUT_TOML   (PROJECT_DIR with its node_modules installed)
//        node check-licenses.mjs --self-test
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";

/** The `accepted = [...]` strings of an about.toml. */
export function acceptedLicences(toml) {
  const list = /^accepted\s*=\s*\[([^\]]*)\]/m.exec(toml);
  if (!list) throw new Error("no accepted = [...] list in about.toml");
  return new Set([...list[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]));
}

/** Whether the SPDX expression can be satisfied with accepted licences only; throws on a malformed expression. */
export function satisfiable(expression, accepted) {
  const tokens = String(expression).match(/\(|\)|[^\s()]+/g) ?? [];
  let at = 0;
  const peek = () => tokens[at];
  const take = () => tokens[at++];
  const factor = () => {
    const token = take();
    if (token === "(") {
      const value = or();
      if (take() !== ")") throw new Error(`missing ) in '${expression}'`);
      return value;
    }
    if (token === undefined || ["AND", "OR", "WITH", ")"].includes(token)) {
      throw new Error(`unexpected '${token ?? "end"}' in '${expression}'`);
    }
    if (peek() === "WITH") {
      take();
      if (take() === undefined)
        throw new Error(`missing exception in '${expression}'`);
      return false;
    }
    return accepted.has(token);
  };
  const and = () => {
    let value = factor();
    while (peek() === "AND") {
      take();
      value = factor() && value;
    }
    return value;
  };
  const or = () => {
    let value = and();
    while (peek() === "OR") {
      take();
      value = and() || value;
    }
    return value;
  };
  const value = or();
  if (at !== tokens.length)
    throw new Error(`unexpected '${peek()}' in '${expression}'`);
  return value;
}

/** Problems of the production packages installed under projectDir, one string each. */
export function problems(projectDir, accepted) {
  const paths = execFileSync(
    "npm",
    ["ls", "--omit=dev", "--all", "--parseable"],
    {
      cwd: projectDir,
      encoding: "utf8",
    },
  )
    .split("\n")
    .filter(Boolean)
    .slice(1);
  if (paths.length === 0)
    return ["no production packages found; is node_modules installed?"];
  const found = [];
  for (const path of new Set(paths)) {
    const pkg = JSON.parse(readFileSync(join(path, "package.json"), "utf8"));
    const name = `${pkg.name}@${pkg.version}`;
    if (typeof pkg.license !== "string" || pkg.license.trim() === "") {
      found.push(`${name}: no SPDX licence expression in package.json`);
      continue;
    }
    try {
      if (!satisfiable(pkg.license, accepted))
        found.push(`${name}: licence '${pkg.license}' is not accepted`);
    } catch (error) {
      found.push(`${name}: ${error.message}`);
    }
  }
  return found;
}

function selfTest() {
  const accepted = acceptedLicences(
    'accepted = [\n    "MIT",\n    "ISC",\n    "Apache-2.0",\n]\n',
  );
  const cases = [
    ["MIT", true],
    ["(MIT OR GPL-3.0-only)", true],
    ["GPL-3.0-only OR MIT", true],
    ["MIT AND GPL-3.0-only", false],
    ["MIT AND (ISC OR GPL-2.0-only)", true],
    ["(MIT AND ISC) OR GPL-2.0-only", true],
    ["GPL-3.0-only", false],
    ["UNLICENSED", false],
    ["Apache-2.0 WITH LLVM-exception", false],
    ["MIT OR", "throws"],
    ["(MIT", "throws"],
    ["", "throws"],
  ];
  let failed = 0;
  for (const [expression, want] of cases) {
    let got;
    try {
      got = satisfiable(expression, accepted);
    } catch {
      got = "throws";
    }
    if (got !== want) {
      console.error(
        `check-licenses self-test FAILED: '${expression}' -> ${got}, want ${want}`,
      );
      failed++;
    }
  }
  if (failed) process.exit(1);
  console.log("check-licenses self-test: ok");
}

if (process.argv[2] === "--self-test") {
  selfTest();
} else if (process.argv.length === 4) {
  const accepted = acceptedLicences(readFileSync(process.argv[3], "utf8"));
  const found = problems(process.argv[2], accepted);
  if (found.length) {
    for (const problem of found) console.error(`check-licenses: ${problem}`);
    console.error(
      "check-licenses: add a licence to deploy/notices/about.toml only after checking its terms",
    );
    process.exit(1);
  }
  console.log(
    "check-licenses: every production npm package has an accepted licence",
  );
} else {
  console.error(
    "usage: node check-licenses.mjs PROJECT_DIR ABOUT_TOML | --self-test",
  );
  process.exit(2);
}
