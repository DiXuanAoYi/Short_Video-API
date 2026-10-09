// 剪辑工程的纯计算：时间线排布、新建片段、分割、取时间等。
// 排布规则和后端 `src-tauri/src/edit/spec.rs` 的 `layout` 保持一致，改那边时要一起改这里。
// 这个文件不要引入别的运行时模块（用 node 直接跑 scripts 里的检查时需要）。

import type { EditAudio, EditClip, EditOutput, EditProject, EditRegion, EditSource, EditText, NRect, RegionEffect, TrackPt } from '../types'

export const MIN_MS = 100
export const MIN_TRANSITION_MS = 100
export const MAX_CLIPS = 300
export const MAX_TEXTS = 100
export const MAX_AUDIO = 20
/** 一个片段最多几个区域（和后端 `MAX_REGIONS` 一致） */
export const MAX_REGIONS = 8
/** 图片、文字默认的时长 */
export const DEFAULT_STILL_MS = 5000
export const DEFAULT_TEXT_MS = 3000

export const TRANSITIONS: { id: string; label: string }[] = [
  { id: 'fade', label: '淡入淡出' },
  { id: 'fadeblack', label: '闪黑' },
  { id: 'fadewhite', label: '闪白' },
  { id: 'dissolve', label: '溶解' },
  { id: 'wipeleft', label: '向左擦除' },
  { id: 'wiperight', label: '向右擦除' },
  { id: 'wipeup', label: '向上擦除' },
  { id: 'wipedown', label: '向下擦除' },
  { id: 'slideleft', label: '向左滑动' },
  { id: 'slideright', label: '向右滑动' },
  { id: 'slideup', label: '向上滑动' },
  { id: 'slidedown', label: '向下滑动' },
  { id: 'circleopen', label: '圆形展开' },
  { id: 'circleclose', label: '圆形收拢' },
  { id: 'radial', label: '时钟擦除' },
  { id: 'pixelize', label: '马赛克' },
]

export interface Placed {
  startMs: number
  durMs: number
  /** 开头和上一个片段重叠多少（转场时长，已按能放下的长度收紧） */
  overlapMs: number
  endMs: number
}

export const clamp = (v: number, lo: number, hi: number) => (Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : lo)

export function defaultOutput(): EditOutput {
  return { width: 0, height: 0, fps: 0, fit: 'contain', quality: 'balanced', codec: 'h264', format: 'mp4' }
}

export function emptyProject(): EditProject {
  return { title: '', clips: [], texts: [], audio: [], out: defaultOutput() }
}

/** 补全旧版本或手改过的工程文件里缺的字段。 */
export function normalizeProject(raw: Partial<EditProject> | null | undefined): EditProject {
  const p = emptyProject()
  const r = raw ?? {}
  p.title = typeof r.title === 'string' ? r.title : ''
  p.out = { ...defaultOutput(), ...(r.out ?? {}) }
  p.clips = (r.clips ?? []).map((c, i) => ({ ...blankClip(), ...c, id: c.id ?? i + 1, regions: (c.regions ?? []).map((g, j) => ({ ...blankRegion(), ...g, id: g.id ?? j + 1 })) }))
  p.texts = (r.texts ?? []).map((t, i) => ({ ...blankText(), ...t, id: t.id ?? i + 1 }))
  p.audio = (r.audio ?? []).map((a, i) => ({ ...blankAudio(), ...a, id: a.id ?? i + 1 }))
  return p
}

function blankClip(): EditClip {
  return {
    id: 0,
    path: '',
    kind: 'video',
    inMs: 0,
    outMs: 0,
    speed: 1,
    volume: 1,
    mute: false,
    fadeInMs: 0,
    fadeOutMs: 0,
    rotate: 0,
    flipH: false,
    flipV: false,
    brightness: 0,
    contrast: 1,
    saturation: 1,
    regions: [],
    transition: null,
  }
}

