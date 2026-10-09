<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { open } from '@tauri-apps/plugin-dialog'
import { convertFileSrc } from '@tauri-apps/api/core'
import { api, errorText } from '../../api'
import { useAppStore } from '../../stores/app'
import type { CapsSummary, NormPreset, NormSpec, VideoFactsInfo, VideoPreview, VideoReport } from '../../types'
import { parseClock } from '../../utils/time'
import FileInput from './FileInput.vue'

const props = defineProps<{ preset?: string[]; hint?: string }>()
const emit = defineEmits<{ started: [] }>()
const app = useAppStore()

const files = ref<string[]>(props.preset ? [...props.preset] : [])
const caps = ref<CapsSummary | null>(null)
const presets = ref<NormPreset[]>([])
const presetId = ref('compat')
const spec = reactive<NormSpec>({
  size: 'limit', width: 1920, height: 1080, shortSide: 1080, followOrientation: true, fps: 'auto', fpsValue: 30, hdr: true, fixColor: true, levels: 0, lut: null,
  lutStrength: 1, autocrop: false, audioRate: 0, audioChannels: 0, loudness: null, fixSync: true, codec: 'h264', quality: 'balanced',
})
const reports = ref<Record<string, VideoReport | { error: string }>>({})
const analyzing = ref(false)
const showAdvanced = ref(false)
const outDir = ref('')
const busy = ref(false)
const stab = ref<'light' | 'normal' | 'strong'>('normal')
const stabBusy = ref(false)
const previewAt = ref('0:02')
const preview = ref<VideoPreview | null>(null)
const previewing = ref(false)
let token = 0

watch(
  () => props.preset,
  (p) => p && (files.value = [...p]),
)

function applyPreset(p: NormPreset) {
  presetId.value = p.id
  // 色阶和 LUT 是个人风格，不跟着预设走
  Object.assign(spec, p.spec, { lut: spec.lut, lutStrength: spec.lutStrength, levels: spec.levels })
}

onMounted(async () => {
  presets.value = await api.videoPresets().catch(() => [])
  caps.value = await api.videoCaps().catch(() => null)
  const first = presets.value.find((p) => p.id === presetId.value)
  if (first) applyPreset(first)
})

const isReport = (r: VideoReport | { error: string } | undefined): r is VideoReport => !!r && 'facts' in r

/** 分析选中的文件（逐个，避免同时跑太多 ffmpeg）；按多数文件的推荐自动选预设。 */
watch(
  files,
  async (list) => {
    const my = ++token
    preview.value = null
    const next: Record<string, VideoReport | { error: string }> = {}
    for (const f of list) if (reports.value[f]) next[f] = reports.value[f]
    reports.value = next
    analyzing.value = true
    for (const f of list) {
      if (reports.value[f]) continue
      try {
        reports.value[f] = await api.videoAnalyze(f, props.hint)
      } catch (e) {
        reports.value[f] = { error: errorText(e) }
      }
      if (my !== token) return
    }
    analyzing.value = false
    const recs = list.map((f) => reports.value[f]).filter(isReport).map((r) => r.recommended)
    if (recs.length) {
      const top = [...new Set(recs)].sort((a, b) => recs.filter((x) => x === b).length - recs.filter((x) => x === a).length)[0]
      const p = presets.value.find((x) => x.id === top)
      if (p) applyPreset(p)
    }
  },
  { immediate: true },
)

const recommended = computed(() => {
  const recs = files.value.map((f) => reports.value[f]).filter(isReport).map((r) => r.recommended)
  return recs.length ? [...new Set(recs)].sort((a, b) => recs.filter((x) => x === b).length - recs.filter((x) => x === a).length)[0] : ''
})

const name = (p: string) => p.split(/[\\/]/).pop() ?? p
const CH: Record<number, string> = { 1: '单声道', 2: '立体声', 6: '5.1 声道', 8: '7.1 声道' }

