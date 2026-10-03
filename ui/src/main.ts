import { createApp } from 'vue'
import { createPinia } from 'pinia'
import '@fontsource-variable/inter'
import '@fontsource-variable/jetbrains-mono'
import 'vue-sonner/style.css'
import './style.css'
import App from './App.vue'
import { router } from '@/router'

createApp(App).use(createPinia()).use(router).mount('#app')