function blankText(): EditText {
  return {
    id: 0,
    text: '',
    startMs: 0,
    endMs: DEFAULT_TEXT_MS,
    x: 0.5,
    y: 0.85,
    size: 6,
    color: '#FFFFFF',
    opacity: 1,
    outline: true,
    outlineColor: '#000000',
    boxed: false,
    boxColor: '#000000',
    boxOpacity: 0.5,
    font: null,
  }
}

function blankAudio(): EditAudio {
  return { id: 0, path: '', startMs: 0, inMs: 0, outMs: null, volume: 1, fadeInMs: 0, fadeOutMs: 0, looped: false, duck: false }
}

export function nextId(p: EditProject): number {
  let m = 0
  for (const c of p.clips) m = Math.max(m, c.id)
  for (const t of p.texts) m = Math.max(m, t.id)
  for (const a of p.audio) m = Math.max(m, a.id)
  return m + 1
}

/** 素材放上时间线时的默认片段：视频取整段，图片显示 5 秒。 */
export function clipFromSource(src: EditSource, id: number): EditClip {
  const c = blankClip()
  c.id = id
  c.path = src.path
  if (src.kind === 'image') {
    c.kind = 'image'
    c.outMs = DEFAULT_STILL_MS
  } else {
    c.outMs = Math.max(MIN_MS, src.durationMs ?? DEFAULT_STILL_MS)
  }
  return c
}

export function textAt(id: number, startMs: number): EditText {
  const t = blankText()
  t.id = id
  t.text = '输入文字'
  t.startMs = startMs
  t.endMs = startMs + DEFAULT_TEXT_MS
  return t
}

export function audioFromSource(src: EditSource, id: number, startMs: number): EditAudio {
  const a = blankAudio()
  a.id = id
  a.path = src.path
  a.startMs = startMs
  return a
}

/** 片段在时间线上占的时长（毫秒）。 */
export function clipDuration(c: EditClip): number {
  const span = Math.max(0, c.outMs - c.inMs)
  return c.kind === 'image' ? span : Math.round(span / Math.max(0.01, c.speed))
}

export function layout(clips: EditClip[]): Placed[] {
  const d = clips.map(clipDuration)
  const out: Placed[] = []
  let prevOverlap = 0
  clips.forEach((c, i) => {
    let overlap = 0
    if (i > 0) {
      const want = c.transition?.durationMs ?? 0
      const fit = Math.min(want, Math.max(0, d[i - 1] - prevOverlap), d[i])
      overlap = fit < MIN_TRANSITION_MS ? 0 : fit
    }
    const startMs = i === 0 ? 0 : out[i - 1].endMs - overlap
    out.push({ startMs, durMs: d[i], overlapMs: overlap, endMs: startMs + d[i] })
    prevOverlap = overlap
  })
  return out
}

export function totalMs(clips: EditClip[]): number {
  const l = layout(clips)
  return l.length ? l[l.length - 1].endMs : 0
}

/** 时间线上 t 时刻看到的是哪个片段（转场重叠处取后一个）。超出范围返回 -1。 */
export function clipIndexAt(placed: Placed[], t: number): number {
  for (let i = placed.length - 1; i >= 0; i--) {
    if (t >= placed[i].startMs && t < placed[i].endMs) return i
  }
  return -1
}

/** t 时刻对应片段里的素材时间（毫秒）。 */
export function sourceTimeAt(c: EditClip, pl: Placed, t: number): number {
  if (c.kind === 'image') return 0
  const local = clamp(t - pl.startMs, 0, pl.durMs)
  return clamp(c.inMs + local * c.speed, c.inMs, c.outMs)
}

/** 画面淡入淡出在 t 时刻的亮度系数（0–1）。 */
export function fadeFactor(c: EditClip, pl: Placed, t: number): number {
  const local = t - pl.startMs
  let f = 1
  if (c.fadeInMs > 0) f = Math.min(f, local / c.fadeInMs)
  if (c.fadeOutMs > 0) f = Math.min(f, (pl.durMs - local) / c.fadeOutMs)
  return clamp(f, 0, 1)
}

