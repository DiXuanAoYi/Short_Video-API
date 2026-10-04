<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorText } from '../api'
import { useAppStore } from '../stores/app'
import type { Settings, UpdateInfo } from '../types'
import { formatDateTime, previewFilename } from '../utils/format'

const app = useAppStore()
const form = ref<Settings>(JSON.parse(JSON.stringify(app.settings!)))
const saving = ref(false)
const update = ref<UpdateInfo | null>(null)
const checking = ref(false)
const cookieEdit = ref<{ platform: string; value: string } | null>(null)
const loginOpen = ref<Record<string, boolean>>({})
// 快捷键在输入框失焦后才生效，避免输入到一半就注册
const shortcutDraft = ref(form.value.shortcut)

const platforms = computed(() => app.info?.providers ?? [])
const namePreview = computed(() =>
  previewFilename(form.value.filenameTemplate, { author: '山野厨房', title: '秋天第一锅板栗焖鸡', id: '7421234567890123456', platform: '抖音' }),
)

// 任一项修改后自动保存
let timer: number | undefined
watch(
  form,
  () => {
    window.clearTimeout(timer)
    timer = window.setTimeout(save, 400)
  },
  { deep: true },
)

async function save() {
  saving.value = true
  try {
    await app.save(form.value)
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    saving.value = false
  }
}

async function pickDir() {
  const dir = await open({ directory: true, defaultPath: form.value.downloadDir || undefined, title: '选择保存位置' })
  if (typeof dir === 'string') form.value.downloadDir = dir
}

function cookieStatus(p: string) {
  if (!form.value.cookies[p]) return '未设置（部分作品可能需要登录）'
  const at = form.value.cookieUpdatedAt[p]
  return at ? `已保存 · ${formatDateTime(at)}` : '已保存'
}

