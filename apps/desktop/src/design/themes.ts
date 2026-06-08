export type DesktopThemeKey = "instrument_light" | "minimal_dark" | "jrpg";

export type DesktopThemeDefinition = {
  key: DesktopThemeKey;
  label: string;
  description: string;
  bubbleStyle: "rounded" | "pixel";
  avatarStyle: "circle" | "pixel";
};

export const DESKTOP_THEMES: DesktopThemeDefinition[] = [
  {
    key: "instrument_light",
    label: "Instrument",
    description: "Light, calm, and focused.",
    bubbleStyle: "rounded",
    avatarStyle: "circle"
  },
  {
    key: "minimal_dark",
    label: "Minimal Dark",
    description: "Low-glare, high-contrast workspace.",
    bubbleStyle: "rounded",
    avatarStyle: "circle"
  },
  {
    key: "jrpg",
    label: "JRPG",
    description: "Playful retro-inspired skin.",
    bubbleStyle: "pixel",
    avatarStyle: "pixel"
  }
];

export function isDesktopThemeKey(value: unknown): value is DesktopThemeKey {
  return value === "instrument_light" || value === "minimal_dark" || value === "jrpg";
}

