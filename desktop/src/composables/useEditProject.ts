// 剪辑工程的状态：工程内容、素材信息、选中项、播放位置、撤销 / 重做、打开 / 保存、草稿。

import { computed, reactive, ref, watch } from 'vue'
import { api, errorText } from '../api'
import type { EditAudio, EditClip, EditProject, EditSource, EditText } from '../types'
import {
  MAX_AUDIO,
  MAX_CLIPS,
  MAX_TEXTS,
  MIN_MS,
  audioFromSource,
  clamp,
  clipDuration,
  clipFromSource,
  clipIndexAt,
  emptyProject,
  layout,
  mediaPaths,
  moveItem,
  nextId,
  normalizeProject,
  splitClip,
  textAt,
  totalMs,
} from '../utils/edit'

export interface Sel {
  kind: 'clip' | 'text' | 'audio'
  id: number
}

const DRAFT_KEY = 'clearclip.edit.draft'
const HISTORY_LIMIT = 100

export interface AddReport {
  added: number
  errors: string[]
}

export function useEditProject() {
  const project = ref<EditProject>(emptyProject())
  const sources = reactive<Record<string, EditSource>>({})
  /** 工程里用到、但现在找不到或读不出来的文件 */
  const broken = reactive<Record<string, string>>({})
  const sel = ref<Sel | null>(null)
  const playhead = ref(0)
  const playing = ref(false)
  const filePath = ref('')
  const dirty = ref(false)
  const busy = ref(false)

  const placed = computed(() => layout(project.value.clips))
  const total = computed(() => totalMs(project.value.clips))
  /** 时间线上所有东西里最晚结束的时间（文字、音频可以超出主轨） */
  const extent = computed(() => {
    let m = total.value
    for (const t of project.value.texts) m = Math.max(m, t.endMs)
    for (const a of project.value.audio) m = Math.max(m, a.startMs + 1000)
    return m
  })

  // ---------- 撤销 / 重做 ----------

  const undoStack = ref<string[]>([])
  const redoStack = ref<string[]>([])
  let lastKey = ''
  let lastAt = 0
  const snapshot = () => JSON.stringify(project.value)

  /** 修改工程。`key` 相同且间隔很短的连续修改（拖滑块、拖片段边缘）合并成一步撤销。 */
  function mutate(fn: (p: EditProject) => void, key = '') {
    const now = Date.now()
    const merge = key !== '' && key === lastKey && now - lastAt < 800
    if (!merge) {
      undoStack.value.push(snapshot())
      if (undoStack.value.length > HISTORY_LIMIT) undoStack.value.shift()
      redoStack.value = []
    }
    lastKey = key
    lastAt = now
    fn(project.value)
    dirty.value = true
  }

  function restore(json: string) {
    project.value = normalizeProject(JSON.parse(json))
    dirty.value = true
    lastKey = ''
    if (sel.value && !find(sel.value)) sel.value = null
    playhead.value = clamp(playhead.value, 0, Math.max(0, total.value))
  }
  function undo() {
    const prev = undoStack.value.pop()
    if (prev === undefined) return
    redoStack.value.push(snapshot())
    restore(prev)
  }
  function redo() {
    const next = redoStack.value.pop()
    if (next === undefined) return
    undoStack.value.push(snapshot())
    restore(next)
  }
  const canUndo = computed(() => undoStack.value.length > 0)
  const canRedo = computed(() => redoStack.value.length > 0)

  // ---------- 取项目 ----------

  function find(s: Sel): EditClip | EditText | EditAudio | undefined {
    const p = project.value
    return (s.kind === 'clip' ? p.clips : s.kind === 'text' ? p.texts : p.audio).find((x) => x.id === s.id)
  }
  const clipOf = (id: number) => project.value.clips.find((c) => c.id === id)
  const textOf = (id: number) => project.value.texts.find((c) => c.id === id)
  const audioOf = (id: number) => project.value.audio.find((c) => c.id === id)
  const selectedClip = computed(() => (sel.value?.kind === 'clip' ? clipOf(sel.value.id) : undefined))
  const selectedText = computed(() => (sel.value?.kind === 'text' ? textOf(sel.value.id) : undefined))
  const selectedAudio = computed(() => (sel.value?.kind === 'audio' ? audioOf(sel.value.id) : undefined))

  function patchClip(id: number, patch: Partial<EditClip>, key = '') {
    mutate((p) => {
      const c = p.clips.find((x) => x.id === id)
      if (c) Object.assign(c, patch)
    }, key)
  }
  function patchText(id: number, patch: Partial<EditText>, key = '') {
    mutate((p) => {
      const c = p.texts.find((x) => x.id === id)
      if (c) Object.assign(c, patch)
    }, key)
  }
  function patchAudio(id: number, patch: Partial<EditAudio>, key = '') {
    mutate((p) => {
      const c = p.audio.find((x) => x.id === id)
      if (c) Object.assign(c, patch)
    }, key)
  }

  // ---------- 素材 ----------

  async function probe(path: string): Promise<EditSource> {
    const known = sources[path]
    if (known) return known
    try {
      const s = await api.editProbe(path)
      sources[path] = s
      delete broken[path]
      return s
    } catch (e) {
      broken[path] = errorText(e)
      throw e
    }
  }

  /** 并发读取一批素材（每次最多 3 个）。返回的数组和 paths 一一对应，读不出来的是错误信息。 */
  async function probeAll(paths: string[]): Promise<(EditSource | string)[]> {
    const out: (EditSource | string)[] = new Array(paths.length)
    let next = 0
    const worker = async () => {
      while (next < paths.length) {
        const i = next++
        try {
          out[i] = await probe(paths[i])
        } catch (e) {
          out[i] = errorText(e)
        }
      }
    }
    await Promise.all([worker(), worker(), worker()])
    return out
  }

  const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p

  /** 把视频、图片放到主轨末尾；音频文件自动放到音频轨。 */
  async function addClips(paths: string[]): Promise<AddReport> {
    const report: AddReport = { added: 0, errors: [] }
    busy.value = true
    try {
      const infos = await probeAll(paths)
      mutate((p) => {
        infos.forEach((info, i) => {
          if (typeof info === 'string') {
            report.errors.push(`${baseName(paths[i])}：${info}`)
            return
          }
          if (info.kind === 'audio') {
            if (p.audio.length >= MAX_AUDIO) return report.errors.push(`音频轨最多 ${MAX_AUDIO} 条，${info.name} 没有加入。`)
            const a = audioFromSource(info, nextId(p), 0)
            p.audio.push(a)
            report.added++
            return
          }
          if (p.clips.length >= MAX_CLIPS) return report.errors.push(`片段最多 ${MAX_CLIPS} 个，${info.name} 没有加入。`)
          const c = clipFromSource(info, nextId(p))
          p.clips.push(c)
          sel.value = { kind: 'clip', id: c.id }
          report.added++
        })
      })
    } finally {
      busy.value = false
    }
    return report
  }

  /** 添加配乐（音频文件，或者想用它声音的视频）。从播放位置开始。 */
  async function addMusic(paths: string[]): Promise<AddReport> {
    const report: AddReport = { added: 0, errors: [] }
    busy.value = true
    try {
      const infos = await probeAll(paths)
      mutate((p) => {
        infos.forEach((info, i) => {
          if (typeof info === 'string') return report.errors.push(`${baseName(paths[i])}：${info}`)
          if (!info.hasAudio) return report.errors.push(`${info.name}：这个文件里没有声音。`)
          if (p.audio.length >= MAX_AUDIO) return report.errors.push(`音频轨最多 ${MAX_AUDIO} 条，${info.name} 没有加入。`)
          const a = audioFromSource(info, nextId(p), Math.round(playhead.value))
          p.audio.push(a)
          sel.value = { kind: 'audio', id: a.id }
          report.added++
        })
      })
    } finally {
      busy.value = false
    }
    return report
  }

  function addText(): boolean {
    if (project.value.texts.length >= MAX_TEXTS) return false
    mutate((p) => {
      const t = textAt(nextId(p), Math.round(playhead.value))
      p.texts.push(t)
      sel.value = { kind: 'text', id: t.id }
    })
    return true
  }

  // ---------- 编辑操作 ----------

  /** 在播放位置分割。选中了片段就分割它，否则分割播放位置下的片段。成功返回 true。 */
  function splitAtPlayhead(): boolean {
    const t = Math.round(playhead.value)
    const pl = placed.value
    const idx = selectedClip.value ? project.value.clips.findIndex((c) => c.id === selectedClip.value?.id) : clipIndexAt(pl, t)
    if (idx < 0) return false
    const parts = splitClip(project.value.clips[idx], pl[idx], t, nextId(project.value))
    if (!parts) return false
    mutate((p) => {
      p.clips.splice(idx, 1, parts[0], parts[1])
    })
    sel.value = { kind: 'clip', id: parts[1].id }
    return true
  }

  function removeSelected() {
    const s = sel.value
    if (!s) return
    mutate((p) => {
      if (s.kind === 'clip') p.clips = p.clips.filter((c) => c.id !== s.id)
      else if (s.kind === 'text') p.texts = p.texts.filter((c) => c.id !== s.id)
      else p.audio = p.audio.filter((c) => c.id !== s.id)
    })
    sel.value = null
    playhead.value = clamp(playhead.value, 0, total.value)
  }

  function duplicateSelected(): boolean {
    const s = sel.value
    if (!s) return false
    let ok = false
    mutate((p) => {
      const id = nextId(p)
      if (s.kind === 'clip') {
        const i = p.clips.findIndex((c) => c.id === s.id)
        if (i < 0 || p.clips.length >= MAX_CLIPS) return
        p.clips.splice(i + 1, 0, { ...p.clips[i], id, transition: null })
      } else if (s.kind === 'text') {
        const t = p.texts.find((c) => c.id === s.id)
        if (!t || p.texts.length >= MAX_TEXTS) return
        const len = t.endMs - t.startMs
        p.texts.push({ ...t, id, startMs: t.endMs, endMs: t.endMs + len })
      } else {
        const a = p.audio.find((c) => c.id === s.id)
        if (!a || p.audio.length >= MAX_AUDIO) return
        p.audio.push({ ...a, id })
      }
      sel.value = { kind: s.kind, id }
      ok = true
    })
    return ok
  }

  function reorderClip(from: number, to: number) {
    if (from === to || from < 0 || from >= project.value.clips.length) return
    mutate((p) => {
      p.clips = moveItem(p.clips, from, to)
    })
  }

  /** 把一个素材路径换成另一个（找不到文件时重新指定），所有用到它的片段、音频一起换。 */
  async function relink(oldPath: string, newPath: string) {
    await probe(newPath)
    mutate((p) => {
      for (const c of p.clips) if (c.path === oldPath) c.path = newPath
      for (const a of p.audio) if (a.path === oldPath) a.path = newPath
    })
    delete broken[oldPath]
  }

  /** 工程里可用的素材信息（缩略图、时长等）。 */
  const sourceOf = (path: string): EditSource | undefined => sources[path]

  // ---------- 新建 / 打开 / 保存 ----------

  function reset(next: EditProject, path = '') {
    project.value = next
    filePath.value = path
    sel.value = null
    playhead.value = 0
    playing.value = false
    undoStack.value = []
    redoStack.value = []
    lastKey = ''
    dirty.value = false
  }

  function newProject() {
    reset(emptyProject())
  }

  /** 读取工程里所有素材的信息；找不到的记入 `broken`。 */
  async function loadSources(p: EditProject, missing: string[] = []) {
    for (const m of missing) broken[m] = '找不到文件'
    const paths = mediaPaths(p).filter((m) => !missing.includes(m) && !/\.(ttf|otf|ttc)$/i.test(m))
    busy.value = true
    try {
      await probeAll(paths)
    } finally {
      busy.value = false
    }
  }

  async function openProject(path: string): Promise<string[]> {
    const r = await api.editOpen(path)
    const p = normalizeProject(r.project)
    for (const k of Object.keys(broken)) delete broken[k]
    reset(p, path)
    await loadSources(p, r.missing)
    return r.missing
  }

  async function saveProject(path: string) {
    await api.editSave(path, JSON.parse(snapshot()))
    filePath.value = path
    dirty.value = false
  }

  // ---------- 草稿（退出或崩溃后能找回） ----------

  let draftTimer: ReturnType<typeof setTimeout> | undefined
  watch(
    project,
    () => {
      clearTimeout(draftTimer)
      draftTimer = setTimeout(() => {
        try {
          const p = project.value
          if (!p.clips.length && !p.texts.length && !p.audio.length) localStorage.removeItem(DRAFT_KEY)
          else localStorage.setItem(DRAFT_KEY, JSON.stringify({ project: p, filePath: filePath.value }))
        } catch {
          /* 存不了草稿不影响使用 */
        }
      }, 800)
    },
    { deep: true },
  )

  function readDraft(): { project: EditProject; filePath: string } | null {
    try {
      const raw = localStorage.getItem(DRAFT_KEY)
      if (!raw) return null
      const d = JSON.parse(raw) as { project?: EditProject; filePath?: string }
      const p = normalizeProject(d.project)
      return p.clips.length || p.texts.length || p.audio.length ? { project: p, filePath: d.filePath ?? '' } : null
    } catch {
      return null
    }
  }
  function clearDraft() {
    try {
      localStorage.removeItem(DRAFT_KEY)
    } catch {
      /* 同上 */
    }
  }
  async function restoreDraft(d: { project: EditProject; filePath: string }) {
    const gone = await api.editMissing(mediaPaths(d.project).filter((m) => !/\.(ttf|otf|ttc)$/i.test(m)))
    for (const k of Object.keys(broken)) delete broken[k]
    reset(d.project, d.filePath)
    dirty.value = true
    await loadSources(d.project, gone)
  }

  return {
    project,
    sources,
    broken,
    sel,
    playhead,
    playing,
    filePath,
    dirty,
    busy,
    placed,
    total,
    extent,
    canUndo,
    canRedo,
    selectedClip,
    selectedText,
    selectedAudio,
    mutate,
    undo,
    redo,
    clipOf,
    textOf,
    audioOf,
    patchClip,
    patchText,
    patchAudio,
    probe,
    addClips,
    addMusic,
    addText,
    splitAtPlayhead,
    removeSelected,
    duplicateSelected,
    reorderClip,
    relink,
    sourceOf,
    newProject,
    openProject,
    saveProject,
    readDraft,
    clearDraft,
    restoreDraft,
    clipDuration,
    MIN_MS,
  }
}

export type EditApi = ReturnType<typeof useEditProject>
