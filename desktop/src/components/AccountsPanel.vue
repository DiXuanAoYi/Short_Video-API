<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorText } from '../api'
import { useAppStore } from '../stores/app'
import type { AccountSummary } from '../types'
import { formatDateTime } from '../utils/format'

const app = useAppStore()
const accounts = ref<AccountSummary[]>([])
const site = ref('bilibili')
const customDomain = ref('')
const label = ref('')
const loginOpen = ref(false)
const pasteVisible = ref(false)
const pasteText = ref('')
const busy = ref(false)

/** 谷歌系网站会拦截内嵌浏览器登录，只能导入 Cookie。 */
const GOOGLE_SITES = ['youtube.com', 'google.com']

const providers = computed(() => app.info?.providers ?? [])
const targetSite = computed(() => (site.value === 'other' ? normalizeDomain(customDomain.value) : site.value))
const loginBlocked = computed(() => GOOGLE_SITES.includes(targetSite.value))
const now = () => Math.floor(Date.now() / 1000)

function normalizeDomain(v: string) {
  return v
    .trim()
    .toLowerCase()
    .replace(/^https?:\/\//, '')
    .replace(/\/.*$/, '')
    .replace(/^www\./, '')
}

function expiryText(a: AccountSummary) {
  if (!a.expiresAt) return '会话 Cookie'
  const left = a.expiresAt - now()
  if (left <= 0) return '已过期'
  if (left < 3 * 86400) return `${Math.ceil(left / 3600)} 小时后过期`
  return `${formatDateTime(a.expiresAt)} 过期`
}

function expiryClass(a: AccountSummary) {
  if (!a.expiresAt) return ''
  const left = a.expiresAt - now()
  return left <= 0 ? 'bad' : left < 3 * 86400 ? 'warn' : ''
}

async function load() {
  accounts.value = await api.listAccounts()
}
onMounted(load)

async function run(fn: () => Promise<AccountSummary[] | void>, ok?: string) {
  busy.value = true
  try {
    const r = await fn()
    if (Array.isArray(r)) accounts.value = r
    if (ok) ElMessage.success(ok)
  } catch (e) {
    ElMessage.error(errorText(e))
  } finally {
    busy.value = false
  }
}

function requireSite(): string | null {
  if (!targetSite.value) {
    ElMessage.warning('请先选择网站，或填写网站域名（例如 youtube.com）。')
    return null
  }
  return targetSite.value
}

async function openLogin() {
  const s = requireSite()
  if (!s) return
  await run(async () => {
    await api.openLogin(s, site.value === 'other' ? `https://www.${s}/` : undefined)
    loginOpen.value = true
  })
}

async function saveLogin() {
  const s = requireSite()
  if (!s) return
  await run(() => api.saveLoginCookies(s, label.value.trim() || undefined), '登录状态已保存')
  loginOpen.value = false
}

async function importFile() {
  const path = await open({ title: '选择 cookies.txt', filters: [{ name: 'cookies.txt', extensions: ['txt'] }] })
  if (typeof path !== 'string') return
  await run(() => api.importCookiesFile(path, label.value.trim() || undefined), 'Cookie 已导入')
}

async function importPaste() {
  const text = pasteText.value.trim()
  if (!text) return
  const s = text.includes('\t') ? '' : requireSite()
  if (s === null) return
  await run(() => api.importCookiesText(s, text, label.value.trim() || undefined), 'Cookie 已保存')
  pasteVisible.value = false
  pasteText.value = ''
}

async function makeDefault(a: AccountSummary) {
  await run(() => api.setDefaultAccount(a.id), '已设为默认')
}

async function rename(a: AccountSummary) {
  try {
    const { value } = await ElMessageBox.prompt('账号名称', '重命名', { inputValue: a.label, confirmButtonText: '保存', cancelButtonText: '取消' })
    if (value?.trim()) await run(() => api.renameAccount(a.id, value.trim()))
  } catch {
    /* 取消 */
  }
}

async function remove(a: AccountSummary) {
  try {
    await ElMessageBox.confirm(`删除 ${a.siteName} 的账号“${a.label}”？只删除清影里保存的 Cookie，不影响网站上的账号。`, '删除账号', {
      type: 'warning',
      confirmButtonText: '删除',
      cancelButtonText: '取消',
    })
  } catch {
    return
  }
  await run(() => api.deleteAccount(a.id), '已删除')
}
</script>

<template>
  <div class="panel">
    <el-alert v-if="app.info && !app.info.keyInKeyring" type="warning" show-icon :closable="false" title="系统钥匙串不可用，Cookie 的加密密钥保存在本机的应用数据目录中。" />

    <section class="card block">
      <h3>已保存的账号</h3>
      <div v-if="!accounts.length" class="mute empty">还没有保存任何账号。需要登录才能下载的内容，请在下方添加。</div>
      <div v-for="a in accounts" :key="a.id" class="acc">
        <div class="info">
          <div>
            <b>{{ a.siteName }}</b>
            <span class="label">{{ a.label }}</span>
            <span v-if="a.isDefault" class="chip acc-chip">默认</span>
            <span v-if="a.userName" class="mute"> · {{ a.userName }}</span>
          </div>
          <small class="mute">
            {{ a.cookieCount }} 个 Cookie · 更新于 {{ formatDateTime(a.updatedAt) }} ·
            <span :class="expiryClass(a)">{{ expiryText(a) }}</span>
          </small>
        </div>
        <div class="ops">
          <el-button v-if="!a.isDefault" link size="small" type="primary" @click="makeDefault(a)">设为默认</el-button>
          <el-button link size="small" @click="rename(a)">重命名</el-button>
          <el-button link size="small" @click="remove(a)">删除</el-button>
        </div>
      </div>
    </section>

    <section class="card block">
      <h3>添加账号</h3>
      <div class="row">
        <el-select v-model="site" size="small" class="site">
          <el-option v-for="p in providers" :key="p.id" :value="p.id" :label="p.name" />
          <el-option value="youtube.com" label="YouTube" />
          <el-option value="pornhub.com" label="Pornhub" />
          <el-option value="pixiv.net" label="Pixiv" />
          <el-option value="other" label="其他网站…" />
        </el-select>
        <el-input v-if="site === 'other'" v-model="customDomain" size="small" placeholder="网站域名，例如 x.com" class="domain" />
        <el-input v-model="label" size="small" placeholder="账号名称（可选），例如“大会员”" class="label-input" />
      </div>

      <el-alert v-if="loginBlocked" type="info" :closable="false" show-icon>
        <template #title>谷歌会拦截内置登录窗口，YouTube 请导入 cookies.txt</template>
        建议用浏览器的无痕 / 隐私窗口登录 YouTube，用 Cookie 导出扩展导出 cookies.txt 后关闭该窗口，不要再在浏览器里使用这次登录，否则 Cookie 很快会失效。
      </el-alert>

      <div class="methods">
        <div class="method">
          <b>内置登录窗口</b>
          <small class="mute">在弹出的官网窗口里登录，完成后点“保存登录状态”。</small>
          <div class="row">
            <el-button v-if="!loginOpen" size="small" :disabled="loginBlocked" :loading="busy" @click="openLogin">打开登录窗口</el-button>
            <el-button v-else size="small" type="primary" :loading="busy" @click="saveLogin">保存登录状态</el-button>
          </div>
        </div>
        <div class="method">
          <b>导入 cookies.txt</b>
          <small class="mute">Netscape 格式，可用浏览器扩展导出；会按网站自动分组保存。</small>
          <div class="row"><el-button size="small" :loading="busy" @click="importFile">选择文件…</el-button></div>
        </div>
        <div class="method">
          <b>粘贴 Cookie</b>
          <small class="mute">粘贴 cookies.txt 内容，或请求头里的 Cookie 字符串。</small>
          <div class="row"><el-button size="small" @click="pasteVisible = true">粘贴…</el-button></div>
        </div>
      </div>
      <small class="mute">Cookie 加密保存在本机，只会发送给它所属的网站。使用登录账号大量下载有被平台限制的风险。</small>
    </section>

    <el-dialog v-model="pasteVisible" title="粘贴 Cookie" width="560px">
      <p class="mute small">当前网站：{{ targetSite || '未选择' }}。粘贴 cookies.txt 内容时会按文件里的域名自动分组。</p>
      <el-input v-model="pasteText" type="textarea" :rows="8" class="mono" placeholder="SESSDATA=...; bili_jct=...  或  cookies.txt 内容" />
      <template #footer>
        <el-button @click="pasteVisible = false">取消</el-button>
        <el-button type="primary" :loading="busy" @click="importPaste">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<style scoped>
.panel {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.block {
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
h3 {
  margin: 0;
  font-size: 12px;
  color: var(--cc-mute);
  font-weight: 500;
  letter-spacing: 0.08em;
}
.empty {
  padding: 8px 0;
}
.acc {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 12px;
  padding: 8px 0;
  border-top: 1px dashed var(--cc-line);
}
.acc:first-of-type {
  border-top: 0;
}
.info {
  min-width: 0;
  display: flex;
  flex-direction: column;
}
.label {
  margin-left: 8px;
}
.acc-chip {
  margin-left: 6px;
  border-color: var(--cc-acc);
  color: var(--cc-acc);
}
.ops {
  display: flex;
  gap: 4px;
  flex: none;
}
.ops :deep(.el-button) {
  margin-left: 6px;
}
.row {
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}
.row :deep(.el-button) {
  margin-left: 0;
}
.site {
  width: 150px;
}
.domain {
  width: 180px;
}
.label-input {
  width: 220px;
}
.methods {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: 10px;
}
.method {
  border: 1px solid var(--cc-line);
  border-radius: 8px;
  padding: 10px 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.method small {
  font-size: 11.5px;
}
.warn {
  color: #d9a441;
}
.bad {
  color: var(--cc-err);
}
.small {
  font-size: 12px;
  margin: 0 0 8px;
}
</style>
