import { defineConfig } from 'vite';

// Relative base so the build works under any subpath on GitHub Pages
// (e.g. /editor/ and /branch/<name>/editor/). See design.md §17.
export default defineConfig({
  base: './',
  build: {
    outDir: 'dist',
    target: 'es2022',
  },
});
