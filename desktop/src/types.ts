// 与 Rust 端 model.rs / download.rs / db.rs / settings.rs 的结构一一对应。

export type MediaKind = 'video' | 'images' | 'audio' | 'playlist'
export type Protocol = 'http' | 'hls' | 'ytdlp'
export type AssetKind = 'video' | 'image' | 'audio' | 'cover' | 'subtitle'

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
  chapters: Chapter[]
}

export interface Chapter {
  title: string
  startMs: number
  endMs: number
}

/** 只下载 / 保留的时间段 */
export interface Clip {
  startMs: number
  endMs: number | null
  /** 重新编码，起点精确到帧 */
  precise: boolean
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
  /** 登录用的网站（内置平台 ID 或域名） */
  site: string
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
  /** 下载成功但有附带问题（如字幕没有处理成功） */
  warning: string | null
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
  favorite: boolean
  /** 0–5，0 表示未评分 */
  rating: number
  note: string
  tags: string[]
  durationMs: number | null
}

export interface LibraryFilter {
  query: string
  platform: string | null
  kind: string | null
  source: string | null
  since: number | null
  missingOnly: boolean
  tag: string | null
  favoriteOnly: boolean
  minRating: number
  sort: 'finished' | 'size' | 'title' | 'rating' | null
}

export interface TagCount {
  name: string
  count: number
}

export interface Bucket {
  key: string
  name: string
  count: number
  size: number
}

export interface LibraryStats {
  count: number
  totalSize: number
  missing: number
  byKind: Bucket[]
  byPlatform: Bucket[]
  byAuthor: Bucket[]
  byMonth: Bucket[]
  largest: { id: number; title: string; path: string; size: number; exists: boolean }[]
}

export interface TrashItem {
  id: number
  title: string
  originalPath: string
  size: number
  deletedAt: number
}

export interface CueHit {
  itemId: number | null
  title: string
  path: string
  lang: string
  startMs: number
  text: string
}

export interface MovePlan {
  id: number
  from: string
  to: string
}

export interface MoveReport {
  moved: number
  skipped: string[]
}

export interface ImportReport {
  added: number
  subtitles: number
  skipped: number
}

export interface DupGroup {
  kind: 'exact' | 'similar'
  items: LibraryItem[]
}

export interface DeleteReport {
  removed: number
  trashed: number
  failed: string[]
}

export interface SceneList {
  durationMs: number
  scenes: Chapter[]
}

export interface BackupInfo {
  appVersion: string
  createdAt: number
  size: number
}

export interface DanmakuStyle {
  fontSize: number
  opacity: number
  scrollSecs: number
  area: number
  font: string
}

export type ToolOp =
  | { op: 'compress'; quality: 'small' | 'balanced' | 'high'; maxHeight: number | null }
  | { op: 'gif'; startMs: number; durationMs: number; width: number; fps: number }
  | { op: 'landscape'; width: number; height: number; mode: 'blur' | 'black' }
  | { op: 'speed'; factor: number }
  | { op: 'loudness'; target: number }
  | { op: 'rotate'; mode: 'cw' | 'ccw' | 'flip180' | 'hflip' | 'vflip' }
  | { op: 'convert'; format: string; reencode: boolean }
  | { op: 'frame'; atMs: number; format: 'jpg' | 'png' }
  | { op: 'frames'; everySecs: number; format: 'jpg' | 'png' }
  | { op: 'concat'; reencode: boolean }
  | { op: 'trim'; startMs: number; endMs: number | null; precise: boolean }
  | { op: 'mute' }
  | { op: 'chapters'; chapters: Chapter[] }
  | { op: 'normalize'; spec: NormSpec; preset?: string | null }
  | { op: 'stabilize'; strength: 'light' | 'normal' | 'strong' }
  | { op: 'tags'; title?: string | null; artist?: string | null; album?: string | null; year?: string | null; genre?: string | null; comment?: string | null; cover?: string | null }

