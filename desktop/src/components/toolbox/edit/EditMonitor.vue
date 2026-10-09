<script setup lang="ts">
// 监视器：直接播放素材文件来预览（不含转场，转场请用“生成预览”），或者播放生成好的预览成片。
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import type { EditApi } from '../../../composables/useEditProject'
import type { EditClip, EditText } from '../../../types'
import { audioSpan, clamp, clipIndexAt, fadeFactor, outputSize, sourceTimeAt } from '../../../utils/edit'

const props = defineProps<{ ed: EditApi; mode: 'live' | 'preview'; previewUrl: string | null }>()
const ed = props.ed

const MAX_STAGE_H = 330
const wrap = ref<HTMLElement | null>(null)
const stageEl = ref<HTMLElement | null>(null)
const avail = reactive({ w: 480 })
let ro: ResizeObserver | undefined
onMounted(() => {
  if (!wrap.value) return
  avail.w = wrap.value.clientWidth || 480
  ro = new ResizeObserver(() => (avail.w = wrap.value?.clientWidth || 480))
  ro.observe(wrap.value)
})

const size = computed(() => outputSize(ed.project.value, ed.sources))
const stage = computed(() => {
  const { w, h } = size.value
  let sw = avail.w
  let sh = (sw * h) / w
  if (sh > MAX_STAGE_H) {
    sh = MAX_STAGE_H
    sw = (sh * w) / h
  }
  return { w: Math.max(80, Math.floor(sw)), h: Math.max(45, Math.floor(sh)) }
})

// ---------- 播放 ----------

interface Slot {
  path: string
}
const slots = reactive<Slot[]>([{ path: '' }, { path: '' }])
const vEls: (HTMLVideoElement | null)[] = [null, null]
const pending: (number | null)[] = [null, null]
const aEls = new Map<number, HTMLAudioElement>()
const pv = ref<HTMLVideoElement | null>(null)
const activeSlot = ref(0)
const curIdx = ref(-1)
const playError = ref('')

const total = computed(() => ed.total.value)
const clips = computed(() => ed.project.value.clips)

function setVideoEl(i: number, el: unknown) {
  vEls[i] = (el as HTMLVideoElement | null) ?? null
}
function setAudioEl(id: number, el: unknown) {
  if (el) aEls.set(id, el as HTMLAudioElement)
  else aEls.delete(id)
}

function onMeta(i: number) {
  const el = vEls[i]
  if (el && pending[i] !== null) {
    el.currentTime = pending[i] as number
    pending[i] = null
  }
}

function load(i: number, c: EditClip, at: number) {
  const el = vEls[i]
  const src = ed.sources[c.path]
  if (!el || !src) return
  slots[i].path = c.path
  pending[i] = at
  playError.value = ''
  el.src = src.url
  el.load()
  el.currentTime = at
}

function sync(t: number, seek: boolean) {
  if (props.mode === 'preview') return
  const list = clips.value
  const pl = ed.placed.value
  let i = clipIndexAt(pl, t)
  let tt = t
  if (i < 0 && list.length) {
    i = list.length - 1
    tt = pl[i].endMs - 1
  }
  curIdx.value = i
  const c = list[i]
  const playing = ed.playing.value
  if (!c || c.kind === 'image' || !ed.sources[c.path]) {
    for (const el of vEls) if (el && !el.paused) el.pause()
  } else {
    const want = sourceTimeAt(c, pl[i], tt) / 1000
    const near = (s: number) => slots[s].path === c.path && vEls[s] !== null && Math.abs((vEls[s] as HTMLVideoElement).currentTime - want) < 0.35
    const cur = activeSlot.value
    let s = near(cur) ? cur : near(1 - cur) ? 1 - cur : -1
    if (s < 0 && !playing && slots[cur].path === c.path) s = cur
    if (s < 0) {
      s = 1 - cur
      load(s, c, want)
    }
    activeSlot.value = s
    const el = vEls[s]
    if (el) {
      el.playbackRate = clamp(c.speed, 0.25, 4)
      el.muted = c.mute || !playing
      el.volume = clamp(c.volume, 0, 1)
      if (pending[s] === null && (seek || Math.abs(el.currentTime - want) > 0.35)) el.currentTime = want
      if (playing && el.paused) el.play().catch(() => undefined)
      else if (!playing && !el.paused) el.pause()
    }
    const other = vEls[1 - s]
    if (other && !other.paused) other.pause()
    // 预先装好下一个片段，切换时不会卡
    const nx = list[i + 1]
    if (nx && nx.kind === 'video' && ed.sources[nx.path] && slots[1 - s].path !== nx.path) load(1 - s, nx, nx.inMs / 1000)
  }
  syncMusic(t, playing)
}

