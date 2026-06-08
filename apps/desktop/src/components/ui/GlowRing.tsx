import type { ReactNode } from "react";

type GlowState = "idle" | "planning" | "executing" | "approval" | "error" | "completed";

type GlowRingProps = {
  state?: GlowState;
  className?: string;
  children: ReactNode;
};

export function GlowRing({ state = "idle", className, children }: GlowRingProps) {
  return (
    <span className={["glow-ring", `glow-${state}`, className].filter(Boolean).join(" ")}>
      <span className="glow-ring-inner">{children}</span>
    </span>
  );
}
