<script setup lang="ts">
// 监视器：直接播放素材文件来预览（不含转场，转场请用“生成预览”），或者播放生成好的预览成片。
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import type { EditApi } from '../../../composables/useEditProject'
import type { RegionApi } from '../../../composables/useRegions'
import type { EditClip, EditRegion, EditText, NRect } from '../../../types'
import { aspectInSource, audioSpan, boxFromCorners, clamp, clampBox, clipIndexAt, fadeFactor, focusRect, focusWindow, grownBox, monitorGeom, outputSize, regionActiveAt, sourceTimeAt, thin, trackAt } from '../../../utils/edit'

const props = defineProps<{ ed: EditApi; rg: RegionApi; mode: 'live' | 'preview'; previewUrl: string | null }>()
const ed = props.ed
const rg = props.rg

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

// ---------- 区域（跟着物体走） ----------
// 框、路径都画在一个和视频元素摆法完全一样的层里（同样的位置、旋转、翻转），里面再套一个对准“画面内容”的层，
// 所以框的坐标直接用相对素材画面的比例（0–1）；鼠标位置则按同样的变换反推回来。

const showRegions = computed(() => props.mode === 'live' && rg.tabActive.value && !!rg.clip.value && rg.here.value && curIdx.value === rg.index.value && !!curSrc.value && cur.value?.kind === 'video')
const geom = computed(() => (cur.value && curSrc.value ? monitorGeom(stage.value, cur.value, { w: curSrc.value.width, h: curSrc.value.height }, ed.project.value.out.fit) : null))
const ovBox = computed(() => {
  const g = geom.value
  if (!g) return {}
  return { left: `${g.box.left}px`, top: `${g.box.top}px`, width: `${g.box.w}px`, height: `${g.box.h}px`, transform: g.transform }
})
const ovContent = computed(() => {
  const g = geom.value
  if (!g) return {}
  return { left: `${g.content.left}px`, top: `${g.content.top}px`, width: `${g.content.w}px`, height: `${g.content.h}px` }
})
const pct = (r: NRect): Record<string, string> => ({ left: `${r.x * 100}%`, top: `${r.y * 100}%`, width: `${r.w * 100}%`, height: `${r.h * 100}%` })

/** 现在是素材里的哪一刻 */
const rt = computed(() => rg.srcMs.value)
const regionList = computed<EditRegion[]>(() => rg.clip.value?.regions ?? [])
const selBox = computed(() => (rg.region.value ? trackAt(rg.region.value.track, rt.value) : null))
const others = computed(() =>
  regionList.value
    .filter((r) => r.id !== rg.selId.value)
    .map((r) => ({ id: r.id, off: r.effect !== 'focus' && !regionActiveAt(r, rt.value), style: pct(trackAt(r.track, rt.value) ?? { x: 0, y: 0, w: 0, h: 0 }) })),
)

/** 效果的示意：马赛克 / 模糊 / 局部调色用“透过去看”的滤镜近似，精确的效果以“生成预览”和导出为准 */
const fxList = computed(() =>
  regionList.value
    .filter((r) => r.effect !== 'focus' && !r.invert && regionActiveAt(r, rt.value))
    .map((r) => {
      const b = grownBox(r, rt.value)
      if (!b) return null
      const f = r.effect === 'tone' ? `brightness(${(1 + r.brightness).toFixed(3)}) contrast(${r.contrast.toFixed(3)}) saturate(${r.saturation.toFixed(3)})` : `blur(${(2 + r.strength * 16).toFixed(1)}px)`
      return { id: r.id, style: { ...pct(b), backdropFilter: f, WebkitBackdropFilter: f, borderRadius: r.shape === 'ellipse' ? '50%' : '0' } as Record<string, string> }
    })
    .filter((x): x is { id: number; style: Record<string, string> } => x !== null),
)
const growBox = computed(() => (rg.region.value && rg.region.value.effect !== 'focus' && rg.region.value.grow !== 0 ? grownBox(rg.region.value, rt.value) : null))

const focusBox = computed(() => {
  const r = regionList.value.find((x) => x.effect === 'focus')
  const c = cur.value
  const src = curSrc.value
  if (!r || !c || !src) return null
  const o = size.value
  const win = focusWindow(r.zoom, { w: src.width, h: src.height }, r.reframe ? aspectInSource(o.w, o.h, c.rotate) : null)
  return focusRect(r, rt.value, win)
})

