import type { ChangeSizeMark, RepoRef, SlackName } from "../../../api/generated/types";
import { httpUrl } from "./OpenExternal";

/** One pull request of a copied list. */
export interface CopyLine {
  repo: RepoRef;
  number: number;
  url: string;
  title: string;
  change_size?: ChangeSizeMark | null;
  /** Text after the size, e.g. `waits on @x for 5 d`. */
  suffix?: string;
}

/** A GitHub login as copied and shown: `@login`; a team (`team:<slug>`) as given. */
export const atLogin = (login: string) => (login.startsWith("team:") ? login : `@${login}`);

/** What Copy for Slack writes for `login`: its Slack name from Settings (login in any case), else `@login`. */
export function slackName(login: string, names: SlackName[]): string {
  const lower = login.toLowerCase();
  return names.find((name) => name.login.toLowerCase() === lower)?.slack ?? atLogin(login);
}

/** A copied list: Markdown lines as plain text, the same lines with anchors as HTML. */
export interface Clip {
  text: string;
  html: string;
}

const escapeHtml = (value: string) =>
  value.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** The tail after the link: `Title - small (+25 / -3) — waits on @x for 5 d`. */
function tail({ title, change_size: size, suffix }: CopyLine): string {
  const sized = size ? `${title} - ${size.size} (+${size.additions} / -${size.deletions})` : title;
  return suffix ? `${sized} — ${suffix}` : sized;
}

/** One line per PR, `[owner/name#number](url) Title - small (+25 / -3) — suffix`, to paste into Slack. */
export function copyText(lines: CopyLine[]): Clip {
  const rows = lines.map((line) => {
    const ref = `${line.repo.owner}/${line.repo.name}#${line.number}`;
    const url = httpUrl(line.url);
    const rest = tail(line);
    return {
      text: url ? `[${ref}](${url}) ${rest}` : `${ref} ${rest}`,
      html: `${url ? `<a href="${escapeHtml(url)}">${escapeHtml(ref)}</a>` : escapeHtml(ref)} ${escapeHtml(rest)}`,
    };
  });
  return { text: rows.map((r) => r.text).join("\n"), html: rows.map((r) => r.html).join("<br>") };
}

/** Copies from a `copy` event of `execCommand`: works without a secure context and still sets both formats. */
function copyByEvent({ text, html }: Clip): boolean {
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  // A selection to copy, for browsers that fire no copy event without one.
  const area = document.createElement("textarea");
  area.value = text;
  area.readOnly = true;
  area.setAttribute("aria-hidden", "true");
  area.style.position = "fixed";
  area.style.opacity = "0";
  const onCopy = (event: ClipboardEvent) => {
    if (!event.clipboardData) return;
    event.clipboardData.setData("text/plain", text);
    event.clipboardData.setData("text/html", html);
    event.preventDefault();
  };
  document.addEventListener("copy", onCopy);
  document.body.append(area);
  area.select();
  try {
    return document.execCommand("copy");
  } catch {
    return false;
  } finally {
    document.removeEventListener("copy", onCopy);
    area.remove();
    focused?.focus({ preventScroll: true });
  }
}

/**
 * Writes `clip` as HTML and plain text; false when the browser refused. The Clipboard API needs a secure context:
 * over plain HTTP (the LAN deployment) a `copy` event carries both formats instead.
 */
export async function writeClipboard(clip: Clip): Promise<boolean> {
  if (window.isSecureContext && navigator.clipboard?.write && typeof ClipboardItem !== "undefined") {
    try {
      await navigator.clipboard.write([
        new ClipboardItem({
          "text/html": new Blob([clip.html], { type: "text/html" }),
          "text/plain": new Blob([clip.text], { type: "text/plain" }),
        }),
      ]);
      return true;
    } catch {
      // Refused (permission, focus): the copy event below may still work within the same click.
    }
  }
  return copyByEvent(clip);
}
