import type { ButtonHTMLAttributes, ReactNode } from "react";

type FloatingPrimaryButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  children: ReactNode;
};

export function FloatingPrimaryButton({ children, className, ...props }: FloatingPrimaryButtonProps) {
  return (
    <button
      {...props}
      className={["floating-primary-btn", className].filter(Boolean).join(" ")}
    >
      {children}
    </button>
  );
}
