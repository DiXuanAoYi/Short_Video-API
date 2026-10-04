<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { api, errorText } from '../api'
import { useAppStore, useParseStore, useQueueStore } from '../stores/app'
import type { HistoryItem, LibraryItem } from '../types'
import { formatBytes, formatDateTime } from '../utils/format'

const app = useAppStore()
const parse = useParseStore()
const queue = useQueueStore()

const tab = ref<'files' | 'history'>('files')
const query = ref('')
const files = ref<LibraryItem[]>([])
const history = ref<HistoryItem[]>([])
const loading = ref(false)

const platformName = computed<Record<string, string>>(() => Object.fromEntries((app.info?.providers ?? []).map((p) => [p.id, p.name])))

function assetName(id: string) {
  if (id.startsWith('image-')) return `图片 ${Number(id.slice(6)) + 1}`
  return ({ video: '视频', music: '背景音乐', cover: '封面' } as Record<string, string>)[id] ?? id
}

async function load() {
  loading.value = true
  try {
    if (tab.value === 'files') files.value = await api.listLibrary(query.value)
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
watch(tab, load)
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
        <el-radio-button value="history">解析历史</el-radio-button>
      </el-radio-group>
      <el-input v-model="query" size="small" clearable placeholder="搜索标题或作者" class="search" />
      <el-button v-if="tab === 'files' && app.settings" size="small" @click="run(api.revealFile, app.settings!.downloadDir)">打开下载目录</el-button>
      <el-button v-if="tab === 'history' && history.length" size="small" @click="clearHistory">清空历史</el-button>
    </div>

    <div v-loading="loading" class="list">
      <template v-if="tab === 'files'">
        <div v-if="!files.length" class="empty mute">{{ query ? '没有匹配的文件。' : '还没有下载过文件。' }}</div>
        <div v-for="f in files" :key="f.id" class="item card" :class="{ missing: !f.exists }">
          <img v-if="f.cover" :src="f.cover" class="thumb" referrerpolicy="no-referrer" alt="" />
          <div v-else class="thumb" />
          <div class="info">
            <div class="ellipsis" :title="f.title">{{ f.title }}</div>
            <small class="mono mute ellipsis selectable" :title="f.path">
              {{ platformName[f.platform] ?? f.platform }} · {{ assetName(f.assetId) }} · {{ formatBytes(f.size) }} · {{ formatDateTime(f.finishedAt) }}
              <template v-if="!f.exists"> · 文件已不存在</template>
            </small>
          </div>
          <div class="actions">
            <el-button link size="small" type="primary" :disabled="!f.exists" @click="run(api.openFile, f.path)">打开</el-button>
            <el-button link size="small" :disabled="!f.exists" @click="run(api.revealFile, f.path)">文件夹</el-button>
            <el-button link size="small" @click="removeFile(f)">删除</el-button>
          </div>
        </div>
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
  max-width: 980px;
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
}
.empty {
  padding: 40px 0;
  text-align: center;
}
.item {
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
