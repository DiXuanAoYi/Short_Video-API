<script setup lang="ts">
// 剪辑：把多个视频 / 图片在时间线上排好，裁剪、分割、加转场、文字和配乐，导出成一个视频。
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { open, save } from '@tauri-apps/plugin-dialog'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { api, errorText, events } from '../../api'
import type { EditProject, JobSnap } from '../../types'
import { useEditProject, type AddReport } from '../../composables/useEditProject'
import { MAX_AUDIO, MAX_CLIPS, MAX_TEXTS, clamp, msText, outputSize } from '../../utils/edit'
import FileInput from './FileInput.vue'
import EditInspector from './edit/EditInspector.vue'
import EditMonitor from './edit/EditMonitor.vue'
import EditTimeline from './edit/EditTimeline.vue'

const props = defineProps<{ preset?: string[] }>()
const emit = defineEmits<{ consumed: [] }>()

const ed = useEditProject()
const root = ref<HTMLElement | null>(null)
const tlBox = ref<HTMLElement | null>(null)
const timeline = ref<InstanceType<typeof EditTimeline> | null>(null)

const VIDEO_EXTS = ['mp4', 'mkv', 'webm', 'mov', 'avi', 'flv', 'ts', 'm4v', 'wmv', 'mpg', 'mpeg', '3gp']
const IMAGE_EXTS = ['jpg', 'jpeg', 'png', 'webp', 'gif', 'bmp']
const AUDIO_EXTS = ['mp3', 'm4a', 'flac', 'wav', 'ogg', 'opus', 'aac', 'wma']

const pps = ref(50)
const outDir = ref('')
const monitorMode = ref<'live' | 'preview'>('live')

// ---------- 添加素材 ----------

const pickedClips = ref<string[]>([])
const pickedMusic = ref<string[]>([])

function report(r: AddReport) {
  if (r.errors.length) ElMessage({ type: r.added ? 'warning' : 'error', message: r.errors.slice(0, 3).join('\n') + (r.errors.length > 3 ? `\n……还有 ${r.errors.length - 3} 个` : ''), duration: 6000, showClose: true })
}
async function addClips(paths: string[]) {
  if (!paths.length) return
  const before = ed.total.value
  report(await ed.addClips(paths))
  if (!before) fit()
}
async function addMusic(paths: string[]) {
  if (!paths.length) return
  report(await ed.addMusic(paths))
}
watch(pickedClips, (v) => {
  if (!v.length) return
  pickedClips.value = []
  addClips(v)
})
watch(pickedMusic, (v) => {
  if (!v.length) return
  pickedMusic.value = []
  addMusic(v)
})
watch(
  () => props.preset,
  (v) => {
    if (!v?.length) return
    addClips([...v])
    emit('consumed')
  },
  { immediate: true },
)

function addText() {
  if (!ed.addText()) ElMessage.warning(`文字最多 ${MAX_TEXTS} 条。`)
}

// ---------- 播放 ----------

const hasClips = computed(() => ed.project.value.clips.length > 0)
const timeText = computed(() => `${msText(ed.playhead.value).replace(/\.\d+$/, (m) => m.slice(0, 3))} / ${msText(ed.total.value).replace(/\.\d+$/, (m) => m.slice(0, 3))}`)

function togglePlay() {
  if (!hasClips.value) return
  ed.playing.value = !ed.playing.value
}
function goto(ms: number) {
  ed.playhead.value = clamp(Math.round(ms), 0, ed.total.value)
  nextTick(() => timeline.value?.reveal())
}
function step(ms: number) {
  ed.playing.value = false
  goto(ed.playhead.value + ms)
}

watch(
  () => ed.total.value,
  (t) => {
    if (ed.playhead.value > t) ed.playhead.value = t
  },
)

// ---------- 缩放 ----------

function fit() {
  nextTick(() => {
    const w = (tlBox.value?.clientWidth ?? 800) - 64 - 48
    const secs = Math.max(1, (ed.extent.value || 10000) / 1000)
    pps.value = Math.round(clamp(w / secs, 5, 400))
  })
}

// ---------- 编辑操作 ----------

function split() {
  if (!ed.splitAtPlayhead()) ElMessage.info('请先把播放位置移到片段中间（离片段两端至少 0.1 秒），再分割。')
}
function duplicate() {
  if (!ed.duplicateSelected()) ElMessage.warning('没有选中的内容，或已经到了数量上限。')
}
const selectedLabel = computed(() => (ed.sel.value?.kind === 'clip' ? '片段' : ed.sel.value?.kind === 'text' ? '文字' : ed.sel.value?.kind === 'audio' ? '配乐' : ''))

// ---------- 工程文件 ----------

const FILTERS = [{ name: '清影剪辑工程', extensions: ['ccedit'] }]
const hasContent = computed(() => hasClips.value || ed.project.value.texts.length > 0 || ed.project.value.audio.length > 0)

