<script setup lang="ts">
import { onMounted, onBeforeUnmount, nextTick, ref } from 'vue'
import { AppAlert, AppButton, AppModal, AppTag } from '@/components/ui'
import { useUiStore } from '@/stores/ui'
import { useTabsStore } from '@/stores/tabs'
import { useWorkspaceStore } from '@/stores/workspace'
import NaiveThemeScope from '@/components/NaiveThemeScope.vue'

/**
 * 「新建」面板 —— 标签栏 `+` 的落点（Postman 的 New 弹窗 / Apifox 的新建入口）。
 *
 * # 为什么 `+` 弹面板，而 `Ctrl+T` 不弹
 *
 * 02 文档 §1 第 3 条把「新建请求」定为 1 步操作（`Ctrl+T`）。在快捷键前面插一个
 * 选择面板会直接违反这条。所以两条路分工：
 *   - `Ctrl+T`：老手路径，直接开一个快捷请求，不问
 *   - 点 `+`：探索路径，把「还能新建什么」摊开给用户看
 * Apifox 就是这么分的，Postman 也是（`+` 给标签，New 按钮给弹窗）。
 *
 * # 为什么复用 AppModal 与少量 Naive UI 控件
 *
 * 这个面板复用 Naive UI 的遮罩、焦点陷阱、Esc 关闭和滚动锁定，并使用 AppButton
 * 和 AppTag 处理低频的关闭与状态标记；卡片网格仍然保留自研，因为它是产品专用的
 * 二维键盘布局，不是通用对话框内容。NewModal 本身走动态 import，所以首次点击才
 * 加载这段低频交互，不影响首屏。
 *
 * # 未落地的选项为什么还摆在这里
 *
 * 摆着但明确禁用，比藏起来好：用户想找「导入」时能找到它、并且当场知道
 * 「不是我不会用，是还没做」以及大概什么时候有。这与 02 文档 §5.2
 * 「绝不静默丢弃 / 不假装成功」是同一条原则 —— 灰掉的卡片带版本徽章和原因，
 * 点不动但说得清。
 */

const ui = useUiStore()
const tabs = useTabsStore()
const workspace = useWorkspaceStore()

const dialog = ref<HTMLElement | null>(null)
const actionError = ref('')
const actionBusy = ref(false)

/**
 * 打开面板前的焦点持有者（通常是标签栏的 `+`）。
 * 关掉面板要把焦点还回去，否则焦点掉到 body 上，键盘用户当场失去落点。
 */
const opener = typeof document === 'undefined' ? null : (document.activeElement as HTMLElement | null)
/** 选了某一项 → 焦点归新目标（URL 栏），**不**还给 `+`。 */
let chose = false

interface NewOption {
  id: string
  title: string
  desc: string
  icon: keyof typeof ICONS
  /** 快捷键提示，仅可用项有。 */
  kbd?: string
  /** 未落地项的版本徽章。 */
  badge?: string
  /** 未落地的原因，进 title 属性 —— 灰按钮必须能解释自己。 */
  reason?: string
  /** 可用项可以是同步动作，也可以等待一次持久化操作完成。 */
  run?: () => void | Promise<void>
}

const ICONS = {
  bolt: ['M9 1.5 3.5 9.5H7.5L7 14.5 12.5 6.5H8.5L9 1.5Z'],
  file: ['M4 1.5h5l3.5 3.5v9.5H4z', 'M9 1.5V5h3.5'],
  clipboard: ['M6 2.5h4v2H6z', 'M6 3.5H4v11h8v-11h-2'],
  import: ['M8 2v7.5', 'M5 6.5 8 9.5l3-3', 'M2.5 11.5v2h11v-2'],
} satisfies Record<string, string[]>

function quickRequest() {
  tabs.openQuick()
}

async function savedRequest() {
  const request = await workspace.createRequest(undefined, '新建请求')
  if (!request) {
    throw new Error(workspace.lastError?.message ?? '创建请求失败，请先创建一个集合。')
  }
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
        desc: '保存到默认集合，落盘为单个文件（Git 可见）',
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
        reason:
          '解析本身不难，但标签目前只承载方法与 URL —— Headers / Body / Auth 没有落点。只导一半就是静默丢弃，违反 02 文档 §5.2。等请求模型补齐后接上。',
      },
      {
        id: 'file',
        title: '从其他工具导入',
        desc: 'Postman / OpenAPI / Insomnia / HAR',
        icon: 'import',
        badge: 'v0.2',
        reason:
          '按 PRD §6.3 决策 D7 排在 v0.2：导入器的质量取决于语料库回归的规模，塞进 MVP 会同时拖慢 MVP 和降低成功率。',
      },
    ],
  },
]

