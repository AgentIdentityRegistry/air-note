import { InputHTMLAttributes } from "react";

export function Input(props: InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      {...props}
      style={{
        padding: "8px 12px",
        borderRadius: 6,
        border: "1px solid #ccc",
        fontFamily: "inherit",
        fontSize: 14,
        width: "100%",
        boxSizing: "border-box",
        ...props.style,
      }}
    />
  );
}
