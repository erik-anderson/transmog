import type { DebugValue } from "./models.js";
export const consoleLevels = [
  { id: "info", label: "Info" },
  { id: "warn", label: "Warnings" },
  { id: "error", label: "Errors" },
  { id: "verbose", label: "Verbose" },
];
export function restoreLevels(levels?: string[], legacy?: string): string[] {
  if (levels) {
    return consoleLevels.filter((item) => levels.includes(item.id)).map((item) => item.id);
  }
  const group = levelGroup(legacy ?? "");
  return consoleLevels.some((item) => item.id === group) ? [group] : ["info", "warn", "error"];
}
export function levelGroup(level: string): string {
  return level === "debug" ? "verbose" : level === "trace" ? "info" : level;
}
export function levelsLabel(levels: string[]): string {
  if (levels.length === 4) {
    return "All levels";
  }
  if (!levels.length) {
    return "No levels";
  }
  if (levels.join(",") === "info,warn,error") {
    return "Default levels";
  }
  return consoleLevels
    .filter((item) => levels.includes(item.id))
    .map((item) => item.label)
    .join(" + ");
}

export function resultText(value: DebugValue): string {
  const display = value.kind === "string" ? JSON.stringify(value.display) : value.display;
  return display + (value.truncated ? "\n[Shortened]" : "");
}
