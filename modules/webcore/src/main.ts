import { createApp } from 'vue'
import { createPinia } from 'pinia'
import './styles/main.css'
import App from './App.vue'
import { router } from './router'
import { installTooltips } from './lib/tooltip'

// Replaces the native `title` bubble across the app (see lib/tooltip.ts).
installTooltips()

const app = createApp(App)

app.use(createPinia())
app.use(router)

// Resolve the initial route (including its pending workspace target) before
// App.onMounted restores the workspace. Otherwise fast restore can race it.
void router.isReady().then(() => app.mount('#app'))
