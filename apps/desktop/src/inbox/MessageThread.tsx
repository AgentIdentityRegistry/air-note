import { StatusBadge } from "../components/ui/StatusBadge";
import { Button } from "../components/Button";
import { bodyText } from "./bodyText";
import { badgesFor } from "./badges";
import type { ThreadItem } from "./model";

export function MessageThread({ items, onRetry }: { items: ThreadItem[]; onRetry: (it: ThreadItem) => void }) {
  if (items.length === 0) return <div style={{ color: "#666", fontSize: 13, padding: 12 }}>No messages.</div>;
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
      {items.map((it) => {
        const mine = it.direction === "sent";
        return (
          <div key={it.envelope_id} style={{ alignSelf: mine ? "flex-end" : "flex-start", maxWidth: "80%" }}>
            <div style={{
              padding: "8px 12px", borderRadius: 10, fontSize: 14,
              background: mine ? "#2F6BFF" : "#F3F4F6", color: mine ? "white" : "#0B0F17",
              opacity: it.status === "pending" ? 0.6 : 1,
            }}>{bodyText(it.body)}</div>
            <div style={{ display: "flex", gap: 4, marginTop: 2, justifyContent: mine ? "flex-end" : "flex-start", flexWrap: "wrap" }}>
              {badgesFor(it).map((b, i) => <StatusBadge key={i} tone={b.tone}>{b.label}</StatusBadge>)}
              {it.status === "pending" ? <StatusBadge tone="neutral">sending…</StatusBadge> : null}
              {it.status === "err" ? (
                <>
                  <StatusBadge tone="error">{it.reason ?? "failed"}</StatusBadge>
                  {it.retryable ? <Button variant="secondary" onClick={() => onRetry(it)} style={{ padding: "2px 8px", fontSize: 12 }}>Retry</Button> : null}
                </>
              ) : null}
            </div>
          </div>
        );
      })}
    </div>
  );
}
