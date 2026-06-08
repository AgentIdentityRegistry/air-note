export function Loading({ label = "Working..." }: { label?: string }) {
  return (
    <div style={{ color: "#888", fontStyle: "italic" }}>{label}</div>
  );
}
