<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { open } from '@tauri-apps/plugin-dialog'
import { api, errorText } from '../api'
import { useAppStore } from '../stores/app'
import type { Settings, UpdateInfo } from '../types'
import { previewFilename } from '../utils/format'
import AccountsPanel from '../components/AccountsPanel.vue'
import { copyDiagnostics } from '../composables/diagnostics'

const app = useAppStore()
const form = ref<Settings>(JSON.parse(JSON.stringify(app.settings!)))
const saving = ref(false)
const update = ref<UpdateInfo | null>(null)
const checking = ref(false)
// 快捷键在输入框失焦后才生效，避免输入到一半就注册
const shortcutDraft = ref(form.value.shortcut)

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

async function call(fn: () => Promise<unknown>) {
  try {
    await fn()
  } catch (e) {
    ElMessage.error(errorText(e))
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

    <el-tabs v-model="app.settingsTab">
      <el-tab-pane label="下载" name="download">
        <div class="groups">
          <section class="group card">
            <h3>保存</h3>
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
            <div class="kv"><span>跳过已下载过的内容</span><el-switch v-model="form.skipExisting" /></div>
            <div class="kv"><span>全部下载完成后发送通知</span><el-switch v-model="form.notifyOnComplete" /></div>
          </section>

          <section class="group card">
            <h3>队列与断点续传</h3>
            <div class="kv"><span>同时下载数</span><el-input-number v-model="form.concurrency" :min="1" :max="8" size="small" /></div>
            <div class="kv">
              <span>启动时自动继续未完成的任务<small class="mute block">关闭时恢复为“已暂停”，手动继续</small></span>
              <el-switch v-model="form.autoResume" />
            </div>
            <div class="kv"><span>网络中断时自动重试</span><el-input-number v-model="form.maxRetries" :min="0" :max="10" size="small" /></div>
            <div class="kv">
              <span>第一次重试前等待（秒）<small class="mute block">之后每次等待时间翻倍</small></span>
              <el-input-number v-model="form.retryDelaySecs" :min="1" :max="60" size="small" />
            </div>
            <div class="kv">
              <span>取消任务时保留已下载部分<small class="mute block">保留后重新加入同一资源可接着下载</small></span>
              <el-switch v-model="form.keepPartOnCancel" />
            </div>
            <small class="mute">关闭程序、断网或下载地址过期后，任务都会从已下载的位置继续；服务器不支持续传时会自动从头下载。</small>
          </section>
        </div>
      </el-tab-pane>

      <el-tab-pane label="解析" name="parse">
        <div class="groups">
          <section class="group card">
            <h3>剪贴板</h3>
            <div class="kv"><span>监听剪贴板</span><el-switch v-model="form.watchClipboard" /></div>
            <div class="kv">
              <span>识别后自动下载<small class="mute block">按默认选项直接加入队列，不弹出确认</small></span>
              <el-switch v-model="form.autoDownload" :disabled="!form.watchClipboard" />
            </div>
            <div class="field">
              <label>全局快捷键（解析剪贴板）</label>
              <el-input v-model="shortcutDraft" size="small" placeholder="CommandOrControl+Shift+D" class="mono" @change="form.shortcut = shortcutDraft.trim()" />
              <small v-if="app.shortcutError" class="err">{{ app.shortcutError }}</small>
              <small v-else class="mute">格式如 CommandOrControl+Shift+D，留空表示不使用。</small>
            </div>
          </section>
          <section class="group card">
            <h3>解析方式</h3>
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
              <small class="mute">填写已部署的旧版 PHP 接口（本仓库的 jxindex.php），只用于抖音、快手；其他平台始终本地解析。留空则不使用。</small>
            </div>
          </section>
        </div>
      </el-tab-pane>

      <el-tab-pane label="账号与 Cookie" name="accounts">
        <AccountsPanel />
      </el-tab-pane>

      <el-tab-pane label="诊断" name="diagnostics">
        <div class="groups">
          <section class="group card">
            <h3>问题反馈</h3>
            <p class="mute small">诊断信息包含版本、系统、设置概要、失败任务和最近日志。Cookie、令牌等敏感内容会自动隐去。</p>
            <div class="inline wrap">
              <el-button size="small" type="primary" @click="copyDiagnostics">复制诊断信息</el-button>
              <el-button size="small" @click="call(api.openLogDir)">打开日志目录</el-button>
            </div>
          </section>
          <section class="group card">
            <h3>解析样本</h3>
            <div class="kv">
              <span>保存解析时的原始响应<small class="mute block">用于排查网站改版导致的解析失败，平时不需要开启</small></span>
              <el-switch v-model="form.recordSamples" />
            </div>
            <p class="mute small">样本会去掉 Cookie、令牌和链接里的签名参数，但页面内容里仍可能有你的昵称等信息，分享前请检查。</p>
            <div class="inline"><el-button size="small" @click="call(api.openSamplesDir)">打开样本目录</el-button></div>
          </section>
        </div>
      </el-tab-pane>

      <el-tab-pane label="通用" name="general">
        <div class="groups">
          <section class="group card">
            <h3>外观与行为</h3>
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
          </section>
          <section class="group card">
            <h3>关于</h3>
            <div class="kv">
              <span>清影 ClearClip v{{ app.info?.version }}</span>
              <el-button size="small" :loading="checking" @click="checkUpdate">检查更新</el-button>
            </div>
            <div v-if="update?.hasUpdate" class="kv">
              <span>新版本 v{{ update.latest }} 可用</span>
              <el-button size="small" type="primary" @click="api.openUrl(update.url)">前往下载</el-button>
            </div>
            <p class="mute small">
              仅用于下载你有权保存的内容。项目地址：<a href="#" @click.prevent="api.openUrl(`https://github.com/${app.info?.repo}`)">github.com/{{ app.info?.repo }}</a>
            </p>
          </section>
        </div>
      </el-tab-pane>
    </el-tabs>
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
  margin-bottom: 4px;
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
.err {
  color: var(--cc-err);
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
a {
  color: var(--cc-acc);
}
</style>