function close(force = false) {
  if (actionBusy.value && !force) return
  ui.newOpen = false
}

async function pick(o: NewOption) {
  if (!o.run || actionBusy.value) return
  actionError.value = ''
  actionBusy.value = true
  try {
    await o.run()
    chose = true
    close(true)
  } catch (error) {
    actionError.value = error instanceof Error ? error.message : '创建失败，请稍后重试。'
  } finally {
    actionBusy.value = false
  }
}

/**
 * 全部卡片，按 DOM 顺序。方向键导航与初始聚焦都用它。
 *
 * 包含未落地的那些 —— 它们是 `aria-disabled` 而非 `disabled`，本来就在
 * 焦点序列里，跳过它们反而会让方向键的位置感和眼睛看到的排布不一致。
 */
function cards(): HTMLButtonElement[] {
  const el = dialog.value
  if (!el) return []
  return [...el.querySelectorAll<HTMLButtonElement>('.card')]
}

/**
 * 方向键在卡片网格里移动焦点。两列布局，所以上下是 ±2、左右是 ±1。
 *
 * Tab 本来就能遍历（PRD §10.4 的全键盘可达已满足），方向键是给「这是个网格」
 * 这件事一个符合直觉的操作方式 —— 眼睛看到二维排布，手就会去按方向键。
 */
function moveFocus(step: number) {
  const list = cards()
  if (list.length === 0) return
  const at = list.indexOf(document.activeElement as HTMLButtonElement)
  const next = at < 0 ? 0 : (at + step + list.length) % list.length
  list[next]?.focus()
}

/**
 * 方向键仍由本组件负责二维卡片导航；Tab 键的焦点陷阱交给 AppModal，避免重复实现通用模态行为。
 */
