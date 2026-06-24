import { useState } from "react";
import { Input } from "../components/Input";
import { Button } from "../components/Button";

/** When `to` is null the composer shows a recipient (raw DID) field so a NEW conversation can start.
 *  When `to` is set (a conversation is open) it sends to that peer. */
export function Composer({ to, disabled, onSend }: {
  to: string | null; disabled: boolean; onSend: (to: string, text: string) => void;
}) {
  const [recipient, setRecipient] = useState("");
  const [text, setText] = useState("");
  const target = to ?? recipient.trim();
  const canSend = !disabled && !!target && !!text.trim();

  const submit = () => {
    if (!canSend) return;
    onSend(target, text.trim());
    setText("");
    if (!to) setRecipient("");
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 12 }}>
      {!to ? (
        <Input value={recipient} placeholder="Recipient DID (did:wba:…)" disabled={disabled}
          onChange={(e) => setRecipient(e.target.value)} />
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
