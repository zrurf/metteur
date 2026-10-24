<script setup lang="ts">
import App from '@/App.vue'
import { useRouter } from 'vue-router'
import { useTabsStore } from '@/stores/tabs'
import { usePanelStore } from '@/stores/panel'
import FileTree from '@/components/FileTree.vue'
import { fileRoute } from '@/lib/file-token'
import { wurl } from '@/lib/workspace-url'
import { fixture } from './ide-state'
import { vDialogFocus } from '../dialog-focus'
import WorkflowAddon from './WorkflowAddon.vue'
const router = useRouter(),
  tabs = useTabsStore(),
  panel = usePanelStore()
async function select(id: string) {
  fixture.preview = id
  if (id === 'WF-PV-02') {
    const path = 'metrics/README.md'
    tabs.openFile(path)
    panel.show('explorer', FileTree, 'Explorer')
    await router.push(fileRoute(path))
  } else {
    const surface = id === 'WF-PV-03' ? 'tasks' : 'chat'
    tabs.openSurface(surface)
    await router.push(wurl(`/${surface}`))
  }
}
defineExpose({ select })
</script>
<template>
  <App />
  <details class="fixture-controls">
    <summary>Isolated Fixture · r3</summary>
    <div class="fixture-toolbar">
      <label
        >Preview<select
          :value="fixture.preview"
          aria-label="Preview Group"
          @change="select(($event.target as HTMLSelectElement).value)"
        >
          <option>WF-PV-01</option>
          <option>WF-PV-02</option>
          <option>WF-PV-03</option>
        </select></label
      ><label
        >State<select v-model="fixture.state" aria-label="Fixture State">
          <option
            v-for="s in [
              'Running',
              'Starting',
              'Paused',
              'Waiting Approval',
              'Completed',
              'Failed',
              'Stopped',
              'Disconnected',
              'Missing Model',
              'Permission Denied',
              'Unknown Effect',
              'Budget Exhausted',
              'Policy Expired',
              'Permission Revoked',
              'Missing Source',
              'Stale Version',
            ]"
            :key="s"
          >
            {{ s }}
          </option>
        </select></label
      ><span>No daemon, user data or model connected</span>
    </div>
  </details>
  <div v-if="fixture.notice" class="fixture-notice" role="status">
    {{ fixture.notice }}<button class="btn" @click="fixture.notice = ''">Dismiss</button>
  </div>
  <div
    v-if="fixture.dialog"
    class="fixed inset-0 z-50 grid place-items-center p-5"
    style="background: var(--overlay)"
    @keydown.esc="fixture.dialog = ''"
    @mousedown.self="fixture.dialog = ''"
  >
    <section
      v-dialog-focus
      role="dialog"
      aria-modal="true"
      :aria-label="fixture.dialog"
      class="fixture-dialog"
    >
      <WorkflowAddon mode="dialog" />
    </section>
  </div>
</template>
<style>
.fixture-controls {
  position: fixed;
  bottom: 2px;
  right: 8px;
  z-index: 45;
  max-width: calc(100vw - 20px);
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--popover);
  color: var(--foreground);
  font-size: 13px;
  padding: 4px 8px;
  box-shadow: var(--shadow-popover);
}
.fixture-controls summary {
  cursor: pointer;
  color: var(--primary);
}
.fixture-toolbar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px;
  padding: 6px 0;
}
.fixture-toolbar label {
  display: flex;
  gap: 5px;
  align-items: center;
}
.fixture-toolbar select {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 3px;
}
.fixture-toolbar span {
  color: var(--muted-foreground);
}
.fixture-notice {
  position: fixed;
  bottom: 85px;
  right: 16px;
  z-index: 60;
  background: var(--popover);
  color: var(--foreground);
  max-width: min(560px, calc(100vw - 32px));
  padding: 12px 16px;
  border: 1px solid var(--border);
  border-radius: 8px;
  box-shadow: var(--shadow-popover);
  font-size: 14px;
}
.fixture-dialog {
  width: min(780px, 100%);
  max-height: 85vh;
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: 10px;
  background: var(--popover);
  padding: 24px;
}
.wf-addon {
  font-size: 14px;
  line-height: 1.7;
}
.wf-run-card {
  border: 1px solid var(--border);
  border-radius: 7px;
  padding: 12px 14px;
  margin: 12px 0;
  background: var(--surface);
}
.wf-run-card > header {
  display: flex;
  align-items: center;
  gap: 9px;
  flex-wrap: wrap;
}
.wf-run-card p {
  color: var(--muted-foreground);
  margin: 7px 0;
}
.wf-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 10px;
}
.wf-tag {
  font-size: 13px;
  color: var(--muted-foreground);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 2px 6px;
}
.wf-compare {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 12px;
  margin: 18px 0;
}
.wf-compare > div {
  padding: 12px;
  border: 1px solid var(--border);
  border-radius: 5px;
  background: var(--surface);
}
.wf-compare pre {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-size: 13px;
  line-height: 1.8;
}
.wf-results {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
  margin: 20px 0;
}
.wf-results > article {
  padding: 14px;
  border: 1px solid var(--border);
  border-radius: 6px;
}
.wf-addon h2 {
  font-size: 17px;
  font-weight: 600;
  margin: 10px 0;
}
.wf-addon h3 {
  font-size: 14px;
  font-weight: 550;
  margin: 6px 0;
}
.wf-source-link {
  display: block;
  text-align: left;
  color: var(--primary);
  margin: 8px 0;
}
.wf-addon .btn:disabled {
  opacity: 0.4;
}
.wf-addon textarea {
  width: 100%;
  min-height: 100px;
  border: 1px solid var(--border);
  background: var(--surface);
  padding: 10px;
}
.wf-addon .btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 14px;
}
.wf-addon details {
  margin: 10px 0;
}
.wf-addon summary {
  cursor: pointer;
  color: var(--muted-foreground);
}
.wf-options {
  position: relative;
  margin: 0 !important;
}
.wf-options-menu {
  position: absolute;
  top: 100%;
  left: 0;
  z-index: 30;
  display: flex;
  flex-direction: column;
  align-items: stretch;
  gap: 8px;
  min-width: 180px;
  padding: 12px;
  background: var(--popover);
  border: 1px solid var(--border);
  border-radius: 7px;
  box-shadow: var(--shadow-popover);
}
@media (max-width: 650px) {
  .wf-compare {
    grid-template-columns: 1fr;
  }
  .wf-results {
    grid-template-columns: 1fr;
  }
  .fixture-toolbar > span {
    display: none;
  }
  .fixture-dialog {
    padding: 16px;
  }
}
@media (prefers-reduced-motion: reduce) {
  .fixture-controls *,
  .wf-addon * {
    animation: none !important;
    transition: none !important;
  }
}
</style>
