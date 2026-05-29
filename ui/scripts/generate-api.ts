import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const endpoint = process.env.QUARRY_OPENAPI_URL ?? 'http://127.0.0.1:7831/v1/openapi.json';
const output = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '../src/api/generated/openapi.json'
);

const response = await fetch(endpoint);
if (!response.ok) {
  throw new Error(`failed to fetch ${endpoint}: ${response.status}`);
}

await mkdir(dirname(output), { recursive: true });
await writeFile(output, JSON.stringify(await response.json(), null, 2));
