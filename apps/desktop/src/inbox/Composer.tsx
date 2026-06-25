import { useState } from "react";
import { Input } from "../components/Input";
import { Button } from "../components/Button";
import type { ContactView } from "../api/inbox";
import { displayName, handleOf } from "./displayName";

/** When `to` is null the composer shows a recipient picker (contacts dropdown + free-text DID) so a
 *  NEW conversation can start. When `to` is set (a conversation is open) it sends to that peer. */
export function Composer({ to, contacts, disabled, onSend }: {
  to: string | null;
  contacts: ContactView[];
  disabled: boolean;
  onSend: (to: string, text: string) => void;
}) {
  const [recipient, setRecipient] = useState("");
  const [text, setText] = useState("");
  const target = to ?? recipient.trim();
  const canSend = !disabled && !!target && !!text.trim();
  const known = contacts.some((c) => c.did === recipient);

  const submit = () => {
    if (!canSend) return;
    onSend(target, text.trim());
    setText("");
    if (!to) setRecipient("");
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 12 }}>
      {!to ? (
        <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
          {contacts.length > 0 ? (
            <select
              aria-label="Recipient contact"
              value={known ? recipient : ""}
              disabled={disabled}
              onChange={(e) => setRecipient(e.target.value)}
              style={{ padding: "8px 10px", borderRadius: 8, border: "1px solid var(--border-soft)", background: "var(--surface)", color: "var(--text-primary)", fontSize: 13 }}
            >
              <option value="">Choose a contact…  (or type a DID below)</option>
              {contacts.map((c) => {
                const h = handleOf(c);
                return (
                  <option key={c.did} value={c.did}>
                    {displayName(c.did, c)}{h ? ` (${h})` : ""}
                  </option>
                );
              })}
            </select>
          ) : null}
          <Input value={recipient} aria-label="Recipient DID" placeholder="Recipient DID (did:wba:…)" disabled={disabled}
            onChange={(e) => setRecipient(e.target.value)} />
        </div>
      ) : null}
      <div style={{ display: "flex", gap: 8 }}>
        <Input value={text} placeholder={disabled ? "Agent offline" : "Message…"} disabled={disabled}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); submit(); } }} />
        <Button variant="primary" disabled={!canSend} onClick={submit}>Send</Button>
      </div>
    </div>
  );
}
