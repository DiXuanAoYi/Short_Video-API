<script setup lang="ts">
import { computed } from 'vue'
import { useAppStore } from '../stores/app'
import type { ErrorKind } from '../types'
import { copyDiagnostics } from '../composables/diagnostics'

const props = defineProps<{ message: string; kind?: ErrorKind | null; compact?: boolean }>()
const app = useAppStore()

/** 每类错误的说明和下一步操作。 */
const hint = computed(() => {
  switch (props.kind) {
    case 'need_login':
      return { tip: '这个内容需要登录。添加该网站的 Cookie 后重试。', action: '添加 Cookie', run: () => app.goSettings('accounts') }
    case 'geo_blocked':
      return { tip: '当前网络无法访问这个内容，可能有地区限制。请检查代理设置。', action: '复制诊断信息', run: copyDiagnostics }
    case 'network':
      return { tip: '网络连接失败。请检查网络或代理后重试。', action: null, run: null }
    case 'rate_limited':
      return { tip: '请求太频繁，被网站暂时限制。请过几分钟再试，或登录后重试。', action: '添加 Cookie', run: () => app.goSettings('accounts') }
    case 'encrypted':
      return { tip: '内容已加密（DRM 或付费加密），清影不支持下载这类内容。', action: null, run: null }
    case 'not_found':
      return { tip: '作品可能已删除、设为私密，或链接已失效。', action: null, run: null }
    case 'parser_broken':
      return { tip: '网站可能改版了，解析器需要更新。复制诊断信息反馈给开发者会很有帮助。', action: '复制诊断信息', run: copyDiagnostics }
    case 'unsupported':
      return { tip: '暂不支持这个网站或链接类型。', action: null, run: null }
    case 'need_update':
      return { tip: '需要安装或更新组件后才能继续。', action: '前往设置', run: () => app.goSettings('general') }
    case 'disk':
      return { tip: '磁盘空间不足或没有写入权限。请检查下载目录。', action: '下载设置', run: () => app.goSettings('download') }
    default:
      return null
  }
})
</script>

<template>
  <el-alert :type="kind === 'encrypted' || kind === 'unsupported' ? 'warning' : 'error'" show-icon :closable="false" class="err-alert">
    <template #title>
      <span class="selectable">{{ message }}</span>
    </template>
    <div v-if="hint && !compact" class="hint">
      <span>{{ hint.tip }}</span>
      <el-button v-if="hint.action && hint.run" size="small" @click="hint.run()">{{ hint.action }}</el-button>
    </div>
  </el-alert>
</template>

<style scoped>
.hint {
  display: flex;
  gap: 10px;
  align-items: center;
  flex-wrap: wrap;
  margin-top: 4px;
}
.err-alert :deep(.el-alert__content) {
  min-width: 0;
}
</style>
