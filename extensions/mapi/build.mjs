import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

// Authoring only. The app packages committed ES2022; it never runs TypeScript.
const root = fileURLToPath(new URL('.', import.meta.url));
const compiler = fileURLToPath(new URL('../../apps/desktop/ui/node_modules/typescript/bin/tsc', import.meta.url));
execFileSync(process.execPath, [compiler, '--project', root], {
  cwd: root,
  stdio: 'inherit',
  windowsHide: true,
});
