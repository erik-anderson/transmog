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

/** Track cancellation from command entry through loading and rendering. */
export function capturePresentationIntent(owner: Element): {
  current: () => boolean;
  dispose: () => void;
} {
  const focus = captureFocus(owner),
    root = owner.getRootNode(),
    document = owner.ownerDocument;
  let canceled = false;
  const cancel = (event: Event): void => {
    if (
      event.type === "pointerdown" ||
      (event instanceof KeyboardEvent && event.key === "Escape")
    ) {
      canceled = true;
    }
  };
  root.addEventListener("keydown", cancel, true);
  root.addEventListener("pointerdown", cancel, true);
  if (root !== document) {
    document.addEventListener("keydown", cancel, true);
    document.addEventListener("pointerdown", cancel, true);
  }
  return {
    current: () => owner.isConnected && !canceled && focus(),
    dispose: () => {
      root.removeEventListener("keydown", cancel, true);
      root.removeEventListener("pointerdown", cancel, true);
      if (root !== document) {
        document.removeEventListener("keydown", cancel, true);
        document.removeEventListener("pointerdown", cancel, true);
      }
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
  } catch (error) {
    if (
      error instanceof DOMException &&
      error.name === "AbortError" &&
      (!owner.isConnected || !current())
    ) {
      return () => false;
    }
    throw error;
  } finally {
    intent.dispose();
  }
  return () => current() && intent.current();
}
