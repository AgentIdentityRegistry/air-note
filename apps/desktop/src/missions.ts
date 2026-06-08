import type { Mission, MissionSchedule } from "./models";

export const WEEKDAY_OPTIONS: Array<{ value: number; label: string }> = [
  { value: 0, label: "Sunday" },
  { value: 1, label: "Monday" },
  { value: 2, label: "Tuesday" },
  { value: 3, label: "Wednesday" },
  { value: 4, label: "Thursday" },
  { value: 5, label: "Friday" },
  { value: 6, label: "Saturday" }
];

const DEFAULT_TIME = "09:00";

function pad2(value: number): string {
  return String(value).padStart(2, "0");
}

export function normalizeMissionTime(value: string | null | undefined): string {
  if (typeof value !== "string") {
    return DEFAULT_TIME;
  }
  const match = value.trim().match(/^([01]?\d|2[0-3]):([0-5]\d)$/);
  if (!match) {
    return DEFAULT_TIME;
  }
  return `${pad2(Number(match[1]))}:${pad2(Number(match[2]))}`;
}

export function normalizeMissionWeekday(value: number | null | undefined): number {
  if (typeof value !== "number" || !Number.isInteger(value)) {
    return 1;
  }
  if (value < 0 || value > 6) {
    return 1;
  }
  return value;
}

export function normalizeMissionInterval(value: number | null | undefined): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return 15;
  }
  const rounded = Math.trunc(value);
  if (rounded < 1) {
    return 1;
  }
  return Math.min(rounded, 1_440);
}

export function toCronExpression(schedule: {
  kind: MissionSchedule["kind"];
  time?: string;
  weekday?: number;
  intervalMinutes?: number;
  cron?: string;
}): string {
  const explicitCron = schedule.cron?.trim();
  if (explicitCron) {
    return explicitCron;
  }

  if (schedule.kind === "every_minutes") {
    return `*/${normalizeMissionInterval(schedule.intervalMinutes)} * * * *`;
  }

  if (schedule.kind === "custom") {
    return "0 9 * * *";
  }

  const [hour, minute] = normalizeMissionTime(schedule.time).split(":").map((value) => Number(value));
  if (schedule.kind === "weekdays") {
    return `${minute} ${hour} * * 1-5`;
  }
  if (schedule.kind === "weekly") {
    return `${minute} ${hour} * * ${normalizeMissionWeekday(schedule.weekday)}`;
  }

  return `${minute} ${hour} * * *`;
}

export function buildMissionSchedule(input: {
  kind: MissionSchedule["kind"];
  time?: string;
  weekday?: number;
  intervalMinutes?: number;
  cron?: string;
}): MissionSchedule {
  if (input.kind === "every_minutes") {
    const intervalMinutes = normalizeMissionInterval(input.intervalMinutes);
    return {
      kind: "every_minutes",
      intervalMinutes,
      cron: toCronExpression({ kind: "every_minutes", intervalMinutes, cron: input.cron })
    };
  }

  if (input.kind === "weekly") {
    const time = normalizeMissionTime(input.time);
    const weekday = normalizeMissionWeekday(input.weekday);
    return {
      kind: "weekly",
      time,
      weekday,
      cron: toCronExpression({ kind: "weekly", time, weekday, cron: input.cron })
    };
  }

  if (input.kind === "weekdays") {
    const time = normalizeMissionTime(input.time);
    return {
      kind: "weekdays",
      time,
      cron: toCronExpression({ kind: "weekdays", time, cron: input.cron })
    };
  }

  if (input.kind === "custom") {
    return {
      kind: "custom",
      cron: toCronExpression({ kind: "custom", cron: input.cron })
    };
  }

  const time = normalizeMissionTime(input.time);
  return {
    kind: "daily",
    time,
    cron: toCronExpression({ kind: "daily", time, cron: input.cron })
  };
}

function nextDailyFrom(time: string, from: Date): Date {
  const [hour, minute] = normalizeMissionTime(time).split(":").map((value) => Number(value));
  const candidate = new Date(from);
  candidate.setSeconds(0, 0);
  candidate.setHours(hour, minute, 0, 0);
  if (candidate <= from) {
    candidate.setDate(candidate.getDate() + 1);
  }
  return candidate;
}

