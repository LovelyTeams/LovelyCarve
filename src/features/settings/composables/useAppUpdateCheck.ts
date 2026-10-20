import { computed, onMounted, onScopeDispose, ref, watch } from 'vue';
import { useAppVersion } from '../../../composables/useAppVersion';
import { tr } from '../../../composables/useLocale';
import { notifyInfo } from '../../../composables/useToast';
import { findLatestVersionTag } from '../model/versionTags';
import { fetchVersionTags, TagCheckError, TAGS_PAGE_URL } from '../services/githubTags';

const AUTOMATIC_CHECK_KEY = 'lovelycarve-auto-update-check';

/** Owned by the always-mounted settings shell; closing its dialog keeps the check alive. */
export function useAppUpdateCheck() {
  const { version, buildDate } = useAppVersion();
  const automatic = ref(false);
  try {
    automatic.value = localStorage.getItem(AUTOMATIC_CHECK_KEY) === 'true';
  } catch {
    // A blocked preference store keeps network access opt-in for this session.
  }
  const status = ref<'idle' | 'checking' | 'available' | 'current' | 'error'>('idle');
  const latestTag = ref<string | null>(null);
  const failure = ref<'rate-limit' | 'response' | 'too-many-tags' | 'network' | 'timeout' | null>(null);
  const tagUrl = computed(() => latestTag.value
    ? `https://github.com/LovelyTeams/LovelyCarve/tree/${encodeURIComponent(latestTag.value)}`
    : TAGS_PAGE_URL);
  const errorMessage = computed(() => {
    switch (failure.value) {
      case 'rate-limit': return tr('GitHub 请求受限，请稍后重试。');
      case 'response': return tr('GitHub 返回了无效响应，请稍后重试。');
      case 'too-many-tags': return tr('标签过多，无法完成检查，请前往 GitHub 查看。');
      case 'timeout': return tr('检查更新超时，请重试。');
      case 'network': return tr('无法连接 GitHub，请检查网络后重试。');
      case null: return '';
    }
  });
  let controller: AbortController | null = null;
  let generation = 0;

  function cancelCheck() {
    generation++;
    controller?.abort();
    controller = null;
    if (status.value === 'checking') status.value = 'idle';
  }

  async function check(announce: boolean) {
    if (status.value === 'checking') return;
    const request = ++generation;
    const abort = new AbortController();
    controller = abort;
    status.value = 'checking';
    failure.value = null;
    latestTag.value = null;
    let timedOut = false;
    const timeout = window.setTimeout(() => { timedOut = true; abort.abort(); }, 15_000);
    try {
      const tags = await fetchVersionTags(abort.signal);
      if (request !== generation || abort.signal.aborted) return;
      latestTag.value = findLatestVersionTag(tags, version.value);
      status.value = latestTag.value ? 'available' : 'current';
      if (announce && latestTag.value) {
        notifyInfo(tr('发现新版本 {0}', { 0: latestTag.value }), tr('前往「设置中心 → 关于」查看。'));
      }
    } catch (cause) {
      if (request !== generation) return;
      failure.value = timedOut ? 'timeout' : cause instanceof TagCheckError ? cause.reason : 'network';
      status.value = 'error';
    } finally {
      window.clearTimeout(timeout);
      if (request === generation) controller = null;
    }
  }

  watch(automatic, value => {
    try {
      localStorage.setItem(AUTOMATIC_CHECK_KEY, JSON.stringify(value));
    } catch {
      // The in-memory choice remains usable when persistence is unavailable.
    }
  }, { flush: 'sync' });
  onMounted(() => {
    watch([automatic, version], ([enabled]) => {
      cancelCheck();
      latestTag.value = null;
      failure.value = null;
      status.value = 'idle';
      if (enabled) void check(true);
    }, { immediate: true, flush: 'sync' });
  });
  onScopeDispose(cancelCheck);

  return { version, buildDate, automatic, status, latestTag, tagUrl, errorMessage, checkNow: () => check(false) };
}

export type AppUpdateCheck = ReturnType<typeof useAppUpdateCheck>;
