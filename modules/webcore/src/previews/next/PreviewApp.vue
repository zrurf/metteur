<script setup lang="ts">
import { ref } from 'vue'
import registry from './registry.json'
import MemoryPreview from './MemoryPreview.vue'
import PromptPreview from './PromptPreview.vue'
import ContinuationPreview from './ContinuationPreview.vue'
import SchedulePreview from './SchedulePreview.vue'
import BlueprintHandoffPreview from './BlueprintHandoffPreview.vue'
import AddonPagePreview from './AddonPagePreview.vue'
import './preview.css'
const initial = new URLSearchParams(location.search).get('preview')
const page = ref(initial && ['PV-01','PV-02','PV-03','PV-04','PV-05','PV-06','PV-07'].includes(initial) ? initial : 'PV-03')
const scenario = ref('Ready')
const reset = ref(0)
const dark = ref(false)
function theme() { dark.value = !dark.value; document.documentElement.classList.toggle('dark', dark.value) }
</script>
<template>
  <div class="pv-shell"><header class="pv-top"><span class="pv-brand">M / METTEUR</span><span class="pv-badge">ISOLATED FIXTURE · {{page==='PV-03'?'r2':'r1'}}</span><button class="btn" :class="{ 'btn-primary': page === 'PV-01' }" @click="page = 'PV-01'; scenario = 'Ready'">Project memory</button><button class="btn" :class="{ 'btn-primary': page === 'PV-02' }" @click="page = 'PV-02'; scenario = 'Ready'">Prompt resources</button><button class="btn" :class="{ 'btn-primary': page === 'PV-03' }" @click="page='PV-03';scenario='Ready'">Task continuation</button><button class="btn" :class="{ 'btn-primary': page === 'PV-04' }" @click="page='PV-04';scenario='Ready'">Schedules</button><button class="btn" :class="{'btn-primary':page==='PV-05'}" @click="page='PV-05';scenario='Ready'">Blueprint handoff</button><button class="btn" :class="{'btn-primary':page==='PV-06'}" @click="page='PV-06';scenario='Ready'">Addon pages</button><button class="btn" :class="{'btn-primary':page==='PV-07'}" @click="page='PV-07';scenario='Ready'">Preview checks</button><button class="btn" @click="theme">{{ dark ? 'Light theme' : 'Dark theme' }}</button></header>
    <main class="pv-main"><div class="pv-between"><p class="pv-muted">No daemon, model API or user data connected. User approval pending.</p><label class="pv-muted">Scenario <select v-model="scenario" aria-label="Scenario" class="input"><option v-for="item in ['Ready','Loading','Empty','Error','Disconnected','Read only','Permission denied','Conflict','Changed version','Missing source','Waiting','Paused','Stopped','Needs attention','Budget exhausted']" :key="item">{{ item }}</option></select></label><button class="btn" @click="reset++; scenario = 'Ready'">Reset fixture</button></div><MemoryPreview v-if="page === 'PV-01'" :key="`memory-${reset}`" :scenario="scenario" /><PromptPreview v-else-if="page === 'PV-02'" :key="`prompts-${reset}`" :scenario="scenario" /><ContinuationPreview v-else-if="page === 'PV-03'" :key="`task-${reset}`" :scenario="scenario" /><SchedulePreview v-else-if="page==='PV-04'" :key="`schedule-${reset}`" :scenario="scenario" /><AddonPagePreview v-else-if="page==='PV-06'||page==='PV-07'" :key="`${page}-${reset}`" :scenario="scenario" :inspection="page==='PV-07'"/><BlueprintHandoffPreview v-else :key="`handoff-${reset}`" :scenario="scenario"/></main>
    <footer class="pv-footer">{{ page }} · revision {{page==='PV-03'?2:1}} · {{ page==='PV-06'||page==='PV-07' ? 'nx-m5-fixture-1' : page==='PV-05' ? 'nx-m3-fixture-1' : page === 'PV-03' || page === 'PV-04' ? 'nx-m2-fixture-1' : registry.fixtureVersion }} · Source and bundle digests accompany the review record. Fixture actions never constitute product approval. PV-03/r2 and PV-04/r1 are provisionally approved. PV-05/r1 is provisionally approved; final page review remains required.</footer>
  </div>
</template>
