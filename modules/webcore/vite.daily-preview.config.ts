import { fileURLToPath, URL } from 'node:url'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'

// Independent entry with no production router, gateway, storage or model access.
export default defineConfig({
  root: fileURLToPath(new URL('./src/previews/next/daily', import.meta.url)),
  plugins: [vue(), tailwindcss()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  build: { target: 'es2022' },
  preview: { host: '127.0.0.1', strictPort: true },
})
