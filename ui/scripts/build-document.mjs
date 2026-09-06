import { existsSync, mkdirSync, readdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const target = resolve(root, process.env.CARGO_TARGET_DIR ?? 'target');
const version = '0.2.127';
const tools = join(root, 'target/tools');
const local = existsSync(tools)
  ? readdirSync(tools).find((name) => name.startsWith(`wasm-bindgen-${version}-`))
  : undefined;
const bindgen = process.env.QUARRY_WASM_BINDGEN
  ?? (local ? join(tools, local, 'wasm-bindgen') : 'wasm-bindgen');

function run(command, args) {
  const result = spawnSync(command, args, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}

const check = spawnSync(bindgen, ['--version'], { encoding: 'utf8' });
if (check.status !== 0 || check.stdout.trim() !== `wasm-bindgen ${version}`) {
  throw new Error(`Building the document engine requires wasm-bindgen-cli ${version}. Install with: cargo install wasm-bindgen-cli --version ${version} --locked. Also run: rustup target add wasm32-unknown-unknown`);
}
const release = !process.argv.includes('--dev');
run('cargo', ['build', '--locked', '-p', 'quarry-document-wasm',
  '--target', 'wasm32-unknown-unknown', ...(release ? ['--release'] : [])]);
const output = join(root, 'ui/src/generated/document');
mkdirSync(output, { recursive: true });
run(bindgen, [join(target, `wasm32-unknown-unknown/${release ? 'release' : 'debug'}/quarry_document_wasm.wasm`),
  '--target', 'web', '--out-dir', output, '--out-name', 'quarry_document']);
