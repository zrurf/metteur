<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref } from 'vue'
import { PanelBottom, PanelRight, ShieldCheck, X } from '@lucide/vue'
const showDetails = ref(true), showSupervisor = ref(true)
const width = ref(420), height = ref(200)
const body = ref<HTMLElement>(), column = ref<HTMLElement>()
const bottom = ref('Node details'), right = ref('Chat')
defineExpose({ inspect: () => { bottom.value = 'Node details'; showDetails.value = true } })
let observer: ResizeObserver | undefined
let drag: { axis: 'width' | 'height'; start: number; value: number } | null = null
function clamp() {
  width.value = Math.max(240, Math.min(width.value, 600, (body.value?.clientWidth ?? 1000) - 360))
  height.value = Math.max(120, Math.min(height.value, 480, (column.value?.clientHeight ?? 720) - 280))
}
function start(event: PointerEvent, axis: 'width' | 'height') {
  if (event.button !== 0) return
  event.preventDefault()
  drag = { axis, start: axis === 'width' ? event.clientX : event.clientY, value: axis === 'width' ? width.value : height.value }
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
}
function move(event: PointerEvent) {
  if (!drag) return
  const target = drag.axis === 'width' ? width : height
  target.value = drag.value + drag.start - (drag.axis === 'width' ? event.clientX : event.clientY)
  clamp()
}
function key(event: KeyboardEvent, axis: 'width' | 'height') {
  const keys = axis === 'width' ? ['ArrowLeft', 'ArrowRight'] : ['ArrowUp', 'ArrowDown']
  if (![...keys, 'Home'].includes(event.key)) return
  event.preventDefault()
  const target = axis === 'width' ? width : height
  target.value = event.key === 'Home' ? (axis === 'width' ? 420 : 200) : target.value + (event.key === keys[0] ? 20 : -20)
  clamp()
}
onMounted(() => { observer = new ResizeObserver(clamp); if (body.value) observer.observe(body.value); if (column.value) observer.observe(column.value) })
onBeforeUnmount(() => observer?.disconnect())
</script>
<template>
  <div class="execution-layout">
    <header class="run-header"><slot name="header" /></header>
    <div ref="body" class="run-body">
      <section ref="column" class="main-column">
        <div class="graph-area">
          <div class="floating-toolbar" role="toolbar" aria-label="Blueprint view tools">
            <slot name="tools" />
            <button :aria-pressed="showDetails" aria-label="Node details panel" @click="showDetails = !showDetails"><PanelBottom :size="15" /></button>
            <button :aria-pressed="showSupervisor" aria-label="Supervision panel" @click="showSupervisor = !showSupervisor"><PanelRight :size="15" /></button>
          </div>
          <slot name="graph" />
        </div>
        <section v-if="showDetails" class="details-panel" :style="{ height: `${height}px` }">
          <div role="separator" aria-label="Resize details" aria-orientation="horizontal" :aria-valuenow="height" tabindex="0" class="details-resizer" @pointerdown="start($event, 'height')" @pointermove="move" @pointerup="drag = null" @lostpointercapture="drag = null" @keydown="key($event, 'height')" @dblclick="height = 200; clamp()" />
          <nav class="panel-tabs"><button v-for="tab in ['Node details', 'Blackboard', 'Execution log', 'Usage']" :key="tab" :class="{ active: bottom === tab }" @click="bottom = tab">{{ tab }}</button><button class="close-detail" aria-label="Close details" @click="showDetails = false"><X :size="13" /></button></nav>
          <div class="details-content"><slot name="details" :tab="bottom" /></div>
        </section>
      </section>
      <aside v-if="showSupervisor" class="supervisor" :style="{ width: `${width}px` }">
        <div role="separator" aria-label="Resize supervision" aria-orientation="vertical" :aria-valuenow="width" tabindex="0" class="supervisor-resizer" @pointerdown="start($event, 'width')" @pointermove="move" @pointerup="drag = null" @lostpointercapture="drag = null" @keydown="key($event, 'width')" @dblclick="width = 420; clamp()" />
        <header class="supervisor-heading"><ShieldCheck :size="16" /><strong>Supervision</strong><button aria-label="Close supervision" @click="showSupervisor = false"><X :size="14" /></button></header>
        <nav class="panel-tabs"><button v-for="tab in ['Chat', 'Requests', 'Reviews']" :key="tab" :class="{ active: right === tab }" @click="right = tab">{{ tab }}</button></nav>
        <slot name="supervision" :tab="right">
        <div class="unavailable">{{ right }} is not connected yet.</div>
        <div v-if="right === 'Chat'" class="supervision-composer"><div class="chat-composer"><textarea class="chat-composer-textarea" aria-label="Supervision message" placeholder="Concierge is not available yet" disabled /></div></div>
        </slot>
      </aside>
    </div>
  </div>
