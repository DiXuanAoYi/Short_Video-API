<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { api, errorText } from '../api'
import { useAppStore, useQueueStore } from '../stores/app'
import type { OrphanPart, TaskSnapshot } from '../types'
import { formatBytes, formatEta, formatSpeed } from '../utils/format'

const queue = useQueueStore()
const app = useAppStore()
const orphans = ref<OrphanPart[]>([])
const orphanSize = computed(() => orphans.value.reduce((s, o) => s + o.size, 0))

onMounted(async () => {
  try {
    orphans.value = await api.listOrphanParts()
  } catch {
    orphans.value = []
  }
})

async function cleanOrphans() {
  try {
    await ElMessageBox.confirm(
      `删除 ${orphans.value.length} 个不属于任何任务的未完成文件（共 ${formatBytes(orphanSize.value)}）？这些通常是旧版本或异常退出留下的。`,
      '清理残留文件',
      { type: 'warning', confirmButtonText: '删除', cancelButtonText: '取消' },
    )
  } catch {
    return
  }
  const n = await api.deleteOrphanParts(orphans.value.map((o) => o.path))
  ElMessage.success(`已删除 ${n} 个残留文件`)
  orphans.value = await api.listOrphanParts()
}
const tasks = computed(() => [...queue.tasks].reverse())

const statusText: Record<TaskSnapshot['status'], string> = {
  queued: '等待',
  running: '下载中',
  paused: '已暂停',
  done: '完成',
  failed: '失败',
  canceled: '已取消',
}

function percent(t: TaskSnapshot) {
  if (t.status === 'done') return 100
  if (!t.total) return 0
  return Math.min(100, Math.floor((t.received / t.total) * 100))
}

function detail(t: TaskSnapshot) {
  if (t.note) return t.note
  switch (t.status) {
    case 'running':
      return [t.total ? `${formatBytes(t.received)} / ${formatBytes(t.total)}` : formatBytes(t.received), formatSpeed(t.speed), formatEta(t.received, t.total, t.speed)]
        .filter(Boolean)
        .join(' · ')
    case 'queued':
      return '排队中'
    case 'paused':
      return t.received ? `已下载 ${formatBytes(t.received)}，继续时从断点开始` : '已暂停'
    case 'failed':
      return t.error ?? '下载失败'
    case 'done':
      return `${formatBytes(t.total ?? t.received)} · ${t.filePath}`
    case 'canceled':
      return '已取消'
  }
}

