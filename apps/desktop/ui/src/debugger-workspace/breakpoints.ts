import type { SourceBreakpoint, BreakpointResolution } from "./models.js";

export interface Point {
  artifact?: string;
  file: string;
  breakpoint: SourceBreakpoint;
  enabled: boolean;
  anchor: string;
}
export interface PointGroup {
  key: string;
  label: string;
  open: boolean;
  points: Array<Point & { label: string; line: number; snippet: string; status: string }>;
}
export function breakpointGroups(
  points: Point[],
  bindings: BreakpointResolution[],
  groups: Map<string, boolean>,
): PointGroup[] {
  const files = new Map<string, PointGroup>();
  for (const point of points) {
    const key = `breakpoints/${point.file}`;
    let file = files.get(key);
    if (!file) {
      file = {
        key,
        label: point.file.split("/").slice(1).join("/"),
        open: groups.get(key) ?? true,
        points: [],
      };
      files.set(key, file);
    }
    const bound = bindings.some(
      (binding) => binding.id === point.breakpoint.id && binding.resolved,
    );
    file.points.push({
      ...point,
      label: `${file.label}:${point.breakpoint.location.line}`,
      line: point.breakpoint.location.line,
      snippet: point.anchor.trim().slice(0, 160),
      status: !point.enabled ? "Disabled" : bound ? "Bound" : "Pending",
    });
  }
  for (const file of files.values()) {
    file.points.sort((a, b) => a.line - b.line || a.breakpoint.id - b.breakpoint.id);
  }
  return [...files.values()].sort((a, b) => a.label.localeCompare(b.label));
}