/** 在时间线 t 处把片段一分为二。离两端不足 MIN_MS 时返回 null。第二段不带转场，淡入归第一段、淡出归第二段。 */
export function splitClip(c: EditClip, pl: Placed, t: number, newId: number): [EditClip, EditClip] | null {
  const local = t - pl.startMs
  if (local < MIN_MS || pl.durMs - local < MIN_MS) return null
  const first: EditClip = { ...c, transition: c.transition ? { ...c.transition } : null, regions: c.regions.map(copyRegion) }
  const second: EditClip = { ...c, id: newId, transition: null, regions: c.regions.map(copyRegion) }
  if (c.kind === 'image') {
    first.outMs = local
    second.outMs = pl.durMs - local
  } else {
    const cut = Math.round(c.inMs + local * c.speed)
    first.outMs = cut
    second.inMs = cut
  }
  first.fadeOutMs = 0
  second.fadeInMs = 0
  if (c.kind === 'video') {
    // 区域的轨迹用的是素材时间，两半各留下自己那一段（边上多留一个点，插值才对得上）
    first.regions = first.regions.map((r) => ({ ...r, track: trimTrack(r.track, first.inMs, first.outMs) }))
    second.regions = second.regions.map((r) => ({ ...r, track: trimTrack(r.track, second.inMs, second.outMs) }))
  }
  return [first, second]
}

/** 文字块排到哪一行：时间上重叠的错开，免得在时间线上叠在一起。 */
export function textLanes(texts: EditText[]): number[] {
  const order = texts.map((_, i) => i).sort((a, b) => texts[a].startMs - texts[b].startMs)
  const laneEnd: number[] = []
  const lane: number[] = new Array(texts.length).fill(0)
  for (const i of order) {
    let l = laneEnd.findIndex((e) => e <= texts[i].startMs)
    if (l < 0) {
      l = laneEnd.length
      laneEnd.push(0)
    }
    laneEnd[l] = texts[i].endMs
    lane[i] = l
  }
  return lane
}

/** 音频轨在时间线上显示的长度（循环的一直到视频结束）。 */
export function audioSpan(a: EditAudio, src: EditSource | undefined, total: number): number {
  const seg = a.outMs !== null ? a.outMs - a.inMs : src?.durationMs ? Math.max(0, src.durationMs - a.inMs) : null
  const avail = Math.max(0, total - a.startMs)
  if (a.looped) return avail
  return seg === null ? avail : Math.min(seg, Math.max(avail, MIN_MS))
}

/** 输出画面的宽高（界面里监视器的比例用）；和后端 `output_size` 一致。 */
export function outputSize(p: EditProject, sources: Record<string, EditSource | undefined>): { w: number; h: number } {
  const even = (n: number) => Math.max(2, Math.floor(n / 2) * 2)
  if (p.out.width > 0 && p.out.height > 0) return { w: even(p.out.width), h: even(p.out.height) }
  const first = p.clips[0]
  const s = first ? sources[first.path] : undefined
  if (first && s && s.width > 0 && s.height > 0) {
    const swap = first.rotate === 90 || first.rotate === 270
    return { w: even(Math.min(7680, swap ? s.height : s.width)), h: even(Math.min(7680, swap ? s.width : s.height)) }
  }
  return { w: 1280, h: 720 }
}

export function mediaPaths(p: EditProject): string[] {
  const set = new Set<string>()
  for (const c of p.clips) if (c.path) set.add(c.path)
  for (const a of p.audio) if (a.path) set.add(a.path)
  for (const t of p.texts) if (t.font) set.add(t.font)
  return [...set]
}

export function moveItem<T>(list: T[], from: number, to: number): T[] {
  const next = [...list]
  const [x] = next.splice(from, 1)
  next.splice(clamp(to, 0, next.length), 0, x)
  return next
}

/** 毫秒 → `m:ss.mmm`（精确到毫秒，输入框里用）。 */
export function msText(ms: number): string {
  const t = Math.max(0, Math.round(ms))
  const m = Math.floor(t / 60000)
  const s = Math.floor((t % 60000) / 1000)
  const r = t % 1000
  return `${m}:${String(s).padStart(2, '0')}.${String(r).padStart(3, '0')}`
}

