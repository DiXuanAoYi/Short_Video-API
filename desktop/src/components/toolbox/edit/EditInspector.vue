<script setup lang="ts">
// 检查器：选中的片段 / 文字 / 音频的属性，以及输出设置。
import { computed, ref, watch } from 'vue'
import { open } from '@tauri-apps/plugin-dialog'
import { ElMessage } from 'element-plus'
import { errorText } from '../../../api'
import type { EditApi } from '../../../composables/useEditProject'
import type { RegionApi } from '../../../composables/useRegions'
import type { EditClip, EditOutput } from '../../../types'
import { MIN_MS, MIN_TRANSITION_MS, TRANSITIONS, clipDuration, msText, outputSize } from '../../../utils/edit'
import EditField from './EditField.vue'
import EditRegions from './EditRegions.vue'
import EditSlider from './EditSlider.vue'
import TimeInput from './TimeInput.vue'

const props = defineProps<{ ed: EditApi; rg: RegionApi }>()
const emit = defineEmits<{ goto: [ms: number] }>()
const ed = props.ed
const outDir = defineModel<string>('outDir', { default: '' })

const tab = ref<'item' | 'region' | 'out'>('item')
watch(
  () => ed.sel.value,
  (s) => {
    // 选了别的片段时留在“区域”页，方便连着给几个镜头加；选了文字、配乐就回到“属性”
    if (s && !(tab.value === 'region' && s.kind === 'clip')) tab.value = 'item'
  },
)

const clip = ed.selectedClip
const text = ed.selectedText
const audio = ed.selectedAudio
const src = computed(() => (clip.value ? ed.sources[clip.value.path] : audio.value ? ed.sources[audio.value.path] : undefined))
const index = computed(() => (clip.value ? ed.project.value.clips.findIndex((c) => c.id === clip.value?.id) : -1))
const placed = computed(() => (index.value >= 0 ? ed.placed.value[index.value] : undefined))
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p

// ---------- 修改 ----------

function pc(patch: Partial<EditClip>, key: string) {
  if (clip.value) ed.patchClip(clip.value.id, patch, `${key}:${clip.value.id}`)
}
const pt = (patch: Parameters<typeof ed.patchText>[1], key: string) => text.value && ed.patchText(text.value.id, patch, `${key}:${text.value.id}`)
const pa = (patch: Parameters<typeof ed.patchAudio>[1], key: string) => audio.value && ed.patchAudio(audio.value.id, patch, `${key}:${audio.value.id}`)

const clipLimit = computed(() => src.value?.durationMs ?? Number.MAX_SAFE_INTEGER)

function setTransition(kind: string) {
  if (!clip.value) return
  if (!kind || kind === 'none') pc({ transition: null }, 'tr')
  else pc({ transition: { kind, durationMs: clip.value.transition?.durationMs ?? 500 } }, 'tr')
}
function setTransitionMs(ms: number) {
  if (clip.value?.transition) pc({ transition: { ...clip.value.transition, durationMs: Math.round(ms) } }, 'trd')
}
const appliedOverlap = computed(() => placed.value?.overlapMs ?? 0)

function resetTone() {
  pc({ brightness: 0, contrast: 1, saturation: 1 }, 'tone')
}
const toned = computed(() => !!clip.value && (clip.value.brightness !== 0 || clip.value.contrast !== 1 || clip.value.saturation !== 1))

