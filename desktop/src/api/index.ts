import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AppInfo,
  AutoResult,
  ClipboardLink,
  DetectedLink,
  HistoryItem,
  LibraryItem,
  MediaInfo,
  Settings,
  TaskSnapshot,
  UpdateInfo,
} from '../types'

export const api = {
  appInfo: () => invoke<AppInfo>('get_app_info'),
  getSettings: () => invoke<Settings>('get_settings'),
  saveSettings: (settings: Settings) => invoke<Settings>('save_settings', { settings }),

  detectLinks: (text: string) => invoke<DetectedLink[]>('detect_links', { text }),
  resolve: (text: string) => invoke<MediaInfo>('resolve_link', { text }),
  resolveAndEnqueue: (text: string) => invoke<string>('resolve_and_enqueue', { text }),
  enqueue: (media: MediaInfo, assetIds: string[]) => invoke<TaskSnapshot[]>('enqueue', { media, assetIds }),

  listTasks: () => invoke<TaskSnapshot[]>('list_tasks'),
  pauseTask: (id: number) => invoke<void>('pause_task', { id }),
  resumeTask: (id: number) => invoke<void>('resume_task', { id }),
  cancelTask: (id: number) => invoke<void>('cancel_task', { id }),
  removeTask: (id: number) => invoke<void>('remove_task', { id }),
  clearFinished: () => invoke<void>('clear_finished'),
  pauseAll: () => invoke<void>('pause_all'),
  resumeAll: () => invoke<void>('resume_all'),

  listHistory: (query = '') => invoke<HistoryItem[]>('list_history', { query }),
  deleteHistory: (id: number) => invoke<void>('delete_history', { id }),
  clearHistory: () => invoke<void>('clear_history'),
  listLibrary: (query = '') => invoke<LibraryItem[]>('list_library', { query }),
  deleteLibrary: (id: number, deleteFile: boolean) => invoke<void>('delete_library', { id, deleteFile }),

  copyText: (text: string) => invoke<void>('copy_text', { text }),
  openFile: (path: string) => invoke<void>('open_file', { path }),
  revealFile: (path: string) => invoke<void>('reveal_file', { path }),
  openUrl: (url: string) => invoke<void>('open_url', { url }),
  showMain: () => invoke<void>('show_main'),
  hideMini: () => invoke<void>('hide_mini'),
  openLogin: (platform: string) => invoke<void>('open_login', { platform }),
  saveLoginCookies: (platform: string) => invoke<Settings>('save_login_cookies', { platform }),
  checkUpdate: () => invoke<UpdateInfo>('check_update'),
}

export const events = {
  onTasks: (cb: (tasks: TaskSnapshot[]) => void): Promise<UnlistenFn> => listen<TaskSnapshot[]>('tasks://updated', (e) => cb(e.payload)),
  onProgress: (cb: (task: TaskSnapshot) => void): Promise<UnlistenFn> => listen<TaskSnapshot>('tasks://progress', (e) => cb(e.payload)),
  onClipboardLink: (cb: (p: ClipboardLink) => void): Promise<UnlistenFn> => listen<ClipboardLink>('clipboard://link', (e) => cb(e.payload)),
  onAutoResult: (cb: (p: AutoResult) => void): Promise<UnlistenFn> => listen<AutoResult>('clipboard://auto-result', (e) => cb(e.payload)),
  onParseRequest: (cb: (text: string) => void): Promise<UnlistenFn> => listen<string>('app://parse-request', (e) => cb(e.payload)),
}

/** Rust 端的错误会序列化成字符串。 */
export function errorText(e: unknown): string {
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  return String(e)
}
