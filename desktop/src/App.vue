<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox, ElNotification } from 'element-plus'
import { getCurrentWebview } from '@tauri-apps/api/webview'
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
const dragging = ref(false)
const countdown = ref<{ action: string; left: number } | null>(null)
let countdownTimer: number | undefined

async function importText(text: string) {
  if (!text.trim()) return
  app.view = 'parse'
  parse.text = text
  // 多条链接时由解析页显示“全部解析并下载”；单条直接解析
  const urls = text.match(/https?:\/\/\S+/g) ?? []
  if (urls.length === 1) parse.parse(text)
  else ElMessage.success(`已导入 ${urls.length} 条链接`)
}

function onHtmlDrop(e: DragEvent) {
  dragging.value = false
  const text = e.dataTransfer?.getData('text/uri-list') || e.dataTransfer?.getData('text/plain') || ''
  if (text) importText(text)
}

function cancelPower() {
  window.clearInterval(countdownTimer)
  countdown.value = null
  api.setAfterAllDone('none').catch(() => {})
  ElMessage.info('已取消')
}
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
  // 手机发送：新设备请求配对
  await events.onPairRequest(async (p) => {
    try {
      await ElMessageBox.confirm(`“${p.name}”（${p.ip}）想要向清影发送链接。允许后这台设备以后可以直接发送，可在“设置 → 手机发送”中撤销。`, '新设备请求配对', {
        confirmButtonText: '允许',
        cancelButtonText: '拒绝',
        type: 'info',
      })
      await api.phonePairRespond(p.deviceId, true)
      ElMessage.success('已配对')
    } catch (e) {
      if (e === 'cancel' || e === 'close') await api.phonePairRespond(p.deviceId, false).catch(() => {})
      else ElMessage.error(errorText(e))
    }
  })
  await events.onPhoneReceived((text) => {
    const url = text.match(/https?:\/\/\S+/)?.[0] ?? text
    ElNotification({ title: '收到手机发送的链接', message: url.slice(0, 80), type: 'info', duration: 3000 })
  })
  // 全部完成后睡眠 / 关机的倒计时
  await events.onPowerCountdown((p) => {
    window.clearInterval(countdownTimer)
    countdown.value = { action: p.action, left: p.seconds }
    countdownTimer = window.setInterval(() => {
      if (!countdown.value) return
      countdown.value.left--
      if (countdown.value.left <= 0) {
        window.clearInterval(countdownTimer)
        countdown.value = null
      }
    }, 1000)
  })
  // 拖入包含链接的文本文件
  await getCurrentWebview().onDragDropEvent(async (e) => {
    if (e.payload.type === 'over' || e.payload.type === 'enter') dragging.value = true
    else if (e.payload.type === 'leave') dragging.value = false
    else if (e.payload.type === 'drop') {
      dragging.value = false
      const texts: string[] = []
      for (const path of e.payload.paths.slice(0, 20)) {
        try {
          texts.push(await api.readLinksFile(path))
        } catch (err) {
          ElMessage.warning(`${path.split(/[\\/]/).pop()}：${errorText(err)}`)
        }
      }
      if (texts.length) importText(texts.join('\n'))
    }
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
  <div v-else-if="ready && app.settings" class="shell" @dragover.prevent="dragging = true" @dragleave.self="dragging = false" @drop.prevent="onHtmlDrop">
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
    <div v-if="dragging" class="dropzone">松开以导入链接（支持链接文本和 .txt 文件）</div>
    <el-dialog :model-value="!!countdown" :title="countdown?.action === 'shutdown' ? '即将关机' : '即将睡眠'" width="360px" :close-on-click-modal="false" :show-close="false">
      <p>全部下载已完成，将在 <b class="mono">{{ countdown?.left }}</b> 秒后{{ countdown?.action === 'shutdown' ? '关机' : '睡眠' }}。</p>
      <template #footer>
        <el-button type="primary" @click="cancelPower">取消</el-button>
      </template>
    </el-dialog>
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
.dropzone {
  position: fixed;
  inset: 12px;
  border: 2px dashed var(--cc-acc);
  border-radius: 12px;
  background: color-mix(in srgb, var(--cc-bg, #000) 70%, transparent);
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
  z-index: 3000;
  pointer-events: none;
}
.fatal {
  padding: 40px;
}
</style>
