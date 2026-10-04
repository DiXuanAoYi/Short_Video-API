<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { api, errorText } from '../api'
import { useAppStore, useParseStore } from '../stores/app'
import type { Asset, DetectedLink, HistoryItem } from '../types'
import { formatDate, formatDuration } from '../utils/format'

const app = useAppStore()
const parse = useParseStore()

const selected = ref<Set<string>>(new Set())
const links = ref<DetectedLink[]>([])
const batchRunning = ref(false)
const batchDone = ref(0)
const preview = ref(false)
const enqueuing = ref(false)

const result = computed(() => parse.result)
const images = computed(() => result.value?.assets.filter((a) => a.kind === 'image') ?? [])
const extras = computed(() => result.value?.assets.filter((a) => a.kind !== 'image') ?? [])
const videoAsset = computed(() => result.value?.assets.find((a) => a.kind === 'video'))
const selectedImages = computed(() => images.value.filter((a) => selected.value.has(a.id)).length)
const supported = computed(() => app.info?.providers.map((p) => p.name).join('、') ?? '')

watch(
  result,
  (r) => {
    selected.value = new Set(r ? defaultSelection(r.assets, r.kind) : [])
  },
  { immediate: true },
)

let detectTimer: number | undefined
watch(
  () => parse.text,
  (t) => {
    window.clearTimeout(detectTimer)
    detectTimer = window.setTimeout(async () => {
      links.value = t.trim() ? await api.detectLinks(t) : []
    }, 200)
  },
)

function defaultSelection(assets: Asset[], kind: string) {
  return assets.filter((a) => (kind === 'video' ? a.kind === 'video' : a.kind === 'image')).map((a) => a.id)
}

