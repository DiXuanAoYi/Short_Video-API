export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—'
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 100 ? 0 : 1)} ${units[i]}`
}

export function formatSpeed(bps: number): string {
  return `${formatBytes(bps)}/s`
}

export function formatDuration(ms: number | null | undefined): string {
  if (!ms) return ''
  const total = Math.round(ms / 1000)
  const m = Math.floor(total / 60)
  const s = total % 60
  return `${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`
}

export function formatEta(received: number, total: number | null, speed: number): string {
  if (!total || speed <= 0 || received >= total) return ''
  const secs = Math.ceil((total - received) / speed)
  if (secs < 60) return `剩余 ${secs} 秒`
  if (secs < 3600) return `剩余 ${Math.ceil(secs / 60)} 分钟`
  return `剩余 ${(secs / 3600).toFixed(1)} 小时`
}

export function formatDate(unixSecs: number | null | undefined): string {
  if (!unixSecs) return ''
  const d = new Date(unixSecs * 1000)
  const pad = (x: number) => String(x).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

export function formatDateTime(unixSecs: number | null | undefined): string {
  if (!unixSecs) return ''
  const d = new Date(unixSecs * 1000)
  const pad = (x: number) => String(x).padStart(2, '0')
  return `${formatDate(unixSecs)} ${pad(d.getHours())}:${pad(d.getMinutes())}`
}

/** 与 Rust 端 naming.rs 相同规则的文件名预览。 */
export function previewFilename(template: string, sample: { author: string; title: string; id: string; platform: string }): string {
  const d = new Date()
  const date = `${d.getFullYear()}${String(d.getMonth() + 1).padStart(2, '0')}${String(d.getDate()).padStart(2, '0')}`
  const raw = template
    .replaceAll('{author}', sample.author)
    .replaceAll('{title}', sample.title)
    .replaceAll('{date}', date)
    .replaceAll('{id}', sample.id)
    .replaceAll('{platform}', sample.platform)
  const clean = raw.replace(/[\\/:*?"<>|]/g, '_').replace(/\s+/g, ' ').trim()
  return (clean || sample.id) + '.mp4'
}