/** 解析 `83`、`83.5`、`1:23`、`1:23.456`、`1:02:03`（秒、分:秒、时:分:秒）。无法识别返回 null。 */
export function parseMs(input: string): number | null {
  const s = input.trim()
  if (!s) return null
  const parts = s.split(':')
  if (parts.length > 3 || parts.some((p) => !/^\d+(\.\d+)?$/.test(p.trim()))) return null
  const nums = parts.map(Number)
  if (nums.slice(0, -1).some((n) => !Number.isInteger(n))) return null
  return Math.round(nums.reduce((acc, n) => acc * 60 + n, 0) * 1000)
}

/** 刻度间隔（毫秒）：让相邻刻度大约相隔 80 像素以上。 */
export function tickStep(pxPerSec: number): number {
  const steps = [100, 200, 500, 1000, 2000, 5000, 10000, 15000, 30000, 60000, 120000, 300000, 600000, 1800000, 3600000]
  const want = (80 / pxPerSec) * 1000
  return steps.find((s) => s >= want) ?? steps[steps.length - 1]
}

/** 把 v 吸附到最近的候选值（距离在 range 以内）。 */
export function snap(v: number, candidates: number[], range: number): number {
  let best = v
  let bd = range + 1
  for (const c of candidates) {
    const d = Math.abs(c - v)
    if (d <= range && d < bd) {
      best = c
      bd = d
    }
  }
  return best
}


// ---------- 跟着物体走的区域 ----------
// 轨迹点的位置是相对整个画面的比例（左上角为原点），方向是素材的显示方向（旋转翻转之前），时间是素材里的时间。
// 插值、放大的算法和后端 `src-tauri/src/track/mod.rs` 的 `interpolate`、`edit/region.rs` 的 `box_at` 保持一致。

/** 拖动框时，这个时刻前后多长时间内自动追踪出来的点会被让开，路径从手动的点平滑过渡过去 */
export const PIN_EASE_MS = 300
/** 框的最小尺寸（占画面的比例） */
export const MIN_BOX = 0.01
/** 一次追踪最长（毫秒），和后端一致 */
export const MAX_TRACK_SPAN_MS = 30 * 60 * 1000

export const REGION_EFFECTS: { id: RegionEffect; label: string; hint: string }[] = [
  { id: 'mosaic', label: '马赛克', hint: '把区域打上马赛克（遮住脸、车牌、水印）' },
  { id: 'blur', label: '模糊', hint: '把区域虚化' },
  { id: 'tone', label: '局部调色', hint: '只调区域里的亮度、对比度、饱和度' },
  { id: 'focus', label: '跟随聚焦', hint: '放大并让画面始终跟着这个区域走（也可以把横屏素材裁成竖屏）' },
]

export function blankRegion(): EditRegion {
  return {
    id: 0,
    name: '',
    track: [],
    shape: 'rect',
    feather: 0.2,
    grow: 0,
    invert: false,
    effect: 'mosaic',
    strength: 0.5,
    brightness: 0,
    contrast: 1,
    saturation: 1,
    zoom: 2,
    reframe: false,
    smooth: 0.6,
    startMs: null,
    endMs: null,
  }
}

export function copyRegion(r: EditRegion): EditRegion {
  return { ...r, track: r.track.map((p) => ({ ...p })) }
}

export function nextRegionId(c: EditClip): number {
  return c.regions.reduce((m, r) => Math.max(m, r.id), 0) + 1
}

/** 轨迹在 t（素材时间，毫秒）处的矩形：相邻两点之间直线插值，两头停在第一个 / 最后一个点。没有点返回 null。 */
export function trackAt(pts: TrackPt[], t: number): NRect | null {
  const first = pts[0]
  const last = pts[pts.length - 1]
  if (!first || !last) return null
  const rect = (p: TrackPt): NRect => ({ x: p.x, y: p.y, w: p.w, h: p.h })
  if (t <= first.tMs) return rect(first)
  if (t >= last.tMs) return rect(last)
  // 二分找到 t 所在的区间：第一个时间 > t 的点
  let lo = 0
  let hi = pts.length
  while (lo < hi) {
    const mid = (lo + hi) >> 1
    if (pts[mid].tMs <= t) lo = mid + 1
    else hi = mid
  }
  const a = pts[lo - 1]
  const b = pts[lo]
  const span = b.tMs - a.tMs
  const f = span > 0 ? (t - a.tMs) / span : 0
  const l = (u: number, v: number) => u + (v - u) * f
  return { x: l(a.x, b.x), y: l(a.y, b.y), w: l(a.w, b.w), h: l(a.h, b.h) }
}

