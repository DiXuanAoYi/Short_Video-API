<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { convertFileSrc } from '@tauri-apps/api/core'
import { api, errorText } from '../api'
import VirtualList from '../components/VirtualList.vue'
import InboxPanel from '../components/InboxPanel.vue'
import { useAppStore, useParseStore, useQueueStore } from '../stores/app'
import type { HistoryItem, LibraryItem, PlatformCount } from '../types'
import { formatBytes, formatDateTime } from '../utils/format'

const app = useAppStore()
const parse = useParseStore()
const queue = useQueueStore()

const tab = computed({ get: () => app.libraryTab, set: (v) => (app.libraryTab = v) })
const query = ref('')
const files = ref<LibraryItem[]>([])
const history = ref<HistoryItem[]>([])
const loading = ref(false)
const platforms = ref<PlatformCount[]>([])
const fPlatform = ref('')
const fKind = ref('')
const fSince = ref('')
const fMissing = ref(false)
const mode = ref<'list' | 'grid'>('list')
const COLS = 5

const gridRows = computed(() => {
  const rows: { key: number; items: LibraryItem[] }[] = []
  for (let i = 0; i < files.value.length; i += COLS) rows.push({ key: files.value[i].id, items: files.value.slice(i, i + COLS) })
  return rows
})

function coverOf(f: LibraryItem): string | null {
  if (f.coverPath) return convertFileSrc(f.coverPath)
  return f.cover
}

function sinceValue(): number | null {
  const day = 86400
  const now = Math.floor(Date.now() / 1000)
  if (fSince.value === 'today') {
    const d = new Date()
    d.setHours(0, 0, 0, 0)
    return Math.floor(d.getTime() / 1000)
  }
  if (fSince.value === '7d') return now - 7 * day
  if (fSince.value === '30d') return now - 30 * day
  return null
}

