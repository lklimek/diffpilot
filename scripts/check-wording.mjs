// Reports banned wording (docs/design-brief.md §1, docs/implementation-plan.md §12.4) in UI text of web
// sources: string literal values, template literal static parts and JSX text, parsed with the TypeScript 6
// compiler API (TypeScript 7 has no synchronous parser API). Entry point: scripts/check-wording.sh.
//
// Usage: node check-wording.mjs TYPESCRIPT_PACKAGE_DIR SRC_DIR < NUL-separated file paths
// Prints `file:line: "match"` per violation and exits 0; exits 2 when the scan cannot run.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { relative } from "node:path";

const [tsDir, srcDir] = process.argv.slice(2);

function fail(message) {
  process.stderr.write(`wording check: ${message}\n`);
  process.exit(2);
}

let ts;
try {
  ts = createRequire(import.meta.url)(tsDir);
} catch (error) {
  fail(`cannot load TypeScript from ${tsDir} (${error.code ?? error.message}); run npm ci in web/ or set ` +
    "CHECK_WORDING_TYPESCRIPT to a TypeScript 6 package directory");
}
if (typeof ts.createSourceFile !== "function") {
  fail(`TypeScript ${ts.version} at ${tsDir} has no classic compiler API (TypeScript 7+); set ` +
    "CHECK_WORDING_TYPESCRIPT to a TypeScript 6 package directory");
}

const SK = ts.SyntaxKind;
// The rules are shared with the rendered-text checks of e2e/support/app.ts. `{}` stands for an interpolated
// value, so `${share}%` and `{share}%` count as percentages.
const RULES = JSON.parse(readFileSync(new URL("wording-rules.json", import.meta.url), "utf8"));
const WORDS = new RegExp(RULES.words, "giu");
const PERCENT = new RegExp(RULES.percent, "gu");
// Claims the UI cannot back (the agent is only told not to run code); .json files hold agent output.
const CLAIMS = new RegExp(RULES.claims, "giu");
const SLOT = "{}";
// Exempts the percentage rule (never the words) on its line; must be in a comment, with a reason.
const ALLOW = /wording-check: allow[ \t]+[^\s*]/;
const KINDS = {
  ".ts": ts.ScriptKind.TS, ".mts": ts.ScriptKind.TS, ".cts": ts.ScriptKind.TS, ".tsx": ts.ScriptKind.TSX,
  ".js": ts.ScriptKind.JS, ".mjs": ts.ScriptKind.JS, ".cjs": ts.ScriptKind.JS, ".jsx": ts.ScriptKind.JSX,
  ".json": ts.ScriptKind.JSON,
};
const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };
// JSX nodes after which JSX text (not trivia) may follow.
const JSX_TEXT_NEIGHBOURS = new Set([SK.JsxText, SK.JsxElement, SK.JsxFragment, SK.JsxSelfClosingElement,
  SK.JsxOpeningElement, SK.JsxClosingElement, SK.JsxOpeningFragment, SK.JsxClosingFragment, SK.JsxExpression]);

