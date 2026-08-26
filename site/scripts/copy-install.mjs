import { copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
await mkdir(resolve(root, 'public'), { recursive: true });
await copyFile(resolve(root, '..', 'scripts', 'install.sh'), resolve(root, 'public', 'install.sh'));