export type ToolJob = ToolOp & { inputs: string[]; outputDir?: string | null }

// ---------- 视频规整 ----------

export interface NormSpec {
  size: 'keep' | 'limit' | 'fit' | 'blur' | 'fill'
  width: number
  height: number
  shortSide: number
  followOrientation: boolean
  fps: 'keep' | 'auto' | 'fixed'
  fpsValue: number
  hdr: boolean
  fixColor: boolean
  levels: number
  lut: string | null
  lutStrength: number
  autocrop: boolean
  audioRate: number
  audioChannels: number
  loudness: number | null
  fixSync: boolean
  codec: 'keep' | 'h264' | 'hevc'
  quality: 'small' | 'balanced' | 'high'
}

export interface NormPreset {
  id: string
  name: string
  desc: string
  spec: NormSpec
}

export interface VideoStreamFacts {
  codec: string
  width: number
  height: number
  pixFmt: string
  bitDepth: number
  range: string | null
  matrix: string | null
  primaries: string | null
  transfer: string | null
  fps: number | null
  tbr: number | null
  rotation: number
  sar: [number, number] | null
  bitrateKbps: number | null
  hdr: 'none' | 'pq' | 'hlg'
  dolbyVision: boolean
  dolbyProfile: number | null
}

export interface AudioStreamFacts {
  codec: string
  sampleRate: number
  channels: number
  bitrateKbps: number | null
}

export interface VideoFactsInfo {
  durationMs: number | null
  container: string
  video: VideoStreamFacts | null
  audio: AudioStreamFacts | null
}

export interface VideoAnalysis {
  vfr: { variable: boolean; medianFps: number; minFps: number; maxFps: number; frames: number } | null
  crop: { w: number; h: number; x: number; y: number; srcW: number; srcH: number } | null
  loudness: { inputI: number; inputTp: number; inputLra: number; inputThresh: number; targetOffset: number } | null
}

export interface VideoIssue {
  id: string
  label: string
  detail: string
  level: 'info' | 'warn'
}

export interface VideoReport {
  facts: VideoFactsInfo
  analysis: VideoAnalysis
  issues: VideoIssue[]
  recommended: string
}

export interface VideoPreview {
  before: string
  after: string
  notes: string[]
  warnings: string[]
  changed: boolean
}

export interface CapsSummary {
  version: string
  edition: 'full' | 'lite'
  h264: string | null
  hevc: string | null
  hardware: string[]
  tonemap: boolean
  lut: boolean
  levels: boolean
  cropDetect: boolean
  stabilize: 'vidstab' | 'deshake' | 'none'
  notes: string[]
}

export interface JobSnap {
  id: number
  title: string
  op: string
  status: 'queued' | 'running' | 'done' | 'failed' | 'canceled'
  percent: number
  output: string | null
  error: string | null
  note: string | null
  finishedAt: number | null
}

export interface MediaInfoLite {
  durationMs: number | null
  width: number | null
  height: number | null
  hasVideo: boolean
  hasAudio: boolean
  tags: Record<string, string>
}

export type SubOp =
  | { op: 'shift'; offsetMs: number }
  | { op: 'rescale'; factor: number }
  | { op: 'align'; aFrom: number; aTo: number; bFrom: number; bTo: number }
  | { op: 'merge'; second: string }
  | { op: 'clean'; dropHearing: boolean }
  | { op: 'convert' }
  | { op: 'danmaku'; width?: number | null; height?: number | null }

export type SubJob = SubOp & { input: string; format?: string | null }

export interface SiteRule {
  name: string
  enabled: boolean
  pattern: string
  videoRegex: string
  titleRegex: string
  coverRegex: string
  referer: string
  userAgent: string
}

export interface NotifyChannel {
  id: string
  name: string
  enabled: boolean
  kind: 'webhook' | 'telegram' | 'bark' | 'serverchan' | 'wecom' | 'dingtalk' | 'feishu' | 'ntfy'
  target: string
}