function syncMusic(t: number, playing: boolean) {
  const end = total.value
  for (const a of ed.project.value.audio) {
    const el = aEls.get(a.id)
    const src = ed.sources[a.path]
    if (!el || !src) continue
    const span = audioSpan(a, src, end)
    const active = playing && t >= a.startMs && t < a.startMs + span
    if (!active) {
      if (!el.paused) el.pause()
      continue
    }
    const seg = a.outMs !== null ? a.outMs - a.inMs : Math.max(1, (src.durationMs ?? 1) - a.inMs)
    const local = t - a.startMs
    const want = (a.inMs + (a.looped ? local % Math.max(1, seg) : local)) / 1000
    let f = 1
    if (a.fadeInMs > 0) f = Math.min(f, local / a.fadeInMs)
    if (a.fadeOutMs > 0) f = Math.min(f, (span - local) / a.fadeOutMs)
    el.volume = clamp(a.volume * clamp(f, 0, 1), 0, 1)
    if (Math.abs(el.currentTime - want) > 0.35) el.currentTime = want
    if (el.paused) el.play().catch(() => undefined)
  }
}

function pauseAll() {
  for (const el of vEls) if (el && !el.paused) el.pause()
  for (const el of aEls.values()) if (!el.paused) el.pause()
  if (pv.value && !pv.value.paused) pv.value.pause()
}

let raf = 0
let last = 0
let clock = 0
function tick(now: number) {
  if (!ed.playing.value) return
  const dt = Math.min(100, now - last)
  last = now
  if (props.mode === 'preview') clock = (pv.value?.currentTime ?? 0) * 1000
  else clock += dt
  const end = props.mode === 'preview' ? Math.min(total.value, (pv.value?.duration ?? total.value) * 1000) : total.value
  if (clock >= end - 1 || (props.mode === 'preview' && pv.value?.ended)) {
    ed.playhead.value = end
    ed.playing.value = false
    return
  }
  ed.playhead.value = clock
  sync(clock, false)
  raf = requestAnimationFrame(tick)
}

function startPlay() {
  clock = ed.playhead.value
  if (clock >= total.value - 30) clock = 0
  ed.playhead.value = clock
  last = performance.now()
  if (props.mode === 'preview') {
    if (pv.value) {
      pv.value.currentTime = clock / 1000
      pv.value.play().catch(() => (ed.playing.value = false))
    }
  } else {
    sync(clock, true)
  }
  raf = requestAnimationFrame(tick)
}
function stopPlay() {
  cancelAnimationFrame(raf)
  pauseAll()
  seekTo(ed.playhead.value)
}

function seekTo(t: number) {
  if (props.mode === 'preview') {
    if (pv.value && Math.abs(pv.value.currentTime * 1000 - t) > 40) pv.value.currentTime = t / 1000
  } else sync(t, true)
}