async function redownload(f: LibraryItem) {
  try {
    const r = await api.redownload(f.id)
    if (r.tasks.length) ElMessage.success('已重新加入下载队列')
    else ElMessage.info('已在队列中')
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

const platformName = computed<Record<string, string>>(() => Object.fromEntries((app.info?.providers ?? []).map((p) => [p.id, p.name])))

const KIND_NAME: Record<string, string> = { video: '视频', image: '图片', audio: '音频', cover: '封面', subtitle: '字幕' }

function assetName(f: LibraryItem) {
  const id = f.assetId
  if (id.startsWith('image-')) return `图片 ${Number(id.slice(6)) + 1}`
  return ({ video: '视频', music: '背景音乐', cover: '封面' } as Record<string, string>)[id] ?? KIND_NAME[f.kind] ?? id
}

async function load() {
  loading.value = true
  try {
    if (tab.value === 'inbox') return
    if (tab.value === 'files') {
      files.value = await api.listLibrary({ query: query.value, platform: fPlatform.value || null, kind: fKind.value || null, since: sinceValue(), missingOnly: fMissing.value })
      platforms.value = await api.libraryPlatforms()
    }
    else history.value = await api.listHistory(query.value)
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    loading.value = false
  }
}

let timer: number | undefined
watch(query, () => {
  window.clearTimeout(timer)
  timer = window.setTimeout(load, 250)
})
watch([tab, fPlatform, fKind, fSince, fMissing], load)
// 有任务完成时刷新媒体库
watch(
  () => queue.counts.done,
  () => tab.value === 'files' && load(),
)
onMounted(load)

async function run<A extends unknown[]>(fn: (...args: A) => Promise<unknown>, ...args: A) {
  try {
    await fn(...args)
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

async function removeFile(item: LibraryItem) {
  try {
    await ElMessageBox.confirm(`删除“${item.title}”的记录。是否同时删除磁盘上的文件？`, '删除记录', {
      confirmButtonText: '删除记录和文件',
      cancelButtonText: '只删除记录',
      distinguishCancelAndClose: true,
      type: 'warning',
    })
    await run(api.deleteLibrary, item.id, true)
  } catch (action) {
    if (action === 'cancel') await run(api.deleteLibrary, item.id, false)
    else return
  }
  await load()
}

function reopen(item: HistoryItem) {
  parse.result = item.info
  parse.text = item.sourceUrl
  parse.error = ''
  app.view = 'parse'
}

function reparse(item: HistoryItem) {
  app.view = 'parse'
  parse.parse(item.sourceUrl)
}

async function clearHistory() {
  try {
    await ElMessageBox.confirm('清空全部解析历史？已下载的文件不受影响。', '清空历史', { type: 'warning', confirmButtonText: '清空', cancelButtonText: '取消' })
  } catch {
    return
  }
  await run(api.clearHistory)
  await load()
  await parse.loadRecent()
}
</script>

<template>
  <div class="page">
    <div class="head">
      <el-radio-group v-model="tab" size="small">
        <el-radio-button value="files">已下载</el-radio-button>
        <el-radio-button value="inbox">收到的链接<template v-if="app.inboxUnhandled"> ({{ app.inboxUnhandled }})</template></el-radio-button>
        <el-radio-button value="history">解析历史</el-radio-button>
      </el-radio-group>
      <el-input v-model="query" size="small" clearable :placeholder="tab === 'inbox' ? '搜索链接、标题或设备' : '搜索标题或作者'" class="search" />
      <el-button v-if="tab === 'files' && app.settings" size="small" @click="run(api.revealFile, app.settings!.downloadDir)">打开下载目录</el-button>
      <el-radio-group v-if="tab === 'files'" v-model="mode" size="small">
        <el-radio-button value="list">列表</el-radio-button>
        <el-radio-button value="grid">网格</el-radio-button>
      </el-radio-group>
      <el-button v-if="tab === 'history' && history.length" size="small" @click="clearHistory">清空历史</el-button>
    </div>

    <div v-if="tab === 'files'" class="filters">
      <el-select v-model="fPlatform" size="small" clearable placeholder="全部平台" class="fsel">
        <el-option v-for="p in platforms" :key="p.platform" :value="p.platform" :label="`${platformName[p.platform] ?? p.name}（${p.count}）`" />
      </el-select>
      <el-select v-model="fKind" size="small" clearable placeholder="全部类型" class="fsel">
        <el-option value="video" label="视频" />
        <el-option value="image" label="图片" />
        <el-option value="audio" label="音频" />
        <el-option value="cover" label="封面" />
        <el-option value="subtitle" label="字幕 / 弹幕" />
      </el-select>
      <el-select v-model="fSince" size="small" clearable placeholder="全部时间" class="fsel">
        <el-option value="today" label="今天" />
        <el-option value="7d" label="最近 7 天" />
        <el-option value="30d" label="最近 30 天" />
      </el-select>
      <el-checkbox v-model="fMissing" size="small">只看文件已丢失的</el-checkbox>
      <span class="mute count">{{ files.length }} 项</span>
    </div>

    <InboxPanel v-if="tab === 'inbox'" :query="query" />
    <div v-else v-loading="loading" class="list">
      <template v-if="tab === 'files'">
        <div v-if="!files.length" class="empty mute">{{ query || fPlatform || fKind || fSince || fMissing ? '没有匹配的文件。' : '还没有下载过文件。' }}</div>
        <VirtualList v-else-if="mode === 'grid'" :items="gridRows" :item-height="190" :gap="10" :item-key="(r) => r.key" class="vl">
          <template #default="{ item: r }">
            <div class="grid-row">
              <div v-for="f in r.items" :key="f.id" class="tile card" :class="{ missing: !f.exists }" :title="f.title" @dblclick="f.exists && run(api.openFile, f.path)">
                <img v-if="coverOf(f)" :src="coverOf(f)!" referrerpolicy="no-referrer" loading="lazy" alt="" />
                <div v-else class="noimg" />
                <div class="tname ellipsis">{{ f.title }}</div>
                <small class="mute ellipsis">{{ platformName[f.platform] ?? f.platformName ?? f.platform }}<template v-if="!f.exists"> · 已丢失</template></small>
              </div>
            </div>
          </template>
        </VirtualList>
        <VirtualList v-else :items="files" :item-height="58" :gap="8" :item-key="(f) => f.id" class="vl">
          <template #default="{ item: f }">
        <div class="item card" :class="{ missing: !f.exists }">
          <img v-if="coverOf(f)" :src="coverOf(f)!" class="thumb" referrerpolicy="no-referrer" loading="lazy" alt="" />
          <div v-else class="thumb" />
          <div class="info">
            <div class="ellipsis" :title="f.title">{{ f.title }}</div>
            <small class="mono mute ellipsis selectable" :title="f.path">
              {{ platformName[f.platform] ?? (f.platformName || f.platform) }} · {{ assetName(f) }} · {{ formatBytes(f.size) }} · {{ formatDateTime(f.finishedAt) }}
              <template v-if="!f.exists"> · 文件已不存在</template>
            </small>
          </div>
          <div class="actions">
            <el-button link size="small" type="primary" :disabled="!f.exists" @click="run(api.openFile, f.path)">打开</el-button>
            <el-button link size="small" :disabled="!f.exists" @click="run(api.revealFile, f.path)">文件夹</el-button>
            <el-button v-if="!f.exists && f.sourceUrl" link size="small" type="primary" @click="redownload(f)">重新下载</el-button>
            <el-button link size="small" @click="removeFile(f)">删除</el-button>
          </div>
        </div>
          </template>
        </VirtualList>
      </template>

      <template v-else>
        <div v-if="!history.length" class="empty mute">{{ query ? '没有匹配的记录。' : '还没有解析记录。' }}</div>
        <div v-for="h in history" :key="h.id" class="item card">
          <img v-if="h.cover" :src="h.cover" class="thumb" referrerpolicy="no-referrer" alt="" />
          <div v-else class="thumb" />
          <div class="info">
            <div class="ellipsis" :title="h.title">{{ h.title }}</div>
            <small class="mono mute ellipsis">
              {{ h.info.platformName }} · {{ h.kind === 'video' ? '视频' : '图集' }}<template v-if="h.author"> · @{{ h.author }}</template> · {{ formatDateTime(h.createdAt) }}
            </small>
          </div>
          <div class="actions">
            <el-button link size="small" type="primary" @click="reopen(h)">查看</el-button>
            <el-button link size="small" @click="reparse(h)">重新解析</el-button>
            <el-button link size="small" @click="run(api.deleteHistory, h.id).then(load)">删除</el-button>
          </div>
        </div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.page {
  padding: 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  max-width: 1080px;
  height: 100%;
  box-sizing: border-box;
}
.filters {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}
.fsel {
  width: 150px;
}
.count {
  font-size: 12px;
  margin-left: auto;
}
.vl {
  flex: 1;
  margin-right: -8px;
  padding-right: 8px;
}
.grid-row {
  display: grid;
  grid-template-columns: repeat(5, minmax(0, 1fr));
  gap: 10px;
  height: 100%;
}
.tile {
  padding: 6px;
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
  cursor: default;
}
.tile img,
.tile .noimg {
  width: 100%;
  height: 130px;
  object-fit: cover;
  border-radius: 5px;
  background: var(--cc-line);
}
.tile .tname {
  font-size: 12px;
}
.tile small {
  font-size: 10.5px;
}
.tile.missing {
  opacity: 0.55;
}
.head {
  display: flex;
  gap: 10px;
  align-items: center;
  flex-wrap: wrap;
}
.head :deep(.el-button) {
  margin-left: 0;
}
.search {
  width: 240px;
}
.list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-height: 120px;
  flex: 1;
}
.empty {
  padding: 40px 0;
  text-align: center;
}
.item {
  height: 100%;
  box-sizing: border-box;
  display: grid;
  grid-template-columns: 32px 1fr auto;
  gap: 12px;
  align-items: center;
  padding: 8px 12px;
}
.item .thumb {
  width: 32px;
  height: 42px;
}
.item.missing {
  opacity: 0.6;
}
.info {
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.info small {
  font-size: 10.5px;
}
.actions {
  display: flex;
  gap: 2px;
}
.actions :deep(.el-button) {
  margin-left: 6px;
}
</style>
