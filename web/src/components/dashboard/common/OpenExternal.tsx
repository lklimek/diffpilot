import { Tooltip } from "../../tooltip/Tooltip";
import { ExternalIcon } from "./icons";

/** The URL when it is http(s), else null (item URLs are untrusted data). */
export function httpUrl(url: string | null | undefined): string | null {
  if (!url) return null;
  try {
    const { protocol } = new URL(url);
    return protocol === "http:" || protocol === "https:" ? url : null;
  } catch {
    return null;
  }
}

/** Name of an item's link out: `Open PLAT-1203 on GitHub`, or `Open TODO in MemCan`. */
export const openLabel = (item: { kind?: string; id: string }): string =>
  item.kind === "todo" ? "Open TODO in MemCan" : `Open ${item.id} on GitHub`;

/** An always visible icon link to `href` in a new tab, named and tipped `label`; nothing unless http(s). */
export function OpenExternal({ href, label }: { href: string | null | undefined; label: string }) {
  const url = httpUrl(href);
  if (!url) return null;
  return (
    <Tooltip tip={label}>
      <a
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        aria-label={label}
        // The row's own handlers stay out of it.
        onClick={(event) => event.stopPropagation()}
        className="inline-flex min-h-6 min-w-6 shrink-0 items-center justify-center rounded border border-border text-accent hover:border-accent hover:bg-accent-bg"
      >
        <ExternalIcon />
      </a>
    </Tooltip>
  );
}
