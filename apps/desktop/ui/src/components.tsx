// Shared visual components (P12-003). Every workflow uses these, so states look and read the
// same everywhere. Meaning never relies on color alone: badges and notices carry an icon and
// text, and the icon is hidden from screen readers because the text already says it.
import type { ButtonHTMLAttributes, ReactNode } from "react";

export type Tone = "ok" | "info" | "warning" | "error" | "progress";

const ICON: Record<Tone, string> = {
  ok: "✓",
  info: "i",
  warning: "!",
  error: "×",
  progress: "…",
};

export function Card({
  id,
  heading,
  children,
}: {
  id: string;
  heading: string;
  children: ReactNode;
}) {
  return (
    <section className="card" aria-labelledby={id}>
      <h2 id={id}>{heading}</h2>
      {children}
    </section>
  );
}

/** A short status such as "Working well". */
export function StatusBadge({ tone, children }: { tone: Tone; children: ReactNode }) {
  return (
    <span className={`badge badge-${tone}`}>
      <span className="badge-icon" aria-hidden="true">
        {ICON[tone]}
      </span>
      {children}
    </span>
  );
}

/**
 * A message block. Errors interrupt assistive technology (`alert`); everything else is a
 * polite `status`. `live={false}` renders a static notice with no live region.
 */
export function Notice({
  tone,
  title,
  children,
  live = true,
}: {
  tone: Exclude<Tone, "progress">;
  title?: string;
  children?: ReactNode;
  live?: boolean;
}) {
  const role = !live ? undefined : tone === "error" ? "alert" : "status";
  return (
    <div className={`notice notice-${tone}`} role={role}>
      <span className="notice-icon" aria-hidden="true">
        {ICON[tone]}
      </span>
      <div>
        {title && <strong className="notice-title">{title}</strong>}
        {children && <div>{children}</div>}
      </div>
    </div>
  );
}

export function Button({
  variant = "secondary",
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "secondary" | "danger" }) {
  return (
    <button
      type="button"
      {...rest}
      className={["btn", `btn-${variant}`, className].filter(Boolean).join(" ")}
    />
  );
}

export function EmptyState({ children }: { children: ReactNode }) {
  return <p className="empty">{children}</p>;
}

export function Loading({ children = "Loading…" }: { children?: ReactNode }) {
  return (
    <p className="loading" role="status" aria-busy="true">
      {children}
    </p>
  );
}
