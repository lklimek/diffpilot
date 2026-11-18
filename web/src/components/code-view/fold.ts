/** A displayed item, or a run of consecutive hidden items folded into one row. */
export type Piece<T> = { kind: "item"; item: T } | { kind: "fold"; items: T[] };

/** Groups consecutive hidden items into folds, in order. */
export function foldRuns<T>(items: readonly T[], hidden: (item: T) => boolean): Piece<T>[] {
  const pieces: Piece<T>[] = [];
  for (const item of items) {
    const last = pieces.at(-1);
    if (!hidden(item)) pieces.push({ kind: "item", item });
    else if (last?.kind === "fold") last.items.push(item);
    else pieces.push({ kind: "fold", items: [item] });
  }
  return pieces;
}

// Import, use and include statements of common languages; any other rule-covered line counts as a comment.
const IMPORT =
  /^\s*(pub(\([^)]*\))?\s+)?(use\s|import\s|from\s+\S+\s+import\s|#include\b|extern\s+crate\s|require\(|(const|let|var)\s+\S+\s*=\s*require\()/;

/** What a run of rule-covered lines is, from their text: `comment`, `import`, or both. */
export function foldKind(texts: readonly string[]): string {
  const imports = texts.filter((t) => IMPORT.test(t)).length;
  if (imports === 0) return "comment";
  return imports === texts.length ? "import" : "comment and import";
}