function onKeydown(e: KeyboardEvent) {
  switch (e.key) {
    case 'Escape':
      e.preventDefault()
      close()
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

onBeforeUnmount(() => {
  if (!chose) opener?.focus()
})
</script>

<template>
  <NaiveThemeScope>
    <AppModal
      v-model:show="ui.newOpen"
      :mask-closable="!actionBusy"
      :close-on-esc="!actionBusy"
      :auto-focus="false"
      :trap-focus="true"
    >
    <div
      ref="dialog"
      class="panel"
      role="dialog"
      aria-modal="true"
      aria-labelledby="new-title"
      @keydown="onKeydown"
    >
      <header class="hd">
        <h2 id="new-title">新建</h2>
        <AppButton
          class="x"
          quaternary
          circle
          size="small"
          :disabled="actionBusy"
          aria-label="关闭"
          title="关闭"
          @click="close()"
        >
          <template #icon>
            <svg class="ico" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
              <path d="M4 4l8 8M12 4l-8 8" />
            </svg>
          </template>
        </AppButton>
      </header>

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
        <section v-for="g in GROUPS" :key="g.title">
          <h3 class="gt">{{ g.title }}</h3>
          <div class="grid">
            <!--
              未落地项用 `aria-disabled` 而不是 `disabled`。

              `disabled` 的按钮既不进 Tab 序列、也不触发 hover 事件 —— 那等于
              把「为什么用不了」这条信息只留给鼠标用户。`aria-disabled` 让它
              照样可聚焦、照样能读到徽章和原因，只是按下去没有动作
              （由 pick() 里的守卫拦住）。
            -->
            <button
              v-for="o in g.items"
              :key="o.id"
              class="card"
              :class="{ off: !o.run }"
              type="button"
              :aria-disabled="!o.run || actionBusy"
              :title="o.reason"
              @click="pick(o)"
            >
              <span class="ic">
                <svg class="ico" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
                  <path v-for="(d, i) in ICONS[o.icon]" :key="i" :d="d" />
                </svg>
              </span>
              <span class="tx">
                <span class="top">
                  <span class="t">{{ o.title }}</span>
                  <kbd v-if="o.kbd">{{ o.kbd }}</kbd>
                  <AppTag
                    v-else-if="o.badge"
                    class="badge"
                    size="small"
                    type="warning"
                    :bordered="false"
                  >
                    {{ o.badge }}
                  </AppTag>
                </span>
                <span class="d">{{ o.desc }}</span>
              </span>
            </button>
          </div>
        </section>
      </div>

      <footer class="ft">
        <span><kbd>Ctrl</kbd>+<kbd>T</kbd> 直接新建快捷请求，跳过这一步</span>
        <span class="dim">灰色项带版本徽章，鼠标放上看原因</span>
      </footer>
    </div>
    </AppModal>
  </NaiveThemeScope>
</template>

<style scoped>
/* AppModal 负责遮罩、层级、焦点陷阱和滚动锁定；这里仅保留产品专用的面板布局。 */
.panel {
  width: 560px;
  max-width: calc(100vw - 32px);
  max-height: calc(100vh - 64px);
  display: flex;
  flex-direction: column;
  background: var(--a-bg-2);
  border: 1px solid var(--a-border-strong);
  /* 模态用 10px 圆角（§7.3 的圆角分档） */
  border-radius: var(--a-radius-lg);
  box-shadow: var(--a-shadow-lg);
}

.hd {
  flex: 0 0 auto;
  display: flex; align-items: center; justify-content: space-between;
  padding: 12px 12px 8px 16px;
}
.hd h2 {
  margin: 0;
  font: var(--a-fw-semibold) var(--a-fs-lg) / 1.4 var(--a-font-ui);
  color: var(--a-text);
}
.x {
  flex: 0 0 auto;
  width: 24px; height: 24px; padding: 0;
  color: var(--a-text-dim);
}
.x:hover { color: var(--a-text); }
.ico {
  width: 14px; height: 14px;
  fill: none; stroke: currentcolor; stroke-width: 1.5;
  stroke-linecap: round; stroke-linejoin: round;
}

.groups { flex: 1 1 auto; overflow-y: auto; min-height: 0; padding: 0 16px 4px; }
.gt {
  margin: 8px 0 6px;
  font: var(--a-fw-medium) var(--a-fs-xs) / 1 var(--a-font-ui);
  color: var(--a-text-faint);
  text-transform: none;
}
/* 两列固定，方向键的 ±2 步长依赖这个列数（见 moveFocus） */
.grid { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; }

.card {
  display: flex; align-items: flex-start; gap: 10px;
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
/*
 * 未落地态：逐项压低颜色，**不用 `opacity`**。
 *
 * 整卡降透明度是最省事的写法，但它会连徽章一起压暗 —— 而徽章（「v0.2」）
 * 恰好是这张卡片上最该被读到的东西。父级 opacity 又没法被子元素撤销。
 * 所以改成显式给标题/描述/图标各自换一档更暗的文字色，徽章保持满饱和度。
 *
 * 光标用 default 而不是 not-allowed：后者的红圈在深色界面上很吵，而「不可用」
 * 已经由徽章、灰度、无 hover 反馈和 aria-disabled 讲了四遍。
 */
.card.off { cursor: default; background: transparent; border-color: var(--a-border); }
.card.off .top .t { color: var(--a-text-dim); }
.card.off .d { color: var(--a-text-faint); }
.card .ic { flex: 0 0 auto; color: var(--a-primary); margin-top: 1px; }
.card.off .ic { color: var(--a-text-ghost); }
.card .ic .ico { width: 16px; height: 16px; }
.tx { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.top { display: flex; align-items: center; gap: 6px; }
.top .t { font: var(--a-fw-semibold) var(--a-fs-base) / 1.3 var(--a-font-ui); }
.card .d {
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
  color: var(--a-text-dim);
}
/* AppTag 提供语义色与无边框胶囊；这里仅对齐卡片里的字号和间距。 */
.badge {
  flex: 0 0 auto;
  border-radius: var(--a-radius-pill);
  font: var(--a-fw-medium) var(--a-fs-xs) / 16px var(--a-font-ui);
  white-space: nowrap;
}

.action-error { margin: 0 16px 4px; }

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
  display: flex; align-items: center; justify-content: space-between; gap: 12px;
  padding: 10px 16px 12px;
  font: var(--a-fs-sm) / 1.5 var(--a-font-ui);
  color: var(--a-text-dim);
}
.ft .dim { color: var(--a-text-faint); }
</style>
