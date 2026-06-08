import type { ReactNode } from "react";

type StatusBadgeTone = "neutral" | "primary" | "accent" | "success" | "warning" | "error";

type StatusBadgeProps = {
  tone?: StatusBadgeTone;
  className?: string;
  children: ReactNode;
};

export function StatusBadge({ tone = "neutral", className, children }: StatusBadgeProps) {
  return (
    <span className={["status-badge", "status-badge-compact", `status-${tone}`, className].filter(Boolean).join(" ")}>
      {children}
    </span>
  );
}
