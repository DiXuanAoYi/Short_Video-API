<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { api, errorText, events } from './api'
import { useAppStore, useParseStore, useQueueStore, type ViewName } from './stores/app'
import ParseView from './views/ParseView.vue'
import QueueView from './views/QueueView.vue'
import LibraryView from './views/LibraryView.vue'
import SettingsView from './views/SettingsView.vue'
import DisclaimerDialog from './components/DisclaimerDialog.vue'

const app = useAppStore()
const queue = useQueueStore()
const parse = useParseStore()
const ready = ref(false)
const loadError = ref('')

const navs: { id: ViewName; label: string }[] = [
  { id: 'parse', label: '解析' },
  { id: 'queue', label: '下载队列' },
  { id: 'library', label: '媒体库' },
  { id: 'settings', label: '设置' },
]

const footer = computed(() => {
  if (queue.active > 0) return `下载中 ${queue.counts.running} · 等待 ${queue.counts.queued}`
  return app.info ? `v${app.info.version}` : ''
})

onMounted(async () => {
  try {
    await app.load()
    await queue.start()
    await parse.loadRecent()
  } catch (e) {
    loadError.value = errorText(e)
  }
  ready.value = true

  // 主窗口在前台时复制到链接：直接填入并解析
  await events.onClipboardLink((p) => {
    if (app.settings?.autoDownload) return
    app.view = 'parse'
    parse.parse(p.text)
  })
  await events.onAutoResult((r) => {
    if (r.ok) ElMessage.success(r.message)
    else ElMessage.error(r.message)
  })
  // 全局快捷键 / 托盘“解析剪贴板”
  await events.onParseRequest((text) => {
    app.view = 'parse'
    if (text.trim()) parse.parse(text)
  })

  if (app.settings?.checkUpdate) {
    api
      .checkUpdate()
      .then((u) => {
        if (u.hasUpdate) {
          ElMessage({ type: 'info', message: `发现新版本 v${u.latest}，可在“设置 → 关于”中下载。`, duration: 6000 })
        }
      })
      .catch(() => {})
  }
})
</script>

<template>
  <div v-if="loadError" class="fatal">
    <h2>启动失败</h2>
    <p class="selectable">{{ loadError }}</p>
  </div>
  <div v-else-if="ready && app.settings" class="shell">
    <aside class="side">
      <div class="brand">清<i>影</i></div>
      <button
        v-for="n in navs"
        :key="n.id"
        class="nav"
        :class="{ on: app.view === n.id }"
        type="button"
        @click="app.view = n.id"
      >
        <span>{{ n.label }}</span>
        <em v-if="n.id === 'queue' && queue.active > 0">{{ queue.active }}</em>
      </button>
      <div class="foot">
        <div>{{ footer }}</div>
        <div v-if="app.settings.watchClipboard" class="watch">● 剪贴板监听中</div>
      </div>
    </aside>
    <main class="main">
      <ParseView v-show="app.view === 'parse'" />
      <QueueView v-if="app.view === 'queue'" />
      <LibraryView v-if="app.view === 'library'" />
      <SettingsView v-if="app.view === 'settings'" />
    </main>
    <DisclaimerDialog />
  </div>
</template>

<style scoped>
.shell {
  display: grid;
  grid-template-columns: 184px 1fr;
  height: 100%;
}
.side {
  background: var(--cc-side);
  border-right: 1px solid var(--cc-line);
  padding: 16px 10px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.brand {
  font-weight: 900;
  font-size: 18px;
  padding: 2px 12px 16px;
  letter-spacing: 0.06em;
}
.brand i {
  font-style: normal;
  color: var(--cc-acc);
}
.nav {
  all: unset;
  cursor: pointer;
  padding: 8px 12px;
  border-radius: 7px;
  color: var(--cc-mute);
  display: flex;
  justify-content: space-between;
  align-items: center;
  font-size: 13px;
}
.nav:hover {
  color: var(--cc-fg);
}
.nav.on {
  background: var(--cc-acc-soft);
  color: var(--cc-fg);
  font-weight: 600;
}
.nav em {
  font-style: normal;
  font-family: var(--cc-mono);
  font-size: 11px;
  background: var(--cc-acc);
  color: #1a1208;
  border-radius: 9px;
  padding: 0 7px;
}
.nav:focus-visible {
  outline: 2px solid var(--cc-acc);
}
.foot {
  margin-top: auto;
  font-size: 11px;
  color: var(--cc-mute);
  padding: 10px 12px 0;
  border-top: 1px solid var(--cc-line);
  display: flex;
  flex-direction: column;
  gap: 2px;
}
.watch {
  color: var(--cc-ok);
}
.main {
  min-width: 0;
  overflow: auto;
}
.fatal {
  padding: 40px;
}
</style>
