<script setup lang="ts">
import TaskPanel from '@/components/continuations/TaskPanel.vue'
import SchedulePanel from '@/components/continuations/SchedulePanel.vue'
import PageHeader from '@/components/PageHeader.vue'
import PageTabs from '@/components/PageTabs.vue'
import { computed, ref } from 'vue'
import { Plus, RefreshCw } from '@lucide/vue'
import { useRoute, useRouter } from 'vue-router'
import { useTabsStore } from '@/stores/tabs'
import { useCodingWorkflowsStore } from '@/stores/coding-workflows'
import { useWorkspaceStore } from '@/stores/workspace'
import WorkflowWorkbench from '@/components/workflows/WorkflowWorkbench.vue'
const workflows=useCodingWorkflowsStore(),workspace=useWorkspaceStore()
const route = useRoute(),
  router = useRouter()
const schedules = computed(() => route.query.view === 'schedules')
const taskPanel = ref<InstanceType<typeof TaskPanel> | null>(null)
const schedulePanel = ref<InstanceType<typeof SchedulePanel> | null>(null)
function select(view: string) {
  void router.replace({ query: view === 'schedules' ? { view } : {} })
}
useTabsStore().openSurface('tasks')
</script>
<template>
  <WorkflowWorkbench v-if="workflows.current.mode" />
  <div v-else class="surface-page">
    <PageHeader title="Tasks">
      <template v-if="!schedules">
        <button class="btn" @click="workspace.active && workflows.openPlan(workspace.active.path)">Start from template</button>
        <button
          class="btn btn-primary"
          :disabled="!taskPanel?.available || taskPanel.launching"
          @click="taskPanel?.start()"
        >
          <Plus :size="15" />New task
        </button>
        <button
          class="btn btn-outline"
          :disabled="!taskPanel?.available"
          aria-label="Refresh tasks"
          @click="taskPanel?.refresh()"
        >
          <RefreshCw :size="16" />
        </button>
      </template>
      <button
        v-else-if="schedulePanel?.eligible"
        class="btn btn-primary"
        :disabled="!schedulePanel.available"
        @click="schedulePanel.open()"
      >
        <Plus :size="16" />New schedule
      </button>
    </PageHeader>
    <PageTabs
      :model-value="schedules ? 'schedules' : 'tasks'"
      label="Tasks"
      :items="[
        { value: 'tasks', label: 'Tasks' },
        { value: 'schedules', label: 'Schedules' },
      ]"
      @update:model-value="select"
    />
    <SchedulePanel v-if="schedules" ref="schedulePanel" embedded />
    <TaskPanel v-else ref="taskPanel" embedded />
  </div>
</template>