/** 效果真正作用的范围：追踪的框按“扩大”系数放大（中心不变）。 */
export function grownBox(r: EditRegion, t: number): NRect | null {
  const b = trackAt(r.track, t)
  if (!b) return null
  const k = 1 + r.grow
  const w = b.w * k
  const h = b.h * k
  return { x: b.x + (b.w - w) / 2, y: b.y + (b.h - h) / 2, w, h }
}

/** 效果在这个素材时刻生效吗（聚焦不受限制）。 */
export function regionActiveAt(r: EditRegion, t: number): boolean {
  return (r.startMs === null || t >= r.startMs) && (r.endMs === null || t < r.endMs)
}

/** 把框收进画面：尺寸不小于 MIN_BOX、不大于整个画面，位置不超出画面。 */
export function clampBox(b: NRect): NRect {
  const w = clamp(b.w, MIN_BOX, 1)
  const h = clamp(b.h, MIN_BOX, 1)
  return { x: clamp(b.x, 0, 1 - w), y: clamp(b.y, 0, 1 - h), w, h }
}

/** 由对角两点（拖出来的框）得到矩形。 */
export function boxFromCorners(ax: number, ay: number, bx: number, by: number): NRect {
  return { x: Math.min(ax, bx), y: Math.min(ay, by), w: Math.abs(bx - ax), h: Math.abs(by - ay) }
}

/** 新区域：参照时刻 `refMs` 只有一个手动指定的点。 */
export function newRegion(id: number, rect: NRect, refMs: number, effect: RegionEffect = 'mosaic'): EditRegion {
  const r = blankRegion()
  const b = clampBox(rect)
  r.id = id
  r.effect = effect
  r.track = [{ tMs: Math.round(refMs), ...b, pin: true }]
  if (effect === 'tone') r.brightness = 0.2
  return r
}

/** 手动把区域放到 `rect`：在 `tMs` 放一个手动点，前后 PIN_EASE_MS 内自动追踪出来的点让开。 */
export function setPin(track: TrackPt[], tMs: number, rect: NRect): TrackPt[] {
  const t = Math.round(tMs)
  const keep = track.filter((p) => p.pin || Math.abs(p.tMs - t) > PIN_EASE_MS)
  const next = keep.filter((p) => p.tMs !== t)
  next.push({ tMs: t, ...rect, pin: true })
  return next.sort((a, b) => a.tMs - b.tMs)
}

/** 改区域大小：整段一起变（每个点保持中心不变）。 */
export function resizeAll(track: TrackPt[], w: number, h: number): TrackPt[] {
  const nw = clamp(w, MIN_BOX, 1)
  const nh = clamp(h, MIN_BOX, 1)
  return track.map((p) => {
    const cx = p.x + p.w / 2
    const cy = p.y + p.h / 2
    return { ...p, x: clamp(cx - nw / 2, 0, 1 - nw), y: clamp(cy - nh / 2, 0, 1 - nh), w: nw, h: nh }
  })
}

