import { InputHTMLAttributes } from "react";

export function Input({ style, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  // The base `input` rule in styles.css supplies border/radius/focus tokens; only width is enforced here.
  return <input {...rest} style={{ width: "100%", ...style }} />;
}