watch(
  () => ed.playing.value,
  (p) => (p ? startPlay() : stopPlay()),
)
watch(
  () => ed.playhead.value,
  (p) => {
    if (ed.playing.value) {
      if (Math.abs(p - clock) > 250) {
        clock = p
        if (props.mode === 'preview' && pv.value) pv.value.currentTime = p / 1000
        else sync(p, true)
      }
    } else seekTo(p)
  },
)
watch(
  () => props.mode,
  () => {
    ed.playing.value = false
    pauseAll()
    queueMicrotask(() => seekTo(ed.playhead.value))
  },
)
let resyncQueued = false
watch(
  () => ed.project.value,
  () => {
    if (resyncQueued) return
    resyncQueued = true
    requestAnimationFrame(() => {
      resyncQueued = false
      if (!ed.playing.value) sync(ed.playhead.value, true)
    })
  },
  { deep: true },
)
onMounted(() => sync(ed.playhead.value, true))
onBeforeUnmount(() => {
  cancelAnimationFrame(raf)
  ro?.disconnect()
  pauseAll()
})

// ---------- 画面 ----------

const cur = computed(() => (curIdx.value >= 0 ? clips.value[curIdx.value] : undefined))
const curPl = computed(() => (curIdx.value >= 0 ? ed.placed.value[curIdx.value] : undefined))
const curSrc = computed(() => (cur.value ? ed.sources[cur.value.path] : undefined))

function mediaStyle(c: EditClip, pl: { startMs: number; durMs: number; overlapMs: number; endMs: number }): Record<string, string> {
  const { w, h } = stage.value
  const turned = c.rotate === 90 || c.rotate === 270
  const bw = turned ? h : w
  const bh = turned ? w : h
  const f: string[] = []
  if (Math.abs(c.brightness) > 1e-6) f.push(`brightness(${(1 + c.brightness).toFixed(3)})`)
  if (Math.abs(c.contrast - 1) > 1e-6) f.push(`contrast(${c.contrast.toFixed(3)})`)
  if (Math.abs(c.saturation - 1) > 1e-6) f.push(`saturate(${c.saturation.toFixed(3)})`)
  return {
    position: 'absolute',
    left: `${(w - bw) / 2}px`,
    top: `${(h - bh) / 2}px`,
    width: `${bw}px`,
    height: `${bh}px`,
    objectFit: ed.project.value.out.fit === 'cover' ? 'cover' : 'contain',
    transform: `scale(${c.flipH ? -1 : 1}, ${c.flipV ? -1 : 1}) rotate(${c.rotate}deg)`,
    filter: f.join(' ') || 'none',
    opacity: String(fadeFactor(c, pl, clamp(ed.playhead.value, pl.startMs, pl.endMs))),
  }
}

const videoStyle = (slot: number) => {
  const c = cur.value
  const pl = curPl.value
  if (slot !== activeSlot.value || !c || !pl || c.kind !== 'video' || props.mode !== 'live') return { visibility: 'hidden' } as Record<string, string>
  return mediaStyle(c, pl)
}
const imageStyle = computed(() => (cur.value && curPl.value && cur.value.kind === 'image' ? mediaStyle(cur.value, curPl.value) : { display: 'none' }))

// ---------- 文字 ----------

const visibleTexts = computed(() => {
  if (props.mode !== 'live') return []
  const t = ed.playhead.value
  return ed.project.value.texts.filter((x) => t >= x.startMs && t < x.endMs)
})

