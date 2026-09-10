export type Theme = "system" | "light" | "dark";
export type ColorScheme = "jade" | "indigo" | "plum";
const preference = window.matchMedia("(prefers-color-scheme: dark)");
let current: Theme = "system";
let currentScheme: ColorScheme = "indigo";
export function applyTheme(
  theme: Theme = "system",
  scheme: ColorScheme = "indigo",
) {
  theme = theme === "light" || theme === "dark" ? theme : "system";
  scheme = scheme === "jade" || scheme === "plum" ? scheme : "indigo";
  current = theme;
  currentScheme = scheme;
  const resolved =
    theme === "system" ? (preference.matches ? "dark" : "light") : theme;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.dataset.scheme = scheme;
  document.documentElement.style.colorScheme = resolved;
}
preference.addEventListener("change", () => applyTheme(current, currentScheme));
applyTheme();
