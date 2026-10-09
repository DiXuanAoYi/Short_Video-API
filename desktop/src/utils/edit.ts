// 剪辑工程的纯计算：时间线排布、新建片段、分割、取时间等。
// 排布规则和后端 `src-tauri/src/edit/spec.rs` 的 `layout` 保持一致，改那边时要一起改这里。
// 这个文件不要引入别的运行时模块（用 node 直接跑 scripts 里的检查时需要）。

import type { EditAudio, EditClip, EditOutput, EditProject, EditSource, EditText } from '../types'

export const MIN_MS = 100
export const MIN_TRANSITION_MS = 100
export const MAX_CLIPS = 300
export const MAX_TEXTS = 100
export const MAX_AUDIO = 20
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
  p.clips = (r.clips ?? []).map((c, i) => ({ ...blankClip(), ...c, id: c.id ?? i + 1 }))
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
  const first: EditClip = { ...c, transition: c.transition ? { ...c.transition } : null }
  const second: EditClip = { ...c, id: newId, transition: null }
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
