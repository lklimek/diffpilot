import { useRef, type ReactNode } from "react";
import { Tooltip } from "../../tooltip/Tooltip";
import { filterId, filterLabel, type Filter, type FilterValue } from "./filters";

export type OnFilter = (value: FilterValue, excluded: boolean) => void;

export function FilterButton({
  value,
  onFilter,
  tip,
  children,
  className = "",
  ...data
}: {
  value: FilterValue;
  onFilter: OnFilter;
  tip?: string;
  children: ReactNode;
  className?: string;
  "data-fact-tag"?: boolean;
  "data-size-pill"?: string;
}) {
  return (
    <Tooltip tip={`${tip ? `${tip}. ` : ""}Alt-click to exclude. Click to show only these`}>
      <button
        type="button"
        aria-label={`Filter by ${value.field} ${value.label}`}
        className={`min-h-6 min-w-6 cursor-pointer rounded hover:bg-accent-bg ${className}`}
        onClick={(event) => onFilter(value, event.altKey)}
        {...data}
      >
        {children}
      </button>
    </Tooltip>
  );
}

export function FilterChips({
  filters,
  labels,
  onChange,
}: {
  filters: Filter[];
  labels: readonly FilterValue[];
  onChange: (filters: Filter[]) => void;
}) {
  const region = useRef<HTMLDivElement>(null);
  const names = new Map(labels.map((value) => [filterId(value), value.label]));
  const clear = () => {
    region.current?.focus();
    onChange([]);
  };
  return (
    <div
      ref={region}
      role="region"
      aria-label="Active filters"
      tabIndex={-1}
      className="mt-1.5 min-w-0"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || !filters.length) return;
        event.preventDefault();
        event.stopPropagation();
        clear();
      }}
    >
      {filters.length === 0 ? (
        <p className="text-xs text-muted">Click a label, size, author or repository to filter.</p>
      ) : (
        <ul className="m-0 flex list-none items-center gap-1.5 overflow-x-auto p-1 text-xs whitespace-nowrap">
          {filters.map((filter, i) => {
            const id = filterId(filter);
            const label = names.get(id) ?? filterLabel(filter);
            return (
              <li
                key={id}
                className="inline-flex shrink-0 items-center gap-0.5 rounded-full border border-border bg-surface-2 px-1.5"
              >
                <Tooltip tip="Switch between including and excluding this filter.">
                  <button
                    type="button"
                    aria-pressed={filter.excluded}
                    data-filter-body
                    className="min-h-6 cursor-pointer px-1"
                    onClick={() =>
                      onChange(
                        filters.map((value) =>
                          value === filter ? { ...value, excluded: !value.excluded } : value,
                        ),
                      )
                    }
                  >
                    {filter.excluded && <span aria-hidden>≠ </span>}
                    {filter.field}: <b>{label}</b>
                  </button>
                </Tooltip>
                <Tooltip tip="Remove this filter.">
                  <button
                    type="button"
                    aria-label={`Remove filter ${filter.field} ${label}`}
                    className="size-6 cursor-pointer rounded-full text-muted hover:bg-accent-bg"
                    onClick={(event) => {
                      const row = event.currentTarget.closest("ul");
                      const buttons = row?.querySelectorAll<HTMLButtonElement>("[data-filter-body]");
                      (buttons?.[i + 1] ?? buttons?.[i - 1] ?? region.current)?.focus();
                      onChange(filters.filter((value) => value !== filter));
                    }}
                  >
                    ×
                  </button>
                </Tooltip>
              </li>
            );
          })}
          <li>
            <Tooltip tip="Clear all filters. You can also press Escape in the chip row.">
              <button
                type="button"
                className="min-h-6 cursor-pointer px-1 text-accent underline"
                onClick={clear}
              >
                Clear all
              </button>
            </Tooltip>
          </li>
          <li className="text-muted">P0 alarms stay shown</li>
        </ul>
      )}
    </div>
  );
}
