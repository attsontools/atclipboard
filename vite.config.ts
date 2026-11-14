/// <reference types="vitest" />
import vue from '@vitejs/plugin-vue';
import { defineConfig } from 'vite';

export default defineConfig({
  base: './',
  plugins: [vue()],
  server: { strictPort: true, host: '127.0.0.1', port: 1429 },
  build: { target: 'es2022' },
  test: { exclude: ['**/node_modules/**', '**/dist/**'] }
});