function summary(f: VideoFactsInfo): string {
  const v = f.video
  if (!v) return ''
  const swap = v.rotation === 90 || v.rotation === 270
  const parts = [`${swap ? v.height : v.width}×${swap ? v.width : v.height}`, v.codec.toUpperCase()]
  if (v.fps) parts.push(`${+v.fps.toFixed(2)} 帧`)
  if (v.hdr !== 'none') parts.push(v.hdr === 'hlg' ? 'HLG' : 'HDR10')
  else if (v.bitDepth > 8) parts.push(`${v.bitDepth} 位`)
  if (f.audio) parts.push(`${+(f.audio.sampleRate / 1000).toFixed(1)} kHz ${CH[f.audio.channels] ?? `${f.audio.channels} 声道`}`)
  else parts.push('没有音轨')
  return parts.join(' · ')
}

const presetName = (id: string) => presets.value.find((p) => p.id === id)?.name ?? id
const curPreset = computed(() => presets.value.find((p) => p.id === presetId.value))

const loudnessOn = computed({
  get: () => spec.loudness !== null,
  set: (v: boolean) => (spec.loudness = v ? -16 : null),
})

const BOXES = [
  { v: '1920x1080', t: '1920×1080（横屏 1080p）' },
  { v: '1280x720', t: '1280×720（横屏 720p）' },
  { v: '3840x2160', t: '3840×2160（横屏 4K）' },
  { v: '1080x1920', t: '1080×1920（竖屏 1080p）' },
  { v: '720x1280', t: '720×1280（竖屏 720p）' },
]
const box = computed({
  get: () => `${spec.width}x${spec.height}`,
  set: (v: string) => {
    const [w, h] = v.split('x').map(Number)
    spec.width = w
    spec.height = h
  },
})
const FPS_VALUES = [23.976, 24, 25, 29.97, 30, 50, 59.94, 60]

async function pickLut() {
  const r = await open({ multiple: false, filters: [{ name: '3D LUT', extensions: ['cube'] }] })
  if (typeof r === 'string') spec.lut = r
}

async function pickDir() {
  const d = await open({ directory: true, multiple: false })
  if (typeof d === 'string') outDir.value = d
}

async function makePreview() {
  const f = files.value[0]
  if (!f) return
  const ms = parseClock(previewAt.value)
  if (ms === null) return ElMessage.warning('时间点的写法不对，请用 5、0:05 或 1:02:03 这样的格式。')
  previewing.value = true
  try {
    preview.value = await api.videoPreview(f, { ...spec }, ms)
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    previewing.value = false
  }
}

async function start() {
  if (!files.value.length) return ElMessage.warning('请先选择文件。')
  busy.value = true
  try {
    for (const f of files.value) {
      await api.mediaJobStart({ op: 'normalize', spec: { ...spec }, preset: presetId.value, inputs: [f], outputDir: outDir.value || null })
    }
    ElMessage.success(files.value.length > 1 ? `已加入 ${files.value.length} 个任务，可以在下方查看进度。` : '已开始处理，可以在下方查看进度。')
    emit('started')
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    busy.value = false
  }
}

async function stabilize() {
  if (!files.value.length) return ElMessage.warning('请先选择文件。')
  stabBusy.value = true
  try {
    for (const f of files.value) await api.mediaJobStart({ op: 'stabilize', strength: stab.value, inputs: [f], outputDir: outDir.value || null })
    ElMessage.success('已开始防抖，可以在下方查看进度。')
    emit('started')
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    stabBusy.value = false
  }
}

const stabNote = computed(() => {
  switch (caps.value?.stabilize) {
    case 'vidstab':
      return '使用 vidstab 两遍防抖：先分析抖动，再平滑并适当放大裁掉边缘，效果好，耗时约为视频长度的 1–2 倍。'
    case 'deshake':
      return '当前的 ffmpeg 没有 vidstab，会改用 deshake，效果较弱。在“设置 → 组件”里安装完整版 ffmpeg 可以使用 vidstab。'
    case 'none':
      return '当前的 ffmpeg 没有防抖滤镜。在“设置 → 组件”里安装完整版 ffmpeg 后可以使用。'
    default:
      return ''
  }
})

