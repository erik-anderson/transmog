let pending: Promise<typeof import('./monaco.js')> | null = null;

/** One module graph and Trusted Types policy, shared by all editors. */
export function loadMonaco(): Promise<typeof import('./monaco.js')> {
  pending ??= import('./monaco-environment.js')
    .then(() => import('./monaco.js'))
    .catch((error: unknown) => { pending = null; throw error; });
  return pending;
}

export function waitForStyles(link: HTMLLinkElement): Promise<void> {
  if (link.sheet) return Promise.resolve();
  return new Promise((resolve, reject) => {
    const loaded = (): void => { cleanup(); resolve(); };
    const failed = (): void => { cleanup(); reject(new Error('Editor stylesheet could not be loaded')); };
    const cleanup = (): void => {
      link.removeEventListener('load', loaded);
      link.removeEventListener('error', failed);
    };
    link.addEventListener('load', loaded, { once: true });
    link.addEventListener('error', failed, { once: true });
  });
}