async function run<A extends unknown[]>(fn: (...args: A) => Promise<unknown>, ...args: A) {
  try {
    await fn(...args)
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}
</script>

<template>
  <div class="page">
    <div class="head">
      <h2>下载队列</h2>
      <div class="stats mono">
        <span>进行中 <b>{{ queue.counts.running }}</b></span>
        <span>等待 <b>{{ queue.counts.queued }}</b></span>
        <span>完成 <b>{{ queue.counts.done }}</b></span>
        <span>失败 <b>{{ queue.counts.failed }}</b></span>
        <span>↓ <b>{{ formatSpeed(queue.totalSpeed) }}</b></span>
      </div>
    </div>
    <div class="toolbar">
      <el-button size="small" :disabled="queue.active === 0" @click="run(api.pauseAll)">全部暂停</el-button>
      <el-button size="small" :disabled="queue.counts.paused + queue.counts.failed === 0" @click="run(api.resumeAll)">全部继续 / 重试</el-button>
      <el-button size="small" :disabled="!queue.tasks.some((t) => t.status === 'done' || t.status === 'canceled')" @click="run(api.clearFinished)">清除已完成</el-button>
    </div>

    <el-alert v-if="orphans.length" type="info" show-icon :closable="false">
      <template #title>下载目录里有 {{ orphans.length }} 个不属于任何任务的未完成文件（共 {{ formatBytes(orphanSize) }}）</template>
      <el-button size="small" @click="cleanOrphans">清理</el-button>
    </el-alert>

    <div v-if="tasks.length === 0" class="empty mute">队列是空的。在“解析”页选择内容并点击“下载所选”后，任务会出现在这里。</div>

    <div v-for="t in tasks" :key="t.id" class="task card" :class="t.status">
      <img v-if="t.cover" :src="t.cover" class="thumb" referrerpolicy="no-referrer" alt="" />
      <div v-else class="thumb" />
      <div class="info">
        <div class="name ellipsis" :title="t.title">{{ t.title }} <span class="mute">· {{ t.assetLabel }}</span></div>
        <div class="bar"><i :style="{ width: percent(t) + '%' }" /></div>
        <small class="mono detail ellipsis selectable" :title="detail(t)">{{ detail(t) }}</small>
        <div class="tags">
          <span v-if="t.resumable === false && t.status !== 'done'" class="chip">服务器不支持续传</span>
          <span v-else-if="t.resumable && (t.status === 'paused' || t.status === 'running')" class="chip">可续传</span>
          <el-button v-if="t.status === 'failed' && (t.errorKind === 'need_login' || t.errorKind === 'rate_limited')" link size="small" type="primary" @click="app.goSettings('accounts')">添加 Cookie 后重试</el-button>
        </div>
      </div>
      <div class="st">
        <span class="state">{{ statusText[t.status] }}<template v-if="t.status === 'running' && t.total"> {{ percent(t) }}%</template></span>
        <div class="actions">
          <el-button v-if="t.status === 'running' || t.status === 'queued'" link size="small" @click="run(api.pauseTask, t.id)">暂停</el-button>
          <el-button v-if="t.status === 'paused'" link size="small" type="primary" @click="run(api.resumeTask, t.id)">继续</el-button>
          <el-button v-if="t.status === 'failed' || t.status === 'canceled'" link size="small" type="primary" @click="run(api.resumeTask, t.id)">重试</el-button>
          <el-button v-if="t.status === 'done'" link size="small" type="primary" @click="run(api.openFile, t.filePath)">打开</el-button>
          <el-button v-if="t.status === 'done'" link size="small" @click="run(api.revealFile, t.filePath)">文件夹</el-button>
          <el-button v-if="t.status === 'running' || t.status === 'paused' || t.status === 'queued'" link size="small" @click="run(api.cancelTask, t.id)">取消</el-button>
          <el-button v-else link size="small" @click="run(api.removeTask, t.id)">移除</el-button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page {
  padding: 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-width: 980px;
}
.head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
h2 {
  margin: 0;
  font-size: 16px;
}
.stats {
  display: flex;
  gap: 14px;
  font-size: 11.5px;
  color: var(--cc-mute);
  flex-wrap: wrap;
}
.stats b {
  color: var(--cc-fg);
  font-weight: 600;
}
.toolbar {
  display: flex;
  gap: 8px;
}
.toolbar :deep(.el-button) {
  margin-left: 0;
}
.empty {
  padding: 40px 0;
  text-align: center;
}
.task {
  display: grid;
  grid-template-columns: 36px 1fr 180px;
  gap: 12px;
  align-items: center;
  padding: 10px 12px;
}
.task .thumb {
  width: 36px;
  height: 46px;
}
.info {
  min-width: 0;
}
.bar {
  height: 5px;
  background: var(--cc-line);
  border-radius: 3px;
  margin: 6px 0 3px;
  overflow: hidden;
}
.bar i {
  display: block;
  height: 100%;
  background: var(--cc-acc);
  border-radius: 3px;
  transition: width 0.25s;
}
.done .bar i {
  background: var(--cc-ok);
}
.failed .bar i {
  background: var(--cc-err);
  width: 100% !important;
}
.paused .bar i,
.canceled .bar i {
  background: var(--cc-mute);
}
.detail {
  display: block;
  font-size: 10.5px;
  color: var(--cc-mute);
}
.tags {
  display: flex;
  gap: 6px;
  align-items: center;
  min-height: 0;
}
.tags:empty {
  display: none;
}
.failed .detail {
  color: var(--cc-err);
}
.st {
  text-align: right;
  font-size: 12px;
}
.running .state {
  color: var(--cc-acc);
}
.done .state {
  color: var(--cc-ok);
}
.failed .state {
  color: var(--cc-err);
}
.actions {
  display: flex;
  justify-content: flex-end;
  gap: 2px;
  flex-wrap: wrap;
}
.actions :deep(.el-button) {
  margin-left: 6px;
}
</style>
