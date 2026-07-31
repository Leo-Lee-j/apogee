# Spike S5 结果：首屏 chunk 体积

## 结论

PASS：按 02 文档 §7.5 原方案（naive-ui 进首屏）+ 打包器默认分包，首屏 gzip 107.2 KB < 150KB，预算占用 71%

预算：首屏 gzip < 150.0 KB（03 文档 §10 S5）。
「首屏」= 从 entry 出发**只走静态 import 边**可达的全部 JS + CSS，不含 `dynamicImports`。

## 测量矩阵（2 变体 × 2 分包策略）

| 组合 | naive-ui 归属 | 分包 | 首屏 raw | 首屏 gzip | 首屏 brotli | 预算占用 | naive在首屏 | CM在首屏 | 达标 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **A + 默认分包** | 首屏 | 打包器默认 | 360.8 KB | **107.2 KB** | 92.4 KB | 71% | 是 | 否 | PASS |
| **B + 默认分包** | 懒加载 | 打包器默认 | 131.4 KB | **47.6 KB** | 42.7 KB | 32% | 否 | 否 | PASS |
| **A + 手动分包** | 首屏 | 手动 vendor | 585.0 KB | **169.2 KB** | 142.4 KB | 113% | 是 | 否 | FAIL |
| **B + 手动分包** | 懒加载 | 手动 vendor | 500.9 KB | **149.9 KB** | 128.2 KB | 100% | 是 | 否 | PASS |

变体说明：
- **A** = naive-ui 进首屏（按 02 文档 §7.5：n-config-provider 放根节点）
- **B** = naive-ui 全部懒加载（首屏零 naive-ui）

## 发现

1. **分包策略比 naive-ui 放哪更关键。** 默认分包下首屏 gzip：变体A 107.2 KB、变体B 47.6 KB；换成手动 vendor 分包后变成 169.2 KB / 149.9 KB。手动分包把变体B 的首屏放大了 3.1 倍。

2. 原因：把只被动态 import 用到的依赖强行归入命名 chunk（vendor-naive）后，打包器会把该 chunk 提升为 entry 的**静态**依赖，懒加载在产物层面失效。手动分包下 naive-ui 是否在首屏：A=是、B=是；默认分包下：A=是、B=否。

3. 按 02 文档 §7.5 原样实现（变体A：n-config-provider 放根节点）+ 默认分包，首屏 gzip 107.2 KB，预算占用 71%，在预算内，文档原方案不需要改。

4. naive-ui 进首屏的实际代价（默认分包下 A 与 B 之差）：59.6 KB gzip，占 150KB 预算的 40%。

5. CodeMirror 在所有组合下都不在首屏（A=否、B=否），符合 03 文档 §4.1 的要求。它单独一个 chunk 约 107KB gzip —— 如果当初选了 Monaco，这一块进首屏就直接击穿预算。

## 建议

1. **vite.config 里不要做手动 vendor 分包。** 已改为默认关闭，仅保留 S5_MANUAL_CHUNKS=1 作为可复现对照。这条要写进工程规范：手动分包看起来是优化，实际会破坏动态 import 的懒加载边界。

2. 02 文档 §7.5 第 3 条「n-config-provider 只包一层放根节点」**可以保留**，首屏预算撑得住。但 §7.5 需要补一条：naive-ui 的组件本身仍要按需引入且尽量只出现在懒加载模块里，因为它是首屏里最大的单一可变项（本次占 59.6 KB）。

3. CI 门禁复用本脚本：`node scripts/measure-bundle.mjs`。门禁只看「默认分包」的两个组合，首屏 gzip 超 150KB 即 fail。同时断言 CodeMirror 不在首屏 —— 这条比体积数字更容易被无意破坏。

## 逐组合明细

### A + 默认分包

首屏（静态 import 图可达）

| 文件 | raw | gzip | brotli |
| --- | --- | --- | --- |
| `assets/index-BAiNGhG6.js` | 282.5 KB | 79.0 KB | 66.7 KB |
| `assets/index-qtpygHAl.css` | 14.5 KB | 3.4 KB | 3.0 KB |
| `assets/rolldown-runtime-DK3Fl9T5.js` | 0.2 KB | 0.2 KB | 0.1 KB |
| `assets/runtime-core.esm-bundler-P4Wfz0mC.js` | 63.6 KB | 24.6 KB | 22.5 KB |
| **合计** | **360.8 KB** | **107.2 KB** | **92.4 KB** |

懒加载 chunk（不计入首屏预算，top 6）

| 文件 | raw | gzip |
| --- | --- | --- |
| `assets/SettingsModal-B8qklvNB.js` | 229.0 KB | 64.6 KB |
| `assets/dist-TKs8oRgz.js` | 195.0 KB | 62.1 KB |
| `assets/dist-BQbKu8tb.js` | 63.2 KB | 21.3 KB |
| `assets/dist-dzxfaMLV.js` | 48.1 KB | 15.9 KB |
| `assets/dist-C5hDaWea.js` | 27.9 KB | 9.7 KB |
| `assets/S2Panel-CygIna-Q.js` | 8.0 KB | 3.5 KB |