const path = computed(() => {
  const r = rg.region.value
  if (!r || r.track.length < 2) return null
  const pts = thin(r.track, 400)
  const centre = (p: { x: number; y: number; w: number; h: number }) => `${(p.x + p.w / 2).toFixed(5)},${(p.y + p.h / 2).toFixed(5)}`
  const lost: string[] = []
  let run: string[] = []
  pts.forEach((p, i) => {
    if (p.lost && !p.pin) {
      if (!run.length && i > 0) run.push(centre(pts[i - 1]))
      run.push(centre(p))
    } else if (run.length) {
      run.push(centre(p))
      lost.push(run.join(' '))
      run = []
    }
  })
  if (run.length) lost.push(run.join(' '))
  return { all: pts.map(centre).join(' '), lost }
})
const pinDots = computed(() => (rg.region.value && rg.region.value.track.length > 1 ? rg.region.value.track.filter((p) => p.pin).map((p) => ({ left: `${(p.x + p.w / 2) * 100}%`, top: `${(p.y + p.h / 2) * 100}%`, tMs: p.tMs })) : []))

type Corner = 'nw' | 'ne' | 'sw' | 'se'
const CORNERS: Corner[] = ['nw', 'ne', 'sw', 'se']
type Drag =
  | { kind: 'draw'; a: { x: number; y: number }; b: { x: number; y: number } }
  | { kind: 'move'; id: number; p0: { x: number; y: number }; box0: NRect }
  | { kind: 'resize'; id: number; fixed: { x: number; y: number } }
const drag = ref<Drag | null>(null)
const band = computed(() => (drag.value?.kind === 'draw' ? boxFromCorners(drag.value.a.x, drag.value.a.y, drag.value.b.x, drag.value.b.y) : null))

function norm(e: PointerEvent): { x: number; y: number } | null {
  const g = geom.value
  const r = stageEl.value?.getBoundingClientRect()
  if (!g || !r || !r.width) return null
  const k = r.width / stage.value.w
  return g.toNorm((e.clientX - r.left) / k, (e.clientY - r.top) / k)
}
const inFrame = (p: { x: number; y: number }, tol = 0.02) => p.x >= -tol && p.x <= 1 + tol && p.y >= -tol && p.y <= 1 + tol
const clamp01 = (p: { x: number; y: number }) => ({ x: clamp(p.x, 0, 1), y: clamp(p.y, 0, 1) })

