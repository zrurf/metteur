import { fileURLToPath, URL } from 'node:url'
import vue from '@vitejs/plugin-vue'
import tailwindcss from '@tailwindcss/vite'
import { defineConfig } from 'vite'
const addon = "import WorkflowAddon from '@/previews/next/daily/WorkflowAddon.vue'\n"
function replace(source: string, from: string, to: string) {
  if (!source.includes(from)) throw Error(`IDE fixture anchor changed: ${from.slice(0, 60)}`)
  return source.replace(from, to)
}
// Only this isolated build adapts the existing views. Production files, router,
// activity rail, tree, tabs, editor and ExecutionLayout remain unchanged.
export default defineConfig({
  root: fileURLToPath(new URL('./src/previews/next/daily/ide', import.meta.url)),
  publicDir: fileURLToPath(new URL('./public', import.meta.url)),
  define: { 'import.meta.env.VITE_MOCK': JSON.stringify('1') },
  plugins: [
    {
      name: 'daily-ide-fixture',
      enforce: 'pre',
      transform(source, id) {
        const path = id.replaceAll('\\', '/')
        if (path.endsWith('/views/ChatView.vue')) {
          source = replace(source, '<script setup lang="ts">', '<script setup lang="ts">\n' + addon)
          source = replace(source, 'data-chat-draft-status', 'data-chat-draft-status v-if="false"')
          source = replace(source, 'v-if="inputBlocked"', 'v-if="false"')
          return replace(
            source,
            '    <ChatThread',
            '    <WorkflowAddon mode="chat"/>\n    <ChatThread',
          )
        }
        if (path.endsWith('/views/EditorPane.vue')) {
          source = replace(source, '<script setup lang="ts">', '<script setup lang="ts">\n' + addon)
          source = replace(source, 'v-if="loaded && filePath"', 'v-if="false"') // Avoid duplicate real recovery controls in this fixture.
          return replace(
            source,
            '<div ref="pane" data-editor-pane class="flex h-full flex-col bg-background">',
            '<div ref="pane" data-editor-pane class="flex h-full flex-col bg-background"><WorkflowAddon mode="editor"/>',
          )
        }
        if (path.endsWith('/views/TaskContinuationView.vue')) {
          source = replace(source, '<script setup lang="ts">', '<script setup lang="ts">\n' + addon)
          return replace(
            source,
            '<TaskPanel v-else embedded />',
            '<WorkflowAddon v-else mode="tasks"/>',
          )
        }
        if (path.endsWith('/components/execution/ConciergePanel.vue')) {
          source = replace(source, '<script setup lang="ts">', '<script setup lang="ts">\n' + addon)
          return replace(
            source,
            '<section class="concierge-panel" aria-label="Run concierge">',
            '<section class="concierge-panel" aria-label="Run concierge"><WorkflowAddon mode="supervision"/>',
          )
        }
        if (path.endsWith('/components/chat/ChatComposer.vue')) {
          source = replace(
            source,
            '<script setup lang="ts">',
            `<script setup lang="ts">\nimport {fixture,occupied,draftKey} from '@/previews/next/daily/ide-state'\n`,
          )
          source = replace(
            source,
            'const draft = computed({ get: readDraft, set: writeDraft })',
            "const draft = computed({get:()=>fixture.drafts[draftKey(props.concierge)]??'',set:(text:string)=>{fixture.drafts[draftKey(props.concierge)]=text}})",
          )
          source = replace(
            source,
            'function onKeydown(event: KeyboardEvent): void {',
            "function onKeydown(event: KeyboardEvent): void {\nif(event.isComposing || event.keyCode===229 || event.repeat) return\nif(!props.concierge && occupied.value && ['Enter','Tab'].includes(event.key) && !event.shiftKey){event.preventDefault();return}\n",
          )
          source = replace(
            source,
            "async function submit(mode: 'auto' | 'queue' | 'steer'): Promise<void> {",
            "async function submit(mode: 'auto' | 'queue' | 'steer'): Promise<void> {\nif(!props.concierge && occupied.value) return\n",
          )
          source = replace(
            source,
            ':disabled="!ready || !hasDraft || sendBlocked || submitting || readOnly"',
            ':disabled="!ready || !hasDraft || sendBlocked || submitting || readOnly || (!concierge && occupied)"',
          )
          return source
        }
        if (path.endsWith('/components/AppUpdateNotice.vue'))
          return source.replace(
            "if (!import.meta.env.PROD || !('serviceWorker' in navigator)) return",
            'return // No service worker in an isolated fixture',
          )
      },
    },
    vue(),
    tailwindcss(),
  ],
  resolve: {
    dedupe: ['@codemirror/state', '@codemirror/view', '@codemirror/language'],
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  build: { target: 'es2022' },
  preview: { host: '127.0.0.1', strictPort: true },
})
