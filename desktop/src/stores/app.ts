import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, errorKind, errorText, events } from '../api'
import type { AppInfo, ErrorKind, HistoryItem, MediaInfo, Settings, TaskSnapshot } from '../types'

export type ViewName = 'parse' | 'queue' | 'library' | 'settings'
export type SettingsTab = 'download' | 'parse' | 'network' | 'accounts' | 'phone' | 'components' | 'diagnostics' | 'general'

/** 设置、主题与当前页面。 */
export const useAppStore = defineStore('app', () => {
  const settings = ref<Settings | null>(null)
  const info = ref<AppInfo | null>(null)
  const view = ref<ViewName>('parse')
  const settingsTab = ref<SettingsTab>('download')
  const shortcutError = ref<string | null>(null)

  function goSettings(tab: SettingsTab) {
    settingsTab.value = tab
    view.value = 'settings'
  }

  async function load() {
    const [s, i] = await Promise.all([api.getSettings(), api.appInfo()])
    settings.value = s
    info.value = i
    shortcutError.value = i.shortcutError
    applyTheme()
  }

  async function save(next: Settings) {
    const r = await api.saveSettings(next)
    settings.value = r.settings
    shortcutError.value = r.shortcutError
    applyTheme()
  }

  async function patch(p: Partial<Settings>) {
    if (!settings.value) return
    await save({ ...settings.value, ...p })
  }

  const media = window.matchMedia('(prefers-color-scheme: dark)')
  function applyTheme() {
    const t = settings.value?.theme ?? 'system'
    const dark = t === 'dark' || (t === 'system' && media.matches)
    document.documentElement.classList.toggle('dark', dark)
  }
  media.addEventListener('change', applyTheme)

  return { settings, info, view, settingsTab, shortcutError, goSettings, load, save, patch, applyTheme }
})

/** 下载队列，监听后端事件保持同步。 */
export const useQueueStore = defineStore('queue', () => {
  const tasks = ref<TaskSnapshot[]>([])
  let started = false

  async function start() {
    if (started) return
    started = true
    tasks.value = await api.listTasks()
    await events.onTasks((list) => (tasks.value = list))
    await events.onProgress((t) => {
      const i = tasks.value.findIndex((x) => x.id === t.id)
      if (i >= 0) tasks.value[i] = t
    })
  }

  const counts = computed(() => {
    const c = { running: 0, queued: 0, done: 0, failed: 0, paused: 0 }
    for (const t of tasks.value) {
      if (t.status === 'running') c.running++
      else if (t.status === 'queued') c.queued++
      else if (t.status === 'done') c.done++
      else if (t.status === 'failed') c.failed++
      else if (t.status === 'paused') c.paused++
    }
    return c
  })
  const active = computed(() => counts.value.running + counts.value.queued)
  const totalSpeed = computed(() => tasks.value.reduce((sum, t) => sum + (t.status === 'running' ? t.speed : 0), 0))

  return { tasks, start, counts, active, totalSpeed }
})

/** 解析页的状态（App 级的剪贴板 / 快捷键事件也会写入这里）。 */
export const useParseStore = defineStore('parse', () => {
  const text = ref('')
  const loading = ref(false)
  const error = ref('')
  const errorKindRef = ref<ErrorKind>('other')
  const result = ref<MediaInfo | null>(null)
  const recent = ref<HistoryItem[]>([])

  async function parse(input?: string) {
    if (input !== undefined) text.value = input
    const t = text.value.trim()
    if (!t) {
      error.value = '请先粘贴作品的分享链接。'
      return
    }
    loading.value = true
    error.value = ''
    try {
      result.value = await api.resolve(t)
      await loadRecent()
    } catch (e) {
      result.value = null
      error.value = errorText(e)
      errorKindRef.value = errorKind(e)
    } finally {
      loading.value = false
    }
  }

  async function loadRecent() {
    recent.value = (await api.listHistory('')).slice(0, 5)
  }

  function clear() {
    text.value = ''
    error.value = ''
    result.value = null
  }

  return { text, loading, error, errorKind: errorKindRef, result, recent, parse, loadRecent, clear }
})