function ovDown(e: PointerEvent) {
  const p = norm(e)
  if (!p) return
  const t = e.target as HTMLElement
  const box = selBox.value
  const sel = rg.region.value
  if (t.dataset.h && box && sel) {
    // 拖角上的小方块：对角那个角不动
    const c = t.dataset.h as Corner
    const fixed = { x: c.endsWith('w') ? box.x + box.w : box.x, y: c.startsWith('n') ? box.y + box.h : box.y }
    ed.playing.value = false
    drag.value = { kind: 'resize', id: sel.id, fixed }
  } else if (t.dataset.box && box && sel) {
    ed.playing.value = false
    drag.value = { kind: 'move', id: sel.id, p0: p, box0: box }
  } else if (t.dataset.rid) {
    rg.select(Number(t.dataset.rid))
    return
  } else if (rg.drawing.value && inFrame(p)) {
    ed.playing.value = false
    const q = clamp01(p)
    drag.value = { kind: 'draw', a: q, b: q }
  } else return
  ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  e.preventDefault()
}
function ovMove(e: PointerEvent) {
  const d = drag.value
  const p = norm(e)
  if (!d || !p) return
  if (d.kind === 'draw') d.b = clamp01(p)
  else if (d.kind === 'move') rg.place(d.id, clampBox({ ...d.box0, x: d.box0.x + (p.x - d.p0.x), y: d.box0.y + (p.y - d.p0.y) }))
  else {
    const q = clamp01(p)
    rg.place(d.id, clampBox(boxFromCorners(d.fixed.x, d.fixed.y, q.x, q.y)), { resize: true, only: e.shiftKey })
  }
}
function ovUp() {
  const d = drag.value
  drag.value = null
  if (!d || d.kind !== 'draw') return
  const r = boxFromCorners(d.a.x, d.a.y, d.b.x, d.b.y)
  if (r.w >= 0.02 && r.h >= 0.02) return void rg.add(clampBox(r))
  // 只是点了一下：点在感知到的物体上就用它，否则在这里放一个默认大小的框
  const hit = rg.candidates.value.filter((c) => d.a.x >= c.rect.x && d.a.x <= c.rect.x + c.rect.w && d.a.y >= c.rect.y && d.a.y <= c.rect.y + c.rect.h).sort((a, b) => a.rect.w * a.rect.h - b.rect.w * b.rect.h)[0]
  if (hit) rg.add(clampBox(hit.rect))
  else rg.add(clampBox({ x: d.a.x - 0.075, y: d.a.y - 0.075, w: 0.15, h: 0.15 }))
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
        <div v-if="showRegions && geom" class="rov" :class="{ draw: rg.drawing.value }" :style="ovBox" @pointerdown="ovDown" @pointermove="ovMove" @pointerup="ovUp" @pointercancel="ovUp">
          <div class="rcont" :style="ovContent">
            <div v-for="f in fxList" :key="'f' + f.id" class="fx" :style="f.style" />
            <template v-if="rg.drawing.value">
              <div v-for="(c, i) in rg.candidates.value" :key="'c' + i" class="cand" :style="pct(c.rect)" />
            </template>
            <div v-for="o in others" :key="'o' + o.id" class="rbox other" :class="{ off: o.off }" :data-rid="o.id" :style="o.style" />
            <div v-if="focusBox" class="fwin" :style="pct(focusBox)" />
            <div v-if="growBox" class="rbox grow" :style="pct(growBox)" />
            <svg v-if="path" class="rpath" viewBox="0 0 1 1" preserveAspectRatio="none">
              <polyline :points="path.all" class="line" />
              <polyline v-for="(l, i) in path.lost" :key="i" :points="l" class="lost" />
            </svg>
            <i v-for="d in pinDots" :key="'p' + d.tMs" class="pin" :style="{ left: d.left, top: d.top }" />
            <div v-if="selBox" class="rbox sel" data-box="1" :style="pct(selBox)">
              <i v-if="rg.region.value?.shape === 'ellipse'" class="ell" />
              <i v-for="h in CORNERS" :key="h" class="hd" :class="h" :data-h="h" />
            </div>
            <div v-if="band" class="rbox band" :style="pct(band)" />
          </div>
        </div>
      </template>
      <video v-show="mode === 'preview'" ref="pv" class="pv" :src="mode === 'preview' ? (previewUrl ?? undefined) : undefined" preload="auto" playsinline />
      <div v-if="showRegions && rg.drawing.value" class="rhint">{{ rg.detecting.value ? '正在感知运动的物体…' : rg.candidates.value.length ? '点虚线框选中那个物体，或在画面上拖出一个框' : '在画面上拖出一个框，或点一下放一个框' }}</div>
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
.rov {
  position: absolute;
  pointer-events: none;
  touch-action: none;
}
.rov.draw {
  pointer-events: auto;
  cursor: crosshair;
}
.rcont {
  position: absolute;
}
.rcont > * {
  position: absolute;
  box-sizing: border-box;
}
.fx {
  pointer-events: none;
}
.cand {
  border: 1.5px dashed #ffd27a;
  background: rgba(255, 210, 122, 0.12);
  pointer-events: none;
}
.rbox {
  border: 2px solid #3ea6ff;
  pointer-events: none;
}
.rbox.sel {
  pointer-events: auto;
  cursor: move;
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.55);
}
.rbox.other {
  border: 1.5px solid rgba(255, 255, 255, 0.85);
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.5);
  pointer-events: auto;
  cursor: pointer;
}
.rbox.other.off {
  border-style: dotted;
  opacity: 0.6;
}
.rbox.grow {
  border: 1.5px dashed rgba(62, 166, 255, 0.9);
}
.rbox.band {
  border: 1.5px dashed #fff;
  background: rgba(62, 166, 255, 0.18);
}
.fwin {
  border: 2px dashed #ffb02e;
  box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.5);
  pointer-events: none;
}
.ell {
  position: absolute;
  inset: 0;
  border: 1.5px dashed rgba(255, 255, 255, 0.9);
  border-radius: 50%;
  pointer-events: none;
}
.hd {
  position: absolute;
  width: 10px;
  height: 10px;
  background: #fff;
  border: 1.5px solid #3ea6ff;
  border-radius: 2px;
  pointer-events: auto;
}
.hd.nw {
  left: -6px;
  top: -6px;
}
.hd.ne {
  right: -6px;
  top: -6px;
}
.hd.sw {
  left: -6px;
  bottom: -6px;
}
.hd.se {
  right: -6px;
  bottom: -6px;
}
.rpath {
  left: 0;
  top: 0;
  width: 100%;
  height: 100%;
  overflow: visible;
  pointer-events: none;
}
.rpath .line {
  fill: none;
  stroke: #3ea6ff;
  stroke-width: 1.5;
  vector-effect: non-scaling-stroke;
  opacity: 0.9;
}
.rpath .lost {
  fill: none;
  stroke: #ff5d5d;
  stroke-width: 2.5;
  stroke-dasharray: 3 3;
  vector-effect: non-scaling-stroke;
}
.pin {
  width: 8px;
  height: 8px;
  margin: -4px 0 0 -4px;
  border-radius: 50%;
  background: #ffd27a;
  border: 1.5px solid #000;
  pointer-events: none;
}
.rhint {
  position: absolute;
  left: 0;
  right: 0;
  top: 6px;
  text-align: center;
  font-size: 12px;
  color: #fff;
  text-shadow: 0 1px 2px #000;
  pointer-events: none;
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
