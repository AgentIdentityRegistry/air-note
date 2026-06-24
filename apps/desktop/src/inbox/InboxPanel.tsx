import { useEffect, useRef, useState } from "react";
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
  // Keep the message area pinned to the newest message when the thread grows or the
  // conversation changes. `scrollIntoView` is guarded because jsdom does not implement it.
  const bottomRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottomRef.current?.scrollIntoView?.({ block: "end" });
  }, [selected, thread.length]);

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
    <div className="card inbox-root">
      <div className="inbox-header">
        <h2 style={{ margin: 0 }}>Inbox</h2>
        <div style={{ display: "flex", gap: 12, alignItems: "center" }}>
          <ToggleSwitch checked={includeSpam} onChange={setIncludeSpam} label="Show spam" />
          <Button variant="secondary" onClick={() => setComposing(true)}>New Message</Button>
        </div>
      </div>

      {adoption?.state === "adopted" && adoption.dormant_did ? (
        <div className="inbox-banner" style={{ fontSize: 12, color: "var(--text-secondary)" }}>
          This app previously created {short(adoption.dormant_did)}; it is now dormant. Active agent: {short(adoption.did)}.
        </div>
      ) : null}

      {!online ? (
        <div className="inbox-banner" style={{ padding: "8px 12px", borderRadius: 8, background: "color-mix(in srgb, var(--error) 10%, var(--surface))", color: "var(--error)", fontSize: 13 }}>
          Your agent is offline — reconnecting. History is read-only.
        </div>
      ) : null}
      {archiveError ? (
        <div className="inbox-banner" style={{ padding: "8px 12px", borderRadius: 8, background: "color-mix(in srgb, var(--warning) 12%, var(--surface))", color: "var(--warning)", fontSize: 13 }}>
          Couldn&apos;t read the local archive — showing the live feed only.
        </div>
      ) : null}

      <div className="inbox-grid">
        <div className="inbox-list-col">
          <ConversationList conversations={conversations} selected={showConv ? selected : null} onSelect={(k) => { setComposing(false); select(k); }} />
        </div>
        <div className="inbox-thread-col">
          {showPane ? (
            <>
              {showConv && selected ? (
                <div className="inbox-thread-head">
                  <span style={{ fontSize: 13, fontWeight: 600 }}>{isRoom ? "👥 " : ""}{short(selected)}</span>
                  {!isRoom ? <DialControl did={selected} /> : null}
                </div>
              ) : <div className="inbox-thread-head"><span style={{ fontSize: 13, fontWeight: 600 }}>New Message</span></div>}
              <div className="inbox-thread-scroll">
                {showConv ? <MessageThread items={thread} onRetry={onRetry} /> : null}
                {showConv ? <AIPanel selectedPeer={selected ?? null} /> : null}
                <div ref={bottomRef} className="inbox-thread-anchor" />
              </div>
              <Composer key={showNew ? "new" : selected} to={showNew ? null : selected} disabled={!online} onSend={handleSend} />
            </>
          ) : (
            <div style={{ color: "var(--text-secondary)", fontSize: 13, padding: 12 }}>Select a conversation, or start a new message.</div>
          )}
        </div>
      </div>
    </div>
  );
}
