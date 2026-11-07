import { performance } from "node:perf_hooks";

export function createBrowserCheckRunner(
  report = (event) => {
    process.stdout.write(`[browser-check] ${JSON.stringify(event)}\n`);
  },
) {
  return async (check, ...args) => {
    const started = performance.now();
    const name = check.name || "anonymous";
    report({ name, result: "started" });
    try {
      const result = await check(...args);
      report({ name, result: "passed", durationSeconds: (performance.now() - started) / 1000 });
      return result;
    } catch (error) {
      report({ name, result: "failed", durationSeconds: (performance.now() - started) / 1000 });
      throw error;
    }
  };
}

export async function reportBrowserCheckFailure(
  error,
  collectors,
  { timeoutMs = 2000, write = (message) => process.stderr.write(message) } = {},
) {
  // Emit the original failure before touching a page that may already be closed.
  write(`${error?.stack || String(error)}\n`);
  await Promise.all(
    collectors.map(async ({ name, collect }) => {
      let timer;
      try {
        await Promise.race([
          Promise.resolve().then(collect),
          new Promise((_, reject) => {
            timer = setTimeout(() => reject(new Error(`exceeded ${timeoutMs} ms`)), timeoutMs);
          }),
        ]);
      } catch (diagnosticError) {
        write(
          `Browser diagnostic ${name} failed: ${diagnosticError?.message || diagnosticError}\n`,
        );
      } finally {
        clearTimeout(timer);
      }
    }),
  );
  return error;
}