function rgba(hex: string, a: number) {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim())
  if (!m) return `rgba(0,0,0,${a})`
  const n = parseInt(m[1], 16)
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`
}

function textStyle(t: EditText): Record<string, string> {
  const px = (t.size / 100) * stage.value.h
  const s: Record<string, string> = {
    left: `${t.x * 100}%`,
    top: `${t.y * 100}%`,
    fontSize: `${px}px`,
    color: rgba(t.color, t.opacity),
  }
  if (t.outline) {
    const b = Math.max(1, Math.round(px * 0.06))
    const c = t.outlineColor
    s.textShadow = [`${b}px 0`, `-${b}px 0`, `0 ${b}px`, `0 -${b}px`, `${b}px ${b}px`, `-${b}px ${b}px`, `${b}px -${b}px`, `-${b}px -${b}px`].map((o) => `${o} 0 ${c}`).join(',')
  }
  if (t.boxed) {
    s.background = rgba(t.boxColor, t.boxOpacity)
    s.padding = `${px / 4}px`
  }
  return s
}

let dragText: { id: number; dx: number; dy: number } | null = null
function textDown(e: PointerEvent, t: EditText) {
  ed.sel.value = { kind: 'text', id: t.id }
  const r = stageEl.value?.getBoundingClientRect()
  if (!r) return
  dragText = { id: t.id, dx: t.x - (e.clientX - r.left) / r.width, dy: t.y - (e.clientY - r.top) / r.height }
  ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
}
function textMove(e: PointerEvent) {
  const r = stageEl.value?.getBoundingClientRect()
  if (!dragText || !r) return
  const x = clamp((e.clientX - r.left) / r.width + dragText.dx, 0, 1)
  const y = clamp((e.clientY - r.top) / r.height + dragText.dy, 0, 1)
  ed.patchText(dragText.id, { x: Math.round(x * 1000) / 1000, y: Math.round(y * 1000) / 1000 }, `textpos:${dragText.id}`)
}
function textUp() {
  dragText = null
}

const empty = computed(() => !clips.value.length)
const onError = () => (playError.value = '这个素材在预览里播不出来（可能是系统不支持它的编码），导出不受影响。')
</script>

<template>
  <div ref="wrap" class="mon">
    <div ref="stageEl" class="stage" :style="{ width: stage.w + 'px', height: stage.h + 'px' }">
      <template v-if="mode === 'live'">
        <video
          v-for="n in 2"
          :key="n"
          :ref="(el) => setVideoEl(n - 1, el)"
          :style="videoStyle(n - 1)"
          preload="auto"
          playsinline
          @loadedmetadata="onMeta(n - 1)"
          @error="onError"
        />
        <img v-if="cur && cur.kind === 'image' && curSrc" :src="curSrc.url" :style="imageStyle" alt="" draggable="false" />
        <div
          v-for="t in visibleTexts"
          :key="t.id"
          class="txt"
          data-no-i18n
          :class="{ on: ed.sel.value?.kind === 'text' && ed.sel.value.id === t.id }"
          :style="textStyle(t)"
          @pointerdown.prevent="textDown($event, t)"
          @pointermove="textMove"
          @pointerup="textUp"
          @pointercancel="textUp"
        >
          {{ t.text }}
        </div>
      </template>
      <video v-show="mode === 'preview'" ref="pv" class="pv" :src="mode === 'preview' ? (previewUrl ?? undefined) : undefined" preload="auto" playsinline />
      <div v-if="empty" class="hint">添加素材后，在这里预览</div>
      <div v-else-if="playError" class="warn">{{ playError }}</div>
    </div>
    <audio v-for="a in ed.project.value.audio" :key="a.id" :ref="(el) => setAudioEl(a.id, el)" :src="ed.sources[a.path]?.url" preload="auto" />
  </div>
</template>

<style scoped>
.mon {
  display: flex;
  justify-content: center;
  width: 100%;
}
.stage {
  position: relative;
  background: #000;
  overflow: hidden;
  border-radius: 6px;
  flex: none;
}
.stage video {
  background: transparent;
}
.pv {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  object-fit: contain;
}
.txt {
  position: absolute;
  transform: translate(-50%, -50%);
  white-space: pre;
  text-align: left;
  line-height: 1.2;
  cursor: move;
  user-select: none;
  touch-action: none;
}
.txt.on {
  outline: 1px dashed #fff;
  outline-offset: 2px;
}
.hint,
.warn {
  position: absolute;
  left: 0;
  right: 0;
  text-align: center;
  font-size: 12px;
  padding: 0 12px;
}
.hint {
  top: 50%;
  transform: translateY(-50%);
  color: #9a9a9a;
}
.warn {
  bottom: 8px;
  color: #ffd27a;
  text-shadow: 0 1px 2px #000;
}
audio {
  display: none;
}
</style>
