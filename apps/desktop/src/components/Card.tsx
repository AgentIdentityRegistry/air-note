import { ReactNode } from "react";

export function Card({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        border: "1px solid #eee",
        borderRadius: 8,
        padding: 24,
        background: "white",
      }}
    >
      {children}
    </div>
  );
}
