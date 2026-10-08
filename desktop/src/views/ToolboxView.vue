<script setup lang="ts">
import { onMounted, ref, watch } from 'vue'
import { useAppStore } from '../stores/app'
import VideoTools from '../components/toolbox/VideoTools.vue'
import SubtitleTools from '../components/toolbox/SubtitleTools.vue'
import TagTools from '../components/toolbox/TagTools.vue'
import AiTools from '../components/toolbox/AiTools.vue'
import JobList from '../components/toolbox/JobList.vue'

const app = useAppStore()
const tab = ref<'video' | 'subtitle' | 'tags' | 'ai'>('video')
const preset = ref<string[] | undefined>()
const tool = ref<string | undefined>()
const aiMode = ref<'transcribe' | 'translate' | 'summarize' | undefined>()

function consume() {
  const r = app.toolboxRequest
  if (!r) return
  preset.value = r.paths
  tool.value = r.tool
  aiMode.value = r.aiMode
  tab.value = r.tab ?? 'video'
  app.toolboxRequest = null
}
onMounted(consume)
watch(() => app.toolboxRequest, consume)
</script>

<template>
  <div class="page">
    <div class="head">
      <h2>工具箱</h2>
      <el-radio-group v-model="tab" size="small">
        <el-radio-button value="video">视频与音频</el-radio-button>
        <el-radio-button value="subtitle">字幕</el-radio-button>
        <el-radio-button value="tags">音频标签</el-radio-button>
        <el-radio-button value="ai">AI</el-radio-button>
      </el-radio-group>
    </div>
    <p class="mute note">所有处理都在本机完成，会生成新文件并登记到媒体库，不会修改原文件。需要 ffmpeg（在“设置 → 组件”里安装）。</p>
    <VideoTools v-if="tab === 'video'" :preset="preset" :tool="tool" />
    <SubtitleTools v-else-if="tab === 'subtitle'" />
    <TagTools v-else-if="tab === 'tags'" />
    <AiTools v-else :preset="preset" :mode="aiMode" />
    <JobList v-if="tab !== 'subtitle'" class="joblist" />
  </div>
</template>

<style scoped>
.page {
  padding: 20px 24px;
  max-width: 1040px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.head {
  display: flex;
  align-items: center;
  gap: 16px;
}
h2 {
  margin: 0;
  font-size: 16px;
}
.note {
  margin: 0;
  font-size: 12px;
}
.joblist {
  margin-top: 6px;
}
</style>
