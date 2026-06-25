import { StatusBadge } from "../components/ui/StatusBadge";
import type { Conversation } from "./model";
import type { ContactView } from "../api/inbox";
import { conversationLabel } from "./displayName";

export function ConversationList({
  conversations, contacts, selected, onSelect,
}: {
  conversations: Conversation[];
  contacts: Map<string, ContactView>;
  selected: string | null;
  onSelect: (k: string) => void;
}) {
  if (conversations.length === 0) {
    return <div style={{ color: "var(--text-secondary)", fontSize: 13, padding: 12 }}>No conversations yet.</div>;
  }
  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      {conversations.map((c) => {
        const contact = c.kind === "peer" ? contacts.get(c.convKey) : undefined;
        const { label, handle } = conversationLabel(c.convKey, c.kind, contact);
        return (
          <button key={c.convKey} onClick={() => onSelect(c.convKey)}
            style={{
              textAlign: "left", padding: "8px 10px", borderRadius: 8, cursor: "pointer",
              border: "1px solid " + (c.convKey === selected ? "color-mix(in srgb, var(--primary) 26%, transparent)" : "var(--border-soft)"),
              background: c.convKey === selected ? "color-mix(in srgb, var(--primary) 10%, var(--surface))" : "var(--surface)",
            }}>
            <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: 8 }}>
              <span style={{ fontSize: 13, fontWeight: 600, display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
                <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                  {c.kind === "room" ? "👥 " : ""}{label}
                </span>
                {contact?.verified_at_pin ? (
                  <span title="Verified on AIR" aria-label="Verified" style={{ color: "var(--primary)", fontSize: 11 }}>✓</span>
                ) : null}
                {handle ? (
                  <span style={{ color: "var(--text-secondary)", fontWeight: 400, fontSize: 11 }}>{handle}</span>
                ) : null}
              </span>
              {c.unread > 0 ? <StatusBadge tone="primary">{c.unread}</StatusBadge> : null}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-secondary)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {c.lastText}
            </div>
          </button>
        );
      })}
    </div>
  );
}
