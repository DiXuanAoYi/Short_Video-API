// 与清影桌面端通信：桌面端在“设置 → 手机与浏览器扩展”开启后，在本机端口提供接口。
const api = globalThis.browser ?? globalThis.chrome

async function getConfig() {
  const c = await api.storage.local.get(['port', 'token', 'device'])
  if (!c.device) {
    c.device = 'ext-' + Math.random().toString(36).slice(2, 12)
    await api.storage.local.set({ device: c.device })
  }
  return c
}

async function call(path, body) {
  const c = await getConfig()
  if (!c.port || !c.token) throw new Error('请先在扩展设置里填写清影的端口和访问令牌')
  let r
  try {
    r = await fetch(`http://127.0.0.1:${c.port}${path}`, {
      method: body ? 'POST' : 'GET',
      headers: { 'Content-Type': 'application/json', 'X-Token': c.token, 'X-Device': c.device },
      body: body ? JSON.stringify(body) : undefined,
    })
  } catch {
    throw new Error('连接不到清影。请确认清影正在运行，并已开启“手机与浏览器扩展”。')
  }
  const j = await r.json().catch(() => ({}))
  if (!r.ok) throw new Error(j.error || `HTTP ${r.status}`)
  return j
}

async function sendUrl(url) {
  const c = await getConfig()
  return call('/api/send', { text: url, device: c.device, name: browserName() })
}

function browserName() {
  const ua = navigator.userAgent
  if (/Edg\//.test(ua)) return 'Edge 浏览器扩展'
  if (/Firefox\//.test(ua)) return 'Firefox 浏览器扩展'
  return 'Chrome 浏览器扩展'
}

function registrable(host) {
  if (/^[\d.]+$/.test(host) || host.includes(':')) return host
  const parts = host.split('.').filter(Boolean)
  if (parts.length <= 2) return parts.join('.')
  const n = parts.length
  const two = parts[n - 1].length === 2 && ['com', 'net', 'org', 'gov', 'edu', 'co', 'ac'].includes(parts[n - 2])
  return parts.slice(two ? -3 : -2).join('.')
}

async function syncCookies(pageUrl, label) {
  const u = new URL(pageUrl)
  const cookies = await api.cookies.getAll({ domain: registrable(u.hostname) })
  if (!cookies.length) throw new Error('这个网站没有 Cookie，请先在浏览器里登录')
  return call('/api/cookies', { url: pageUrl, label: label || '', cookies: cookies.map((c) => ({ name: c.name, value: c.value, domain: c.domain, path: c.path, expirationDate: c.expirationDate ?? null, secure: c.secure, httpOnly: c.httpOnly, hostOnly: c.hostOnly })) })
}
