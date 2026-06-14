/** Render a message body to display text. Ports agent-bridge-mcp `bodyText` (cli.mjs:81-86) verbatim,
 *  plus the encrypted/absent case the GUI must show (body is omitted on the wire when undecryptable). */
export function bodyText(body: unknown): string {
  if (body == null) return "🔒 (encrypted)";
  if (typeof body !== "object") return String(body);
  const b = body as Record<string, unknown>;
  if (b.type === "text") return typeof b.text === "string" ? b.text : "";
  if (b.type === "room/msg") return typeof b.text === "string" ? b.text : "";
  if (b.type === "room/joined") return `📥 You were added to room "${(b.room_name as string) ?? ""}"`;
  if (b.type === "encrypted") return "🔒 (encrypted)";
  return JSON.stringify(body);
}
