/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';
import tailwindcss from '@tailwindcss/vite';

// The dev server proxies the documented API surface to the Rust server.
// `npm run dev` gives you the UI on :5173 with live /api and /api/events streams.
export default defineConfig({
  plugins: [preact(), tailwindcss()],
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8787',
        changeOrigin: false,
        // The terminal attaches at /api/terminals/:id/socket. HTTP-only proxying
        // passes the tests but leaves a real terminal blank in dev/preview.
        ws: true,
      },
    },
  },
  preview: {
    port: 4173,
    proxy: {
      '/api': { target: 'http://127.0.0.1:8787', changeOrigin: false, ws: true },
    },
  },
  build: {
    target: 'es2022',
    cssCodeSplit: true,
    sourcemap: false,
    reportCompressedSize: true,
    rollupOptions: {
      output: {
        // Three vendor families, split out so a change to app code does not
        // invalidate them and so the browser can fetch them alongside the entry
        // chunk instead of parsing them as part of it.
        //
        // Deliberately NOT split: `mermaid` and `@xterm/*` already travel as
        // their own lazily-imported chunks (`TerminalSurface`, the mermaid
        // diagram renderer), and there is nothing to gain from regrouping
        // chunks that are fetched on demand anyway.
        manualChunks(id: string): string | undefined {
          if (!id.includes('node_modules')) return undefined;
          if (/node_modules\/(preact|@preact\/signals)\//.test(id)) return 'vendor-preact';
          if (
            /node_modules\/(markdown-it|markdown-it-footnote|markdown-it-task-lists|dompurify)\//.test(
              id,
            )
          ) {
            return 'vendor-markdown';
          }
          if (/node_modules\/highlight\.js\//.test(id)) return 'vendor-highlight';
          return undefined;
        },
      },
    },
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}', 'scripts/**/*.test.mjs'],
    restoreMocks: true,
    clearMocks: true,
    css: false,
  },
});