/// <reference types="vitest" />
import vue from '@vitejs/plugin-vue';
import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';

export default defineConfig({
  base: './',
  plugins: [vue()],
  server: { strictPort: true, host: '127.0.0.1', port: 1429 },
  build: {
    target: 'es2022',
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL('./index.html', import.meta.url)),
        history: fileURLToPath(new URL('./history.html', import.meta.url)),
        preview: fileURLToPath(new URL('./preview.html', import.meta.url)),
      },
    },
  },
  test: { exclude: ['**/node_modules/**', '**/dist/**'] }
});
