import { ButtonHTMLAttributes } from "react";

export function Button({
  children,
  variant = "primary",
  className,
  ...rest
}: { variant?: "primary" | "secondary" } & ButtonHTMLAttributes<HTMLButtonElement>) {
  const variantClass = variant === "primary" ? "floating-primary-btn" : "secondary-btn";
  return (
    <button {...rest} className={[variantClass, className].filter(Boolean).join(" ")}>
      {children}
    </button>
  );
}
