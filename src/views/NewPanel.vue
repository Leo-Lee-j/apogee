<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { AppAlert, AppButton, AppTag } from '@/components/ui'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'

/**
 * 新建请求标签里的选项面板。
 *
 * 面板状态归属于当前 Tab：打开新建请求时会先追加一个独立的“新建请求”标签，
 * 因此它不会覆盖原来活跃接口的请求/响应内容。切换到其他标签时，面板自然离开
 * 视口；选择一种请求后，当前标签再转换为普通请求编辑标签。
 */

const tabs = useTabsStore()
const workspace = useWorkspaceStore()
const panel = ref<HTMLElement | null>(null)
const actionError = ref('')
const actionBusy = ref(false)

const targetNode = computed(() => {
  const parentId = tabs.active?.newRequestParentId
  return parentId ? workspace.nodes?.find((node) => node.id === parentId) ?? null : null
})
const targetHint = computed(() =>
  targetNode.value
    ? `创建位置：${targetNode.value.name}`
    : '未指定位置，将保存到当前工作区的默认集合',
)

interface NewOption {
  id: string
  title: string
  desc: string
  icon: keyof typeof ICONS
  kbd?: string
  badge?: string
  reason?: string
  run?: () => void | Promise<void>
}

const ICONS = {
  bolt: ['M9 1.5 3.5 9.5H7.5L7 14.5 12.5 6.5H8.5L9 1.5Z'],
  file: ['M4 1.5h5l3.5 3.5v9.5H4z', 'M9 1.5V5h3.5'],
  clipboard: ['M6 2.5h4v2H6z', 'M6 3.5H4v11h8v-11h-2'],
  import: ['M8 2v7.5', 'M5 6.5 8 9.5l3-3', 'M2.5 11.5v2h11v-2'],
} satisfies Record<string, string[]>

/** 选择快捷请求：把当前独立面板标签转换为空白请求编辑标签。 */
function quickRequest() {
  tabs.resolveNewPanel()
}

/**
 * 选择集合内接口：创建并打开持久化请求，然后移除承载选项面板的临时标签。
 * 这样不会留下一个多余的“新建请求”标签。
 */
async function savedRequest() {
  const panelTab = tabs.active
  if (!panelTab?.newPanel) return
  const panelId = panelTab.id
  const request = await workspace.createRequest(panelTab.newRequestParentId, '新建请求')
  if (!request) {
    throw new Error(workspace.lastError?.message ?? '创建请求失败，请先创建一个集合。')
  }
  tabs.close(panelId)
}

const GROUPS: { title: string; items: NewOption[] }[] = [
  {
    title: '请求',
    items: [
      {
        id: 'quick',
        title: '快捷请求',
        desc: '不进集合，发完即走。适合临时试一个接口',
        icon: 'bolt',
        kbd: 'Ctrl+T',
        run: quickRequest,
      },
      {
        id: 'saved',
        title: '集合内接口',
        desc: '保存到当前创建位置，落盘为单个文件（Git 可见）',
        icon: 'file',
        run: savedRequest,
      },
    ],
  },
  {
    title: '导入',
    items: [
      {
        id: 'curl',
        title: '粘贴 cURL',
        desc: '把一条 curl 命令还原成请求',
        icon: 'clipboard',
        badge: '待请求模型',
        reason: '请求模型尚未覆盖 Headers / Body / Auth 的完整导入，暂不提供半成品导入。',
      },
      {
        id: 'file',
        title: '从其他工具导入',
        desc: 'Postman / OpenAPI / Insomnia / HAR',
        icon: 'import',
        badge: 'v0.2',
        reason: '导入器排在 v0.2，先保证 MVP 请求链路完整。',
      },
    ],
  },
]

function closePanel() {
  const id = tabs.active?.id
  if (id) tabs.close(id)
}

async function pick(option: NewOption) {
  if (!option.run || actionBusy.value) return
  actionError.value = ''
  actionBusy.value = true
  try {
    await option.run()
  } catch (error) {
    actionError.value = error instanceof Error ? error.message : '创建失败，请稍后重试。'
  } finally {
    actionBusy.value = false
  }
}

function cards(): HTMLButtonElement[] {
  return panel.value ? [...panel.value.querySelectorAll<HTMLButtonElement>('.card')] : []
}

function moveFocus(step: number) {
  const list = cards()
  if (list.length === 0) return
  const at = list.indexOf(document.activeElement as HTMLButtonElement)
  const next = at < 0 ? 0 : (at + step + list.length) % list.length
  list[next]?.focus()
}

function onKeydown(e: KeyboardEvent) {
  switch (e.key) {
    case 'Escape':
      e.preventDefault()
      closePanel()
      break
    case 'ArrowRight':
      e.preventDefault()
      moveFocus(1)
      break
    case 'ArrowLeft':
      e.preventDefault()
      moveFocus(-1)
      break
    case 'ArrowDown':
      e.preventDefault()
      moveFocus(2)
      break
    case 'ArrowUp':
      e.preventDefault()
      moveFocus(-2)
      break
    default:
      break
  }
}

onMounted(async () => {
  await nextTick()
  cards()[0]?.focus()
})
</script>