### B + 默认分包

首屏（静态 import 图可达）

| 文件 | raw | gzip | brotli |
| --- | --- | --- | --- |
| `assets/index-B0ZKR4HW.js` | 53.1 KB | 19.4 KB | 17.1 KB |
| `assets/index-qtpygHAl.css` | 14.5 KB | 3.4 KB | 3.0 KB |
| `assets/rolldown-runtime-DK3Fl9T5.js` | 0.2 KB | 0.2 KB | 0.1 KB |
| `assets/runtime-core.esm-bundler-P4Wfz0mC.js` | 63.6 KB | 24.6 KB | 22.5 KB |
| **合计** | **131.4 KB** | **47.6 KB** | **42.7 KB** |

懒加载 chunk（不计入首屏预算，top 6）

| 文件 | raw | gzip |
| --- | --- | --- |
| `assets/SettingsModal-CuadtJb2.js` | 372.4 KB | 104.4 KB |
| `assets/dist-TKs8oRgz.js` | 195.0 KB | 62.1 KB |
| `assets/dist-BQbKu8tb.js` | 63.2 KB | 21.3 KB |
| `assets/dist-dzxfaMLV.js` | 48.1 KB | 15.9 KB |
| `assets/dist-C5hDaWea.js` | 27.9 KB | 9.7 KB |
| `assets/S2Panel-C7SeAFqM.js` | 8.0 KB | 3.5 KB |

### A + 手动分包

首屏（静态 import 图可达）

| 文件 | raw | gzip | brotli |
| --- | --- | --- | --- |
| `assets/index-AouYxEb5.js` | 20.7 KB | 8.5 KB | 7.3 KB |
| `assets/index-qtpygHAl.css` | 14.5 KB | 3.4 KB | 3.0 KB |
| `assets/rolldown-runtime-DK3Fl9T5.js` | 0.2 KB | 0.2 KB | 0.1 KB |
| `assets/vendor-naive-DDRKGeKD.js` | 507.6 KB | 142.9 KB | 118.9 KB |
| `assets/vendor-misc-ePBFPMIn.js` | 36.3 KB | 11.5 KB | 10.5 KB |
| `assets/vendor-vue-dtEOXpyO.js` | 5.7 KB | 2.7 KB | 2.4 KB |
| **合计** | **585.0 KB** | **169.2 KB** | **142.4 KB** |

懒加载 chunk（不计入首屏预算，top 6）

| 文件 | raw | gzip |
| --- | --- | --- |
| `assets/vendor-codemirror-CdTDFMNJ.js` | 334.0 KB | 107.0 KB |
| `assets/S2Panel-ZkUgHES9.js` | 8.1 KB | 3.6 KB |
| `assets/CodeMirrorView-BxlQ1utC.js` | 3.1 KB | 1.7 KB |
| `assets/Sidebar-D8vUP00S.js` | 2.3 KB | 1.3 KB |
| `assets/SettingsModal-B325GCHV.js` | 2.7 KB | 1.3 KB |
| `assets/JsonTreeView-BJR7LkMq.js` | 2.0 KB | 1.2 KB |

### B + 手动分包

首屏（静态 import 图可达）

| 文件 | raw | gzip | brotli |
| --- | --- | --- | --- |
| `assets/index-C-tIPKPn.js` | 19.0 KB | 7.8 KB | 6.7 KB |
| `assets/index-qtpygHAl.css` | 14.5 KB | 3.4 KB | 3.0 KB |
| `assets/rolldown-runtime-DK3Fl9T5.js` | 0.2 KB | 0.2 KB | 0.1 KB |
| `assets/vendor-naive-ccqEpVCo.js` | 425.1 KB | 124.3 KB | 105.5 KB |
| `assets/vendor-misc-BlPlQk4P.js` | 36.3 KB | 11.5 KB | 10.5 KB |
| `assets/vendor-vue-BPUvsnjp.js` | 5.7 KB | 2.7 KB | 2.5 KB |
| **合计** | **500.9 KB** | **149.9 KB** | **128.2 KB** |

懒加载 chunk（不计入首屏预算，top 6）

| 文件 | raw | gzip |
| --- | --- | --- |
| `assets/vendor-codemirror-CdTDFMNJ.js` | 334.0 KB | 107.0 KB |
| `assets/S2Panel-CfeLJUua.js` | 8.1 KB | 3.6 KB |
| `assets/CodeMirrorView-De_c303-.js` | 3.1 KB | 1.7 KB |
| `assets/Sidebar-DohuW2ND.js` | 2.3 KB | 1.3 KB |
| `assets/SettingsModal-BdX07YB4.js` | 2.7 KB | 1.3 KB |
| `assets/JsonTreeView-Bt-QH5AU.js` | 2.0 KB | 1.2 KB |
