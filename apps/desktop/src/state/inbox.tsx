import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  inboxStart, inboxStop, inboxSend, inboxHistory, inboxConversations, inboxContacts, inboxStatus, inboxIdentity,
  onInboxEvent, type InboxMessage, type Adoption, type ConversationSummary, type ContactView,
} from "../api/inbox";
import { getIdentity } from "../api/tauri";
import {
  fromArchiveRow, fromLiveMessage, makeOptimistic, convKey, dedupeById, groupConversations,
  type ThreadItem, type Conversation,
} from "../inbox/model";
import { contactsByDid } from "../inbox/displayName";
import { onSendStart, onSendOk, onSendErr, type SendState } from "../inbox/sendState";
import { addUnread, clearConv } from "../inbox/unread";
import { mergeSidebar } from "../inbox/sidebar";

type InboxCtx = {
  gate: "loading" | "needs_daemon" | "ready";
  adoption: Adoption | null;
  online: boolean;
  archiveError: boolean;
  conversations: Conversation[];
  contacts: Map<string, ContactView>;
  selected: string | null;
  thread: ThreadItem[];
  includeSpam: boolean;
  totalUnread: number;
  select: (convKey: string) => void;
  setIncludeSpam: (v: boolean) => void;
  send: (to: string, text: string) => Promise<void>;
};

const Ctx = createContext<InboxCtx | null>(null);
const BULK_LIMIT = 200;

