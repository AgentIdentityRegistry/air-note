import { useEffect, useState } from "react";
import { inboxPolicyGet, inboxPolicySet, type Autonomy } from "../api/inbox";

const OPTIONS: Autonomy[] = ["off", "draft", "auto"];

export function DialControl({ did }: { did: string }) {
  const [value, setValue] = useState<Autonomy>("draft");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    inboxPolicyGet(did).then((v) => { if (alive) setValue(v); }).catch(() => {});
    return () => { alive = false; };
  }, [did]);

  const change = async (v: Autonomy) => {
    setBusy(true);
    try { await inboxPolicySet(did, v); setValue(v); } finally { setBusy(false); }
  };

  return (
    <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
      <span style={{ fontSize: 12, color: "#666" }}>AI:</span>
      <div style={{ display: "inline-flex", border: "1px solid #ccc", borderRadius: 6, overflow: "hidden" }}>
        {OPTIONS.map((o) => (
          <button key={o} disabled={busy} onClick={() => change(o)}
            style={{ padding: "4px 10px", fontSize: 12, border: "none", cursor: "pointer",
              background: o === value ? "#2F6BFF" : "white", color: o === value ? "white" : "#0B0F17" }}>
            {o}
          </button>
        ))}
      </div>
    </div>
  );
}
