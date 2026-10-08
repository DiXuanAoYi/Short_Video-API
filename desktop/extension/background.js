try {
  importScripts('shared.js')
} catch {
  // Firefox 以 background.scripts 加载，shared.js 需在 manifest 中先加载
}

function notify(title, message) {
  api.notifications.create({ type: 'basic', iconUrl: 'icons/128.png', title, message })
}

api.runtime.onInstalled.addListener(() => {
  api.contextMenus.create({ id: 'cc-page', title: '发送此页面到清影', contexts: ['page'] })
  api.contextMenus.create({ id: 'cc-link', title: '发送链接到清影', contexts: ['link'] })
  api.contextMenus.create({ id: 'cc-media', title: '发送视频 / 图片到清影', contexts: ['video', 'image', 'audio'] })
})

api.contextMenus.onClicked.addListener(async (info, tab) => {
  const url = info.menuItemId === 'cc-link' ? info.linkUrl : info.menuItemId === 'cc-media' ? info.srcUrl || info.pageUrl : info.pageUrl || tab?.url
  if (!url) return
  try {
    const r = await sendUrl(url)
    notify('已发送到清影', r.state === 'pending_pair' ? '第一次使用：请在电脑上点“允许”完成配对' : url)
  } catch (e) {
    notify('发送失败', e.message)
  }
})

// 自动同步：勾选的网站登录状态变化时（Cookie 变化后安静 10 秒）同步给清影。默认关闭。
const pending = new Map()
const MIN_INTERVAL = 5 * 60 * 1000

api.cookies.onChanged.addListener(async ({ cookie }) => {
  const c = await getConfig()
  if (!c.autoSync || !c.syncSites.length) return
  const site = registrable(cookie.domain.replace(/^\./, ''))
  if (!c.syncSites.includes(site)) return
  clearTimeout(pending.get(site))
  pending.set(
    site,
    setTimeout(async () => {
      pending.delete(site)
      const now = await getConfig()
      if (Date.now() - (now.lastSync[site] || 0) < MIN_INTERVAL) return
      try {
        await syncCookies(`https://${site}/`, '', true)
      } catch {
        // 清影没有运行或网站已退出登录：下次变化时再试
      }
    }, 10000),
  )
})
