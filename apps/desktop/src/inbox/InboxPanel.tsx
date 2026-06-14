import { useState } from "react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { ToggleSwitch } from "../components/ui/ToggleSwitch";
import { useInbox } from "../state/inbox";
import { ConversationList } from "./ConversationList";
import { MessageThread } from "./MessageThread";
import { Composer } from "./Composer";
import { DialControl } from "./DialControl";
import { NeedsDaemon } from "./NeedsDaemon";
import { AIPanel } from "./AIPanel";
import type { ThreadItem } from "./model";

const short = (did: string) => did.replace(/^did:wba:[^:]+:agents:/, "");

export function InboxPanel() {
  const { gate, adoption, online, archiveError, conversations, selected, thread, includeSpam, select, setIncludeSpam, send } = useInbox();
  const [composing, setComposing] = useState(false);

  if (gate === "loading") return <Card>Loading…</Card>;
  if (gate === "needs_daemon") return <NeedsDaemon />;

  const selectedConv = conversations.find((c) => c.convKey === selected);
  const isRoom = selectedConv?.kind === "room";
  const showNew = composing;                    // explicit compose mode (overrides selection)
  const showConv = !composing && !!selected;    // viewing an existing conversation
  const showPane = showNew || showConv;

  const onRetry = (it: ThreadItem) => {
    const b = it.body as { type?: string; text?: string } | null;
    if (selected && b?.type === "text" && typeof b.text === "string") send(selected, b.text);
  };
  // On send, leave compose mode and select the recipient so the optimistic row shows immediately.
  const handleSend = (to: string, text: string) => { send(to, text); setComposing(false); select(to); };

  return (
    <Card>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
        <h2 style={{ margin: 0 }}>Inbox</h2>
        <div style={{ display: "flex", gap: 12, alignItems: "center" }}>
          <ToggleSwitch checked={includeSpam} onChange={setIncludeSpam} label="Show spam" />
          <Button variant="secondary" onClick={() => setComposing(true)}>New message</Button>
        </div>
      </div>

      {adoption?.state === "adopted" && adoption.dormant_did ? (
        <div style={{ marginTop: 8, fontSize: 12, color: "#666" }}>
          This app previously created {short(adoption.dormant_did)}; it is now dormant. Active agent: {short(adoption.did)}.
        </div>
      ) : null}

      {!online ? (
        <div style={{ marginTop: 12, padding: "8px 12px", borderRadius: 8, background: "#FFF3F3", color: "#A75D61", fontSize: 13 }}>
          daemon offline — reconnecting. History is read-only.
        </div>
      ) : null}
      {archiveError ? (
        <div style={{ marginTop: 8, padding: "8px 12px", borderRadius: 8, background: "#FFF8EC", color: "#A57C42", fontSize: 13 }}>
          Couldn&apos;t read the local archive — showing the live feed only.
        </div>
      ) : null}

      <div style={{ display: "grid", gridTemplateColumns: "220px 1fr", gap: 16, marginTop: 16 }}>
        <ConversationList conversations={conversations} selected={showConv ? selected : null} onSelect={(k) => { setComposing(false); select(k); }} />
        <div>
          {showPane ? (
            <>
              {showConv && selected ? (
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 8 }}>
                  <span style={{ fontSize: 13, fontWeight: 600 }}>{isRoom ? "👥 " : ""}{short(selected)}</span>
                  {!isRoom ? <DialControl did={selected} /> : null}
                </div>
              ) : <div style={{ fontSize: 13, fontWeight: 600, marginBottom: 8 }}>New message</div>}
              {showConv ? <MessageThread items={thread} onRetry={onRetry} /> : null}
              {showConv ? <AIPanel selectedPeer={selected ?? null} /> : null}
              <Composer key={showNew ? "new" : selected} to={showNew ? null : selected} disabled={!online} onSend={handleSend} />
            </>
          ) : (
            <div style={{ color: "#666", fontSize: 13, padding: 12 }}>Select a conversation, or start a new message.</div>
          )}
        </div>
      </div>
    </Card>
  );
}