async function openLogin(p: string) {
  try {
    await api.openLogin(p)
    loginOpen.value[p] = true
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

async function saveLogin(p: string) {
  try {
    const s = await api.saveLoginCookies(p)
    app.settings = s
    form.value.cookies = { ...s.cookies }
    form.value.cookieUpdatedAt = { ...s.cookieUpdatedAt }
    loginOpen.value[p] = false
    ElMessage.success('登录状态已保存')
  } catch (e) {
    ElMessage.error(errorText(e))
  }
}

function editCookie(p: string) {
  cookieEdit.value = { platform: p, value: form.value.cookies[p] ?? '' }
}

function applyCookie() {
  if (!cookieEdit.value) return
  const { platform, value } = cookieEdit.value
  form.value.cookies = { ...form.value.cookies, [platform]: value.trim() }
  form.value.cookieUpdatedAt = { ...form.value.cookieUpdatedAt, [platform]: Math.floor(Date.now() / 1000) }
  cookieEdit.value = null
}

function clearCookie(p: string) {
  const c = { ...form.value.cookies }
  delete c[p]
  form.value.cookies = c
}

async function checkUpdate() {
  checking.value = true
  try {
    update.value = await api.checkUpdate()
    if (!update.value.hasUpdate) ElMessage.success(update.value.latest ? '已是最新版本' : '还没有发布过正式版本')
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    checking.value = false
  }
}

function insertVar(v: string) {
  form.value.filenameTemplate += v
}
</script>

<template>
  <div class="page">
    <div class="head">
      <h2>设置</h2>
      <span class="mute saving">{{ saving ? '保存中…' : '修改后自动保存' }}</span>
    </div>

    <div class="groups">
      <section class="group card">
        <h3>下载</h3>
        <div class="field">
          <label>保存位置</label>
          <div class="inline">
            <el-input v-model="form.downloadDir" size="small" class="grow" />
            <el-button size="small" @click="pickDir">选择…</el-button>
          </div>
        </div>
        <div class="kv"><span>按平台分子文件夹</span><el-switch v-model="form.subfolderByPlatform" /></div>
        <div class="field">
          <label>文件命名</label>
          <el-input v-model="form.filenameTemplate" size="small" class="mono" />
          <div class="vars">
            <button v-for="v in ['{author}', '{title}', '{date}', '{id}', '{platform}']" :key="v" type="button" class="var mono" @click="insertVar(v)">{{ v }}</button>
          </div>
          <small class="mute ellipsis">示例：{{ namePreview }}</small>
        </div>
        <div class="kv"><span>同时下载数</span><el-input-number v-model="form.concurrency" :min="1" :max="8" size="small" /></div>
        <div class="kv"><span>跳过已下载过的内容</span><el-switch v-model="form.skipExisting" /></div>
        <div class="kv"><span>全部下载完成后发送通知</span><el-switch v-model="form.notifyOnComplete" /></div>
      </section>

      <section class="group card">
        <h3>解析</h3>
        <div class="kv"><span>监听剪贴板</span><el-switch v-model="form.watchClipboard" /></div>
        <div class="kv">
          <span>识别后自动下载<small class="mute block">按默认选项直接加入队列，不弹出确认</small></span>
          <el-switch v-model="form.autoDownload" :disabled="!form.watchClipboard" />
        </div>
        <div class="field">
          <label>解析模式</label>
          <el-select v-model="form.parseMode" size="small">
            <el-option value="local_then_remote" label="本地解析，失败时用远程 API" />
            <el-option value="local" label="只用本地解析" />
            <el-option value="remote" label="只用远程 API" />
          </el-select>
        </div>
        <div class="field">
          <label>远程 API 地址</label>
          <el-input v-model="form.remoteEndpoint" size="small" placeholder="https://your-host/jxindex.php" class="mono" />
          <small class="mute">填写已部署的旧版 PHP 接口（本仓库的 jxindex.php）。留空则不使用。</small>
        </div>
        <div class="field">
          <label>全局快捷键（解析剪贴板）</label>
          <el-input v-model="shortcutDraft" size="small" @change="form.shortcut = shortcutDraft.trim()" placeholder="CommandOrControl+Shift+D" class="mono" />
          <small class="mute">格式如 CommandOrControl+Shift+D，留空表示不使用。</small>
        </div>
      </section>

      <section class="group card">
        <h3>账号与 Cookie</h3>
        <p class="mute small">部分作品需要登录后才能解析。点击“打开登录窗口”，在官网完成登录后回到这里点击“保存登录状态”。Cookie 只保存在本机。</p>
        <div v-for="p in platforms" :key="p.id" class="cookie">
          <div class="kv">
            <span>{{ p.name }}<small class="mute block">{{ cookieStatus(p.id) }}</small></span>
          </div>
          <div class="inline wrap">
            <el-button v-if="!loginOpen[p.id]" size="small" @click="openLogin(p.id)">打开登录窗口</el-button>
            <el-button v-else size="small" type="primary" @click="saveLogin(p.id)">保存登录状态</el-button>
            <el-button size="small" @click="editCookie(p.id)">手动填写</el-button>
            <el-button v-if="form.cookies[p.id]" size="small" link @click="clearCookie(p.id)">清除</el-button>
          </div>
        </div>
      </section>

      <section class="group card">
        <h3>通用</h3>
        <div class="kv">
          <span>外观</span>
          <el-radio-group v-model="form.theme" size="small">
            <el-radio-button value="system">跟随系统</el-radio-button>
            <el-radio-button value="dark">深色</el-radio-button>
            <el-radio-button value="light">浅色</el-radio-button>
          </el-radio-group>
        </div>
        <div class="kv"><span>关闭窗口时最小化到托盘</span><el-switch v-model="form.closeToTray" /></div>
        <div class="kv"><span>启动时检查更新</span><el-switch v-model="form.checkUpdate" /></div>
        <h3 class="about">关于</h3>
        <div class="kv">
          <span>清影 ClearClip v{{ app.info?.version }}</span>
          <el-button size="small" :loading="checking" @click="checkUpdate">检查更新</el-button>
        </div>
        <div v-if="update?.hasUpdate" class="kv">
          <span>新版本 v{{ update.latest }} 可用</span>
          <el-button size="small" type="primary" @click="api.openUrl(update.url)">前往下载</el-button>
        </div>
        <p class="mute small">仅用于下载你有权保存的内容。项目地址：<a href="#" @click.prevent="api.openUrl(`https://github.com/${app.info?.repo}`)">github.com/{{ app.info?.repo }}</a></p>
      </section>
    </div>

    <el-dialog :model-value="!!cookieEdit" title="手动填写 Cookie" width="520px" @close="cookieEdit = null">
      <template v-if="cookieEdit">
        <p class="mute small">在浏览器开发者工具中复制请求头里的 Cookie 值，粘贴到下面。</p>
        <el-input v-model="cookieEdit.value" type="textarea" :rows="6" class="mono" placeholder="name1=value1; name2=value2" />
      </template>
      <template #footer>
        <el-button @click="cookieEdit = null">取消</el-button>
        <el-button type="primary" @click="applyCookie">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
.page {
  padding: 20px 24px;
  max-width: 1040px;
}
.head {
  display: flex;
  align-items: baseline;
  gap: 12px;
  margin-bottom: 12px;
}
h2 {
  margin: 0;
  font-size: 16px;
}
.saving {
  font-size: 11.5px;
}
.groups {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(340px, 1fr));
  gap: 12px;
  align-items: start;
}
.group {
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-width: 0;
}
h3 {
  margin: 0;
  font-size: 12px;
  color: var(--cc-mute);
  font-weight: 500;
  letter-spacing: 0.08em;
}
h3.about {
  margin-top: 6px;
  padding-top: 12px;
  border-top: 1px solid var(--cc-line);
}
.kv {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
}
.kv > span {
  min-width: 0;
}
.block {
  display: block;
  font-size: 11px;
}
.field {
  display: flex;
  flex-direction: column;
  gap: 5px;
}
.field label {
  font-size: 12.5px;
}
.field small {
  font-size: 11px;
}
.inline {
  display: flex;
  gap: 8px;
  align-items: center;
}
.inline :deep(.el-button) {
  margin-left: 0;
}
.wrap {
  flex-wrap: wrap;
}
.grow {
  flex: 1;
  min-width: 0;
}
.vars {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}
.var {
  all: unset;
  cursor: pointer;
  font-size: 11px;
  padding: 0 7px;
  line-height: 20px;
  border: 1px solid var(--cc-line);
  border-radius: 4px;
  color: var(--cc-mute);
}
.var:hover {
  border-color: var(--cc-acc);
  color: var(--cc-acc);
}
.small {
  font-size: 12px;
  margin: 0;
  line-height: 1.6;
}
.cookie {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding-top: 8px;
  border-top: 1px dashed var(--cc-line);
}
a {
  color: var(--cc-acc);
}
</style>
