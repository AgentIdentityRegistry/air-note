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
  /** The published unique @handle, once claimed; null until then. */
  username: string | null;
  created_at: string;
};

/** Result of `check_username` — mirrors the Rust `UsernameCheck`. The registry always answers:
 *  a valid handle carries `available` + `reason` ("available"|"taken"|"cooldown"); an invalid
 *  one carries `error` (and `valid=false`). */
export type UsernameCheck = {
  username: string;
  valid: boolean;
  available: boolean;
  reason: string | null;
  error: string | null;
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

/** Check whether a published @handle is available (un-normalized raw input; the registry
 *  normalizes + decides). Unauthenticated — does NOT claim the handle. */
export async function checkUsername(username: string): Promise<UsernameCheck> {
  return invoke<UsernameCheck>("check_username", { username });
}

/** Claim a published unique @handle for this agent. On success returns the updated metadata
 *  (with `username` set). Throws a bare error string on 409 (taken / in 30-day cooldown). */
export async function claimUsername(username: string): Promise<IdentityMetadata> {
  return invoke<IdentityMetadata>("claim_username", { username });
}

export async function resetIdentity(): Promise<void> {
  return invoke<void>("reset_identity");
}