</template>
<style scoped>
.execution-layout{height:100%;display:flex;flex-direction:column;color:var(--foreground);background:var(--surface);font-size:12px;min-height:0}
.run-header{display:flex;align-items:center;gap:12px;min-height:49px;padding:8px 17px;border-bottom:1px solid var(--divider);flex-wrap:wrap}
.run-body{display:flex;flex:1;min-height:0}.main-column{flex:1;min-width:0;display:flex;flex-direction:column}.graph-area{position:relative;flex:1;min-height:280px;background:var(--background)}
.floating-toolbar{position:absolute;top:12px;right:13px;z-index:6;display:flex;gap:3px;padding:4px;background:var(--surface-glass);border:1px solid var(--border);border-radius:8px;backdrop-filter:blur(12px)}
.floating-toolbar :deep(button){display:grid;place-items:center;width:29px;height:28px;color:var(--muted-foreground);border-radius:5px}.floating-toolbar :deep([aria-pressed=true]){color:var(--primary);background:var(--primary-soft)}
.details-panel{position:relative;display:flex;flex-direction:column;flex-shrink:0;border-top:1px solid var(--border)}.details-content{flex:1;min-height:0;overflow:auto}.panel-tabs{display:flex;gap:19px;align-items:center;padding:0 17px;height:38px;border-bottom:1px solid var(--divider);flex-shrink:0}.panel-tabs>button{height:100%;white-space:nowrap;font-size:11px;color:var(--muted-foreground)}.panel-tabs>button.active{color:var(--foreground);border-bottom:2px solid var(--primary)}.close-detail{margin-left:auto}
.supervisor{position:relative;flex-shrink:0;display:flex;flex-direction:column;border-left:1px solid var(--border);background:var(--background)}.supervisor-heading{display:flex;align-items:center;gap:7px;height:46px;padding:0 17px}.supervisor-heading strong{font-size:15px;font-weight:600}.supervisor-heading button{margin-left:auto}.supervisor>.panel-tabs{height:42px;gap:24px}.supervisor>.panel-tabs button{font-size:13px}.unavailable{padding:24px 18px;color:var(--muted-foreground);font-size:15px;line-height:26px;flex:1}.supervision-composer{padding:12px 14px;border-top:1px solid var(--divider)}.chat-composer-textarea{font-size:15px;min-height:74px;width:100%}
.supervisor-resizer{position:absolute;left:-4px;top:0;bottom:0;width:8px;z-index:12;cursor:col-resize;touch-action:none}.details-resizer{position:absolute;top:-4px;left:0;right:0;height:8px;z-index:12;cursor:row-resize;touch-action:none}[role=separator]:hover,[role=separator]:focus-visible{background:var(--primary-soft)}button:focus-visible{outline:2px solid var(--ring)}button:hover{background:var(--hover)}
@media(max-width:760px){.run-body{flex-direction:column;overflow:auto}.main-column{min-height:570px;flex-shrink:0}.supervisor{width:100%!important;min-height:300px;border-left:0;border-top:1px solid var(--border)}.supervisor-resizer{display:none}.panel-tabs{gap:12px;overflow:auto}}
</style>
