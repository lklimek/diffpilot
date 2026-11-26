import type { MarkedItem } from "../../api/generated/types";
import { buttonClass } from "../button/button";
import { OpenExternal } from "../dashboard/common/OpenExternal";
import { dateTimeText } from "../format/time";
import { StatusMessage } from "../status/StatusMessage";
import { Tooltip } from "../tooltip/Tooltip";
import { Section } from "./Section";

export function MarkedItemsCard({
  items,
  error,
  loadingOlder,
  onShowOlder,
}: {
  items?: MarkedItem[];
  error?: string;
  loadingOlder: boolean;
  onShowOlder?: () => void;
}) {
  return (
    <Section title="Not a review point">
      {error && <StatusMessage kind="error">{error}</StatusMessage>}
      {!items ? (
        !error && <StatusMessage kind="loading">Loading marked items…</StatusMessage>
      ) : items.length === 0 ? (
        <StatusMessage kind="empty">No items marked.</StatusMessage>
      ) : (
        <ol className="m-0 grid list-none gap-2 p-0 text-sm">
          {items.map((item) => {
            const pr = `${item.pr.repo.owner}/${item.pr.repo.name} #${item.pr.number}`;
            return (
              <li
                key={`${pr}:C${item.no}`}
                className="rounded-md border border-border px-3 py-2 wrap-anywhere"
              >
                <p className="m-0">
                  <b>{item.title}</b>{" "}
                  <OpenExternal href={item.url} label={`Open ${pr} C${item.no} on GitHub`} />
                </p>
                <div className="text-xs text-muted">
                  {pr} · C{item.no} · {item.kind.replaceAll("_", " ")} · {item.author} ({item.author_kind})
                  {item.status_comment && " · Status comment"}
                </div>
                <div className="text-xs text-muted">
                  Marked by {item.by} · {dateTimeText(item.at)}
                </div>
              </li>
            );
          })}
        </ol>
      )}
      {onShowOlder && (
        <Tooltip tip="Loads the next page of marked items.">
          <button
            type="button"
            aria-disabled={loadingOlder || undefined}
            onClick={() => !loadingOlder && onShowOlder()}
            className={`${buttonClass("plain")} justify-self-start`}
          >
            {loadingOlder ? "Loading older marked items…" : "Show older marked items"}
          </button>
        </Tooltip>
      )}
    </Section>
  );
}
