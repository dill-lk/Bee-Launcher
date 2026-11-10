import { defineConfig } from 'vite';

export default defineConfig({
  // Tauri serves the packaged frontend through its custom protocol, not HTTP.
  // Relative asset URLs keep the bundled UI and all of its assets resolvable.
  base: './',
});
