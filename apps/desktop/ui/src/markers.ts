export type Marker = {
  id: string;
  runId: string;
  unixMillis: number;
  offsetMillis: number | null;
  origin: string;
  creationKind: string;
  note: string;
};
export type MarkerLink = {
  markerId: string;
  anchorId: string;
  boundary: "request" | "response";
  timeBasis: string;
};
export type MarkerReview = {
  links?: MarkerLink[];
  markerEntries?: Record<string, string>;
  addedMarkers?: number;
  traceId: string;
  name: string;
  revision: number;
  dirty: boolean;
  canUndo: boolean;
  markers: Marker[];
  editedMarkerIds: string[];
  editedNotes: number;
  deletedMarkers: number;
};
export type TraceMarker = Marker & {
  entryId: string | null;
  boundary: string;
  key: string;
  traceId: string;
  source: string;
  timeLabel: string;
  description: string;
};
export function markerTime(marker: Marker): string {
  if (marker.offsetMillis != null) {
    const seconds = marker.offsetMillis / 1000;
    return (
      (seconds >= 60 ? Math.floor(seconds / 60) + "m " : "") +
      (seconds % 60).toLocaleString(undefined, { maximumFractionDigits: 3 }) +
      "s into capture"
    );
  }
  return new Date(marker.unixMillis).toLocaleString(undefined, { timeZoneName: "short" });
}
export function traceMarkers(reviews: MarkerReview[]): TraceMarker[] {
  return reviews.flatMap((review) =>
    review.markers.map((marker) => ({
      ...marker,
      entryId: review.markerEntries?.[marker.id] ?? null,
      boundary: review.links?.find((link) => link.markerId === marker.id)?.boundary ?? "",
      key: review.traceId + ":" + marker.id,
      traceId: review.traceId,
      source: review.name,
      timeLabel: markerTime(marker),
      description: review.name + " · " + markerTime(marker) + " · " + (marker.note || "No note"),
    })),
  );
}
export type MarkerGroup = {
  selected?: boolean;
  position: string;
  key: string;
  x: number;
  label: string;
  description: string;
  markers: TraceMarker[];
};
export function markerGroups(
  markers: TraceMarker[],
  from: number,
  to: number,
  pixels: number,
): MarkerGroup[] {
  const groups: MarkerGroup[] = [];
  for (const marker of markers
    .filter((m) => m.unixMillis >= from && m.unixMillis <= to)
    .sort((a, b) => a.unixMillis - b.unixMillis || a.key.localeCompare(b.key))) {
    const x = Math.max(
        5,
        Math.min(995, ((marker.unixMillis - from) / Math.max(1, to - from)) * 1000),
      ),
      last = groups.at(-1);
    if (last && ((x - last.x) * pixels) / 1000 < 24) {
      last.markers.push(marker);
    } else {
      groups.push({
        key: marker.key,
        x,
        position: x / 10 + "%",
        label: "",
        description: "",
        markers: [marker],
      });
    }
  }
  return groups.map((group) => ({
    ...group,
    label:
      group.markers.length > 1
        ? group.markers.length + " markers"
        : (group.markers[0]!.note || "No note").slice(0, 24),
    description:
      group.markers.length > 1
        ? group.markers.length + " markers. Choose a marker."
        : group.markers[0]!.description,
  }));
}
