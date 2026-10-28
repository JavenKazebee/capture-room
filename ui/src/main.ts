import { createApp } from 'vue'
import { createPinia } from 'pinia'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import 'vue-sonner/style.css'
import './style.css'
import App from './App.vue'
import { router } from '@/router'

// Firefox and Chromium on Linux render unhinted variable Inter thin; style.css adds weight there.
if (/linux/i.test(navigator.userAgent) && !/android/i.test(navigator.userAgent)) {
  document.documentElement.dataset.platform = 'linux'
}

createApp(App).use(createPinia()).use(router).mount('#app')
