<template>
  <!-- 不定量进度条：用于无法测得精确百分比的长任务（如单次 IPC 解析 / 读取，无中间进度回调）。
       用 CSS 动画在合成线程滑动——即便主线程被重计算阻塞，进度条仍持续移动（JS 驱动的
       进度在那一刻会卡死）。reduced-motion 下降级为静态半透明满条（动画在 base.css 被冻结）。 -->
  <div class="ind-loading">
    <div v-if="label" class="ind-label mono">{{ label }}</div>
    <div class="ind-track"><div class="ind-fill"></div></div>
  </div>
</template>

<script setup lang="ts">
defineProps<{ label?: string }>();
</script>

<style scoped>
.ind-loading {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  width: min(360px, 70%);
}
.ind-label {
  font-size: 12px;
  line-height: 1.6;
  text-align: center;
  color: var(--text-secondary);
}
.ind-track {
  position: relative;
  width: 100%;
  height: 6px;
  background: var(--surface-tertiary);
  border: 1px solid var(--border-color);
  border-radius: 999px;
  overflow: hidden;
}
.ind-fill {
  position: absolute;
  top: 0;
  bottom: 0;
  left: 0;
  width: 30%;
  border-radius: 999px;
  background: var(--gradient-accent);
  animation: ind-slide 1.1s var(--ease-in-out) infinite;
}
/* 与 ui-progress 的不定进度同一节奏：整条从左外侧扫到右外侧。 */
@keyframes ind-slide {
  0% {
    transform: translateX(-100%);
  }
  100% {
    transform: translateX(350%);
  }
}
/* 动画关闭（data-motion=reduced，含跟随系统「减少动态效果」）：base.css 会把非 .spin
   动画冻结成 0.01ms/1 次（结束态在右侧，条会空）；这里显式覆盖为静态满条 + 降透明度，
   仍清晰表达「处理中」。 */
:root[data-motion='reduced'] .ind-fill {
  left: 0;
  width: 100%;
  opacity: 0.45;
  animation: none;
}
</style>
