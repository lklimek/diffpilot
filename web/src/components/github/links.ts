import type { LineSide, PatternSite, PrSummary, StepFacts } from "../../api/generated/types";

/** What a pull request's GitHub line links are built from. */
export type PrLinkBase = Pick<PrSummary, "repo" | "number" | "base_sha" | "head_sha">;

/** Lines `start..=end` of `path` on one side of the diff. */
export interface LineSpan {
  path: string;
  side: LineSide;
  start: number;
  end: number;
}

// Only server-built GitHub links become links (threat model T12); anything else stays text.
export const isGithubUrl = (url: string) => url.startsWith("https://github.com/");

type Words8 = [number, number, number, number, number, number, number, number];

const K = new Uint32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98,
  0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
  0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8,
  0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
  0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819,
  0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
  0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
  0xc67178f2,
]);

/**
 * SHA-256 of the UTF-8 bytes of `text` as lowercase hex (FIPS 180-4). Synchronous and pure, because
 * `crypto.subtle` is missing on plain-http origins and links are built while rendering.
 */
export function sha256Hex(text: string): string {
  const bytes = new TextEncoder().encode(text);
  const blocks = Math.ceil((bytes.length + 9) / 64);
  const data = new Uint8Array(blocks * 64);
  data.set(bytes);
  data[bytes.length] = 0x80;
  const view = new DataView(data.buffer);
  view.setUint32(data.length - 8, Math.floor(bytes.length / 0x20000000));
  view.setUint32(data.length - 4, (bytes.length << 3) >>> 0);
  const h = new Uint32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  const w = new Uint32Array(64);
  const rotr = (x: number, n: number) => (x >>> n) | (x << (32 - n));
  for (let offset = 0; offset < data.length; offset += 64) {
    for (let i = 0; i < 16; i++) w[i] = view.getUint32(offset + i * 4);
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15]!, 7) ^ rotr(w[i - 15]!, 18) ^ (w[i - 15]! >>> 3);
      const s1 = rotr(w[i - 2]!, 17) ^ rotr(w[i - 2]!, 19) ^ (w[i - 2]! >>> 10);
      w[i] = w[i - 16]! + s0 + w[i - 7]! + s1;
    }
    let [a, b, c, d, e, f, g, hh] = Array.from(h) as Words8;
    for (let i = 0; i < 64; i++) {
      const t1 = hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[i]! + w[i]!;
      const t2 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c));
      hh = g;
      g = f;
      f = e;
      e = (d + t1) | 0;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) | 0;
    }
    [a, b, c, d, e, f, g, hh].forEach((x, i) => (h[i] = h[i]! + x));
  }
  return Array.from(h, (x) => x.toString(16).padStart(8, "0")).join("");
}

const repoUrl = (pr: PrLinkBase) => `https://github.com/${pr.repo.owner}/${pr.repo.name}`;

/**
 * GitHub link to `span`: in the PR's changes view (`#diff-<sha256 of path>R<a>-R<b>`, `L` for base) when the
 * lines are in the diff, else the file at the analysed commit of their side (`blob/<sha>/<path>#L<a>-L<b>`).
 */
export function githubLineUrl(pr: PrLinkBase, span: LineSpan, inDiff: boolean): string {
  const lines = (prefix: string) =>
    span.end > span.start ? `${prefix}${span.start}-${prefix}${span.end}` : `${prefix}${span.start}`;
  if (inDiff) {
    const side = span.side === "base" ? "L" : "R";
    return `${repoUrl(pr)}/pull/${pr.number}/changes#diff-${sha256Hex(span.path)}${lines(side)}`;
  }
  const sha = span.side === "base" ? pr.base_sha : pr.head_sha;
  const path = span.path.split("/").map(encodeURIComponent).join("/");
  return `${repoUrl(pr)}/blob/${sha}/${path}#${lines("L")}`;
}

/** The step's changed lines (first to last) in the PR's changes view; null without changed lines. */
export function stepDiffUrl(
  pr: PrLinkBase,
  facts: Pick<StepFacts, "file" | "side" | "changed_lines">,
): string | null {
  if (facts.changed_lines.length === 0) return null;
  const start = Math.min(...facts.changed_lines.map((r) => r.start_line));
  const end = Math.max(...facts.changed_lines.map((r) => r.end_line));
  return githubLineUrl(pr, { path: facts.file, side: facts.side, start, end }, true);
}

/** The site's lines that are changed lines of its step, first to last, in the PR's changes view; else null. */
export function siteDiffUrl(
  pr: PrLinkBase,
  site: Pick<PatternSite, "file" | "side" | "range">,
  step: Pick<StepFacts, "file" | "side" | "changed_lines"> | undefined,
): string | null {
  if (!step || step.file !== site.file || step.side !== site.side) return null;
  let [start, end] = [Infinity, -Infinity];
  for (const r of step.changed_lines) {
    const [a, b] = [Math.max(r.start_line, site.range.start_line), Math.min(r.end_line, site.range.end_line)];
    if (a <= b) [start, end] = [Math.min(start, a), Math.max(end, b)];
  }
  if (start > end) return null;
  return githubLineUrl(pr, { path: site.file, side: site.side, start, end }, true);
}

/** A GitHub line link with its tip. */
export interface GithubLineLink {
  url: string;
  tip: string;
}

/** The PR diff link when there is one, else the server's link at the analysed commit of `side`. */
const lineLink = (diff: string | null, commitUrl: string, side: LineSide): GithubLineLink =>
  diff
    ? {
        url: diff,
        tip: "Open the changed lines in the pull request diff on GitHub (new tab). GitHub shows the latest commits, which may differ from the analysed ones.",
      }
    : { url: commitUrl, tip: `Open these lines on GitHub at the ${side} commit (new tab)` };

/** Link of a step's lines: its changed lines in the PR diff, else its lines at the analysed commit. */
export const stepGithubLink = (pr: PrLinkBase | undefined, facts: StepFacts): GithubLineLink =>
  lineLink(pr ? stepDiffUrl(pr, facts) : null, facts.github_url, facts.side);

/** Link of a pattern site: its changed lines in its step's PR diff, else its lines at the analysed commit. */
export const siteGithubLink = (
  pr: PrLinkBase | undefined,
  site: PatternSite,
  step: Pick<StepFacts, "file" | "side" | "changed_lines"> | undefined,
): GithubLineLink => lineLink(pr ? siteDiffUrl(pr, site, step) : null, site.github_url, site.side);
