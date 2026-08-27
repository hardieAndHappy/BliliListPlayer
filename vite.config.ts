import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react],
  // 与 tauri.conf.json 的 devUrl 对齐：Tauri 会等此端口。
  server: { port: 1420, strictPort: true },
});