function toggle(id: string) {
  const next = new Set(selected.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  selected.value = next
}

function selectAllImages(on: boolean) {
  const next = new Set(selected.value)
  for (const a of images.value) {
    if (on) next.add(a.id)
    else next.delete(a.id)
  }
  selected.value = next
}

async function download() {
  if (!result.value) return
  const ids = result.value.assets.filter((a) => selected.value.has(a.id)).map((a) => a.id)
  if (ids.length === 0) {
    ElMessage.warning('请至少选择一项要下载的内容。')
    return
  }
  enqueuing.value = true
  try {
    const tasks = await api.enqueue(result.value, ids)
    const skipped = tasks.filter((t) => t.status === 'done').length
    const added = tasks.length - skipped
    if (added > 0 && skipped > 0) ElMessage.success(`已加入下载 ${added} 项，${skipped} 项之前已下载，已跳过`)
    else if (added > 0) ElMessage.success(`已加入下载队列：${added} 项`)
    else ElMessage.info('所选内容之前都已下载过，已跳过。可在设置中关闭“跳过已下载”。')
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    enqueuing.value = false
  }
}

async function copyLinks() {
  if (!result.value) return
  const urls = result.value.assets.filter((a) => selected.value.has(a.id)).map((a) => a.url)
  const text = (urls.length ? urls : result.value.assets.map((a) => a.url)).join('\n')
  try {
    await api.copyText(text)
    ElMessage.success('直链已复制。直链有时效，请尽快使用。')
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

async function batch() {
  batchRunning.value = true
  batchDone.value = 0
  let failed = 0
  for (const l of links.value) {
    try {
      await api.resolveAndEnqueue(l.url)
    } catch {
      failed++
    }
    batchDone.value++
  }
  batchRunning.value = false
  await parse.loadRecent()
  if (failed) ElMessage.warning(`已加入 ${links.value.length - failed} 条，${failed} 条解析失败`)
  else ElMessage.success(`${links.value.length} 条链接已全部加入下载队列`)
  app.view = 'queue'
}

function openRecent(item: HistoryItem) {
  parse.result = item.info
  parse.text = item.sourceUrl
  parse.error = ''
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault()
    parse.parse()
  }
}

function sizeText(a: Asset) {
  if (a.width && a.height) return `${a.width}×${a.height}`
  return a.ext.toUpperCase()
}
</script>

<template>
  <div class="page">
    <div class="paste">
      <el-input
        v-model="parse.text"
        type="textarea"
        :autosize="{ minRows: 1, maxRows: 4 }"
        resize="none"
        placeholder="粘贴抖音、快手、小红书、B站、微博的分享文案或链接，按 Enter 解析"
        class="paste-input"
        @keydown="onKey"
      />
      <el-button type="primary" size="large" :loading="parse.loading" @click="parse.parse()">解析</el-button>
      <el-button v-if="parse.text || parse.result" size="large" @click="parse.clear()">清空</el-button>
    </div>
    <div class="hint">
      <span v-if="app.settings?.watchClipboard" class="ok">● 剪贴板监听已开启</span>
      <span v-else>剪贴板监听已关闭</span>
      <span>支持：{{ supported }}</span>
      <span>Enter 解析 · Shift+Enter 换行</span>
      <span v-if="app.settings?.shortcut">全局快捷键 {{ app.settings.shortcut.replace('CommandOrControl', 'Ctrl') }}</span>
    </div>

    <div v-if="links.length > 1" class="batch card">
      <span>检测到 <b>{{ links.length }}</b> 条链接</span>
      <span class="mute">批量模式会按默认选项（视频 / 全部图片）直接加入下载队列。</span>
      <el-button type="primary" plain :loading="batchRunning" @click="batch">
        {{ batchRunning ? `处理中 ${batchDone}/${links.length}` : '全部解析并下载' }}
      </el-button>
    </div>

    <el-alert v-if="parse.error" type="error" :title="parse.error" show-icon :closable="false" class="selectable" />

    <div v-if="parse.loading && !result" class="result card skeleton">
      <el-skeleton animated :rows="5" />
    </div>

    <section v-if="result" class="result card">
      <div class="cover-wrap" @click="preview = true">
        <img v-if="result.cover" :src="result.cover" class="thumb cover" referrerpolicy="no-referrer" alt="" />
        <div v-else class="thumb cover" />
        <span v-if="result.kind === 'video'" class="play">▶</span>
        <span v-if="result.durationMs" class="dur mono">{{ formatDuration(result.durationMs) }}</span>
      </div>
      <div class="meta">
        <div class="chips">
          <span class="chip acc">{{ result.platformName }} · {{ result.kind === 'video' ? '视频' : `图集 ${images.length} 张` }}</span>
          <span class="chip">无水印</span>
          <span v-if="result.width && result.height" class="chip">{{ result.width }}×{{ result.height }}</span>
        </div>
        <h3 class="title selectable">{{ result.title }}</h3>
        <div class="mute">
          <span v-if="result.author">@{{ result.author }}</span>
          <span v-if="result.publishedAt"> · {{ formatDate(result.publishedAt) }} 发布</span>
        </div>

        <div v-if="images.length" class="grid-head">
          <span>已选 {{ selectedImages }} / {{ images.length }} 张</span>
          <el-button link type="primary" @click="selectAllImages(selectedImages < images.length)">
            {{ selectedImages < images.length ? '全选' : '全不选' }}
          </el-button>
        </div>
        <div v-if="images.length" class="grid">
          <button
            v-for="img in images"
            :key="img.id"
            type="button"
            class="ph"
            :class="{ sel: selected.has(img.id) }"
            :aria-pressed="selected.has(img.id)"
            :aria-label="img.label"
            @click="toggle(img.id)"
          >
            <img :src="img.url" referrerpolicy="no-referrer" loading="lazy" alt="" />
            <span class="no mono">{{ (img.index ?? 0) + 1 }}</span>
          </button>
        </div>

        <div class="opts">
          <button
            v-for="a in extras"
            :key="a.id"
            type="button"
            class="opt"
            :class="{ on: selected.has(a.id) }"
            :aria-pressed="selected.has(a.id)"
            @click="toggle(a.id)"
          >
            <span>{{ a.label }}</span>
            <small class="mono">{{ sizeText(a) }}</small>
          </button>
        </div>

        <div class="row">
          <el-button type="primary" :loading="enqueuing" @click="download">下载所选</el-button>
          <el-button @click="copyLinks">复制直链</el-button>
          <el-button @click="preview = true">预览</el-button>
        </div>
      </div>
    </section>

    <section v-if="!result && !parse.loading && parse.recent.length" class="recent">
      <div class="label">最近解析</div>
      <button v-for="r in parse.recent" :key="r.id" type="button" class="ritem" @click="openRecent(r)">
        <img v-if="r.cover" :src="r.cover" class="thumb" referrerpolicy="no-referrer" alt="" />
        <div v-else class="thumb" />
        <span class="ellipsis">{{ r.title }}</span>
        <small class="mono mute">{{ r.info.platformName }} · {{ r.kind === 'video' ? '视频' : `图集 ${r.info.assets.filter((a) => a.kind === 'image').length} 张` }}</small>
      </button>
    </section>

    <section v-if="!result && !parse.loading && !parse.recent.length && !parse.error" class="empty">
      <h3>复制分享链接，就能保存无水印作品</h3>
      <ol>
        <li>在 App 里点“分享 → 复制链接”。</li>
        <li>回到这里粘贴（开启剪贴板监听后会自动识别）。</li>
        <li>选择视频、图片或背景音乐，点击“下载所选”。</li>
      </ol>
    </section>

    <el-dialog v-model="preview" :title="result?.title" width="720px" align-center destroy-on-close>
      <div v-if="result" class="preview">
        <video v-if="videoAsset" :src="videoAsset.url" controls autoplay referrerpolicy="no-referrer" />
        <el-carousel v-else-if="images.length" height="460px" indicator-position="outside" :autoplay="false">
          <el-carousel-item v-for="img in images" :key="img.id">
            <img :src="img.url" referrerpolicy="no-referrer" alt="" />
          </el-carousel-item>
        </el-carousel>
      </div>
    </el-dialog>
  </div>
</template>

<style scoped>
.page {
  padding: 20px 24px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  max-width: 980px;
}
.paste {
  display: flex;
  gap: 8px;
  align-items: flex-start;
}
.paste-input {
  flex: 1;
  min-width: 0;
}
.paste-input :deep(textarea) {
  font-family: var(--cc-mono);
  font-size: 12.5px;
  padding: 9px 12px;
  min-height: 40px !important;
}
.paste :deep(.el-button) {
  margin-left: 0;
}
.hint {
  font-size: 11.5px;
  color: var(--cc-mute);
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
  margin-top: -4px;
}
.hint .ok {
  color: var(--cc-ok);
}
.batch {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  flex-wrap: wrap;
}
.batch .el-button {
  margin-left: auto;
}
.result {
  display: grid;
  grid-template-columns: 168px 1fr;
  gap: 18px;
  padding: 16px;
}
.skeleton {
  display: block;
}
.cover-wrap {
  position: relative;
  cursor: pointer;
  align-self: start;
}
.cover {
  width: 168px;
  aspect-ratio: 9 / 16;
  display: block;
}
.play {
  position: absolute;
  inset: 0;
  display: grid;
  place-items: center;
  font-size: 28px;
  color: #fffd;
  text-shadow: 0 2px 8px #0008;
}
.dur {
  position: absolute;
  right: 6px;
  bottom: 6px;
  font-size: 10.5px;
  background: #000a;
  color: #fff;
  padding: 0 5px;
  border-radius: 3px;
}
.meta {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.chips {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.title {
  margin: 0;
  font-size: 15px;
  line-height: 1.45;
  font-weight: 600;
  overflow-wrap: anywhere;
}
.grid-head {
  display: flex;
  justify-content: space-between;
  align-items: center;
  font-size: 12px;
  color: var(--cc-mute);
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(92px, 1fr));
  gap: 8px;
  max-height: 320px;
  overflow: auto;
  padding: 2px;
}
.ph {
  all: unset;
  cursor: pointer;
  position: relative;
  aspect-ratio: 3 / 4;
  border-radius: 6px;
  overflow: hidden;
  border: 2px solid transparent;
  background: var(--cc-side);
}
.ph img {
  width: 100%;
  height: 100%;
  object-fit: cover;
  display: block;
  opacity: 0.55;
}
.ph.sel {
  border-color: var(--cc-acc);
}
.ph.sel img {
  opacity: 1;
}
.ph.sel::after {
  content: '✓';
  position: absolute;
  top: 5px;
  right: 5px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--cc-acc);
  color: #1a1208;
  font-size: 11px;
  display: grid;
  place-items: center;
  font-weight: 700;
}
.ph .no {
  position: absolute;
  left: 5px;
  bottom: 4px;
  font-size: 10px;
  color: #fff;
  text-shadow: 0 1px 3px #000;
}
.ph:focus-visible {
  outline: 2px solid var(--cc-acc);
}
.opts {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
  gap: 8px;
}
.opt {
  all: unset;
  cursor: pointer;
  border: 1px solid var(--cc-line);
  border-radius: 8px;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  font-size: 12.5px;
}
.opt small {
  color: var(--cc-mute);
  font-size: 10.5px;
}
.opt.on {
  border-color: var(--cc-acc);
  background: var(--cc-acc-soft);
}
.opt:focus-visible {
  outline: 2px solid var(--cc-acc);
}
.row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.row :deep(.el-button) {
  margin-left: 0;
}
.recent {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.recent .label {
  font-size: 11px;
  color: var(--cc-mute);
  letter-spacing: 0.08em;
}
.ritem {
  all: unset;
  cursor: pointer;
  display: grid;
  grid-template-columns: 30px 1fr auto;
  gap: 12px;
  align-items: center;
  padding: 7px 10px;
  border-radius: 7px;
  background: var(--cc-card);
  border: 1px solid var(--cc-line);
}
.ritem:hover {
  border-color: var(--cc-acc);
}
.ritem .thumb {
  width: 30px;
  height: 40px;
}
.empty {
  padding: 28px 4px;
  color: var(--cc-mute);
}
.empty h3 {
  color: var(--cc-fg);
  margin: 0 0 8px;
  font-size: 15px;
}
.empty ol {
  padding-left: 18px;
  margin: 0;
  line-height: 1.9;
}
.preview video,
.preview img {
  width: 100%;
  max-height: 460px;
  object-fit: contain;
  background: #000;
  display: block;
}
</style>
