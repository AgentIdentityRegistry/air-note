export function Loading({ label = "Working..." }: { label?: string }) {
  return <div style={{ color: "var(--text-tertiary)", fontStyle: "italic" }}>{label}</div>;
}
