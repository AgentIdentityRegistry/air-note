import type { ElementType, ReactNode } from "react";

type SurfaceProps = {
  as?: ElementType;
  className?: string;
  elevation?: "level1" | "level2" | "level3";
  children: ReactNode;
};

export function Surface({
  as: Component = "section",
  className,
  elevation = "level2",
  children
}: SurfaceProps) {
  const classes = ["surface", `surface-${elevation}`, className].filter(Boolean).join(" ");

  return <Component className={classes}>{children}</Component>;
}
