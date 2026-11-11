/** Keeps the document theme in step with the saved preference. */
export function prefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function applyTheme(
  theme: string,
  systemTheme: "dark" | "light" | "unknown" = "unknown",
): void {
  const dark =
    theme === "dark" ||
    (theme === "system" &&
      (systemTheme === "dark" ||
        (systemTheme === "unknown" && prefersDark())));
  document.documentElement.dataset.theme = dark ? "dark" : "light";
}

/** The GNOME accent colour, when the bootstrap command found one. */
export function applyAccent(accent: string | null): void {
  if (accent) {
    document.documentElement.style.setProperty("--accent", accent);
  }
}