export interface NotifySettings {
  channels: NotifyChannel[]
  onDone: boolean
  onFailed: boolean
  onLive: boolean
  onSub: boolean
  onAccount: boolean
}

export interface UploadSettings {
  enabled: boolean
  kind: 'webdav' | 'folder'
  url: string
  user: string
  remoteDir: string
  kinds: string[]
  deleteAfter: boolean
}

export interface RuleWhen {
  platform: string
  author: string
  title: string
  kind: string
  source: string
  minSizeMb: number
}

export interface RuleThen {
  addTags: string[]
  favorite: boolean
  moveTo: string
  extractAudio: string
  upload: boolean
  notify: boolean
  normalize: string
}

export interface AutoRule {
  id: string
  name: string
  enabled: boolean
  when: RuleWhen
  then: RuleThen
}

export interface AiSettings {
  baseUrl: string
  model: string
  targetLang: string
  bilingual: boolean
  batchSize: number
  sttEngine: 'api' | 'local'
  sttBaseUrl: string
  sttModel: string
  sttLanguage: string
  whisperBin: string
  whisperModel: string
  autoTranslate: boolean
  autoTranscribe: boolean
}

export interface AiStatus {
  chatReady: boolean
  chatKey: boolean
  sttReady: boolean
  sttKey: boolean
  whisperFound: boolean
}

export interface LibrarySettings {
  useTrash: boolean
  trashKeepDays: number
  reorganizeTemplate: string
  probeNewFiles: boolean
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

export interface RouteSpeed {
  route: Route
  name: string
  ok: boolean
  ttfbMs: number
  kbps: number
  bytes: number
  error: string | null
}

export interface SpeedWindow {
  days: number[]
  start: string
  end: string
  /** KB/s，0 表示不限 */
  limitKbps: number
}

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
  ffmpegEdition: 'lite' | 'full'
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
  liveMaxRecordings: number
  liveCleanupDays: number
  inbox: InboxSettings
  subtitleLangs: string[]
  subtitleAuto: boolean
  subtitleMode: 'off' | 'file' | 'embed' | 'burn'
  subtitleConvert: boolean
  danmakuAss: boolean
  verifyDownloads: boolean
  library: LibrarySettings
  danmaku: DanmakuStyle
  maxSizeMb: number
  autoDowngrade: boolean
  ai: AiSettings
  customSites: SiteRule[]
  notify: NotifySettings
  upload: UploadSettings
  rules: AutoRule[]
  speedSchedule: SpeedWindow[]
  meteredMode: boolean
  security: SecuritySettings
  onboarded: boolean
  language: 'zh' | 'en' | 'ja'
  floatBall: boolean
}

export interface SecuritySettings {
  /** 空闲多少分钟后自动锁定，0 表示不自动锁定 */
  autoLockMinutes: number
  lockOnHide: boolean
  privacyMode: boolean
  contentProtection: boolean
  /** 老板键：立刻隐藏窗口并锁定；空表示不启用 */
  panicShortcut: string
  safeboxAutoLockMinutes: number
}

export interface LockStatus {
  enabled: boolean
  locked: boolean
  lockedOutSecs: number
}

export interface SafeStatus {
  exists: boolean
  unlocked: boolean
  count: number
  totalBytes: number
  lockedOutSecs: number
}

export interface SafeEntry {
  id: string
  name: string
  size: number
  addedAt: number
  kind: 'video' | 'audio' | 'image' | 'other'
  from: 'file' | 'library'
}

export interface WipeOptions {
  tasks: boolean
  history: boolean
  library: boolean
  inbox: boolean
  subscriptions: boolean
  cookies: boolean
  secrets: boolean
  thumbnails: boolean
  logs: boolean
  safebox: boolean
  quit: boolean
}

export interface WipeReport {
  done: string[]
  skipped: string[]
}

