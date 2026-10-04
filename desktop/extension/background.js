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
