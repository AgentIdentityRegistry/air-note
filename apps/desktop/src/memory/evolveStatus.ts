import type { EvolveStatusDto } from "../api/engine";

/** One-line summary of the evolve loop's state (null tick → em dash). */
export const formatEvolve = (s: EvolveStatusDto): string =>
  `${s.enabled ? "On" : "Off"} · ${s.queue_depth} queued · ` +
  `last tick ${s.last_tick_ms == null ? "—" : `${s.last_tick_ms}ms`} · ${s.error_count} errors`;