async function discardOk(): Promise<boolean> {
  if (!ed.dirty.value || !hasContent.value) return true
  try {
    await ElMessageBox.confirm('当前的工程还没有保存，继续会丢掉这些修改。', '放弃未保存的修改？', { type: 'warning', confirmButtonText: '放弃修改', cancelButtonText: '取消' })
    return true
  } catch {
    return false
  }
}

async function newProject() {
  if (!(await discardOk())) return
  ed.newProject()
  ed.clearDraft()
}

async function openProject() {
  if (!(await discardOk())) return
  try {
    const r = await open({ multiple: false, filters: FILTERS })
    if (!r || Array.isArray(r)) return
    const missing = await ed.openProject(r)
    fit()
    if (missing.length) ElMessage({ type: 'warning', message: `有 ${missing.length} 个素材文件找不到，已在时间线上标红。选中后可以重新指定。`, duration: 6000, showClose: true })
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

async function saveAs(): Promise<boolean> {
  try {
    const name = (ed.project.value.title.trim() || '未命名') + '.ccedit'
    let p = await save({ filters: FILTERS, defaultPath: ed.filePath.value || name })
    if (!p) return false
    if (!/\.ccedit$/i.test(p)) p += '.ccedit'
    await ed.saveProject(p)
    ElMessage.success('已保存')
    return true
  } catch (e) {
    ElMessage.error(errorText(e))
    return false
  }
}
async function saveProject() {
  if (!ed.filePath.value) return void (await saveAs())
  try {
    await ed.saveProject(ed.filePath.value)
    ElMessage.success('已保存')
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

// ---------- 草稿 ----------

const draft = ref<{ project: EditProject; filePath: string } | null>(null)
onMounted(() => {
  if (!props.preset?.length) draft.value = ed.readDraft()
})
async function resumeDraft() {
  const d = draft.value
  draft.value = null
  if (!d) return
  try {
    await ed.restoreDraft(d)
    fit()
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}
function dropDraft() {
  draft.value = null
  ed.clearDraft()
}
watch(hasContent, (v) => {
  if (v) draft.value = null
})

// ---------- 预览成片 ----------

const projectKey = computed(() => JSON.stringify(ed.project.value))
const preview = ref<{ url: string; key: string } | null>(null)
const previewJob = ref<JobSnap | null>(null)
const previewId = ref<number | null>(null)
const previewError = ref('')
const building = computed(() => previewJob.value !== null && (previewJob.value.status === 'queued' || previewJob.value.status === 'running'))
const stale = computed(() => !!preview.value && preview.value.key !== projectKey.value)

let unlisten: UnlistenFn | undefined
onMounted(async () => {
  unlisten = await events.onMediaJobs(async (list) => {
    const id = previewId.value
    if (id === null) return
    const j = list.find((x) => x.id === id)
    if (!j) return
    previewJob.value = j
    if (j.status === 'done' && j.output) {
      previewId.value = null
      try {
        const url = await api.editPreviewUrl(j.output)
        preview.value = { url, key: buildKey }
        monitorMode.value = 'preview'
        ed.playing.value = false
      } catch (e) {
        previewError.value = errorText(e)
      }
    } else if (j.status === 'failed') {
      previewId.value = null
      previewError.value = j.error ?? '生成预览失败。'
    } else if (j.status === 'canceled') {
      previewId.value = null
    }
  })
})
onBeforeUnmount(() => unlisten?.())

let buildKey = ''
async function buildPreview() {
  if (!hasClips.value || building.value) return
  if (!checkMedia()) return
  previewError.value = ''
  buildKey = projectKey.value
  try {
    ed.playing.value = false
    previewId.value = await api.editPreviewStart(JSON.parse(buildKey))
    previewJob.value = null
  } catch (e) {
    previewError.value = errorText(e)
  }
}
async function cancelPreview() {
  if (previewId.value !== null) await api.mediaJobCancel(previewId.value).catch(() => undefined)
}
watch(stale, (s) => {
  if (s && monitorMode.value === 'preview') {
    ed.playing.value = false
    monitorMode.value = 'live'
  }
})

// ---------- 导出 ----------

const exporting = ref(false)

/** 有素材找不到时不能导出 / 预览，提示是哪几个。 */
function checkMedia(): boolean {
  const used = new Set([...ed.project.value.clips.map((c) => c.path), ...ed.project.value.audio.map((a) => a.path)])
  const bad = [...used].filter((p) => p in ed.broken)
  if (!bad.length) return true
  ElMessage({ type: 'error', message: `有 ${bad.length} 个素材找不到或读不出来（在时间线上标红）：${bad.slice(0, 2).map((p) => p.split(/[\\/]/).pop()).join('、')}${bad.length > 2 ? '…' : ''}。请重新指定或删除这些片段。`, duration: 7000, showClose: true })
  return false
}

async function exportNow() {
  if (!hasClips.value || exporting.value) return
  if (!checkMedia()) return
  exporting.value = true
  try {
    await api.mediaJobStart({ op: 'edit', project: JSON.parse(projectKey.value), inputs: [], outputDir: outDir.value || null })
    ElMessage.success('已开始导出，进度见下方的“处理进度”。')
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    exporting.value = false
  }
}

// ---------- 键盘 ----------

function visible(): boolean {
  return !!root.value && root.value.offsetParent !== null
}
function onKey(e: KeyboardEvent) {
  if (!visible()) return
  const t = e.target as HTMLElement | null
  if (t?.closest('input, textarea, select, [contenteditable="true"], .el-overlay, .el-popper')) return
  const mod = e.ctrlKey || e.metaKey
  const key = e.key.toLowerCase()
  if (mod && key === 'z') {
    e.preventDefault()
    e.shiftKey ? ed.redo() : ed.undo()
  } else if (mod && key === 'y') {
    e.preventDefault()
    ed.redo()
  } else if (mod && key === 's') {
    e.preventDefault()
    saveProject()
  } else if (mod || e.altKey) {
    return
  } else if (e.key === ' ' && t?.tagName !== 'BUTTON') {
    e.preventDefault()
    togglePlay()
  } else if (key === 's') {
    split()
  } else if (e.key === 'Delete' || e.key === 'Backspace') {
    e.preventDefault()
    ed.removeSelected()
  } else if (e.key === 'ArrowLeft') {
    e.preventDefault()
    step(e.shiftKey ? -100 : -1000)
  } else if (e.key === 'ArrowRight') {
    e.preventDefault()
    step(e.shiftKey ? 100 : 1000)
  } else if (e.key === 'Home') {
    goto(0)
  } else if (e.key === 'End') {
    goto(ed.total.value)
  }
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))

// ---------- 信息 ----------

const outInfo = computed(() => {
  const s = outputSize(ed.project.value, ed.sources)
  return `${s.w}×${s.h}${ed.project.value.out.fps ? ` · ${ed.project.value.out.fps} 帧/秒` : ''}`
})
const clipCount = computed(() => ed.project.value.clips.length)
const sizeWarn = computed(() => (ed.project.value.clips.length >= MAX_CLIPS ? `片段已达上限 ${MAX_CLIPS} 个` : ed.project.value.audio.length >= MAX_AUDIO ? `配乐已达上限 ${MAX_AUDIO} 条` : ''))
</script>

<template>
  <div ref="root" class="ed">
    <el-alert v-if="draft" type="info" :closable="false" show-icon class="draft">
      <template #title>
        <span>找到上次没保存的剪辑（{{ draft.project.clips.length }} 个片段）。</span>
        <el-button size="small" type="primary" class="gap" @click="resumeDraft">继续编辑</el-button>
        <el-button size="small" @click="dropDraft">不要了</el-button>
      </template>
    </el-alert>

    <!-- 工具栏 -->
    <div class="bar">
      <div class="grp">
        <el-button size="small" @click="newProject">新建</el-button>
        <el-button size="small" @click="openProject">打开…</el-button>
        <el-button size="small" :disabled="!hasContent" @click="saveProject">保存</el-button>
        <el-button size="small" :disabled="!hasContent" @click="saveAs">另存为…</el-button>
      </div>
      <div class="grp">
        <FileInput
          v-model="pickedClips"
          multiple
          compact
          plain
          :extensions="[...VIDEO_EXTS, ...IMAGE_EXTS, ...AUDIO_EXTS]"
          :kinds="['video', 'image']"
          browse-text="添加片段…"
          library-text="从媒体库添加…"
          label="视频或图片"
        />
      </div>
      <div class="grp">
        <FileInput v-model="pickedMusic" multiple compact plain :extensions="[...AUDIO_EXTS, ...VIDEO_EXTS]" :kinds="['audio']" browse-text="添加配乐…" library-text="从媒体库添加配乐…" label="音频" />
        <el-button size="small" @click="addText">添加文字</el-button>
      </div>
      <div class="grp end">
        <el-button size="small" :disabled="!ed.canUndo.value" @click="ed.undo">撤回</el-button>
        <el-button size="small" :disabled="!ed.canRedo.value" @click="ed.redo">重做</el-button>
      </div>
    </div>

    <div class="main">
      <!-- 监视器 -->
      <div class="left">
        <EditMonitor :ed="ed" :mode="monitorMode" :preview-url="preview?.url ?? null" />
        <div class="transport">
          <el-button size="small" :disabled="!hasClips" @click="goto(0)">回到开头</el-button>
          <el-button size="small" type="primary" :disabled="!hasClips" class="play" @click="togglePlay">{{ ed.playing.value ? '暂停' : '播放' }}</el-button>
          <span class="mono time">{{ timeText }}</span>
          <span class="sp" />
          <el-radio-group v-model="monitorMode" size="small">
            <el-radio-button value="live">实时画面</el-radio-button>
            <el-radio-button value="preview" :disabled="!preview || stale">预览成片</el-radio-button>
          </el-radio-group>
        </div>
        <div class="pvbar">
          <el-button v-if="!building" size="small" :disabled="!hasClips" @click="buildPreview">生成预览（含转场、文字、配乐）</el-button>
          <template v-else>
            <el-progress :percentage="Math.round(previewJob?.percent ?? 0)" :stroke-width="6" class="pg" />
            <el-button size="small" link @click="cancelPreview">取消</el-button>
          </template>
          <span v-if="stale && !building" class="mute small">预览已过期（工程改过了），需要重新生成</span>
          <span v-else-if="preview && !building" class="ok small">预览是最新的</span>
          <span v-else-if="!building" class="mute small">实时画面不显示转场；要看完整效果请生成预览</span>
        </div>
        <div v-if="previewError" class="err selectable">{{ previewError }}</div>
      </div>

      <!-- 检查器 -->
      <div class="right">
        <EditInspector :ed="ed" v-model:out-dir="outDir" @goto="goto" />
      </div>
    </div>

    <!-- 时间线 -->
    <div ref="tlBox" class="tlbox" tabindex="-1">
      <div class="tlbar">
        <el-button size="small" :disabled="!hasClips" @click="split">分割</el-button>
        <el-button size="small" :disabled="!ed.sel.value" @click="duplicate">复制{{ selectedLabel }}</el-button>
        <el-button size="small" :disabled="!ed.sel.value" @click="ed.removeSelected">删除{{ selectedLabel }}</el-button>
        <span class="sp" />
        <span class="mute small">缩放</span>
        <el-slider v-model="pps" :min="5" :max="400" :show-tooltip="false" size="small" class="zoom" />
        <el-button size="small" link @click="fit">适合窗口</el-button>
      </div>
      <EditTimeline ref="timeline" :ed="ed" :pps="pps" />
      <div class="keys mute small">空格 播放 / 暂停 · S 分割 · Delete 删除 · Ctrl+Z 撤销 · ← → 前后 1 秒（加 Shift 为 0.1 秒）</div>
    </div>

    <!-- 导出 -->
    <div class="export card">
      <div class="info">
        <span>共 {{ clipCount }} 个片段</span>
        <span class="mono">{{ msText(ed.total.value) }}</span>
        <span class="mono">{{ outInfo }}</span>
        <span v-if="ed.filePath.value" class="mute ellipsis" :title="ed.filePath.value">{{ ed.filePath.value.split(/[\\/]/).pop() }}{{ ed.dirty.value ? '（有未保存的修改）' : '' }}</span>
        <span v-else-if="hasContent" class="mute">（工程还没有保存）</span>
        <span v-if="sizeWarn" class="warn">{{ sizeWarn }}</span>
      </div>
      <el-button type="primary" :disabled="!hasClips" :loading="exporting" @click="exportNow">导出视频</el-button>
    </div>
  </div>
</template>

<style scoped>
.ed {
  display: flex;
  flex-direction: column;
  gap: 10px;
  outline: none;
}
.draft .gap {
  margin-left: 12px;
}
.bar {
  display: flex;
  flex-wrap: wrap;
  gap: 8px 18px;
  align-items: center;
}
.grp {
  display: flex;
  gap: 8px;
  align-items: center;
}
.grp.end {
  margin-left: auto;
}
.main {
  display: grid;
  grid-template-columns: minmax(360px, 1fr) 380px;
  gap: 16px;
  align-items: start;
}
.left {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
}
.right {
  min-width: 0;
  max-height: 470px;
  overflow: auto;
  padding-right: 4px;
}
.transport {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.play {
  min-width: 64px;
}
.time {
  font-size: 12.5px;
}
.sp {
  flex: 1;
}
.pvbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.pg {
  width: 200px;
}
.small {
  font-size: 11.5px;
}
.ok {
  color: var(--cc-ok);
}
.err {
  color: var(--cc-err);
  font-size: 12px;
  word-break: break-all;
}
.tlbox {
  display: flex;
  flex-direction: column;
  gap: 6px;
  outline: none;
}
.tlbar {
  display: flex;
  align-items: center;
  gap: 8px;
}
.zoom {
  width: 150px;
  margin: 0 8px;
}
.keys {
  padding-left: 2px;
}
.export {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 14px;
}
.info {
  display: flex;
  gap: 16px;
  align-items: center;
  flex-wrap: wrap;
  font-size: 12.5px;
  min-width: 0;
}
.warn {
  color: var(--cc-err);
}
.ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 320px;
}
</style>
