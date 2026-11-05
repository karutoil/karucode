/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import preact from '@preact/preset-vite';

// The dev server proxies the documented API surface to the Rust server.
// `npm run dev` gives you the UI on :5173 with live /api and /api/events streams.
export default defineConfig({
  plugins: [preact()],
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
  },
  test: {
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
    restoreMocks: true,
    clearMocks: true,
    css: false,
  },
});