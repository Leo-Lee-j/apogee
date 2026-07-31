import { defineConfig, type Plugin } from 'vite'
import vue from '@vitejs/plugin-vue'
import Components from 'unplugin-vue-components/vite'
import { NaiveUiResolver } from 'unplugin-vue-components/resolvers'
import { fileURLToPath, URL } from 'node:url'

/**
 * Spike S5 变体开关。
 *
 * 02 文档 §7.5 要求「`n-config-provider` 只包一层，放在应用根节点」。
 * 问题是：这会把 naive-ui 的运行时（含 CSS-in-JS 引擎）钉死在首屏 chunk 里。
 * 首屏 gzip 预算只有 150KB，这笔开销到底付不付得起，得测出来而不是猜。
 *
 *   VITE_S5_VARIANT=a  按 02 文档做法：naive-ui 进首屏
 *   VITE_S5_VARIANT=b  对照方案：naive-ui 全部推到懒加载 chunk，首屏零 naive-ui
 *
 * 用 resolve.alias 切换而不是运行时判断，保证未选中的那份**完全不进产物**
 * （运行时条件 + 动态 import 依赖 tree-shaking，不够可靠）。
 */
const S5_VARIANT = (process.env.VITE_S5_VARIANT ?? 'a').toLowerCase()
const APP_SHELL =
  S5_VARIANT === 'b'
    ? './src/shell/AppShellLean.vue'
    : './src/shell/AppShellNaive.vue'

/**
 * 分包归属 dump 插件。
 *
 * S5 期间踩过一次坑：`manualChunks` 的匹配写错，导致 Vue 运行时被分到
 * naive-ui 的 chunk 里，量出来的「naive-ui 是否在首屏」完全失真。
 * 分包配置是「写完看不出对不对」的那类代码，所以留一个可验证的出口。
 *
 * 用法：S5_DUMP_CHUNKS=1 vite build → dist/chunk-map.json
 */
function dumpChunkMap(): Plugin {
  return {
    name: 'apogee:dump-chunk-map',
    apply: 'build',
    generateBundle(_opts, bundle) {
      if (process.env.S5_DUMP_CHUNKS !== '1') return
      const map: Record<string, { pkgs: Record<string, number>; moduleCount: number }> = {}
      for (const [name, out] of Object.entries(bundle)) {
        if (out.type !== 'chunk') continue
        const pkgs: Record<string, number> = {}
        for (const id of Object.keys(out.modules ?? {})) {
          const p = packageOf(id) ?? '(app)'
          pkgs[p] = (pkgs[p] ?? 0) + 1
        }
        map[name] = { pkgs, moduleCount: Object.keys(out.modules ?? {}).length }
      }
      this.emitFile({
        type: 'asset',
        fileName: 'chunk-map.json',
        source: JSON.stringify(map, null, 2),
      })
    },
  }
}

