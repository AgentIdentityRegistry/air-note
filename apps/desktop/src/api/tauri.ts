import { invoke } from "@tauri-apps/api/core";

// ── A2A types ─────────────────────────────────────────────────────────────────

export type A2AValue =
  | { type: "cash"; amount_cents: number; currency: string }
  | { type: "item"; item_id: string; quantity: number };

export type A2AMessageBody =
  | { type: "offer"; item_id: string; offered_value: A2AValue; note?: string }
  | { type: "counter"; item_id: string; counter_value: A2AValue; note?: string }
  | { type: "accept"; item_id: string; note?: string }
  | { type: "decline"; item_id: string; reason?: string }
  | { type: "withdraw"; item_id: string; reason?: string };

export type Envelope = {
  id: string;
  from: string;
  to: string;
  timestamp: string;
  in_reply_to: string | null;
  thread_id: string;
  nonce: string;
  body: A2AMessageBody;
  signature: string | null;
};

export type A2ADemoResult = {
  envelope: Envelope;
  verified: boolean;
};

/** Generate a fresh keypair, sign a sample Offer envelope, verify it, and
 *  return the signed envelope plus a `verified: true` flag. Useful for
 *  smoke-testing the a2a-rs integration end-to-end from the frontend. */
export const a2aDemoRoundTrip = (): Promise<A2ADemoResult> =>
  invoke<A2ADemoResult>("a2a_demo_round_trip");

// ── Identity types ─────────────────────────────────────────────────────────────

export type IdentityMetadata = {
  did: string;
  name: string;
  created_at: string;
};

export async function isOnboarded(): Promise<boolean> {
  return invoke<boolean>("is_onboarded");
}

export async function getIdentity(): Promise<IdentityMetadata | null> {
  return invoke<IdentityMetadata | null>("get_identity");
}

export async function getTrustScore(): Promise<number | null> {
  return invoke<number | null>("get_trust_score");
}

export async function createIdentity(
  name: string,
  domain: string,
): Promise<IdentityMetadata> {
  return invoke<IdentityMetadata>("create_identity", { name, domain });
}

/** Rename the LOCAL display name only (freely editable; shown in the UI/chat). Does NOT touch
 *  the DID, keypair, or created_at, and is NOT the published unique @handle. Tauri v2 maps the
 *  JS `newName` key to the Rust command's `new_name` argument. */
export async function renameIdentity(name: string): Promise<IdentityMetadata> {
  return invoke<IdentityMetadata>("rename_identity", { newName: name });
}

export async function resetIdentity(): Promise<void> {
  return invoke<void>("reset_identity");
}