async function relink() {
  const c = clip.value
  const a = audio.value
  const old = c?.path ?? a?.path
  if (!old) return
  try {
    const r = await open({ multiple: false })
    if (!r || Array.isArray(r)) return
    await ed.relink(old, r)
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

async function pickFont() {
  try {
    const r = await open({ multiple: false, filters: [{ name: '字体', extensions: ['ttf', 'otf', 'ttc'] }] })
    if (r && !Array.isArray(r)) pt({ font: r }, 'font')
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

/** 配乐的终点：勾上“到结尾”就清空，取消勾选时落在素材结尾。 */
function audioToEnd(v: unknown) {
  const a = audio.value
  if (a) pa({ outMs: v ? null : (src.value?.durationMs ?? a.inMs + 1000) }, 'ao')
}

function stampStart() {
  const t = text.value
  if (!t) return
  const at = Math.round(ed.playhead.value)
  pt({ startMs: at, endMs: Math.max(t.endMs, at + MIN_MS) }, 'tstart')
}
function stampEnd() {
  const t = text.value
  if (!t) return
  const at = Math.round(ed.playhead.value)
  if (at > t.startMs + MIN_MS - 1) pt({ endMs: at }, 'tend')
}

// ---------- 输出 ----------

const RES: { key: string; label: string; w: number; h: number }[] = [
  { key: 'auto', label: '跟随第一个片段', w: 0, h: 0 },
  { key: '1920x1080', label: '1920×1080 横屏 1080p', w: 1920, h: 1080 },
  { key: '1280x720', label: '1280×720 横屏 720p', w: 1280, h: 720 },
  { key: '3840x2160', label: '3840×2160 横屏 4K', w: 3840, h: 2160 },
  { key: '1080x1920', label: '1080×1920 竖屏', w: 1080, h: 1920 },
  { key: '720x1280', label: '720×1280 竖屏', w: 720, h: 1280 },
  { key: '1080x1080', label: '1080×1080 方形', w: 1080, h: 1080 },
  { key: '1080x1350', label: '1080×1350 竖版 4:5', w: 1080, h: 1350 },
]
const out = computed(() => ed.project.value.out)
const resKey = computed(() => {
  const o = out.value
  const hit = RES.find((r) => r.w === o.width && r.h === o.height)
  return hit?.key ?? 'custom'
})
function setRes(key: string) {
  if (key === 'custom') {
    const s = outputSize(ed.project.value, ed.sources)
    setOut({ width: s.w, height: s.h })
    return
  }
  const r = RES.find((x) => x.key === key)
  if (r) setOut({ width: r.w, height: r.h })
}
function setOut(patch: Partial<EditOutput>) {
  ed.mutate((p) => Object.assign(p.out, patch), `out:${Object.keys(patch).join(',')}`)
}
const FPS = [0, 24, 25, 30, 50, 60]
const fpsOptions = computed(() => (FPS.includes(out.value.fps) ? FPS : [...FPS, out.value.fps]))

async function pickDir() {
  try {
    const r = await open({ directory: true, multiple: false })
    if (r && !Array.isArray(r)) outDir.value = r
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}
</script>

<template>
  <div class="insp">
    <el-radio-group v-model="tab" size="small" class="seg">
      <el-radio-button value="item">属性</el-radio-button>
      <el-radio-button value="region">区域</el-radio-button>
      <el-radio-button value="out">输出</el-radio-button>
    </el-radio-group>

    <!-- 片段 -->
    <div v-if="tab === 'item' && clip" class="sec">
      <div class="title">
        <span class="chip">{{ clip.kind === 'image' ? '图片' : '视频' }}</span>
        <span class="ellipsis" :title="clip.path">{{ baseName(clip.path) }}</span>
      </div>
      <div v-if="clip.path in ed.broken" class="alert">
        <span>{{ ed.broken[clip.path] }}</span>
        <el-button size="small" type="primary" @click="relink">重新指定文件…</el-button>
      </div>
      <div v-else-if="src && src.hdr" class="alert soft">这个素材是 HDR，导出时不会自动转成普通画面，颜色可能偏灰。可以先在“视频规整”里转成普通画面。</div>

      <template v-if="clip.kind === 'video'">
        <EditField label="入点">
          <TimeInput :model-value="clip.inMs" :max="clip.outMs - MIN_MS" @update:model-value="(v) => pc({ inMs: v }, 'in')" />
          <el-button size="small" link @click="emit('goto', placed?.startMs ?? 0)">定位</el-button>
        </EditField>
        <EditField label="出点">
          <TimeInput :model-value="clip.outMs" :min="clip.inMs + MIN_MS" :max="clipLimit" @update:model-value="(v) => pc({ outMs: v }, 'out')" />
          <span class="mute small">素材共 {{ src?.durationMs ? msText(src.durationMs) : '—' }}</span>
        </EditField>
        <EditField label="成片里长">
          <span class="mono">{{ msText(clipDuration(clip)) }}</span>
          <span v-if="clip.speed !== 1" class="mute small">（{{ clip.speed }}× 速度）</span>
        </EditField>
        <EditSlider label="速度" :model-value="clip.speed" :min="0.25" :max="4" :step="0.05" unit="×" :digits="2" @update:model-value="(v) => pc({ speed: v }, 'spd')" />
        <template v-if="!src || src.hasAudio">
          <EditSlider label="音量" :model-value="clip.volume" :min="0" :max="4" :step="0.05" unit="%" :scale="100" :disabled="clip.mute" @update:model-value="(v) => pc({ volume: v }, 'vol')" />
          <EditField label="">
            <el-checkbox :model-value="clip.mute" @update:model-value="(v: unknown) => pc({ mute: !!v }, 'mute')">静音这个片段</el-checkbox>
          </EditField>
        </template>
        <div v-else class="mute small pad">这个素材没有声音。</div>
      </template>
      <template v-else>
        <EditField label="显示时长">
          <TimeInput :model-value="clip.outMs" :min="MIN_MS" @update:model-value="(v) => pc({ outMs: v }, 'out')" />
        </EditField>
      </template>

      <EditField label="淡入（秒）" hint="画面从黑色淡入，声音同步">
        <el-input-number :model-value="clip.fadeInMs / 1000" :min="0" :max="clipDuration(clip) / 1000" :step="0.1" :precision="1" size="small" controls-position="right" @update:model-value="(v: number | undefined) => pc({ fadeInMs: Math.round((v ?? 0) * 1000) }, 'fi')" />
      </EditField>
      <EditField label="淡出（秒）" hint="画面淡出到黑色，声音同步">
        <el-input-number :model-value="clip.fadeOutMs / 1000" :min="0" :max="clipDuration(clip) / 1000" :step="0.1" :precision="1" size="small" controls-position="right" @update:model-value="(v: number | undefined) => pc({ fadeOutMs: Math.round((v ?? 0) * 1000) }, 'fo')" />
      </EditField>

      <EditField label="画面方向">
        <el-radio-group :model-value="clip.rotate" size="small" @update:model-value="(v: string | number | boolean | undefined) => pc({ rotate: Number(v) as EditClip['rotate'] }, 'rot')">
          <el-radio-button :value="0">不转</el-radio-button>
          <el-radio-button :value="90">右转 90°</el-radio-button>
          <el-radio-button :value="180">180°</el-radio-button>
          <el-radio-button :value="270">左转 90°</el-radio-button>
        </el-radio-group>
      </EditField>
      <EditField label="">
        <el-checkbox :model-value="clip.flipH" @update:model-value="(v: unknown) => pc({ flipH: !!v }, 'fh')">左右翻转</el-checkbox>
        <el-checkbox :model-value="clip.flipV" @update:model-value="(v: unknown) => pc({ flipV: !!v }, 'fv')">上下翻转</el-checkbox>
      </EditField>

      <EditSlider label="亮度" :model-value="clip.brightness" :min="-1" :max="1" :step="0.01" :scale="100" :digits="0" @update:model-value="(v) => pc({ brightness: v }, 'tone')" />
      <EditSlider label="对比度" :model-value="clip.contrast" :min="0" :max="3" :step="0.01" :scale="100" unit="%" @update:model-value="(v) => pc({ contrast: v }, 'tone')" />
      <EditSlider label="饱和度" :model-value="clip.saturation" :min="0" :max="3" :step="0.01" :scale="100" unit="%" @update:model-value="(v) => pc({ saturation: v }, 'tone')" />
      <EditField label="">
        <el-button size="small" link type="primary" :disabled="!toned" @click="resetTone">调色归零</el-button>
        <span class="mute small">监视器里的颜色是近似效果，以导出为准。</span>
      </EditField>

      <template v-if="index > 0">
        <EditField label="转场" hint="和上一个片段之间的过渡">
          <el-select :model-value="clip.transition?.kind ?? 'none'" size="small" style="width: 150px" @update:model-value="setTransition">
            <el-option value="none" label="无（直接切换）" />
            <el-option v-for="t in TRANSITIONS" :key="t.id" :value="t.id" :label="t.label" />
          </el-select>
        </EditField>
        <EditSlider v-if="clip.transition" label="转场时长" :model-value="clip.transition.durationMs" :min="MIN_TRANSITION_MS" :max="3000" :step="50" unit=" 秒" :scale="0.001" :digits="2" @update:model-value="setTransitionMs" />
        <div v-if="clip.transition && appliedOverlap < clip.transition.durationMs" class="mute small pad">
          {{ appliedOverlap ? `片段太短，实际转场 ${(appliedOverlap / 1000).toFixed(2)} 秒。` : '片段太短放不下这个转场，会直接切换。' }}
        </div>
        <div v-if="clip.transition" class="mute small pad">转场在监视器里不显示，点“生成预览”查看。</div>
      </template>
    </div>

    <!-- 文字 -->
    <div v-else-if="tab === 'item' && text" class="sec">
      <div class="title"><span class="chip">文字</span></div>
      <el-input :model-value="text.text" type="textarea" :rows="3" resize="none" placeholder="输入文字，可以多行" @update:model-value="(v: string) => pt({ text: v }, 'txt')" />
      <EditField label="出现于">
        <TimeInput :model-value="text.startMs" :max="text.endMs - MIN_MS" @update:model-value="(v) => pt({ startMs: v }, 'ts')" />
        <el-button size="small" link @click="stampStart">取播放位置</el-button>
      </EditField>
      <EditField label="消失于">
        <TimeInput :model-value="text.endMs" :min="text.startMs + MIN_MS" @update:model-value="(v) => pt({ endMs: v }, 'te')" />
        <el-button size="small" link @click="stampEnd">取播放位置</el-button>
      </EditField>
      <EditSlider label="左右位置" :model-value="text.x" :min="0" :max="1" :step="0.005" unit="%" :scale="100" hint="文字中心离画面左边的距离。也可以在监视器里直接拖动文字" @update:model-value="(v) => pt({ x: v }, 'tx')" />
      <EditSlider label="上下位置" :model-value="text.y" :min="0" :max="1" :step="0.005" unit="%" :scale="100" @update:model-value="(v) => pt({ y: v }, 'ty')" />
      <EditSlider label="字号" :model-value="text.size" :min="1" :max="30" :step="0.5" unit="%" :digits="1" hint="占画面高度的百分比" @update:model-value="(v) => pt({ size: v }, 'tsz')" />
      <EditField label="颜色">
        <el-color-picker :model-value="text.color" size="small" @update:model-value="(v: string | null) => v && pt({ color: v }, 'tc')" />
      </EditField>
      <EditSlider label="不透明度" :model-value="text.opacity" :min="0.05" :max="1" :step="0.05" unit="%" :scale="100" @update:model-value="(v) => pt({ opacity: v }, 'to')" />
      <EditField label="描边">
        <el-checkbox :model-value="text.outline" @update:model-value="(v: unknown) => pt({ outline: !!v }, 'tol')">加描边</el-checkbox>
        <el-color-picker v-if="text.outline" :model-value="text.outlineColor" size="small" @update:model-value="(v: string | null) => v && pt({ outlineColor: v }, 'tolc')" />
      </EditField>
      <EditField label="底色块">
        <el-checkbox :model-value="text.boxed" @update:model-value="(v: unknown) => pt({ boxed: !!v }, 'tb')">文字后面加底色</el-checkbox>
        <el-color-picker v-if="text.boxed" :model-value="text.boxColor" size="small" @update:model-value="(v: string | null) => v && pt({ boxColor: v }, 'tbc')" />
      </EditField>
      <EditSlider v-if="text.boxed" label="底色浓度" :model-value="text.boxOpacity" :min="0.05" :max="1" :step="0.05" unit="%" :scale="100" @update:model-value="(v) => pt({ boxOpacity: v }, 'tbo')" />
      <EditField label="字体">
        <span class="ellipsis small" :title="text.font ?? ''">{{ text.font ? baseName(text.font) : '默认（自动找中文字体）' }}</span>
        <el-button size="small" link type="primary" @click="pickFont">选择字体文件…</el-button>
        <el-button v-if="text.font" size="small" link @click="pt({ font: null }, 'font')">恢复默认</el-button>
      </EditField>
      <div class="mute small pad">导出的文字用选定的字体文件绘制，监视器里用系统字体显示，字形会略有不同。</div>
    </div>

    <!-- 音频 -->
    <div v-else-if="tab === 'item' && audio" class="sec">
      <div class="title">
        <span class="chip">配乐</span>
        <span class="ellipsis" :title="audio.path">{{ baseName(audio.path) }}</span>
      </div>
      <div v-if="audio.path in ed.broken" class="alert">
        <span>{{ ed.broken[audio.path] }}</span>
        <el-button size="small" type="primary" @click="relink">重新指定文件…</el-button>
      </div>
      <EditField label="开始于">
        <TimeInput :model-value="audio.startMs" @update:model-value="(v) => pa({ startMs: v }, 'as')" />
        <el-button size="small" link @click="pa({ startMs: Math.round(ed.playhead.value) }, 'as')">取播放位置</el-button>
      </EditField>
      <EditField label="素材起点">
        <TimeInput :model-value="audio.inMs" :max="(audio.outMs ?? clipLimit) - MIN_MS" @update:model-value="(v) => pa({ inMs: v }, 'ai')" />
      </EditField>
      <EditField label="素材终点">
        <TimeInput :model-value="audio.outMs ?? src?.durationMs ?? 0" :min="audio.inMs + MIN_MS" :max="clipLimit" :disabled="audio.outMs === null" @update:model-value="(v) => pa({ outMs: v }, 'ao')" />
        <el-checkbox :model-value="audio.outMs === null" @update:model-value="audioToEnd">到结尾</el-checkbox>
      </EditField>
      <EditSlider label="音量" :model-value="audio.volume" :min="0" :max="4" :step="0.05" unit="%" :scale="100" @update:model-value="(v) => pa({ volume: v }, 'av')" />
      <EditField label="淡入（秒）">
        <el-input-number :model-value="audio.fadeInMs / 1000" :min="0" :max="60" :step="0.5" :precision="1" size="small" controls-position="right" @update:model-value="(v: number | undefined) => pa({ fadeInMs: Math.round((v ?? 0) * 1000) }, 'afi')" />
      </EditField>
      <EditField label="淡出（秒）">
        <el-input-number :model-value="audio.fadeOutMs / 1000" :min="0" :max="60" :step="0.5" :precision="1" size="small" controls-position="right" @update:model-value="(v: number | undefined) => pa({ fadeOutMs: Math.round((v ?? 0) * 1000) }, 'afo')" />
      </EditField>
      <EditField label="">
        <el-checkbox :model-value="audio.looped" @update:model-value="(v: unknown) => pa({ looped: !!v }, 'al')">不够长时重复播放，直到视频结束</el-checkbox>
      </EditField>
      <EditField label="">
        <el-checkbox :model-value="audio.duck" @update:model-value="(v: unknown) => pa({ duck: !!v }, 'ad')">片段里有人声时自动压低这条配乐</el-checkbox>
      </EditField>
      <div class="mute small pad">监视器里配乐的音量最大 100%，自动压低也不会播放出来，导出时才生效。</div>
    </div>

    <div v-else-if="tab === 'item'" class="sec mute small">
      在时间线上点一个片段、文字或配乐，在这里修改它的属性。<br />
      点时间线空白处可以移动播放位置；拖动片段可以换顺序，拖动两端可以裁剪。
    </div>

    <!-- 区域 -->
    <div v-if="tab === 'region'" class="sec">
      <EditRegions :ed="ed" :rg="rg" @goto="(ms) => emit('goto', ms)" />
    </div>

    <!-- 输出 -->
    <div v-if="tab === 'out'" class="sec">
      <EditField label="文件名">
        <el-input :model-value="ed.project.value.title" size="small" placeholder="留空则用第一个片段的名字" clearable @update:model-value="(v: string) => ed.mutate((p) => (p.title = v), 'title')" />
      </EditField>
      <EditField label="画面尺寸">
        <el-select :model-value="resKey" size="small" style="width: 220px" @update:model-value="setRes">
          <el-option v-for="r in RES" :key="r.key" :value="r.key" :label="r.label" />
          <el-option value="custom" label="自定义…" />
        </el-select>
      </EditField>
      <EditField v-if="resKey === 'custom'" label="宽 × 高">
        <el-input-number :model-value="out.width" :min="64" :max="7680" :step="2" size="small" controls-position="right" @update:model-value="(v: number | undefined) => setOut({ width: v ?? 0 })" />
        <span>×</span>
        <el-input-number :model-value="out.height" :min="64" :max="7680" :step="2" size="small" controls-position="right" @update:model-value="(v: number | undefined) => setOut({ height: v ?? 0 })" />
      </EditField>
      <EditField label="帧率">
        <el-select :model-value="out.fps" size="small" style="width: 160px" @update:model-value="(v: number) => setOut({ fps: v })">
          <el-option v-for="f in fpsOptions" :key="f" :value="f" :label="f ? `${f} 帧/秒` : '跟随第一个视频'" />
        </el-select>
      </EditField>
      <EditField label="画面适配" hint="片段和输出画面比例不同时怎么处理">
        <el-select :model-value="out.fit" size="small" style="width: 220px" @update:model-value="(v: EditOutput['fit']) => setOut({ fit: v })">
          <el-option value="contain" label="完整显示，留黑边" />
          <el-option value="cover" label="铺满画面，裁掉多余部分" />
          <el-option value="blur" label="完整显示，背景用模糊画面" />
        </el-select>
      </EditField>
      <EditField label="画质">
        <el-radio-group :model-value="out.quality" size="small" @update:model-value="(v: string | number | boolean | undefined) => setOut({ quality: v as EditOutput['quality'] })">
          <el-radio-button value="small">体积小</el-radio-button>
          <el-radio-button value="balanced">均衡</el-radio-button>
          <el-radio-button value="high">高画质</el-radio-button>
        </el-radio-group>
      </EditField>
      <EditField label="编码">
        <el-radio-group :model-value="out.codec" size="small" @update:model-value="(v: string | number | boolean | undefined) => setOut({ codec: v as EditOutput['codec'] })">
          <el-radio-button value="h264">H.264（通用）</el-radio-button>
          <el-radio-button value="hevc">H.265（更小）</el-radio-button>
        </el-radio-group>
      </EditField>
      <EditField label="格式">
        <el-radio-group :model-value="out.format" size="small" @update:model-value="(v: string | number | boolean | undefined) => setOut({ format: v as EditOutput['format'] })">
          <el-radio-button value="mp4">MP4</el-radio-button>
          <el-radio-button value="mkv">MKV</el-radio-button>
        </el-radio-group>
      </EditField>
      <EditField label="保存到">
        <span class="ellipsis small" :title="outDir">{{ outDir || '默认（媒体库的下载文件夹）' }}</span>
        <el-button size="small" link type="primary" @click="pickDir">选择文件夹…</el-button>
        <el-button v-if="outDir" size="small" link @click="outDir = ''">恢复默认</el-button>
      </EditField>
    </div>
  </div>
</template>

<style scoped>
.insp {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
}
.seg {
  align-self: flex-start;
}
.sec {
  display: flex;
  flex-direction: column;
  gap: 4px;
  min-width: 0;
}
.title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  font-weight: 500;
  min-width: 0;
}
.small {
  font-size: 11.5px;
}
.pad {
  padding-left: 82px;
}
.alert {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 10px;
  border-radius: 6px;
  background: color-mix(in srgb, var(--cc-err) 14%, transparent);
  color: var(--cc-err);
  font-size: 12px;
}
.alert.soft {
  background: var(--cc-acc-soft);
  color: var(--cc-fg);
}
.ellipsis {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}
</style>