export default defineConfig({
  plugins: [
    vue(),
    dumpChunkMap(),
    // Naive UI 按需引入（02 文档 §7.5 第 1 条）。
    // 不做全量 import —— 全量会把整个组件库拖进首屏。
    Components({
      dts: 'src/types/components.d.ts',
      resolvers: [NaiveUiResolver()],
      // 只在这些目录里自动注册，避免把首屏组件也扫进去
      dirs: [],
    }),
  ],

  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      '@app-shell': fileURLToPath(new URL(APP_SHELL, import.meta.url)),
    },
  },

  define: {
    __S5_VARIANT__: JSON.stringify(S5_VARIANT),
  },

  // Tauri 相关：固定端口，不自动打开浏览器
  server: {
    port: 5183,
    strictPort: true,
  },

  build: {
    // WebView2 / WKWebView 都是较新的内核，不需要为老浏览器降级，
    // 降级会显著增大产物（决策 D2 只支持 Windows + macOS，这里能吃到红利）
    target: ['edge120', 'safari16'],
    // 关掉 sourcemap，否则 dist 体积统计会被干扰；正式发布另开
    sourcemap: false,
    cssCodeSplit: true,
    reportCompressedSize: true,
    chunkSizeWarningLimit: 150,
    // Spike S5 需要靠 manifest 走静态 import 图，才能区分「首屏 chunk」和「懒加载 chunk」。
    // 只看 dist 目录总大小是错的 —— 懒加载的部分不该算进首屏预算。
    manifest: true,
    rollupOptions: {
      output: {
        // 手动分包：把重依赖固定到独立 chunk，便于 S5 归因。
        // naive-ui 必须单独成 chunk，否则测不出「naive-ui 是否进首屏」。
        //
        // ⚠️ 默认**关闭**手动分包，只有 S5_MANUAL_CHUNKS=1 才启用。
        //
        // Spike S5 实测结论：手动 vendor 分包在这个项目里是净损失。
        // 把只被动态 import 用到的依赖（naive-ui）强行归入一个命名 chunk 后，
        // 打包器会把该 chunk 提升为 entry 的**静态**依赖，懒加载在产物层面直接失效：
        //     手动分包：首屏 gzip 144.6 KB（naive-ui 被拖进首屏）
        //     默认分包：首屏 gzip  42.3 KB（naive-ui 正确留在懒加载 chunk）
        // 差 3.4 倍。保留这段代码是为了留下可复现的对照，不是为了用它。
        manualChunks: process.env.S5_MANUAL_CHUNKS === '1' ? function (id: string) {
          const pkg = packageOf(id)
          if (!pkg) return
          // 顺序重要：先判 Vue 与 CodeMirror，再判 naive-ui。
          // 之前用子串匹配（`id.includes('vueuc')` 之类）踩过坑 ——
          // pnpm 的路径里带 peer 后缀（如 `naive-ui@2.44.1_vue@3.5.40_...`），
          // 子串匹配会把 Vue 运行时误判进 naive 包，导致 S5 量出的
          // 「naive-ui 是否在首屏」完全失真。所以改成按包名精确匹配。
          if (VUE_PKGS.has(pkg)) return 'vendor-vue'
          if (isCodeMirror(pkg)) return 'vendor-codemirror'
          if (NAIVE_PKGS.has(pkg)) return 'vendor-naive'
          return 'vendor-misc'
        } : undefined,
      },
    },
  },
})

/**
 * 从模块 id 里取出包名。
 *
 * 用**最后一个** `node_modules/` 之后的路径段，这样 pnpm 的符号链接/嵌套结构
 * 也能拿到正确的包名（`.../node_modules/a/node_modules/b/...` → `b`）。
 */
function packageOf(id: string): string | null {
  const norm = id.replace(/\\/g, '/')
  const i = norm.lastIndexOf('node_modules/')
  if (i < 0) return null
  const parts = norm.slice(i + 'node_modules/'.length).split('/')
  if (!parts[0]) return null
  return parts[0].startsWith('@') && parts[1] ? `${parts[0]}/${parts[1]}` : parts[0]
}

/** Vue 生态核心。必须优先于 naive-ui 判定。 */
const VUE_PKGS = new Set([
  'vue',
  'pinia',
  '@vue/shared',
  '@vue/reactivity',
  '@vue/runtime-core',
  '@vue/runtime-dom',
  '@vue/compiler-core',
  '@vue/compiler-dom',
  '@vue/compiler-sfc',
  '@vue/devtools-api',
])

/** naive-ui 及其运行时依赖（含 css-render 这套 CSS-in-JS 引擎）。 */
const NAIVE_PKGS = new Set([
  'naive-ui',
  'vueuc',
  'vdirs',
  'vooks',
  'evtd',
  'seemly',
  'treemate',
  'css-render',
  '@css-render/vue3-ssr',
  '@css-render/plugin-bem',
  '@juggle/resize-observer',
  'async-validator',
  'date-fns',
  'date-fns-tz',
  'highlight.js',
])

function isCodeMirror(pkg: string): boolean {
  return (
    pkg.startsWith('@codemirror/') ||
    pkg.startsWith('@lezer/') ||
    pkg === 'codemirror' ||
    pkg === 'style-mod' ||
    pkg === 'w3c-keyname' ||
    pkg === 'crelt'
  )
}
