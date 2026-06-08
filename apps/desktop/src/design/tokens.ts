export const spacing = {
  4: 4,
  8: 8,
  12: 12,
  16: 16,
  24: 24,
  32: 32,
  48: 48
} as const;

export const radius = {
  sm: 8,
  md: 10,
  lg: 12
} as const;

export const elevation = {
  level1: "0 1px 2px rgba(11, 15, 23, 0.05)",
  level2: "0 8px 20px rgba(11, 15, 23, 0.12)",
  level3: "0 10px 24px rgba(11, 15, 23, 0.14)"
} as const;

export const colors = {
  background: "#F3F4F6",
  surface: "#FFFFFF",
  surfaceSoft: "#EEF0F3",
  borderSoft: "rgba(17,24,39,0.06)",
  primary: "#2F6BFF",
  accent: "#2F6BFF",
  success: "#4D8F6F",
  warning: "#A57C42",
  error: "#A75D61",
  textPrimary: "#0B0F17",
  textSecondary: "rgba(11,15,23,0.62)",
  textTertiary: "rgba(11,15,23,0.42)",
  backgroundDark: "#0D1016",
  surfaceDark: "#131824",
  surfaceSoftDark: "#171E2C",
  borderSoftDark: "rgba(255,255,255,0.07)",
  accentDark: "#4B82FF",
  textPrimaryDark: "rgba(255,255,255,0.92)",
  textSecondaryDark: "rgba(255,255,255,0.62)",
  textTertiaryDark: "rgba(255,255,255,0.42)"
} as const;

export const motion = {
  fast: "150ms",
  normal: "220ms",
  slow: "320ms",
  easing: "cubic-bezier(0.4, 0, 0.2, 1)"
} as const;

export const typography = {
  title: 16,
  section: 13,
  body: 12,
  label: 10,
  lineHeightTitle: 1.18,
  lineHeightBody: 1.26
} as const;
