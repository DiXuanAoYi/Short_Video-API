import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  AccountStatus,
  AccountSummary,
  RouteTest,
  AppErrorPayload,
  AppInfo,
  EnqueueResult,
  ErrorKind,
  OrphanPart,
  SaveSettingsResult,
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
  saveSettings: (settings: Settings) => invoke<SaveSettingsResult>('save_settings', { settings }),

  detectLinks: (text: string) => invoke<DetectedLink[]>('detect_links', { text }),
  resolve: (text: string) => invoke<MediaInfo>('resolve_link', { text }),
  resolveAndEnqueue: (text: string) => invoke<string>('resolve_and_enqueue', { text }),
  enqueue: (media: MediaInfo, assetIds: string[]) => invoke<EnqueueResult>('enqueue', { media, assetIds }),

  listTasks: () => invoke<TaskSnapshot[]>('list_tasks'),
  pauseTask: (id: number) => invoke<void>('pause_task', { id }),
  resumeTask: (id: number) => invoke<void>('resume_task', { id }),
  cancelTask: (id: number) => invoke<void>('cancel_task', { id }),
  removeTask: (id: number) => invoke<void>('remove_task', { id }),
  clearFinished: () => invoke<void>('clear_finished'),
  pauseAll: () => invoke<void>('pause_all'),
  resumeAll: () => invoke<void>('resume_all'),
  listOrphanParts: () => invoke<OrphanPart[]>('list_orphan_parts'),
  deleteOrphanParts: (paths: string[]) => invoke<number>('delete_orphan_parts', { paths }),

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
  openLogin: (site: string, url?: string) => invoke<void>('open_login', { site, url }),
  saveLoginCookies: (site: string, label?: string) => invoke<AccountSummary[]>('save_login_cookies', { site, label }),
  listAccounts: () => invoke<AccountSummary[]>('list_accounts'),
  importCookiesFile: (path: string, label?: string) => invoke<AccountSummary[]>('import_cookies_file', { path, label }),
  importCookiesText: (site: string, text: string, label?: string) => invoke<AccountSummary[]>('import_cookies_text', { site, text, label }),
  renameAccount: (id: string, label: string) => invoke<AccountSummary[]>('rename_account', { id, label }),
  setDefaultAccount: (id: string) => invoke<AccountSummary[]>('set_default_account', { id }),
  deleteAccount: (id: string) => invoke<AccountSummary[]>('delete_account', { id }),
  checkAccount: (id: string) => invoke<AccountStatus>('check_account', { id }),
  testRoute: (url: string) => invoke<RouteTest>('test_route', { url }),
  getDiagnostics: () => invoke<string>('get_diagnostics'),
  openLogDir: () => invoke<void>('open_log_dir'),
  openSamplesDir: () => invoke<void>('open_samples_dir'),
  checkUpdate: () => invoke<UpdateInfo>('check_update'),
}

export const events = {
  onTasks: (cb: (tasks: TaskSnapshot[]) => void): Promise<UnlistenFn> => listen<TaskSnapshot[]>('tasks://updated', (e) => cb(e.payload)),
  onProgress: (cb: (task: TaskSnapshot) => void): Promise<UnlistenFn> => listen<TaskSnapshot>('tasks://progress', (e) => cb(e.payload)),
  onClipboardLink: (cb: (p: ClipboardLink) => void): Promise<UnlistenFn> => listen<ClipboardLink>('clipboard://link', (e) => cb(e.payload)),
  onAutoResult: (cb: (p: AutoResult) => void): Promise<UnlistenFn> => listen<AutoResult>('clipboard://auto-result', (e) => cb(e.payload)),
  onParseRequest: (cb: (text: string) => void): Promise<UnlistenFn> => listen<string>('app://parse-request', (e) => cb(e.payload)),
}

function isAppError(e: unknown): e is AppErrorPayload {
  return typeof e === 'object' && e !== null && 'message' in e && 'kind' in e
}

/** Rust 端的错误序列化为 `{ kind, message }`。 */
export function errorText(e: unknown): string {
  if (isAppError(e)) return e.message
  if (typeof e === 'string') return e
  if (e instanceof Error) return e.message
  return String(e)
}

export function errorKind(e: unknown): ErrorKind {
  return isAppError(e) ? e.kind : 'other'
}
