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