<template>
  <section
    id="new-request-panel"
    ref="panel"
    class="panel"
    role="region"
    aria-labelledby="new-title"
    @keydown="onKeydown"
  >
    <NaiveThemeScope>
      <header class="hd">
        <div>
          <h2 id="new-title">新建请求</h2>
          <p class="sub">这是一个独立标签，选择请求类型后开始调试</p>
        </div>
        <AppButton
          class="x"
          quaternary
          circle
          size="small"
          :disabled="actionBusy"
          aria-label="关闭新建请求标签"
          title="关闭新建请求标签"
          @click="closePanel"
        >
          <template #icon>
            <svg class="ico" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </template>
        </AppButton>
      </header>

      <p class="target" aria-live="polite">{{ targetHint }}</p>

      <AppAlert
        v-if="actionError"
        class="action-error"
        type="error"
        :show-icon="false"
        closable
        @close="actionError = ''"
      >
        {{ actionError }}
      </AppAlert>

      <div class="groups">
        <section v-for="group in GROUPS" :key="group.title">
          <h3 class="gt">{{ group.title }}</h3>
          <div class="grid">
            <button
              v-for="option in group.items"
              :key="option.id"
              class="card"
              :class="{ off: !option.run }"
              type="button"
              :aria-disabled="!option.run || actionBusy"
              :title="option.reason"
              @click="pick(option)"
            >
              <span class="ic">
                <svg class="ico" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
                  <path v-for="(d, i) in ICONS[option.icon]" :key="i" :d="d" />
                </svg>
              </span>
              <span class="tx">
                <span class="top">
                  <span class="t">{{ option.title }}</span>
                  <kbd v-if="option.kbd">{{ option.kbd }}</kbd>
                  <AppTag
                    v-else-if="option.badge"
                    class="badge"
                    size="small"
                    type="warning"
                    :bordered="false"
                  >
                    {{ option.badge }}
                  </AppTag>
                </span>
                <span class="d">{{ option.desc }}</span>
              </span>
            </button>
          </div>
        </section>
      </div>

      <footer class="ft">
        <span><kbd>Ctrl</kbd>+<kbd>T</kbd> 直接创建空白请求标签</span>
        <span class="dim">按 <kbd>Esc</kbd> 关闭此标签</span>
      </footer>
    </NaiveThemeScope>
  </section>
</template>

<style scoped>
.panel {
  width: 100%;
  height: 100%;
  flex: 1 1 0;
  min-height: 0;
  box-sizing: border-box;
  display: flex;
  flex-direction: column;
  background: var(--a-bg-2);
}
.hd {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  padding: 16px 16px 8px;
}
.hd h2 {
  margin: 0;
  font: var(--a-fw-semibold) var(--a-fs-lg) / 1.4 var(--a-font-ui);
  color: var(--a-text);
}
.sub {
  margin: 3px 0 0;
  color: var(--a-text-dim);
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
}
.x {
  flex: 0 0 auto;
  width: 24px;
  height: 24px;
  padding: 0;
  color: var(--a-text-dim);
}
.x:hover { color: var(--a-text); }
.ico {
  width: 14px;
  height: 14px;
  fill: none;
  stroke: currentcolor;
  stroke-width: 1.5;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.target {
  flex: 0 0 auto;
  margin: 0 16px 8px;
  padding: 6px 8px;
  background: var(--a-bg-3);
  border-left: 2px solid var(--a-primary);
  color: var(--a-text-dim);
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
}
.groups {
  flex: 1 1 auto;
  overflow-y: auto;
  min-height: 0;
  padding: 0 16px 4px;
}
.gt {
  margin: 8px 0 6px;
  font: var(--a-fw-medium) var(--a-fs-xs) / 1 var(--a-font-ui);
  color: var(--a-text-faint);
  text-transform: none;
}
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }
.card {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 10px 12px;
  text-align: left;
  background: var(--a-bg-3);
  border: 1px solid var(--a-border);
  border-radius: var(--a-radius-md);
  color: var(--a-text);
  font: inherit;
  cursor: pointer;
}
.card:not(.off):hover { border-color: var(--a-primary); background: var(--a-selected); }
.card.off { cursor: default; background: transparent; border-color: var(--a-border); }
.card.off .top .t { color: var(--a-text-dim); }
.card.off .d { color: var(--a-text-faint); }
.card .ic { flex: 0 0 auto; color: var(--a-primary); margin-top: 1px; }
.card.off .ic { color: var(--a-text-ghost); }
.card .ic .ico { width: 16px; height: 16px; }
.tx { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.top { display: flex; align-items: center; gap: 6px; }
.top .t { font: var(--a-fw-semibold) var(--a-fs-base) / 1.3 var(--a-font-ui); }
.card .d { font: var(--a-fs-sm) / 1.5 var(--a-font-ui); color: var(--a-text-dim); }
.badge {
  flex: 0 0 auto;
  border-radius: var(--a-radius-pill);
  font: var(--a-fw-medium) var(--a-fs-xs) / 16px var(--a-font-ui);
  white-space: nowrap;
}
.action-error { flex: 0 0 auto; margin: 0 16px 4px; }
kbd {
  padding: 0 4px;
  background: var(--a-bg-2);
  border: 1px solid var(--a-border-strong);
  border-radius: var(--a-radius-sm);
  color: var(--a-text-dim);
  font: var(--a-fs-xs) / 15px var(--a-font-mono);
}
.card kbd { background: var(--a-bg-2); }
.ft {
  flex: 0 0 auto;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 16px 12px;
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
  color: var(--a-text-dim);
}
.ft .dim { color: var(--a-text-faint); }
</style>
