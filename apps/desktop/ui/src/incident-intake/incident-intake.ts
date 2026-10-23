import { attr, observable } from "@microsoft/webui-framework";
import { invoke } from "@tauri-apps/api/core";
import { WorkspaceElement } from "../workspace-element.js";
import { describeError } from "../utilities.js";
import { markerTime, type Marker, type MarkerReview } from "../markers.js";
export type IncidentWindow = {
  traceId: string;
  markerId: string;
  beforeSeconds: number;
  afterSeconds: number;
  relevantHostsOnly: boolean;
};
type Intake = {
  review: MarkerReview;
  traceId: string;
  source: string;
  profile: null | {
    title: string;
    instructions: string;
    steps: string[];
    expectedApplication: string | null;
    relevantHosts: Array<{ host: string; includeSubdomains: boolean }>;
  };
  markers: Marker[];
  expectedBehavior: string;
  actualBehavior: string;
  enteredClientVersion: string;
  collector: string;
  observedClients: string[];
  observedUserAgents: string[];
  entries: number;
  incompleteBodies: number;
  note: string;
};
export class IncidentIntake extends WorkspaceElement {
  @attr traceId = "";
  @attr refreshToken = "";
  @observable selectedMarkerId = "";
  @observable activeMarker = { id: "", note: "", originLabel: "", edited: false };
  @observable editingMarkerId = "";
  @observable markerDraft = "";
  @observable markerDirty = false;
  @observable markerCanUndo = false;
  private markerRevision = 0;
  private drafts = new Map<string, string>();
  private mutation: Promise<boolean> | null = null;
  @observable intakeTitle = "Markers";
  @observable intakeInstructions = "";
  @observable intakeSteps: Array<{ id: string; text: string }> = [];
  @observable intakeMarkers: Array<
    Marker & { timeLabel: string; originLabel: string; edited: boolean }
  > = [];
  @observable intakeHosts: Array<{ id: string; text: string }> = [];
  @observable intakeFacts: Array<{ label: string; value: string }> = [];
  @observable intakeNote = "";
  @observable intakeStatus = "";
  @observable intakeBusy = false;
  @observable incidentBefore = 30;
  @observable incidentAfter = 30;
  @observable incidentRelevantOnly = false;
  markerEditor!: HTMLTextAreaElement;
  private revision = 0;
  override connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }
  refreshTokenChanged(): void {
    if (this.isConnected) {
      void this.load();
    }
  }
  traceIdChanged(): void {
    this.editingMarkerId = "";
    if (this.isConnected) {
      void this.load();
    }
  }
  private async load(): Promise<void> {
    const id = this.traceId,
      revision = ++this.revision;
    this.intakeMarkers = [];
    this.intakeSteps = [];
    this.intakeFacts = [];
    this.intakeHosts = [];
    this.intakeInstructions = "";
    this.intakeNote = "";
    this.intakeTitle = "Markers";
    this.incidentRelevantOnly = false;
    if (!id) {
      this.intakeBusy = false;
      this.intakeStatus = "Choose a trace to review its markers.";
      return;
    }
    this.intakeBusy = true;
    this.intakeStatus = "Loading original incident context…";
    try {
      const intake = await invoke<Intake>("incident_intake", { traceId: id });
      if (!this.isConnected || revision !== this.revision) {
        return;
      }
      this.intakeTitle = intake.profile?.title ?? "";
      this.markerRevision = intake.review?.revision ?? 0;
      this.markerDirty = intake.review?.dirty ?? false;
      this.markerCanUndo = intake.review?.canUndo ?? false;
      this.intakeInstructions = intake.profile?.instructions ?? "";
      this.intakeSteps = (intake.profile?.steps ?? []).map((text, index) => ({
        id: String(index),
        text,
      }));
      this.intakeHosts = (intake.profile?.relevantHosts ?? []).map((host, index) => ({
        id: String(index),
        text: host.host + (host.includeSubdomains ? " (and subdomains)" : ""),
      }));
      this.intakeMarkers = intake.markers.map((marker) => ({
        ...marker,
        timeLabel: markerTime(marker),
        edited: intake.review?.editedMarkerIds.includes(marker.id) ?? false,
        originLabel:
          (intake.review?.links?.find((link) => link.markerId === marker.id)?.boundary ===
          "response"
            ? "Response · "
            : intake.review?.links?.some((link) => link.markerId === marker.id)
              ? "Request · "
              : "") +
          new Date(marker.unixMillis).toLocaleString(undefined, { timeZoneName: "short" }) +
          " · " +
          (marker.origin === "cli"
            ? "Command-line capture"
            : marker.origin === "desktop"
              ? "Desktop capture"
              : "Reviewer"),
      }));
      this.selectMarker(
        this.intakeMarkers.some((m) => m.id === this.selectedMarkerId)
          ? this.selectedMarkerId
          : (this.intakeMarkers[0]?.id ?? ""),
        false,
      );
      this.intakeFacts = [
        { label: "Retained source entries", value: String(intake.entries) },
        { label: "Body records without full content", value: String(intake.incompleteBodies) },
        { label: "Expected behavior", value: intake.expectedBehavior || "Not entered" },
        { label: "Actual behavior", value: intake.actualBehavior || "Not entered" },
        {
          label: "Expected application (profile)",
          value: intake.profile?.expectedApplication || "Not specified",
        },
        {
          label: "Client/version entered by customer",
          value: intake.enteredClientVersion || "Not entered",
        },
        {
          label: "Observed client identity (sample)",
          value: intake.observedClients.join(" · ") || "Unavailable",
        },
        {
          label: "Observed User-Agent (sample)",
          value: intake.observedUserAgents.join(" · ") || "Unavailable",
        },
        { label: "Collector version", value: intake.collector || "Unrecorded" },
      ];
      this.intakeNote = intake.note;
      this.intakeStatus = "";
    } catch (error: unknown) {
      if (revision === this.revision) {
        this.intakeStatus = describeError(error);
      }
    } finally {
      if (revision === this.revision) {
        this.intakeBusy = false;
      }
    }
  }
  selectMarker(id: string, navigate = true): void {
    this.selectedMarkerId = id;
    this.activeMarker = this.intakeMarkers.find((m) => m.id === id) ?? {
      id: "",
      note: "",
      originLabel: "",
      edited: false,
    };
    if (navigate) {
      this.$emit("marker-time-request", { traceId: this.traceId, markerId: id });
    }
  }
  editMarker(id: string): void {
    this.selectedMarkerId = id;
    this.editingMarkerId = id;
    this.markerDraft =
      this.drafts.get(this.traceId + ":" + id) ??
      this.intakeMarkers.find((m) => m.id === id)?.note ??
      "";
    this.$flushUpdates();
    this.markerEditor.value = this.markerDraft;
    this.markerEditor.focus();
  }
  draftInput(event: Event): void {
    this.markerDraft = (event.target as HTMLTextAreaElement).value;
    this.drafts.set(this.traceId + ":" + this.editingMarkerId, this.markerDraft);
    this.$emit("marker-draft-change", this.drafts.size > 0);
  }
  cancelEdit(): void {
    this.drafts.delete(this.traceId + ":" + this.editingMarkerId);
    this.editingMarkerId = "";
    this.$emit("marker-draft-change", this.drafts.size > 0);
  }
  discardDrafts(): void {
    this.drafts.clear();
    this.editingMarkerId = "";
    this.$emit("marker-draft-change", false);
  }
  applyPending(): Promise<boolean> {
    if (this.mutation) {
      return this.mutation;
    }
    this.intakeBusy = true;
    this.mutation = this.applyDrafts().finally(() => {
      this.intakeBusy = false;
      this.mutation = null;
    });
    return this.mutation;
  }
  private async applyDrafts(): Promise<boolean> {
    const drafts = [...this.drafts];
    for (const [key, note] of drafts) {
      if (new TextEncoder().encode(note).length > 2048) {
        this.intakeStatus = "Note is too long. Shorten it to continue.";
        return false;
      }
      const split = key.lastIndexOf(":"),
        traceId = key.slice(0, split),
        id = key.slice(split + 1);
      try {
        const reviews = await invoke<MarkerReview[]>("trace_marker_reviews"),
          review = reviews.find((r) => r.traceId === traceId);
        if (!review) {
          throw new Error("Trace is no longer available");
        }
        await invoke("change_trace_marker", {
          traceId,
          revision: review.revision,
          change: { action: "edit", id, note },
        });
        if (this.drafts.get(key) === note) {
          this.drafts.delete(key);
        }
        this.$emit("markers-changed", { traceId, deletedId: "" });
      } catch (error: unknown) {
        this.intakeStatus = describeError(error);
        return false;
      }
    }
    if (!this.drafts.has(this.traceId + ":" + this.editingMarkerId)) {
      this.editingMarkerId = "";
    }
    this.$emit("marker-draft-change", this.drafts.size > 0);
    await this.load();
    return true;
  }
  changeMarker(action: "edit" | "delete" | "undo", id = "", note = ""): Promise<boolean> {
    if (this.mutation) {
      return this.mutation;
    }
    const traceId = this.traceId,
      revision = this.markerRevision;
    if (action === "edit" && new TextEncoder().encode(note).length > 2048) {
      this.intakeStatus = "Note is too long. Shorten it to continue.";
      return Promise.resolve(false);
    }
    this.intakeBusy = true;
    this.mutation = (async () => {
      try {
        await invoke("change_trace_marker", {
          traceId,
          revision,
          change:
            action === "undo" ? { action } : { action, id, ...(action === "edit" ? { note } : {}) },
        });
        if (action !== "edit" || this.drafts.get(traceId + ":" + id) === note) {
          this.drafts.delete(traceId + ":" + id);
        }
        if (traceId === this.traceId) {
          if (!this.drafts.has(traceId + ":" + id)) {
            this.editingMarkerId = "";
          }
          await this.load();
        }
        this.$emit("markers-changed", { traceId, deletedId: action === "delete" ? id : "" });
        this.$emit("marker-draft-change", this.drafts.size > 0);
        return true;
      } catch (error: unknown) {
        if (traceId === this.traceId) {
          this.intakeStatus = describeError(error);
        }
        return false;
      } finally {
        this.intakeBusy = false;
        this.mutation = null;
      }
    })();
    return this.mutation;
  }
  setInterval(side: string, event: Event): void {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    if (side === "before") {
      this.incidentBefore = value;
    } else {
      this.incidentAfter = value;
    }
  }
  setRelevant(event: Event): void {
    this.incidentRelevantOnly = (event.currentTarget as HTMLInputElement).checked;
  }
  showMarker(markerId: string): void {
    if (
      !Number.isInteger(this.incidentBefore) ||
      !Number.isInteger(this.incidentAfter) ||
      this.incidentBefore < 0 ||
      this.incidentAfter < 0 ||
      this.incidentBefore > 86400 ||
      this.incidentAfter > 86400
    ) {
      this.intakeStatus = "Choose 0–86400 seconds before and after the marker.";
      return;
    }
    const window: IncidentWindow = {
      traceId: this.traceId,
      markerId,
      beforeSeconds: this.incidentBefore,
      afterSeconds: this.incidentAfter,
      relevantHostsOnly: markerId ? this.incidentRelevantOnly : false,
    };
    const detail = {
      window,
      label:
        (this.intakeTitle || "Trace") +
        (markerId
          ? ` · ${this.incidentBefore}s before / ${this.incidentAfter}s after marker`
          : " · full trace") +
        (window.relevantHostsOnly ? " · suggested hosts only" : " · all hosts"),
    };
    this.$emit("incident-window-request", detail);
  }
  override disconnectedCallback(): void {
    this.revision++;
    super.disconnectedCallback();
  }
}
IncidentIntake.define("incident-intake");
