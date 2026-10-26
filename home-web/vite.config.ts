import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';

const backend = 'http://127.0.0.1:8470';

export default defineConfig({
  plugins: [react()],
  build: {
    // The dot-home binary embeds these files at compile time.
    outDir: '../crates/dot-home/web-dist',
    emptyOutDir: true,
    target: 'es2022',
    // The server's CSP allows only same-origin scripts; keep everything in files.
    modulePreload: { polyfill: false },
    assetsInlineLimit: 0,
    cssCodeSplit: true,
  },
  server: {
    host: '127.0.0.1',
    port: 5180,
    strictPort: true,
    proxy: {
      '/api': { target: backend, ws: true, changeOrigin: false },
      '/mcp': backend,
      '/oauth': backend,
      '/.well-known': backend,
    },
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
