<script setup lang="ts">
import { computed } from 'vue'
import { Copy, FileCode, History, Pencil } from '@lucide/vue'

/**
 * One user turn.
 *
 * Kept to a quiet block rather than a saturated bubble: the user's words matter,
 * but the answer is the subject of the page. File references render as chips
 * that open the file, and a turn typed while the agent works carries a "queued"
 * marker until the engine injects it.
 */
const props = defineProps<{
  readOnly?: boolean
  content: string
  createdAt: number
  queued: boolean
  /** Snapshot taken before this turn, when versioning is available. */
  checkpoint?: string
  onOpenFile: (path: string) => void
  onCopy: (text: string) => void
  onEdit: (text: string) => void
  onRestore: (snapshotId: string) => void
}>()

/** Splits the turn into `@file` chips and the text between them. */
const parts = computed(() => {
  const out: Array<{ kind: 'text'; text: string } | { kind: 'file'; path: string }> = []
  let text = ''
  for (const line of props.content.split('\n')) {
    const match = /^@file\s+(.+)$/.exec(line)
    if (match) {
      if (text) {
        out.push({ kind: 'text', text })
        text = ''
      }
      out.push({ kind: 'file', path: match[1].trim() })
    } else {
      text = text ? `${text}\n${line}` : line
    }
  }
  if (text) out.push({ kind: 'text', text })
  return out
})

const time = computed(() =>
  new Date(props.createdAt).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }),
)
</script>

<template>
  <div class="chat-turn group/turn flex flex-col items-end gap-1">
    <div class="chat-user-turn max-w-[85%] text-[15px] leading-[26px]">
      <template v-for="(part, i) in parts" :key="i">
        <button
          v-if="part.kind === 'file' && !readOnly"
          class="mr-1 inline-flex items-center gap-1 rounded-md border border-divider bg-background px-1.5 py-0.5 font-mono text-[12.5px] text-foreground/80 transition-colors duration-100 hover:bg-hover"
          type="button"
          :title="part.path"
          @click="onOpenFile(part.path)"
        >
          <FileCode class="h-3 w-3 shrink-0" /> {{ part.path }}
        </button>
        <template v-else>{{ part.kind === 'file' ? part.path : part.text }}</template>
      </template>
      <span v-if="queued" class="chat-queued-mark">queued</span>
    </div>
    <div class="chat-turn-meta">
      <span class="tabular-nums">{{ time }}</span>
      <button
        class="chat-action !h-5 !px-1"
        type="button"
        title="Copy"
        aria-label="Copy message"
        @click="onCopy(content)"
      >
        <Copy class="h-3 w-3" />
      </button>
      <button
        class="chat-action !h-5 !px-1"
        type="button"
        v-if="!readOnly"
        title="Edit and send again"
        aria-label="Edit message"
        @click="onEdit(content)"
      >
        <Pencil class="h-3 w-3" />
      </button>
      <button
        v-if="checkpoint && !readOnly"
        class="chat-action !h-5 !px-1"
        type="button"
        title="Restore files and conversation to this turn's start"
        aria-label="Restore files and conversation to before this turn"
        @click="onRestore(checkpoint)"
      >
        <History class="h-3 w-3" />
      </button>
    </div>
  </div>
</template>
