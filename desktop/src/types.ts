// 与 Rust 端 model.rs / download.rs / db.rs / settings.rs 的结构一一对应。

export type MediaKind = 'video' | 'images' | 'audio' | 'playlist'
export type Protocol = 'http' | 'hls' | 'ytdlp'
export type AssetKind = 'video' | 'image' | 'audio' | 'cover'

export interface Asset {
  id: string
  kind: AssetKind
  url: string
  label: string
  ext: string
  width: number | null
  height: number | null
  index: number | null
  protocol: Protocol
  headers: [string, string][]
  quality: string | null
  vcodec: string | null
  acodec: string | null
  bitrate: number | null
  filesize: number | null
  fps: number | null
  hasAudio: boolean | null
  pairAudio: string | null
  formatId: string | null
  extra: Record<string, unknown> | null
}

export interface PlaylistEntry {
  id: string
  title: string
  url: string
  durationMs: number | null
  thumbnail: string | null
  index: number
}

export interface SeriesInfo {
  name: string
  season: number | null
  episode: number | null
}

export interface MediaInfo {
  platform: string
  platformName: string
  id: string
  sourceUrl: string
  title: string
  author: string
  cover: string | null
  durationMs: number | null
  kind: MediaKind
  width: number | null
  height: number | null
  publishedAt: number | null
  assets: Asset[]
  entries: PlaylistEntry[]
  series: SeriesInfo | null
  extractor: string | null
}

export type TaskStatus = 'queued' | 'running' | 'paused' | 'done' | 'failed' | 'canceled'

export type ErrorKind =
  | 'need_login'
  | 'geo_blocked'
  | 'not_found'
  | 'encrypted'
  | 'rate_limited'
  | 'network'
  | 'parser_broken'
  | 'unsupported'
  | 'need_update'
  | 'disk'
  | 'invalid'
  | 'other'

/** Rust 端 AppError 的序列化形式。 */
export interface AppErrorPayload {
  kind: ErrorKind
  message: string
}

export interface TaskSnapshot {
  id: number
  platform: string
  platformName: string
  mediaId: string
  title: string
  author: string
  cover: string | null
  assetId: string
  assetLabel: string
  assetKind: AssetKind
  filePath: string
  status: TaskStatus
  received: number
  total: number | null
  speed: number
  error: string | null
  errorKind: ErrorKind | null
  note: string | null
  resumable: boolean | null
  step: 'download' | 'merge' | 'post' | null
  inputs: number
  priority: number
  startAt: number | null
  createdAt: number
  finishedAt: number | null
}

export interface HistoryItem {
  id: number
  platform: string
  mediaId: string
  title: string
  author: string
  cover: string | null
  kind: MediaKind
  sourceUrl: string
  createdAt: number
  info: MediaInfo
}

export interface LibraryItem {
  id: number
  platform: string
  mediaId: string
  assetId: string
  title: string
  author: string
  cover: string | null
  path: string
  size: number
  finishedAt: number
  exists: boolean
  kind: string
  source: string
  sourceUrl: string
  platformName: string
  coverPath: string | null
}

export interface LibraryFilter {
  query: string
  platform: string | null
  kind: string | null
  source: string | null
  since: number | null
  missingOnly: boolean
}

export interface PlatformCount {
  platform: string
  name: string
  count: number
}

export interface PairedDevice {
  id: string
  name: string
  addedAt: number
  lastSeen: number
}

export interface PhoneSettings {
  enabled: boolean
  port: number
  token: string
  devices: PairedDevice[]
}

export interface PairRequest {
  deviceId: string
  name: string
  ip: string
}

export interface PhoneInfo {
  enabled: boolean
  running: boolean
  port: number
  url: string | null
  apiUrl: string | null
  token: string
  qrSvg: string | null
  devices: PairedDevice[]
  pending: PairRequest[]
  error: string | null
}

export interface HealthResult {
  platform: string
  name: string
  sample: string | null
  ok: boolean
  millis: number
  message: string
  kind: ErrorKind | null
}

export type ParseMode = 'local' | 'remote' | 'local_then_remote'

export type Route = { kind: 'direct' } | { kind: 'system' } | { kind: 'proxy'; id: string }

export interface RouteRule {
  pattern: string
  route: Route
}

export interface ProxyDef {
  id: string
  name: string
  url: string
}

export interface NetworkSettings {
  defaultRoute: Route
  rules: RouteRule[]
  proxies: ProxyDef[]
}

export type ConflictPolicy = 'rename' | 'skip' | 'overwrite'

export interface RouteTest {
  route: Route
  status: number
  millis: number
}

export interface AccountStatus {
  loggedIn: boolean
  userName: string | null
  vip: string | null
}