export interface TimeWindow {
  /** 1 = 周一 … 7 = 周日；空表示每天 */
  days: number[]
  start: string
  end: string
}

export interface LiveSettings {
  autoRecord: boolean
  quality: string
  checkIntervalS: number
  segmentMinutes: number
  segmentMb: number
  convertMp4: boolean
  mergeSegments: boolean
  normalize: string
  dir: string
  notify: boolean
  schedule: TimeWindow[]
}

export interface LiveStream {
  quality: string
  rank: number
  url: string
  format: 'flv' | 'hls'
}

export interface LiveStatus {
  platform: string
  platformName: string
  roomId: string
  streamer: string
  title: string
  cover: string | null
  avatar: string | null
  live: boolean
  streams: LiveStream[]
}

export interface LiveRoom {
  id: number
  url: string
  platform: string
  platformName: string
  streamer: string
  title: string
  avatar: string | null
  cover: string | null
  settings: LiveSettings
  monitoring: boolean
  createdAt: number
  state: 'offline' | 'live' | 'recording' | 'error' | 'checking'
  lastCheck: number | null
  error: string | null
  recStarted: number | null
  recBytes: number
  recFile: string | null
  recSegments: number
  qualities: string[]
}

export interface Recording {
  id: number
  liveId: number
  streamer: string
  title: string
  startedAt: number
  endedAt: number | null
  files: string[]
  size: number
  status: string
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
  keepLatest: number
  keepDays: number
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
  /** 选中的字幕：soft 内嵌为字幕轨 / burn 烧录进画面；为空时字幕单独保存为文件 */
  subMode?: 'soft' | 'burn' | null
  clip?: Clip | null
  /** 按这些章节另外拆分成多个文件 */
  splitChapters?: Chapter[]
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
  edition: 'full' | 'lite' | null
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
  checkedAt: number | null
  /** 最近一次检查结果：true 有效 / false 已失效 / null 未检查 */
  valid: boolean | null
  /** 能否检查登录状态 */
  checkable: boolean
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
  canInstall: boolean
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

export type InboxSource = 'clipboard' | 'phone' | 'extension' | 'manual' | 'api' | 'cli'
export type InboxStatus = 'pending_pair' | 'rejected' | 'confirm' | 'playlist' | 'resolving' | 'queued' | 'downloading' | 'done' | 'failed' | 'ignored'

/** 收到的链接 */
export interface InboxItem {
  id: number
  source: InboxSource
  device: string
  deviceName: string
  text: string
  url: string
  site: string
  status: InboxStatus
  message: string | null
  errorKind: ErrorKind | null
  title: string | null
  platform: string | null
  mediaId: string | null
  taskIds: number[]
  seenBefore: number
  createdAt: number
  updatedAt: number
}

export interface InboxFilter {
  source: InboxSource | 'all' | null
  status: 'all' | 'unhandled' | 'failed' | 'done' | 'active' | 'ignored' | null
  query: string
  limit: number | null
}

export interface InboxCounts {
  unhandled: number
  total: number
}

export interface InboxSettings {
  recordClipboard: boolean
  keepDays: number
  maxItems: number
}

export interface LogLine {
  /** Unix 毫秒 */
  at: number
  text: string
}

export interface RequestInfo {
  label: string
  protocol: string
  url: string
  headers: [string, string][]
  route: string
}

export interface TaskDetail {
  task: TaskSnapshot
  sourceUrl: string
  log: LogLine[]
  requests: RequestInfo[]
  partFiles: string[]
  post: string[]
}

export interface PlayerCue {
  startMs: number
  endMs: number
  text: string
}

export interface PlayerTrack {
  label: string
  path: string
  cues: PlayerCue[]
}

export interface PlayerSource {
  path: string
  /** 播放地址（本机媒体服务） */
  url: string
  title: string
  kind: 'video' | 'audio'
  ext: string
  tracks: PlayerTrack[]
}
