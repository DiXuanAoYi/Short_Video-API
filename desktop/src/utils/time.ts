/** 解析时间输入：`83`、`83.5`（秒）、`1:23`、`01:02:03`、`1:02:03.5`。无法识别返回 null。 */
export function parseClock(input: string): number | null {
  const s = input.trim()
  if (!s) return null
  const parts = s.split(':')
  if (parts.length > 3 || parts.some((p) => !/^\d+(\.\d+)?$/.test(p.trim()))) return null
  const nums = parts.map((p) => Number(p))
  // 只有最后一段可以有小数；前面的是整数
  if (nums.slice(0, -1).some((n) => !Number.isInteger(n))) return null
  const secs = nums.reduce((acc, n) => acc * 60 + n, 0)
  return Math.round(secs * 1000)
}

/** 毫秒 → `m:ss` 或 `h:mm:ss`（有小数秒时保留一位小数）。 */
export function formatClock(ms: number): string {
  const total = Math.max(0, ms) / 1000
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const sec = total % 60
  const secText = Number.isInteger(sec) ? String(sec).padStart(2, '0') : sec.toFixed(1).padStart(4, '0')
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${secText}` : `${m}:${secText}`
}