export interface Settings {
  downloadDir: string
  subfolderByPlatform: boolean
  filenameTemplate: string
  concurrency: number
  skipExisting: boolean
  watchClipboard: boolean
  autoDownload: boolean
  notifyOnComplete: boolean
  parseMode: ParseMode
  remoteEndpoint: string
  theme: 'system' | 'dark' | 'light'
  closeToTray: boolean
  shortcut: string
  checkUpdate: boolean
  disclaimerAccepted: boolean
  autoResume: boolean
  maxRetries: number
  retryDelaySecs: number
  keepPartOnCancel: boolean
  recordSamples: boolean
  network: NetworkSettings
  segments: number
  segmentMinMb: number
  speedLimitKbps: number
  perSiteConcurrency: number
  siteRequestIntervalMs: number
  tempDir: string
  conflictPolicy: ConflictPolicy
  diskReserveMb: number
  qualityPreset: QualityPreset
  preferH264: boolean
  mergeContainer: string
  audioFormat: string
  hlsConcurrency: number
  hlsSkipAds: boolean
  seriesTemplate: string
  useYtdlp: boolean
  genericSniffer: boolean
  componentMirrors: string[]
  clipboardAllSites: boolean
  clipboardDomains: string[]
  phone: PhoneSettings
  embedMetadata: boolean
  writeInfoJson: boolean
  writeNfo: boolean
  openFolderOnDone: boolean
  postScriptEnabled: boolean
  postScript: string
  preventSleep: boolean
  subscriptionsEnabled: boolean
  launchAtLogin: boolean
}

export interface SubSettings {
  intervalHours: number
  firstRun: 'new_only' | 'latest' | 'all'
  firstN: number
  maxAuto: number
  include: string[]
  exclude: string[]
  minDurationS: number | null
  maxDurationS: number | null
  maxAgeDays: number | null
  quality: QualityPreset | null
  dir: string
  template: string
  notify: boolean
}

export interface Subscription {
  id: number
  url: string
  title: string
  platform: string
  platformName: string
  avatar: string | null
  settings: SubSettings
  status: 'active' | 'paused' | 'error'
  lastCheck: number | null
  nextCheck: number | null
  lastError: string | null
  failCount: number
  newCount: number
  createdAt: number
  downloaded: number
  pending: number
  ignored: number
  checking: boolean
}

export interface SubEntry {
  id: string
  title: string
  url: string
  thumbnail: string | null
  publishedAt: number | null
  durationMs: number | null
}

export interface ListResult {
  title: string
  platform: string
  platformName: string
  avatar: string | null
  entries: SubEntry[]
}

export type SubItemStatus = 'seen' | 'pending' | 'queued' | 'downloaded' | 'ignored' | 'failed'

export interface SubItem {
  itemId: string
  title: string
  url: string
  thumbnail: string | null
  publishedAt: number | null
  durationMs: number | null
  status: SubItemStatus
  reason: string | null
  createdAt: number
}

export type QualityPreset = 'best' | 'max1080' | 'small' | 'audio'

export interface PostOptions {
  extractAudio: string | null
  embedMetadata: boolean
}

export type ToolId = 'yt-dlp' | 'ffmpeg'

export interface ToolStatus {
  id: ToolId
  installed: boolean
  managed: boolean
  path: string | null
  version: string | null
  hasPrevious: boolean
  autoInstall: boolean
  note: string | null
  busy: boolean
}

export interface ToolProgress {
  tool: ToolId
  stage: 'prepare' | 'download' | 'verify' | 'extract' | 'done'
  received: number
  total: number | null
}

export interface BatchProgress {
  title: string
  done: number
  total: number
  queued: number
  skipped: number
  failed: string[]
  finished: boolean
}

export interface SaveSettingsResult {
  settings: Settings
  shortcutError: string | null
}

export interface EnqueueResult {
  tasks: TaskSnapshot[]
  alreadyDownloaded: number
  alreadyQueued: number
}

export interface OrphanPart {
  path: string
  size: number
  modified: number
}

export interface AccountSummary {
  id: string
  site: string
  siteName: string
  label: string
  cookieCount: number
  updatedAt: number
  userName: string | null
  expiresAt: number | null
  isDefault: boolean
}

export interface DetectedLink {
  url: string
  platform: string
  platformName: string
}

export interface AppInfo {
  version: string
  providers: { id: string; name: string }[]
  repo: string
  os: string
  keyInKeyring: boolean
  shortcutError: string | null
}

export interface UpdateInfo {
  current: string
  latest: string | null
  hasUpdate: boolean
  url: string
}

export interface ClipboardLink {
  text: string
  links: DetectedLink[]
}

export interface AutoResult {
  url: string
  ok: boolean
  message: string
}
