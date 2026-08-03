import { resolve } from 'node:path';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react()],
  publicDir: 'static',
  build: {
    rollupOptions: {
      // 双入口：index.html 是网页版，index.desktop.html 是桌面外壳。
      // 桌面窗口通过 tauri.conf.json 的 windows[0].url 指向后者，
      // 两份产物共用同一个 dist/ 与同一批公共 chunk。
      input: {
        index: resolve(__dirname, 'index.html'),
        desktop: resolve(__dirname, 'index.desktop.html')
      }
    }
  },
  server: {
    port: 5173,
    proxy: {
      '/api': 'http://127.0.0.1:8000'
    }
  }
});

