import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import type { PostmanImportProgressEvent } from '@/ipc'

export type PostmanImportTarget = 'file' | 'environment'

/**
 * 导入任务不属于某个视图实例：用户切走新建标签再回来时，后台任务仍在继续。
 * 状态放在 store 中，才能同时恢复进度文字和禁用状态。
 */
export const useImportStore = defineStore('import', () => {
  const postmanImport = ref<{
    id: string
    target: PostmanImportTarget
    progress: PostmanImportProgressEvent
  } | null>(null)

  const postmanBusy = computed(() => postmanImport.value !== null)
  const postmanProgress = computed(() => postmanImport.value?.progress ?? null)
  const postmanTarget = computed(() => postmanImport.value?.target ?? null)
  const postmanPercent = computed(() => {
    const progress = postmanProgress.value
    return progress ? Math.round(progress.current / progress.total * 100) : 0
  })

  function beginPostman(id: string, target: PostmanImportTarget) {
    postmanImport.value = {
      id,
      target,
      progress: { importId: id, current: 0, total: 100, phase: '准备导入' },
    }
  }

  function updatePostman(progress: PostmanImportProgressEvent) {
    if (postmanImport.value?.id === progress.importId) {
      postmanImport.value = { ...postmanImport.value, progress }
    }
  }

  function finishPostman(id: string) {
    if (postmanImport.value?.id === id) postmanImport.value = null
  }

  return {
    postmanBusy,
    postmanProgress,
    postmanTarget,
    postmanPercent,
    beginPostman,
    updatePostman,
    finishPostman,
  }
})
