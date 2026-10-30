import type { ScriptDraft, ScriptRevision, ScriptStatus } from "../models.js";

/** Revision numbers do not describe whether the source or its authority changed. */
export function scriptFingerprint(draft: ScriptDraft): string {
  return JSON.stringify({
    source: draft.source,
    language: draft.language ?? "typescript",
    retainedSource: draft.retainedSource ?? null,
    javascriptHistory: draft.javascriptHistory ?? [],
    handlers: [...new Set(draft.handlers)].sort(),
    prefilter: draft.prefilter,
    capabilities: {
      ...draft.capabilities,
      writeHeaders: [
        ...new Set(draft.capabilities.writeHeaders.map((name) => name.toLowerCase())),
      ].sort(),
    },
    limits: draft.limits,
    priority: draft.priority,
  });
}

export function activeDraft(revision: ScriptRevision): ScriptDraft {
  const {
    id,
    revision: version,
    handlers,
    prefilter,
    capabilities,
    limits,
    priority,
  } = revision.manifest;
  return {
    id,
    revision: version,
    source: revision.source,
    language: revision.language ?? "typescript",
    retainedSource: revision.retainedSource ?? null,
    javascriptHistory: revision.javascriptHistory ?? [],
    handlers,
    prefilter,
    capabilities,
    limits,
    priority,
  };
}

/** Older clients saved and activated separately; prefer active on equal revisions. */
export function editingDraft(status: ScriptStatus, id = "user-script"): ScriptDraft | undefined {
  const saved = status.saved
    .filter((item) => item.id === id)
    .sort((a, b) => b.revision - a.revision)[0];
  const active = status.active.find((item) => item.manifest.id === id);
  return active && (!saved || active.manifest.revision >= saved.revision)
    ? activeDraft(active)
    : saved;
}

export function scriptSettings(draft: ScriptDraft): string {
  return JSON.stringify(
    {
      language: draft.language,
      handlers: draft.handlers,
      prefilter: draft.prefilter,
      capabilities: draft.capabilities,
      limits: draft.limits,
      priority: draft.priority,
      retainedSource: draft.retainedSource,
      javascriptHistory: draft.javascriptHistory,
    },
    null,
    2,
  );
}

/** Alternate-language recovery work never changes the applied executable snapshot. */
export function scriptExecutionFingerprint(draft: ScriptDraft): string {
  return scriptFingerprint({ ...draft, retainedSource: null, javascriptHistory: [] });
}
