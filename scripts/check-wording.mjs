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
const WORDS = /\bno\s+risk|\bnothing\s+to\s+review\b|\bno\s+review\s+needed\b|\bnoise\b|\bverified\b|\bwas\s+wrong\b|\bconfidence\b/giu;
// `{}` stands for an interpolated value, so `${share}%` and `{share}%` count as percentages.
const PERCENT = /(?:\d+|\{\})\s?%/gu;
const SLOT = "{}";
// Exempts the percentage rule (never the words) on its line; must be a comment with a reason.
const ALLOW = /(?:\/\/|\/\*)[ \t]*wording-check: allow[ \t]+[^\s*]/g;
const KINDS = {
  ".ts": ts.ScriptKind.TS, ".mts": ts.ScriptKind.TS, ".cts": ts.ScriptKind.TS, ".tsx": ts.ScriptKind.TSX,
  ".js": ts.ScriptKind.JS, ".mjs": ts.ScriptKind.JS, ".cjs": ts.ScriptKind.JS, ".jsx": ts.ScriptKind.JSX,
  ".json": ts.ScriptKind.JSON,
};
const ENTITIES = { nbsp: " ", amp: "&", lt: "<", gt: ">", quot: '"', apos: "'" };

function decodeEntities(text) {
  return text.replace(/&(?:#(\d+)|#x([0-9a-f]+)|([a-z]+));/gi, (entity, dec, hex, name) =>
    dec ? String.fromCodePoint(Number(dec)) : hex ? String.fromCodePoint(parseInt(hex, 16)) : ENTITIES[name] ?? entity);
}

function isModuleSpecifier(node) {
  const parent = node.parent;
  return ((ts.isImportDeclaration(parent) || ts.isExportDeclaration(parent)) && parent.moduleSpecifier === node)
    || ts.isExternalModuleReference(parent)
    || (ts.isCallExpression(parent) && parent.expression.kind === SK.ImportKeyword);
}

const isJsxContainer = (node) => node.kind === SK.JsxElement || node.kind === SK.JsxFragment;

// Rendered text of a JSX element: nested elements inline, `{"literal"}` inline, other expressions as SLOT.
function jsxPieces(element, sf, pieces) {
  for (const child of element.children) {
    if (child.kind === SK.JsxText) {
      pieces.push({ text: decodeEntities(child.text), pos: child.pos, exact: true });
    } else if (isJsxContainer(child)) {
      jsxPieces(child, sf, pieces);
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

// Returns the UI texts of a file (each a list of pieces) and the source ranges of text nodes.
function extract(sf) {
  const texts = [];
  const ranges = [];
  const visit = (node) => {
    switch (node.kind) {
      case SK.StringLiteral:
        if (isModuleSpecifier(node)) return;
        ranges.push([node.getStart(sf), node.end]);
        texts.push([{
          text: node.parent.kind === SK.JsxAttribute ? decodeEntities(node.text) : node.text,
          pos: node.getStart(sf),
        }]);
        return;
      case SK.NoSubstitutionTemplateLiteral:
        ranges.push([node.getStart(sf), node.end]);
        texts.push([{ text: node.text, pos: node.getStart(sf) }]);
        return;
      case SK.TemplateExpression: {
        ranges.push([node.getStart(sf), node.end]);
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
        ranges.push([node.pos, node.end]);
        return;
      case SK.JsxElement:
      case SK.JsxFragment:
        if (!isJsxContainer(node.parent)) texts.push(jsxPieces(node, sf, []));
        break;
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
  return { texts, ranges };
}

function scan(file) {
  const source = readFileSync(file, "utf8");
  const ext = file.slice(file.lastIndexOf("."));
  const sf = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true, KINDS[ext] ?? ts.ScriptKind.TS);
  const lineOf = (pos) => sf.getLineAndCharacterOfPosition(pos).line + 1;
  const { texts, ranges } = extract(sf);
  const allowed = new Set();
  for (const marker of source.matchAll(ALLOW)) {
    if (!ranges.some(([start, end]) => marker.index >= start && marker.index < end)) allowed.add(lineOf(marker.index));
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
      found.add(`${relative(srcDir, file)}:${line}: "${match[0].replace(/\s+/g, " ")}"`);
    };
    for (const match of joined.matchAll(WORDS)) report(match, false);
    for (const match of joined.matchAll(PERCENT)) report(match, true);
  }
  return found;
}

const files = readFileSync(0, "utf8").split("\0").filter(Boolean);
for (const file of files) {
  for (const violation of scan(file)) process.stdout.write(`${violation}\n`);
}