const imgSrc = (p: string) => convertFileSrc(p)
</script>

<template>
  <div class="nt">
    <div class="card box">
      <h3>视频规整</h3>
      <p class="mute desc">把手机录屏、直播录制、平台下载的成片统一成同一种规格：画面尺寸、固定帧率、色彩（HDR 转 SDR、LUT、自动色阶）、黑边、声音响度。不会修改原文件，处理结果会生成新文件。</p>
      <FileInput v-model="files" multiple :kinds="['video']" :extensions="['mp4', 'mkv', 'webm', 'mov', 'avi', 'flv', 'ts', 'm4v']" label="视频" />

      <div v-if="files.length" class="found">
        <div v-for="f in files" :key="f" class="file">
          <div class="fn ellipsis selectable" :title="f">{{ name(f) }}</div>
          <div v-if="!reports[f]" class="mute small">检测中…</div>
          <div v-else-if="!isReport(reports[f])" class="err small">{{ (reports[f] as { error: string }).error }}</div>
          <template v-else>
            <div class="mute small mono">{{ summary((reports[f] as VideoReport).facts) }}</div>
            <div class="chips">
              <el-tooltip v-for="i in (reports[f] as VideoReport).issues" :key="i.id" :content="i.detail || i.label" placement="top" :disabled="!i.detail">
                <span class="chip" :class="i.level">{{ i.label }}</span>
              </el-tooltip>
              <span v-if="!(reports[f] as VideoReport).issues.length" class="chip ok">没发现问题</span>
              <span class="rec">推荐：{{ presetName((reports[f] as VideoReport).recommended) }}</span>
            </div>
          </template>
        </div>
      </div>

      <h4>预设</h4>
      <div class="presets">
        <button v-for="p in presets" :key="p.id" type="button" class="preset" :class="{ on: presetId === p.id }" @click="applyPreset(p)">
          <span class="pn">{{ p.name }}<span v-if="recommended === p.id" class="badge">推荐</span></span>
          <span class="pd">{{ p.desc }}</span>
        </button>
      </div>

      <el-alert v-if="caps && caps.notes.length" type="info" :closable="false" show-icon class="caps">
        <template #title>当前 ffmpeg（{{ caps.edition === 'full' ? '完整版' : '精简版' }} {{ caps.version }}）有这些限制</template>
        <ul>
          <li v-for="n in caps.notes" :key="n">{{ n }}</li>
        </ul>
        <el-button size="small" link type="primary" @click="app.goSettings('components')">去“设置 → 组件”安装完整版 ffmpeg</el-button>
      </el-alert>

      <el-button link type="primary" class="adv" @click="showAdvanced = !showAdvanced">{{ showAdvanced ? '收起详细设置' : '详细设置（可以改预设里的每一项）' }}</el-button>
      <div v-show="showAdvanced" class="form">
        <h5>画面</h5>
        <label>
          <span>尺寸</span>
          <el-select v-model="spec.size" size="small" class="w">
            <el-option value="keep" label="保持原样" />
            <el-option value="limit" label="缩小到不超过…（不放大）" />
            <el-option value="fit" label="统一到目标尺寸，补黑边" />
            <el-option value="blur" label="统一到目标尺寸，模糊背景补边" />
            <el-option value="fill" label="统一到目标尺寸，铺满并裁掉多余" />
          </el-select>
        </label>
        <label v-if="spec.size === 'limit'">
          <span>短边上限</span>
          <el-select v-model="spec.shortSide" size="small" class="w">
            <el-option :value="2160" label="2160（4K）" />
            <el-option :value="1440" label="1440" />
            <el-option :value="1080" label="1080" />
            <el-option :value="720" label="720" />
            <el-option :value="480" label="480" />
          </el-select>
        </label>
        <template v-if="['fit', 'blur', 'fill'].includes(spec.size)">
          <label>
            <span>目标尺寸</span>
            <el-select v-model="box" size="small" class="w"><el-option v-for="b in BOXES" :key="b.v" :value="b.v" :label="b.t" /></el-select>
          </label>
          <label>
            <span>跟随素材方向</span>
            <el-switch v-model="spec.followOrientation" />
            <small class="mute">打开后，竖屏素材自动用竖屏的目标尺寸（例如 1080×1920），横屏素材用横屏的。</small>
          </label>
        </template>
        <label>
          <span>去黑边</span>
          <el-switch v-model="spec.autocrop" />
          <small class="mute">在视频里取几处画面检测，上下左右至少有 8 像素黑边才会裁；画面本身很暗时不会误裁。</small>
        </label>

        <h5>帧率</h5>
        <label>
          <span>帧率</span>
          <el-select v-model="spec.fps" size="small" class="w">
            <el-option value="keep" label="保持原样" />
            <el-option value="auto" label="可变帧率转固定（本来就固定的不动）" />
            <el-option value="fixed" label="统一为…" />
          </el-select>
        </label>
        <label v-if="spec.fps === 'fixed'">
          <span>统一帧率</span>
          <el-select v-model="spec.fpsValue" size="small" class="w"><el-option v-for="v in FPS_VALUES" :key="v" :value="v" :label="`${v} 帧`" /></el-select>
        </label>

        <h5>色彩</h5>
        <label>
          <span>HDR 转 SDR</span>
          <el-switch v-model="spec.hdr" />
          <small class="mute">检测到 HDR10 / HLG 时转成普通的 SDR，避免在普通播放器和剪辑软件里发灰。</small>
        </label>
        <label>
          <span>补全色彩信息</span>
          <el-switch v-model="spec.fixColor" />
          <small class="mute">补写缺失的色彩标记；全范围色彩转电视范围；高清素材的 BT.601 转 BT.709。</small>
        </label>
        <label>
          <span>自动色阶</span>
          <el-slider v-model="spec.levels" :min="0" :max="1" :step="0.05" :format-tooltip="(v: number) => `${Math.round(v * 100)}%`" size="small" class="sl" />
          <small class="mute">0 为关闭。把画面最暗和最亮的位置拉到黑和白，适合发灰、偏暗的素材。</small>
        </label>
        <label>
          <span>LUT</span>
          <span class="dirbox">
            <el-input :model-value="spec.lut ?? ''" size="small" readonly placeholder="未选择（.cube 文件）" class="w" />
            <el-button size="small" @click="pickLut">选择…</el-button>
            <el-button v-if="spec.lut" size="small" link @click="spec.lut = null">清除</el-button>
          </span>
        </label>
        <label v-if="spec.lut">
          <span>LUT 强度</span>
          <el-slider v-model="spec.lutStrength" :min="0" :max="1" :step="0.05" :format-tooltip="(v: number) => `${Math.round(v * 100)}%`" size="small" class="sl" />
        </label>

        <h5>声音</h5>
        <label>
          <span>采样率</span>
          <el-select v-model="spec.audioRate" size="small" class="w">
            <el-option :value="0" label="保持原样" />
            <el-option :value="44100" label="44.1 kHz" />
            <el-option :value="48000" label="48 kHz" />
          </el-select>
        </label>
        <label>
          <span>声道</span>
          <el-select v-model="spec.audioChannels" size="small" class="w">
            <el-option :value="0" label="保持原样" />
            <el-option :value="1" label="单声道" />
            <el-option :value="2" label="立体声" />
          </el-select>
        </label>
        <label>
          <span>响度标准化</span>
          <el-switch v-model="loudnessOn" />
          <span v-if="spec.loudness !== null" class="dirbox">
            <el-select v-model="spec.loudness" size="small" class="w">
              <el-option :value="-14" label="-14 LUFS（短视频、流媒体）" />
              <el-option :value="-16" label="-16 LUFS（通用）" />
              <el-option :value="-23" label="-23 LUFS（广播）" />
            </el-select>
          </span>
          <small class="mute">先测量整段音频再调整（两遍），保持原有的动态，不会忽大忽小。</small>
        </label>
        <label>
          <span>修正音画不同步</span>
          <el-switch v-model="spec.fixSync" />
          <small class="mute">直播录制（FLV / TS）和可变帧率的视频常有时间戳跳变，打开后会修正。</small>
        </label>

        <h5>输出</h5>
        <label>
          <span>视频编码</span>
          <el-select v-model="spec.codec" size="small" class="w">
            <el-option value="keep" label="能不重新编码就不动" />
            <el-option value="h264" label="H.264（兼容性最好）" />
            <el-option value="hevc" label="HEVC（体积更小）" :disabled="!!caps && !caps.hevc" />
          </el-select>
          <small v-if="caps && !caps.hevc" class="mute">当前的 ffmpeg 没有 HEVC 编码器，选 HEVC 时会改用 H.264。</small>
        </label>
        <label>
          <span>画质</span>
          <el-radio-group v-model="spec.quality" size="small">
            <el-radio-button value="small">体积小</el-radio-button>
            <el-radio-button value="balanced">均衡</el-radio-button>
            <el-radio-button value="high">画质优先</el-radio-button>
          </el-radio-group>
          <small v-if="caps" class="mute">视频编码器：{{ caps.h264 ?? '没有 H.264 编码器' }}{{ caps.hardware.length ? `；可用硬件编码：${caps.hardware.join('、')}` : '' }}</small>
        </label>
      </div>

      <h4>预览对比</h4>
      <div class="pv">
        <el-input v-model="previewAt" size="small" class="t mono" placeholder="0:02" />
        <el-button size="small" :loading="previewing" :disabled="!files.length" @click="makePreview">生成第一个文件的对比图</el-button>
        <small class="mute">只含画面处理（尺寸、HDR、色阶、LUT、去黑边），不含帧率和声音。</small>
      </div>
      <div v-if="preview" class="cmp">
        <figure>
          <img :src="imgSrc(preview.before)" alt="处理前" />
          <figcaption>处理前（普通播放器里的样子）</figcaption>
        </figure>
        <figure>
          <img :src="imgSrc(preview.after)" alt="处理后" />
          <figcaption>{{ preview.changed ? '处理后' : '处理后（画面不需要处理）' }}</figcaption>
        </figure>
        <div v-if="preview.notes.length || preview.warnings.length" class="pnotes mute small">
          <div v-for="n in preview.notes" :key="n">· {{ n }}</div>
          <div v-for="w in preview.warnings" :key="w" class="warn">注意：{{ w }}</div>
        </div>
      </div>

      <label class="dir">
        <span>保存到</span>
        <span class="dirbox"><el-input v-model="outDir" size="small" clearable placeholder="留空放在原文件旁边" class="w" /><el-button size="small" @click="pickDir">选择…</el-button></span>
      </label>
      <div class="go">
        <el-button type="primary" :loading="busy" :disabled="!files.length || analyzing" @click="start">
          开始规整{{ files.length > 1 ? `（${files.length} 个文件）` : '' }}{{ curPreset ? ` · ${curPreset.name}` : '' }}
        </el-button>
        <small class="mute">已经符合规格的文件不会重新编码；没有需要改的会直接告诉你。</small>
      </div>
    </div>

    <div class="card box">
      <h3>防抖</h3>
      <p class="mute desc">稳定手持拍摄的抖动画面，会略微放大裁掉边缘。声音不动。</p>
      <div class="pv">
        <el-radio-group v-model="stab" size="small">
          <el-radio-button value="light">轻微</el-radio-button>
          <el-radio-button value="normal">标准</el-radio-button>
          <el-radio-button value="strong">强力</el-radio-button>
        </el-radio-group>
        <el-button size="small" type="primary" :loading="stabBusy" :disabled="!files.length || caps?.stabilize === 'none'" @click="stabilize">对上面选中的文件防抖</el-button>
      </div>
      <small class="mute">{{ stabNote }}</small>
    </div>
  </div>