export function InboxProvider({ children }: { children: ReactNode }) {
  const [gate, setGate] = useState<"loading" | "needs_daemon" | "ready">("loading");
  const [adoption, setAdoption] = useState<Adoption | null>(null);
  const [online, setOnline] = useState(false);
  const [archiveError, setArchiveError] = useState(false);
  const [summaries, setSummaries] = useState<ConversationSummary[]>([]);
  const [contacts, setContacts] = useState<Map<string, ContactView>>(new Map());
  const [recent, setRecent] = useState<ThreadItem[]>([]);       // bulk cross-peer backfill (previews)
  const [threadRows, setThreadRows] = useState<ThreadItem[]>([]); // deep history for the open conv
  const [live, setLive] = useState<ThreadItem[]>([]);
  const [optimistic, setOptimistic] = useState<ThreadItem[]>([]);
  const [sendState, setSendState] = useState<SendState>({});
  const [selected, setSelected] = useState<string | null>(null);
  const [includeSpam, setIncludeSpam] = useState(false);
  const [unread, setUnread] = useState<Set<string>>(new Set());
  const selectedRef = useRef<string | null>(null);
  selectedRef.current = selected;

  // Probe daemon presence + adopted identity (design §4 gate). Forward the desktop's prior self-created
  // DID (if onboarding ran here before) so the adoption can name the now-dormant agent (I1).
  useEffect(() => {
    let alive = true;
    (async () => {
      try {
        const prior = await getIdentity().catch(() => null);
        const [status, adopt] = await Promise.all([inboxStatus(), inboxIdentity(prior?.did)]);
        if (!alive) return;
        setAdoption(adopt);
        setGate(adopt.state === "needs_daemon" || !status.identity_exists ? "needs_daemon" : "ready");
      } catch {
        if (alive) setGate("needs_daemon");
      }
    })();
    return () => { alive = false; };
  }, []);

  // Connect + subscribe once we're ready. Listener teardown is race-safe (M2): if the effect is torn
  // down mid-await, unlisten the just-registered handler instead of leaking it.
  useEffect(() => {
    if (gate !== "ready") return;
    let cancelled = false;
    const unlisten: Array<() => void> = [];
    const reg = async (p: Promise<() => void>) => {
      const off = await p;
      if (cancelled) { off(); return; } // torn down mid-await → unlisten instead of leaking (M2)
      unlisten.push(off);
    };
    (async () => {
      await reg(onInboxEvent("inbox_attached", () => setOnline(true)));
      await reg(onInboxEvent("inbox_offline", () => setOnline(false)));
      await reg(onInboxEvent("inbox_detached", () => setOnline(false)));
      await reg(onInboxEvent("inbox_message", (m: InboxMessage) => {
        const it = fromLiveMessage(m);
        setLive((prev) => [...prev, it]);
        if (convKey(it) !== selectedRef.current) setUnread((u) => addUnread(u, it.envelope_id));
      }));
      await reg(onInboxEvent("inbox_send_ok", (a) => setSendState((s) => onSendOk(s, a))));
      await reg(onInboxEvent("inbox_send_err", (a) => setSendState((s) => onSendErr(s, a))));
      if (!cancelled) await inboxStart();
    })();
    return () => { cancelled = true; unlisten.forEach((fn) => fn()); inboxStop().catch(() => {}); };
  }, [gate]);

  // C1: seed the sidebar from the archive (complete list + recent previews). Re-runs on spam toggle.
  useEffect(() => {
    if (gate !== "ready") return;
    let alive = true;
    inboxConversations().then((s) => { if (alive) setSummaries(s); }).catch(() => {});
    inboxHistory(undefined, undefined, BULK_LIMIT, includeSpam)
      .then((rows) => { if (alive) { setRecent(rows.map(fromArchiveRow)); setArchiveError(false); } })
      .catch(() => { if (alive) setArchiveError(true); });
    return () => { alive = false; };
  }, [gate, includeSpam]);

  // Load the contact book once ready, for did→display-name resolution (Milestone C).
  useEffect(() => {
    if (gate !== "ready") return;
    let alive = true;
    inboxContacts().then((cs) => { if (alive) setContacts(contactsByDid(cs)); }).catch(() => {});
    return () => { alive = false; };
  }, [gate]);

  // Is the selected conversation a room? Derived so the deep-load effect can depend on a stable
  // boolean (not the whole summaries/recent/live arrays) and re-fire only on real changes (m1/m2).
  const selectedIsRoom = useMemo(
    () => summaries.find((s) => s.conv_key === selected)?.kind === "room"
      || recent.some((r) => r.room_id === selected)
      || live.some((r) => r.room_id === selected), // m2: a room first seen live this session
    [selected, summaries, recent, live],
  );

  // M1: deep-load the selected conversation (peer vs room).
  useEffect(() => {
    if (!selected) { setThreadRows([]); return; }
    let alive = true;
    const p = selectedIsRoom
      ? inboxHistory(undefined, selected, BULK_LIMIT, includeSpam)
      : inboxHistory(selected, undefined, BULK_LIMIT, includeSpam);
    p.then((rows) => { if (alive) { setThreadRows(rows.map(fromArchiveRow)); setArchiveError(false); } })
     .catch(() => { if (alive) setArchiveError(true); });
    return () => { alive = false; };
  }, [selected, selectedIsRoom, includeSpam]);

  const resolvedOptimistic = useMemo(
    () => optimistic.map((o) => {
      const st = o.correlationId ? sendState[o.correlationId] : undefined;
      if (!st) return o;
      if (st.status === "ok") return { ...o, status: "ok" as const, envelope_id: st.envelope_id };
      if (st.status === "err") return { ...o, status: "err" as const, retryable: st.retryable, reason: st.reason };
      return o;
    }),
    [optimistic, sendState],
  );

  // confirmed rows (threadRows, recent) BEFORE optimistic so dedupe keeps the confirmed copy.
  const all = useMemo(
    () => dedupeById([...threadRows, ...recent, ...live, ...resolvedOptimistic]),
    [threadRows, recent, live, resolvedOptimistic],
  );
  const grouped = useMemo(() => groupConversations(all, unread), [all, unread]);
  const conversations = useMemo(() => mergeSidebar(summaries, grouped), [summaries, grouped]);
  const thread = useMemo(
    () => all.filter((it) => convKey(it) === selected).sort((a, b) => (a.timestamp < b.timestamp ? -1 : 1)),
    [all, selected],
  );
  const totalUnread = useMemo(() => conversations.reduce((n, c) => n + c.unread, 0), [conversations]);

  const select = (key: string) => {
    setSelected(key);
    setUnread((u) => clearConv(u, all, key));
  };

  const send = async (to: string, text: string) => {
    const body = { type: "text", text };
    const id = await inboxSend(to, body);
    setSendState((s) => onSendStart(s, id));
    setOptimistic((prev) => [...prev, makeOptimistic(id, to, body, new Date().toISOString())]);
  };

  return (
    <Ctx.Provider value={{
      gate, adoption, online, archiveError, conversations, contacts, selected, thread, includeSpam, totalUnread,
      select, setIncludeSpam, send,
    }}>
      {children}
    </Ctx.Provider>
  );
}

export function useInbox() {
  const c = useContext(Ctx);
  if (!c) throw new Error("useInbox must be inside InboxProvider");
  return c;
}
