import { createApp } from 'vue'
import { createPinia } from 'pinia'
import ElementPlus from 'element-plus'
import zhCn from 'element-plus/es/locale/lang/zh-cn'
import 'element-plus/dist/index.css'
import 'element-plus/theme-chalk/dark/css-vars.css'
import './styles/theme.css'
import App from './App.vue'
import MiniApp from './MiniApp.vue'

// 迷你窗与主窗口共用同一份前端，按 hash 区分。
const root = location.hash === '#mini' ? MiniApp : App

createApp(root).use(createPinia()).use(ElementPlus, { locale: zhCn }).mount('#app')