/** 追踪出来的结果并进现有轨迹：范围外的老点保留；范围内用新点，但手动点（及其前后让开的范围）保持原样。 */
export function mergeTracked(old: TrackPt[], fresh: TrackPt[], fromMs: number, toMs: number): TrackPt[] {
  const outside = old.filter((p) => p.tMs < fromMs || p.tMs > toMs)
  const pins = old.filter((p) => p.pin && p.tMs >= fromMs && p.tMs <= toMs)
  const freshPins = fresh.filter((p) => p.pin)
  // 这次追踪的参照点（也是手动点）盖过同一时刻的老点
  const keepPins = pins.filter((p) => !freshPins.some((f) => f.tMs === p.tMs))
  const tracked = fresh.filter((p) => p.pin || !keepPins.some((q) => Math.abs(q.tMs - p.tMs) <= PIN_EASE_MS))
  const all = [...outside, ...keepPins, ...tracked].sort((a, b) => a.tMs - b.tMs)
  // 同一时刻只留一个（手动点优先）
  const out: TrackPt[] = []
  for (const p of all) {
    const last = out[out.length - 1]
    if (last && last.tMs === p.tMs) {
      if (p.pin && !last.pin) out[out.length - 1] = p
    } else out.push(p)
  }
  return out
}

/** 只留下 [fromMs, toMs] 这一段用得到的点：范围里的，加上两边各最近的一个（让两端的插值不变）。 */
export function trimTrack(track: TrackPt[], fromMs: number, toMs: number): TrackPt[] {
  if (track.length <= 2) return track.map((p) => ({ ...p }))
  const firstIn = track.findIndex((p) => p.tMs >= fromMs)
  let lastIn = -1
  for (let i = track.length - 1; i >= 0; i--) {
    if (track[i].tMs <= toMs) {
      lastIn = i
      break
    }
  }
  // 整条轨迹都在范围一边：留下离范围最近的那个点
  if (firstIn < 0) return [{ ...track[track.length - 1] }]
  if (lastIn < 0) return [{ ...track[0] }]
  const from = Math.max(0, firstIn - 1)
  const to = Math.min(track.length - 1, lastIn + 1)
  return track.slice(from, Math.max(from, to) + 1).map((p) => ({ ...p }))
}

/** 追踪不到（丢失）的时间段（素材毫秒）：连续丢失的点合成一段。 */
export function lostSpans(track: TrackPt[]): [number, number][] {
  const out: [number, number][] = []
  let start: number | null = null
  for (let i = 0; i < track.length; i++) {
    const p = track[i]
    if (p.lost && !p.pin) {
      if (start === null) start = i > 0 ? track[i - 1].tMs : p.tMs
    } else if (start !== null) {
      out.push([start, p.tMs])
      start = null
    }
  }
  if (start !== null) out.push([start, track[track.length - 1].tMs])
  return out
}

/** 轨迹覆盖了素材的哪一段（没有点返回 null）。 */
export function trackSpan(track: TrackPt[]): [number, number] | null {
  return track.length ? [track[0].tMs, track[track.length - 1].tMs] : null
}

/** 只有一个点（还没追踪过，或者手动放的一个固定位置）。 */
export const isStatic = (track: TrackPt[]) => track.length <= 1

/** 画路径用：点太多时均匀抽稀（保留首尾）。 */
export function thin<T>(list: T[], max: number): T[] {
  if (list.length <= max) return list
  const out: T[] = []
  const step = (list.length - 1) / (max - 1)
  for (let i = 0; i < max; i++) out.push(list[Math.round(i * step)])
  return out
}

/** 区域给人看的名字：没起名就用序号（效果已经在旁边的标签里写了）。 */
export function regionTitle(r: EditRegion, index: number): string {
  return r.name.trim() || `#${index + 1}`
}

// ---------- 监视器里框和素材画面的对应 ----------

export interface MonitorGeom {
  /** 视频元素（旋转翻转之前）在舞台里的位置和大小 */
  box: { left: number; top: number; w: number; h: number }
  /** 画面内容在视频元素里的位置和大小（按“完整显示 / 铺满”缩放之后） */
  content: { left: number; top: number; w: number; h: number }
  /** 视频元素的 transform（旋转、翻转） */
  transform: string
  /** 舞台上的像素位置（相对舞台左上角）→ 素材画面里的比例（可以超出 0–1） */
  toNorm(px: number, py: number): { x: number; y: number }
  /** 素材画面里的比例 → 舞台上的像素位置 */
  fromNorm(x: number, y: number): { px: number; py: number }
}

