import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Output goes to ../src/panel with fixed names: the Rust binary embeds these files,
// so servers never run Node. `fragments: 'tree'` builds DOM without innerHTML.
export default defineConfig({
  base: '/panel/',
  plugins: [
    svelte({ compilerOptions: { fragments: 'tree', css: 'external' } }),
    // Svelte error messages embed documentation links; keep them as plain text so
    // the bundle names no external URL (the panel never loads anything remote).
    { name: 'brainkeep-no-remote-urls', renderChunk: (code) => code.replaceAll('https://svelte.dev/e/', 'svelte.dev/e/') },
  ],
  build: {
    outDir: '../src/panel',
    emptyOutDir: true,
    target: 'es2022',
    modulePreload: false,
    cssCodeSplit: false,
    assetsInlineLimit: 0,
    sourcemap: false,
    minify: process.env.BRAINKEEP_DEBUG ? false : 'oxc',
    rolldownOptions: {
      output: {
        entryFileNames: 'app.js',
        // One script only: the binary serves a fixed file list.
        codeSplitting: false,
        assetFileNames: (info) => {
          const name = info.names?.[0] || info.name || '';
          if (name.endsWith('.css')) return 'app.css';
          if (name.endsWith('.woff2')) return 'fonts/[name][extname]';
          return '[name][extname]';
        },
      },
    },
  },
});
