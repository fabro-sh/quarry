import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const result = spawnSync('cargo', ['build', '--locked', '-p', 'quarry', '--features', 'lib-documents',
  ...(process.argv.includes('--release') ? ['--release'] : [])], { cwd: root, stdio: 'inherit' });
if (result.error) throw result.error;
process.exit(result.status ?? 1);