/**
 * 监视器里素材画面的几何：和 EditMonitor 里视频元素的摆法一致——元素按“转之前”的宽高居中摆放，
 * 画面按 object-fit 放进元素，再整体 `scale(翻转) rotate(旋转)`。框的位置据此换算，旋转、翻转、留黑边、铺满都对得上。
 */
export function monitorGeom(stage: { w: number; h: number }, c: { rotate: number; flipH: boolean; flipV: boolean }, src: { w: number; h: number }, fit: 'contain' | 'cover' | 'blur'): MonitorGeom {
  const turned = c.rotate === 90 || c.rotate === 270
  const bw = turned ? stage.h : stage.w
  const bh = turned ? stage.w : stage.h
  const sw = Math.max(1, src.w)
  const sh = Math.max(1, src.h)
  const k = fit === 'cover' ? Math.max(bw / sw, bh / sh) : Math.min(bw / sw, bh / sh)
  const cw = sw * k
  const ch = sh * k
  const th = (c.rotate * Math.PI) / 180
  const cos = Math.round(Math.cos(th) * 1e12) / 1e12
  const sin = Math.round(Math.sin(th) * 1e12) / 1e12
  const sx = c.flipH ? -1 : 1
  const sy = c.flipV ? -1 : 1
  return {
    box: { left: (stage.w - bw) / 2, top: (stage.h - bh) / 2, w: bw, h: bh },
    content: { left: (bw - cw) / 2, top: (bh - ch) / 2, w: cw, h: ch },
    transform: `scale(${sx}, ${sy}) rotate(${c.rotate}deg)`,
    toNorm(px, py) {
      // transform 是 scale·rotate：先转再翻，反过来先翻再反向转
      const ux = sx * (px - stage.w / 2)
      const uy = sy * (py - stage.h / 2)
      const lx = ux * cos + uy * sin
      const ly = -ux * sin + uy * cos
      return { x: lx / cw + 0.5, y: ly / ch + 0.5 }
    },
    fromNorm(x, y) {
      const lx = (x - 0.5) * cw
      const ly = (y - 0.5) * ch
      const rx = lx * cos - ly * sin
      const ry = lx * sin + ly * cos
      return { px: stage.w / 2 + sx * rx, py: stage.h / 2 + sy * ry }
    },
  }
}

/**
 * 跟随聚焦的取景窗口大小（占素材画面的比例）：不跟随成片比例时是整个画面除以放大倍数；
 * 跟随成片比例（`aspect` = 成片宽 / 高，已换算成素材方向）时先取画面里最大的这个比例的矩形，再除以放大倍数。
 * 和后端 `edit/region.rs` 的 `focus_window` 一致（后端算整数像素，这里只用来在监视器里画示意框）。
 */
export function focusWindow(zoom: number, src: { w: number; h: number }, aspect: number | null): { w: number; h: number } {
  const sw = Math.max(1, src.w)
  const sh = Math.max(1, src.h)
  let bw = sw
  let bh = sh
  if (aspect !== null && Number.isFinite(aspect) && aspect > 0) {
    if (aspect >= sw / sh) bh = sw / aspect
    else bw = sh * aspect
  }
  const z = Math.max(1, zoom)
  return { w: Math.min(1, bw / z / sw), h: Math.min(1, bh / z / sh) }
}

/** 取景窗口在 t 时刻的位置：以区域中心为中心，收在画面里（没有做镜头平滑，只是示意）。 */
export function focusRect(r: EditRegion, t: number, win: { w: number; h: number }): NRect | null {
  const b = trackAt(r.track, t)
  if (!b) return null
  return { x: clamp(b.x + b.w / 2 - win.w / 2, 0, 1 - win.w), y: clamp(b.y + b.h / 2 - win.h / 2, 0, 1 - win.h), w: win.w, h: win.h }
}

/** 成片比例换算到素材方向（旋转 90 / 270 度时宽高对调）。 */
export function aspectInSource(outW: number, outH: number, rotate: number): number {
  const a = outH > 0 ? outW / outH : 1
  return rotate === 90 || rotate === 270 ? 1 / a : a
}
