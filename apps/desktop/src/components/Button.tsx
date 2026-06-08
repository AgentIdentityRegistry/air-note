import { ButtonHTMLAttributes } from "react";

export function Button({
  children,
  variant = "primary",
  ...rest
}: { variant?: "primary" | "secondary" } & ButtonHTMLAttributes<HTMLButtonElement>) {
  const styles =
    variant === "primary"
      ? { background: "#1a1a1a", color: "white", border: "1px solid #1a1a1a" }
      : { background: "white", color: "#1a1a1a", border: "1px solid #ccc" };
  return (
    <button
      {...rest}
      style={{
        ...styles,
        padding: "8px 16px",
        borderRadius: 6,
        fontFamily: "inherit",
        fontSize: 14,
        cursor: "pointer",
        ...rest.style,
      }}
    >
      {children}
    </button>
  );
}