function nextWeekdaysFrom(time: string, from: Date): Date {
  const candidate = nextDailyFrom(time, from);
  while (candidate.getDay() === 0 || candidate.getDay() === 6) {
    candidate.setDate(candidate.getDate() + 1);
  }
  return candidate;
}

function nextWeeklyFrom(time: string, weekday: number, from: Date): Date {
  const [hour, minute] = normalizeMissionTime(time).split(":").map((value) => Number(value));
  const targetWeekday = normalizeMissionWeekday(weekday);
  const candidate = new Date(from);
  candidate.setSeconds(0, 0);
  candidate.setHours(hour, minute, 0, 0);

  const dayDiff = (targetWeekday - candidate.getDay() + 7) % 7;
  candidate.setDate(candidate.getDate() + dayDiff);
  if (candidate <= from) {
    candidate.setDate(candidate.getDate() + 7);
  }

  return candidate;
}

function nextEveryMinutesFrom(intervalMinutes: number, from: Date): Date {
  const interval = normalizeMissionInterval(intervalMinutes);
  const candidate = new Date(from);
  candidate.setSeconds(0, 0);

  const minute = candidate.getMinutes();
  const remainder = minute % interval;
  const addMinutes = remainder === 0 ? interval : interval - remainder;
  candidate.setMinutes(minute + addMinutes);

  if (candidate <= from) {
    candidate.setMinutes(candidate.getMinutes() + interval);
  }

  return candidate;
}

export function computeNextRunAt(schedule: MissionSchedule, from: Date = new Date()): string {
  let nextDate: Date;
  if (schedule.kind === "every_minutes") {
    nextDate = nextEveryMinutesFrom(schedule.intervalMinutes ?? 15, from);
  } else if (schedule.kind === "weekdays") {
    nextDate = nextWeekdaysFrom(schedule.time ?? DEFAULT_TIME, from);
  } else if (schedule.kind === "weekly") {
    nextDate = nextWeeklyFrom(schedule.time ?? DEFAULT_TIME, schedule.weekday ?? 1, from);
  } else if (schedule.kind === "custom") {
    nextDate = nextEveryMinutesFrom(60, from);
  } else {
    nextDate = nextDailyFrom(schedule.time ?? DEFAULT_TIME, from);
  }

  return nextDate.toISOString();
}

export function formatMissionSchedule(schedule: MissionSchedule): string {
  if (schedule.kind === "every_minutes") {
    const interval = normalizeMissionInterval(schedule.intervalMinutes);
    return interval === 1 ? "Every minute" : `Every ${interval} minutes`;
  }

  if (schedule.kind === "weekdays") {
    return `Weekdays at ${normalizeMissionTime(schedule.time)}`;
  }

  if (schedule.kind === "weekly") {
    const weekday = WEEKDAY_OPTIONS.find((option) => option.value === normalizeMissionWeekday(schedule.weekday));
    return `Weekly on ${weekday?.label ?? "Monday"} at ${normalizeMissionTime(schedule.time)}`;
  }

  if (schedule.kind === "custom") {
    return "Custom schedule";
  }

  return `Daily at ${normalizeMissionTime(schedule.time)}`;
}

export function formatMissionNextRun(nextRunAt: string): string {
  const parsed = new Date(nextRunAt);
  if (Number.isNaN(parsed.getTime())) {
    return "Not scheduled";
  }
  return parsed.toLocaleString();
}

export function isMissionDue(mission: Mission, now: Date = new Date()): boolean {
  if (!mission.enabled || mission.archived) {
    return false;
  }
  const nextRun = new Date(mission.nextRunAt);
  if (Number.isNaN(nextRun.getTime())) {
    return false;
  }
  return nextRun.getTime() <= now.getTime();
}

export function needsMissionNextRunRepair(mission: Mission): boolean {
  if (!mission.enabled || mission.archived) {
    return false;
  }
  const nextRun = new Date(mission.nextRunAt);
  return Number.isNaN(nextRun.getTime());
}
