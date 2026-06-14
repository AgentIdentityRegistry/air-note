export type BadgeTone = "neutral" | "success" | "warning" | "error";
export type Badge = { label: string; tone: BadgeTone };
export type BadgeInput = { encrypted: boolean; verified: boolean; key_changed?: boolean; spam?: boolean };

/** Badge vocabulary mirrors the CLI (🔒 ✓) plus the GUI's key-changed/spam flags (design §6). */
export function badgesFor(m: BadgeInput): Badge[] {
  const out: Badge[] = [];
  if (m.encrypted) out.push({ label: "🔒", tone: "neutral" });
  out.push(m.verified ? { label: "✓", tone: "success" } : { label: "unverified", tone: "warning" });
  if (m.key_changed) out.push({ label: "⚠ key changed", tone: "error" });
  if (m.spam) out.push({ label: "spam", tone: "warning" });
  return out;
}
