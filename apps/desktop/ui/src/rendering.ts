/** Capture ownership after applying state and before awaiting its render. */
export function captureRenderScope(owner: Element, read: () => readonly unknown[]): () => boolean {
  const values = read();
  return () => {
    if (!owner.isConnected) {
      return false;
    }
    const current = read();
    return (
      values.length === current.length &&
      values.every((value, index) => Object.is(value, current[index]))
    );
  };
}

function activeElement(document: Document): Element | null {
  let active = document.activeElement;
  while (active?.shadowRoot?.activeElement) {
    active = active.shadowRoot.activeElement;
  }
  return active;
}

/** Distinguish new reading input from scroll clamping caused by our own render. */
export function trackScrollInput(element: Element): {
  unchanged: () => boolean;
  dispose: () => void;
} {
  let moved = false;
  const mark = (): void => {
    moved = true;
  };
  const events = ["wheel", "touchmove", "pointerdown", "keydown"];
  for (const event of events) {
    element.addEventListener(event, mark, { capture: true, passive: true });
  }
  return {
    unchanged: () => !moved,
    dispose: () => {
      for (const event of events) {
        element.removeEventListener(event, mark, true);
      }
    },
  };
}

/** Preserve newer focus, including moves outside the owner's shadow tree. */
export function captureFocus(owner: Element): () => boolean {
  const document = owner.ownerDocument,
    focused = activeElement(document),
    root = owner.getRootNode(),
    host = root instanceof ShadowRoot ? root.host : null;
  return () => {
    const current = activeElement(document);
    return (
      current === focused ||
      ((!focused?.isConnected ||
        (focused instanceof HTMLElement &&
          (!focused.getClientRects().length || focused.matches(":disabled")))) &&
        (!current || current === document.body || current === owner || current === host))
    );
  };
}

const presentationRevisions = new WeakMap<Node, { value: number }>();

function presentationRevision(root: Node): { value: number } {
  let revision = presentationRevisions.get(root);
  if (!revision) {
    const state = { value: 0 };
    const advance = (event: Event): void => {
      if (
        event.type === "pointerdown" ||
        (event instanceof KeyboardEvent && event.key === "Escape")
      ) {
        state.value++;
      }
    };
    root.addEventListener("keydown", advance, true);
    root.addEventListener("pointerdown", advance, true);
    presentationRevisions.set(root, state);
    revision = state;
  }
  return revision;
}

/** Track cancellation from command entry through loading and rendering. */
export function capturePresentationIntent(owner: Element): {
  current: () => boolean;
  dispose: () => void;
} {
  const focus = captureFocus(owner),
    root = presentationRevision(owner.getRootNode()),
    document = presentationRevision(owner.ownerDocument),
    rootRevision = root.value,
    documentRevision = document.value;
  // Render waits can be abandoned on teardown, so listeners live at the root.
  const unchanged = (): boolean =>
    root.value === rootRevision && document.value === documentRevision;
  let unchangedAtCompletion: boolean | undefined;
  return {
    current: () => owner.isConnected && (unchangedAtCompletion ?? unchanged()) && focus(),
    dispose: () => {
      unchangedAtCompletion ??= unchanged();
    },
  };
}

/** Await presentation without reopening it after newer user intent. */
export async function renderPresentation(
  owner: Element & { $whenRendered(): Promise<void> },
  read: () => readonly unknown[],
): Promise<() => boolean> {
  if (!owner.isConnected) {
    return () => false;
  }
  const current = captureRenderScope(owner, read),
    intent = capturePresentationIntent(owner);
  try {
    await owner.$whenRendered();
  } finally {
    intent.dispose();
  }
  return () => current() && intent.current();
}
