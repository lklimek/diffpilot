import type { ReactNode } from "react";
import { navigateOnClick, type Navigate } from "./navigate";

/** The item's title, linked to its DiffPilot PR page when available. */
export function WorkTitleLink({
  href,
  onNavigate,
  className,
  children,
}: {
  href?: string;
  onNavigate?: Navigate;
  className?: string;
  children: ReactNode;
}) {
  if (!href) return <span className={className}>{children}</span>;
  return (
    <a
      className={`${className ?? ""} text-accent hover:underline`}
      href={href}
      onClick={navigateOnClick(href, onNavigate)}
    >
      {children}
    </a>
  );
}
