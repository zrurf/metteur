import { createApp } from 'vue'
import '@/styles/main.css'
import { installTooltips } from '@/lib/tooltip'
import UsabilityPreview from './UsabilityPreview.vue'
installTooltips()
createApp(UsabilityPreview).mount('#app')
