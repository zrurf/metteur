<script setup lang="ts">
import { Check, ChevronDown, GitBranch, ShieldCheck } from '@lucide/vue'
import { ref } from 'vue'

defineProps<{ state: 'pending' | 'approved' | 'applied' | 'denied'; readonly?: boolean }>()
const emit = defineEmits<{ approve: []; reject: []; inspect: [] }>()
const expanded = ref(false)
</script>

<template>
  <section class="review-proposal">
    <div class="proposal-heading"><ShieldCheck :size="14" /><strong>{{ state === 'pending' ? 'Review proposed change' : state === 'approved' ? 'Approved · Waiting to apply' : state === 'applied' ? 'Change applied' : 'Change rejected' }}</strong><span>P-001</span></div>
    <h3>Add expired-session coverage</h3>
    <p>Extend the validation checks to verify the redirect when credentials expire.</p>
    <button class="affected-node" @click="emit('inspect')"><GitBranch :size="13" />Validate<span>1 parameter</span></button>
    <button class="change-toggle" :aria-expanded="expanded" @click="expanded = !expanded"><ChevronDown :size="14" :class="{ rotated: expanded }" />{{ expanded ? 'Hide change details' : 'View change details' }}</button>
    <div v-if="expanded" class="change-details">
      <dl><div><dt>Base version</dt><dd>a31c9f2</dd></div><div><dt>Source</dt><dd>User request R-001</dd></div></dl>
      <div class="change-code"><span>Validate / checks</span><p>  Restore session after refresh</p><p class="added">+ Redirect when session expires</p></div>
      <p>No changes to the API, permissions, or completed nodes.</p>
    </div>
    <div v-if="state === 'pending' && !readonly" class="proposal-actions"><button @click="emit('reject')">Reject</button><button class="approve" @click="emit('approve')"><Check :size="14" />Approve change</button></div>
    <p v-else class="decision"><Check v-if="state === 'applied'" :size="14" />{{ readonly && (state === 'pending' || state === 'approved') ? 'No change was applied. This run is not accepting actions.' : state === 'approved' ? 'Approved, not applied yet. Advance to the next node to preview a safe boundary.' : state === 'applied' ? 'Applied at a safe boundary. Active version: b74e210.' : state === 'denied' ? 'Rejected. The original plan is unchanged.' : 'This run is closed. The proposal was not applied.' }}</p>
  </section>
</template>

<style scoped>
.review-proposal { border:1px solid color-mix(in srgb,var(--primary) 22%,var(--border)); border-radius:12px; padding:16px; background:linear-gradient(135deg,var(--primary-soft),var(--surface) 45%); font-size:13px; }
.proposal-heading { display:flex; gap:7px; align-items:center; color:var(--muted-foreground); font-size:12px; }
.proposal-heading strong { font-weight:500; color:var(--foreground); }
.proposal-heading>span { margin-left:auto; font-size:11px; white-space:nowrap; }
h3 { font-size:15px; font-weight:600; margin:14px 0 7px; line-height:1.5; }
p { font-size:13px; color:var(--muted-foreground); line-height:1.7; margin:0; }
button { cursor:pointer; display:flex; align-items:center; gap:7px; border-radius:6px; }
button:hover { background:var(--hover); }
button:focus-visible { outline:2px solid var(--ring); outline-offset:2px; }
.affected-node { width:100%; margin:12px 0 4px; padding:8px 10px; background:var(--surface-muted); color:var(--foreground); text-align:left; }
.affected-node span { margin-left:auto; color:var(--muted-foreground); font-size:12px; }
.change-toggle { padding:8px 0; color:var(--muted-foreground); font-size:12px; }
.change-toggle svg { transform:rotate(-90deg); }.change-toggle .rotated { transform:none; }
.change-details { margin-top:7px; padding-top:10px; border-top:1px solid var(--divider); }
dl { margin:0 0 12px; font-size:12px; } dl>div { display:flex; gap:12px; justify-content:space-between; margin:6px 0; } dt { color:var(--muted-foreground); }
.change-code { border:1px solid var(--divider); border-radius:6px; background:var(--surface-muted); padding:9px; overflow:auto; margin-bottom:10px; font-family:ui-monospace,Consolas,monospace; }
.change-code>span { color:var(--muted-foreground); font-size:11px; }.change-code p { font-size:11px; white-space:pre; }.change-code .added { color:var(--status-done, #229d77); }
.change-details>p { font-size:12px; }.proposal-actions { display:flex; gap:8px; justify-content:flex-end; margin-top:13px; }
.proposal-actions button { padding:7px 11px; border:1px solid var(--border); font-size:12px; }.proposal-actions .approve { background:var(--chat-action-bg, var(--foreground)); color:var(--chat-action-fg, var(--surface)); border-color:transparent; }
.decision { display:flex; gap:6px; align-items:flex-start; margin-top:12px; font-size:12px; }.decision svg { flex-shrink:0; margin-top:3px; }
</style>
