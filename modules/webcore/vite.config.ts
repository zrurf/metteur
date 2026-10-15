import { fileURLToPath, URL } from 'node:url'

import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig, loadEnv } from 'vite'
import { VitePWA } from 'vite-plugin-pwa'

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, process.cwd(), 'VITE_')
  const mock = (process.env.VITE_MOCK ?? env.VITE_MOCK) === '1'
  return {
    plugins: [
      {
        name: 'metteur-gateway-identity',
        transformIndexHtml: () => [
          {
            tag: 'meta',
            attrs: { name: 'metteur-gateway', content: mock ? 'mock' : 'grpc' },
            injectTo: 'head' as const,
          },
        ],
      },
      vue(),
      tailwindcss(),
      VitePWA({
        // Notify open clients rather than silently leaving old JS mounted, or
        // reloading over unsaved source edits when a new build is deployed.
        registerType: 'prompt',
        injectRegister: false,
        manifest: {
          name: 'Metteur',
          short_name: 'Metteur',
          start_url: '/',
          display: 'standalone',
          background_color: '#0f172a',
          theme_color: '#0f172a',
          icons: [{ src: 'favicon.svg', sizes: '192x192', type: 'image/svg+xml' }],
        },
        // Monaco editor bundles exceed the 2 MiB workbox default.
        workbox: { maximumFileSizeToCacheInBytes: 10 * 1024 * 1024 },
      }),
    ],
    resolve: {
      // CodeMirror extensions use instanceof checks: nested dependency copies
      // must resolve to the same runtime as the application's extensions.
      dedupe: ['@codemirror/state', '@codemirror/view', '@codemirror/language'],
      alias: {
        '@': fileURLToPath(new URL('./src', import.meta.url)),
      },
    },
    build: {
      target: 'es2022',
    },
    server: {
      // In dev the daemon traffic is proxied to the Web Server Client, while the
      // app itself is served by Vite (set VITE_MOCK=1 to use demo data).
      // `/api` carries the chat SSE stream and the folder picker, so it needs the
      // same proxy as grpc-web — without it every chat turn 404s in dev.
      proxy: mock
        ? undefined
        : {
            '/metteur.Daemon': {
              target: 'http://127.0.0.1:8787',
              changeOrigin: true,
            },
            '/api': {
              target: 'http://127.0.0.1:8787',
              changeOrigin: true,
            },
          },
    },
  }
})
