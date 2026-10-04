import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vite'

// The node the dev server proxies to; override to point the UI at another
// instance, e.g. `CAPTURE_ROOM_NODE=localhost:7701 pnpm dev`.
const node = process.env.CAPTURE_ROOM_NODE ?? 'localhost:7700'

export default defineConfig({
  plugins: [vue(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
    },
  },
  server: {
    proxy: {
      '/api': `http://${node}`,
      '/ws': { target: `ws://${node}`, ws: true },
    },
  },
})
