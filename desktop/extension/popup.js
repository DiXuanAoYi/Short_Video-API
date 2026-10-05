const $ = (id) => document.getElementById(id)
const msg = (t, cls = '') => { $('msg').textContent = t; $('msg').className = cls }

function show(settings) {
  $('main').classList.toggle('hidden', settings)
  $('settings').classList.toggle('hidden', !settings)
}

async function currentTab() {
  const [tab] = await api.tabs.query({ active: true, currentWindow: true })
  return tab
}

async function init() {
  const c = await getConfig()
  $('port').value = c.port || ''
  $('token').value = c.token || ''
  const tab = await currentTab()
  $('page').textContent = tab?.title || tab?.url || ''
  show(location.hash === '#settings' || !c.port || !c.token)
}

$('send').onclick = async () => {
  const tab = await currentTab()
  if (!tab?.url?.startsWith('http')) return msg('当前页面不能发送', 'err')
  msg('发送中…')
  try {
    const r = await sendUrl(tab.url)
    msg(r.state === 'pending_pair' ? '第一次使用：请在电脑上点“允许”完成配对' : '已发送，清影会开始解析下载', 'ok')
  } catch (e) {
    msg(e.message, 'err')
  }
}

$('cookies').onclick = async () => {
  const tab = await currentTab()
  if (!tab?.url?.startsWith('http')) return msg('当前页面不能同步', 'err')
  msg('同步中…')
  try {
    const r = await syncCookies(tab.url)
    msg(`已同步 ${r.count} 个 Cookie 到清影（${r.site}）`, 'ok')
  } catch (e) {
    msg(e.message, 'err')
  }
}

$('openSettings').onclick = () => show(true)
$('back').onclick = () => show(false)
$('save').onclick = async () => {
  await api.storage.local.set({ port: Number($('port').value) || null, token: $('token').value.trim() })
  try {
    const r = await call('/api/ping')
    msg(`已连接清影 ${r.version}`, 'ok')
    show(false)
  } catch (e) {
    msg(e.message, 'err')
  }
}

init()