// Numeric entities and the syntax-relevant named ones are decoded; any other named entity (&nbsp;, &ensp;,
// &mdash;, ...) counts as a space: no JSX entity is an ASCII letter, digit or `%`, so matching stays exact.
function decodeEntities(text) {
  return text.replace(/&(?:#(\d+)|#x([0-9a-f]+)|(\w+));/gi, (entity, dec, hex, name) =>
    dec ? String.fromCodePoint(Number(dec)) : hex ? String.fromCodePoint(parseInt(hex, 16)) : ENTITIES[name] ?? " ");
}

// Strings that are code, not UI text: module paths, property names, element access keys.
function isCodeString(node) {
  const parent = node.parent;
  return ((ts.isImportDeclaration(parent) || ts.isExportDeclaration(parent)) && parent.moduleSpecifier === node)
    || ts.isExternalModuleReference(parent)
    || (ts.isCallExpression(parent) && parent.expression.kind === SK.ImportKeyword)
    || parent.name === node
    || (ts.isElementAccessExpression(parent) && parent.argumentExpression === node);
}

const isJsxContainer = (node) => node.kind === SK.JsxElement || node.kind === SK.JsxFragment;

// Rendered text of a JSX element: nested elements inline between spaces (element boundaries keep word
// boundaries), `{"literal"}` inline, other expressions as SLOT.
function jsxPieces(element, sf, pieces) {
  for (const child of element.children) {
    if (child.kind === SK.JsxText) {
      pieces.push({ text: decodeEntities(child.text), pos: child.pos, exact: true });
    } else if (isJsxContainer(child) || child.kind === SK.JsxSelfClosingElement) {
      pieces.push({ text: " ", pos: child.getStart(sf) });
      if (isJsxContainer(child)) jsxPieces(child, sf, pieces);
      pieces.push({ text: " ", pos: child.end });
    } else if (child.kind === SK.JsxExpression && !child.expression) {
      // comment only: renders nothing
    } else if (child.kind === SK.JsxExpression && ts.isStringLiteralLike(child.expression)) {
      pieces.push({ text: child.expression.text, pos: child.expression.getStart(sf) });
    } else {
      pieces.push({ text: SLOT, pos: child.getStart(sf) });
    }
  }
  return pieces;
}

// Returns the UI texts of a file (each a list of pieces; every JSX text and element also on its own) and
// the comments, taken from the trivia around AST nodes outside JSX text.
function extract(sf) {
  const texts = [];
  const comments = [];
  // Leading ranges start after a line break; same-line comments are trailing ranges.
  const commentsAt = (pos) => comments.push(...(ts.getLeadingCommentRanges(sf.text, pos) ?? []),
    ...(ts.getTrailingCommentRanges(sf.text, pos) ?? []));
  const visit = (node) => {
    if (ts.isTypeNode(node)) return;
    if (node.kind !== SK.JsxText) {
      commentsAt(node.pos);
      if (node.kind === SK.JsxExpression) commentsAt(node.pos + 1);
      if (!JSX_TEXT_NEIGHBOURS.has(node.kind)) commentsAt(node.end);
    }
    switch (node.kind) {
      case SK.StringLiteral:
        if (isCodeString(node)) return;
        texts.push([{
          text: node.parent.kind === SK.JsxAttribute ? decodeEntities(node.text) : node.text,
          pos: node.getStart(sf),
        }]);
        return;
      case SK.NoSubstitutionTemplateLiteral:
        texts.push([{ text: node.text, pos: node.getStart(sf) }]);
        return;
      case SK.TemplateExpression: {
        const pieces = [{ text: node.head.text, pos: node.head.getStart(sf) }];
        for (const span of node.templateSpans) {
          pieces.push({ text: SLOT, pos: span.expression.getStart(sf) });
          pieces.push({ text: span.literal.text, pos: span.literal.getStart(sf) });
          visit(span.expression);
        }
        texts.push(pieces);
        return;
      }
      case SK.JsxText:
        texts.push([{ text: decodeEntities(node.text), pos: node.pos, exact: true }]);
        return;
      case SK.JsxElement:
      case SK.JsxFragment:
        texts.push(jsxPieces(node, sf, []));
        break;
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return { texts, comments };
}

function scan(file) {
  const source = readFileSync(file, "utf8");
  const ext = file.slice(file.lastIndexOf("."));
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, KINDS[ext] ?? ts.ScriptKind.TS);
  const lineOf = (pos) => sf.getLineAndCharacterOfPosition(pos).line + 1;
  if (!Array.isArray(sf.parseDiagnostics)) fail(`TypeScript ${ts.version} exposes no parse diagnostics`);
  if (sf.parseDiagnostics.length > 0) {
    fail(`cannot check ${relative(srcDir, file)}, it does not parse:\n` + sf.parseDiagnostics
      .map((d) => `  line ${lineOf(d.start)}: ${ts.flattenDiagnosticMessageText(d.messageText, " ")}`).join("\n"));
  }
  const { texts, comments } = extract(sf);
  const allowed = new Set();
  for (const comment of comments) {
    const marker = ALLOW.exec(source.slice(comment.pos, comment.end));
    if (marker) allowed.add(lineOf(comment.pos + marker.index));
  }
  const found = new Set();
  for (const pieces of texts) {
    let joined = "";
    const starts = pieces.map((piece) => ((joined += piece.text), joined.length - piece.text.length));
    const report = (match, isPercent) => {
      let i = pieces.length - 1;
      while (starts[i] > match.index) i -= 1;
      const piece = pieces[i];
      const before = piece.exact ? piece.text.slice(0, match.index - starts[i]) : "";
      const line = lineOf(piece.pos) + (before.match(/\n/g)?.length ?? 0);
      if (isPercent && allowed.has(line)) return;
      found.add(`${relative(srcDir, file)}:${line}: "${match[0].trim().replace(/\s+/g, " ")}"`);
    };
    for (const match of joined.matchAll(WORDS)) report(match, false);
    for (const match of joined.matchAll(PERCENT)) report(match, true);
    if (ext !== ".json") for (const match of joined.matchAll(CLAIMS)) report(match, false);
  }
  return found;
}

// Directories outside the rules (My work, ADR-016), relative to the source root.
const EXCLUDED = RULES.exclude ?? [];
const files = readFileSync(0, "utf8")
  .split("\0")
  .filter(Boolean)
  .filter((file) => !EXCLUDED.some((dir) => relative(srcDir, file).replaceAll("\\", "/").startsWith(dir)));
for (const file of files) {
  for (const violation of scan(file)) process.stdout.write(`${violation}\n`);
}
