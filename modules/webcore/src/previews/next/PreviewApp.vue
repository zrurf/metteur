<script setup lang="ts">
import { ref } from 'vue'
import registry from './registry.json'
import MemoryPreview from './MemoryPreview.vue'
import PromptPreview from './PromptPreview.vue'
import './preview.css'
const page = ref(new URLSearchParams(location.search).get('preview') === 'PV-02' ? 'PV-02' : 'PV-01')
const scenario = ref('Ready')
const reset = ref(0)
const dark = ref(false)
function theme() { dark.value = !dark.value; document.documentElement.classList.toggle('dark', dark.value) }
</script>
<template>
  <div class="pv-shell"><header class="pv-top"><span class="pv-brand">M / METTEUR</span><span class="pv-badge">ISOLATED FIXTURE · r1</span><button class="btn" :class="{ 'btn-primary': page === 'PV-01' }" @click="page = 'PV-01'; scenario = 'Ready'">Project memory</button><button class="btn" :class="{ 'btn-primary': page === 'PV-02' }" @click="page = 'PV-02'; scenario = 'Ready'">Prompt resources</button><button class="btn" @click="theme">{{ dark ? 'Light theme' : 'Dark theme' }}</button></header>
    <main class="pv-main"><div class="pv-between"><p class="pv-muted">No daemon, model API or user data connected. User approval pending.</p><label class="pv-muted">Scenario <select v-model="scenario" aria-label="Scenario" class="input"><option v-for="item in ['Ready','Loading','Empty','Error','Disconnected','Read only','Permission denied','Conflict','Changed version','Missing source']" :key="item">{{ item }}</option></select></label><button class="btn" @click="reset++; scenario = 'Ready'">Reset fixture</button></div><MemoryPreview v-if="page === 'PV-01'" :key="`memory-${reset}`" :scenario="scenario" /><PromptPreview v-else :key="`prompts-${reset}`" :scenario="scenario" /></main>
    <footer class="pv-footer">{{ page }} · revision 1 · {{ registry.fixtureVersion }} · Source and bundle digests accompany the review record. Fixture actions never constitute product approval. PV-03–PV-07 are outside this stage.</footer>
  </div>
</template>
