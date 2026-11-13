import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  root: path.join(here, 'ui'),
  base: './',
  build: {
    outDir: path.join(here, 'ui/dist'),
    emptyOutDir: true,
    sourcemap: false,
  },
});