</template>

<style scoped>
.nt {
  display: flex;
  flex-direction: column;
  gap: 14px;
}
.box {
  padding: 14px 18px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
h3 {
  margin: 0;
  font-size: 15px;
}
h4 {
  margin: 6px 0 0;
  font-size: 13px;
}
h5 {
  margin: 8px 0 0;
  font-size: 12px;
  color: var(--cc-mute);
  font-weight: 600;
}
.desc {
  margin: 0;
  font-size: 12.5px;
}
.small {
  font-size: 11.5px;
}
.found {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.file {
  border: 1px solid var(--cc-line);
  border-radius: 8px;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.fn {
  font-size: 13px;
}
.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}
.chip {
  font-size: 11px;
  padding: 1px 8px;
  border-radius: 10px;
  background: var(--cc-acc-soft);
  color: var(--cc-fg);
}
.chip.warn {
  background: var(--cc-warn-soft, #fdf0d8);
  color: var(--cc-warn, #b26a00);
}
.chip.ok {
  background: var(--cc-ok-soft, #e1f4e8);
  color: var(--cc-ok, #1b7f46);
}
.rec {
  font-size: 11.5px;
  color: var(--cc-acc);
  margin-left: auto;
}
.err {
  color: var(--cc-err);
}
.presets {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 8px;
}
.preset {
  all: unset;
  cursor: pointer;
  box-sizing: border-box;
  border: 1.5px solid var(--cc-line);
  border-radius: 9px;
  padding: 8px 10px;
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.preset:hover {
  border-color: var(--cc-acc);
}
.preset.on {
  border-color: var(--cc-acc);
  background: var(--cc-acc-soft);
}
.preset:focus-visible {
  outline: 2px solid var(--cc-acc);
}
.pn {
  font-size: 13px;
  font-weight: 600;
}
.badge {
  margin-left: 6px;
  font-size: 10.5px;
  font-weight: 500;
  padding: 0 6px;
  border-radius: 8px;
  background: var(--cc-acc);
  color: #fff;
}
.pd {
  font-size: 11.5px;
  color: var(--cc-mute);
  line-height: 1.5;
}
.caps ul {
  margin: 4px 0;
  padding-left: 18px;
  font-size: 12px;
}
.adv {
  align-self: flex-start;
}
.form {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.form label,
.dir {
  display: grid;
  grid-template-columns: 120px auto;
  gap: 10px;
  align-items: center;
  justify-content: start;
}
.form label > span:first-child,
.dir > span:first-child {
  color: var(--cc-mute);
  font-size: 12.5px;
}
.form label small {
  grid-column: 2;
  font-size: 11.5px;
  max-width: 560px;
}
.w {
  width: 260px;
}
.sl {
  width: 340px;
}
.dirbox {
  display: flex;
  gap: 8px;
  align-items: center;
}
.pv {
  display: flex;
  gap: 10px;
  align-items: center;
  flex-wrap: wrap;
}
.t {
  width: 90px;
}
.cmp {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
}
figure {
  margin: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
figure img {
  width: 100%;
  border-radius: 8px;
  border: 1px solid var(--cc-line);
  background: #000;
}
figcaption {
  font-size: 11.5px;
  color: var(--cc-mute);
  text-align: center;
}
.pnotes {
  grid-column: 1 / -1;
}
.warn {
  color: var(--cc-warn, #b26a00);
}
.go {
  display: flex;
  gap: 12px;
  align-items: center;
  margin-top: 4px;
}
</style>
