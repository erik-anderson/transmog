import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

// Python supplies the same OS lease used by the portable cleanup command.
export async function beginOutputRun(path, kind, python, group = "default") {
  const child = spawn(
    python,
    [
      fileURLToPath(new URL("../scripts/test-output-run.py", import.meta.url)),
      "--path",
      path,
      "--kind",
      kind,
      "--group",
      group,
    ],
    { stdio: ["pipe", "pipe", "pipe"], windowsHide: true },
  );
  let errors = "";
  child.stderr.on("data", (data) => {
    errors += data;
  });
  child.stdin.on("error", () => {});
  const completed = new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", (code) =>
      code === 0 ? resolve() : reject(new Error(`Output lease failed (${code}): ${errors}`)),
    );
  });
  const lines = createInterface({ input: child.stdout });
  try {
    await Promise.race([
      new Promise((resolve, reject) => {
        lines.once("line", (line) => {
          try {
            if (JSON.parse(line).ready !== true) throw new Error("Output lease was not ready");
            resolve();
          } catch (error) {
            reject(error);
          }
        });
      }),
      completed.then(() => {
        throw new Error("Output lease exited before startup");
      }),
    ]);
  } catch (error) {
    child.stdin.end();
    await completed.catch(() => {});
    throw error;
  } finally {
    lines.close();
  }
  // Attach immediately: an unexpected helper failure must not become an
  // unhandled rejection while the test is still shutting down its children.
  completed.catch(() => {});
  return async (passed) => {
    child.stdin.end(typeof passed === "boolean" ? JSON.stringify({ passed }) + "\n" : "");
    await completed;
  };
}
