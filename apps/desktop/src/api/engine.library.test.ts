import { describe, it, expect, vi, beforeEach } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));

import {
  listSessions,
  getSession,
  deleteSession,
  listNotes,
  supersedeNote,
  recallStats,
  type SessionSummaryDto,
  type SessionDetailDto,
  type NoteDto,
  type RecallStatsDto,
} from "./engine";

describe("library bindings", () => {
  beforeEach(() => invoke.mockReset());

  it("listSessions invokes engine_list_sessions and returns typed rows", async () => {
    const rows: SessionSummaryDto[] = [
      {
        session_id: "s1",
        title: "First",
        project: "air-note",
        tool: "claude-code",
        started_at: 100,
        ended_at: 200,
        approx_bytes: 4096,
      },
    ];
    invoke.mockResolvedValue(rows);
    const out = await listSessions();
    expect(invoke).toHaveBeenCalledWith("engine_list_sessions");
    expect(out).toEqual(rows);
    // Numeric wire fields stay numbers (not stringified) so the UI can format bytes/dates.
    expect(typeof out[0].approx_bytes).toBe("number");
    expect(typeof out[0].started_at).toBe("number");
    expect(typeof out[0].ended_at).toBe("number");
  });

  it("getSession invokes engine_get_session with the session id", async () => {
    const detail: SessionDetailDto = {
      summary: {
        session_id: "s1",
        title: "First",
        project: "air-note",
        tool: "claude-code",
        started_at: 100,
        ended_at: 200,
        approx_bytes: 4096,
      },
      markdown: "# hello",
    };
    invoke.mockResolvedValue(detail);
    const out = await getSession("s1");
    expect(invoke).toHaveBeenCalledWith("engine_get_session", { sessionId: "s1" });
    expect(out).toEqual(detail);
  });

  it("getSession surfaces the not-found rejection distinguishably", async () => {
    // mockImplementationOnce (not mockRejectedValue) creates the rejected promise lazily on call,
    // so the await below is its only consumer — no eager dangling promise for vitest to flag.
    invoke.mockImplementationOnce(async () => {
      throw new Error("session not found or deleted");
    });
    // The wrapper must not swallow the reject — it propagates the daemon's bare message so the
    // UI can catch it and show "already deleted" instead of a generic fault.
    let caught: unknown;
    try {
      await getSession("gone");
    } catch (e) {
      caught = e;
    }
    expect((caught as Error).message).toBe("session not found or deleted");
  });

  it("deleteSession invokes engine_delete_session with the session id", async () => {
    invoke.mockResolvedValue(undefined);
    await deleteSession("s1");
    expect(invoke).toHaveBeenCalledWith("engine_delete_session", { sessionId: "s1" });
  });

  it("listNotes returns typed notes with superseded_by nullable", async () => {
    const notes: NoteDto[] = [
      { event_id: "e1", text: "live note", created_at: 10, superseded_by: null },
      { event_id: "e2", text: "old note", created_at: 5, superseded_by: "e3" },
    ];
    invoke.mockResolvedValue(notes);
    const out = await listNotes();
    expect(invoke).toHaveBeenCalledWith("engine_list_notes");
    expect(out).toEqual(notes);
    expect(out[0].superseded_by).toBeNull();
    expect(out[1].superseded_by).toBe("e3");
  });

  it("supersedeNote invokes engine_supersede_note and returns the new event id", async () => {
    invoke.mockResolvedValue("e-new");
    const newId = await supersedeNote("e1", "edited text");
    expect(invoke).toHaveBeenCalledWith("engine_supersede_note", { eventId: "e1", text: "edited text" });
    expect(newId).toBe("e-new");
  });

  it("recallStats returns totals and recent_misses array", async () => {
    const stats: RecallStatsDto = {
      total: 42,
      misses: 3,
      recent_misses: [
        { query: "q1", at: 10 },
        { query: "q2", at: 20 },
      ],
    };
    invoke.mockResolvedValue(stats);
    const out = await recallStats();
    expect(invoke).toHaveBeenCalledWith("engine_recall_stats");
    expect(out).toEqual(stats);
    expect(out.recent_misses).toHaveLength(2);
    expect(out.recent_misses[0]).toEqual({ query: "q1", at: 10 });
  });
});
