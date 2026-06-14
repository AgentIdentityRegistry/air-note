import { StatusBadge } from "../components/ui/StatusBadge";
import type { Conversation } from "./model";

const short = (did: string) => did.replace(/^did:wba:[^:]+:agents:/, "");

export function ConversationList({
  conversations, selected, onSelect,
}: { conversations: Conversation[]; selected: string | null; onSelect: (k: string) => void }) {
  if (conversations.length === 0) {
    return <div style={{ color: "#666", fontSize: 13, padding: 12 }}>No conversations yet.</div>;
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {conversations.map((c) => (
        <button key={c.convKey} onClick={() => onSelect(c.convKey)}
          style={{
            textAlign: "left", padding: "8px 10px", borderRadius: 8, cursor: "pointer",
            border: "1px solid " + (c.convKey === selected ? "#2F6BFF" : "#eee"),
            background: c.convKey === selected ? "#EEF3FF" : "white",
          }}>
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
            <span style={{ fontSize: 13, fontWeight: 600 }}>{c.kind === "room" ? "👥 " : ""}{short(c.convKey)}</span>
            {c.unread > 0 ? <StatusBadge tone="primary">{c.unread}</StatusBadge> : null}
          </div>
          <div style={{ fontSize: 12, color: "#666", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {c.lastText}
          </div>
        </button>
      ))}
    </div>
  );
}
