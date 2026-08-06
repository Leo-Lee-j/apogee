import type { GlobalThemeOverrides } from 'naive-ui'

/**
 * Naive UI 主题覆写。
 *
 * # 为什么是模块级常量（02 文档 §7.5 第 2 条）
 *
 * naive-ui 是 CSS-in-JS 方案。`themeOverrides` 的**引用**变化就会触发整套样式
 * 重新计算并重新注入。如果把它写在 `computed` 里，每次依赖变化都会重建对象，
 * 引用变了 → 全量样式重算 → 首屏和主题切换都会明显卡顿。
 *
 * 所以：定义为模块级 `const`，永不重建。
 * 需要动态换主题时，切换的是「引用哪个常量」，而不是「改这个常量的内容」。
 *
 * # 与 tokens.css 的关系
 *
 * 数值与 src/styles/tokens.css 手工对齐 —— 不能在运行时读 CSS 变量，那会把
 * 首屏拖进一次同步布局。两边必须一致，否则 naive-ui 组件和自研组件会出现色差。
 *
 * 这条约束由 `node scripts/check-contrast.mjs` 守着：它会解析 tokens.css 和
 * 本文件，比对关键色值，不一致直接 fail。改色值时两处一起改。
 */

/** 与 tokens.css `:root` 的 --a-* 对齐。 */
const commonDark = {
  primaryColor: '#4f80ff',
  primaryColorHover: '#6f98ff',
  primaryColorPressed: '#4576f5',
  primaryColorSuppl: '#4f80ff',

  // 语义色用 Apogee 的基色（而非 pill 前景色）：naive-ui 会拿它们做实色填充，
  // pill 前景那一档太浅，填充后压不住白字。
  successColor: '#34d399',
  warningColor: '#fbbf24',
  errorColor: '#f87171',
  infoColor: '#4f80ff',

  textColorBase: '#f1f5f9',
  textColor1: '#f1f5f9',
  textColor2: '#cbd5e1',
  textColor3: '#94a3b8',

  bodyColor: '#0c0e12',
  cardColor: '#14171f',
  modalColor: '#14171f',
  popoverColor: '#1a1e29',
  inputColor: '#1a1e29',
  tableColor: '#14171f',

  borderColor: '#232733',
  dividerColor: '#232733',

  // --a-radius-md / --a-radius-sm。Apogee 的 9px/6px 各下移一档，
  // 28px 行高配 9px 圆角在密集表格里会显胖。
  borderRadius: '6px',
  borderRadiusSmall: '4px',

  // 固定 px，不用 clamp —— 理由见 tokens.css 文件头。
  fontFamily:
    "'Inter', 'Segoe UI', -apple-system, BlinkMacSystemFont, 'PingFang SC', 'Microsoft YaHei UI', sans-serif",
  fontFamilyMono: "'JetBrains Mono', 'Cascadia Code', 'SF Mono', Consolas, monospace",
  fontSize: '13px',
  fontSizeSmall: '12px',
  lineHeight: '20px',
} as const

/** 与 tokens.css `:root[data-theme='light']` 对齐。 */
const commonLight = {
  ...commonDark,
  primaryColor: '#2563eb',
  primaryColorHover: '#1d4fd8',
  primaryColorPressed: '#1a45c0',
  primaryColorSuppl: '#2563eb',

  successColor: '#059669',
  warningColor: '#d97706',
  errorColor: '#dc2626',
  infoColor: '#2563eb',

  textColorBase: '#0f172a',
  textColor1: '#0f172a',
  textColor2: '#334155',
  textColor3: '#475569',

  // 亮主题里画布是云母白、面板是纯白（层级方向与暗主题相反，见 tokens.css）
  bodyColor: '#f8fafb',
  cardColor: '#ffffff',
  modalColor: '#ffffff',
  popoverColor: '#ffffff',
  inputColor: '#f1f4f7',
  tableColor: '#ffffff',

  borderColor: '#dbe2ea',
  dividerColor: '#dbe2ea',
} as const

/** 组件级只覆写必要的部分。改得越多，样式计算越慢。 */
const components = {
  Input: {
    heightSmall: '24px',
    paddingSmall: '0 6px',
    fontSizeSmall: '12px',
    lineHeight: '20px',
    borderRadius: '4px',
    heightMedium: '28px',
    paddingMedium: '0 8px',
    fontSizeMedium: '13px',
  },
  Button: {
    heightSmall: '24px',
    paddingSmall: '0 8px',
    fontSizeSmall: '12px',
    borderRadiusSmall: '4px',
    heightMedium: '28px',
    paddingMedium: '0 12px',
    fontSizeMedium: '13px',
    borderRadiusMedium: '6px',
    textColorPrimary: '#0c0e12',
    textColorHoverPrimary: '#0c0e12',
    textColorPressedPrimary: '#0c0e12',
    textColorFocusPrimary: '#0c0e12',
    textColorDisabledPrimary: '#0c0e12',
  },
  Card: {
    borderRadius: '6px',
    paddingSmall: '12px',
    fontSizeSmall: '12px',
    titleFontSizeSmall: '13px',
  },
  Form: {
    lineHeight: '20px',
    labelFontSizeLeftSmall: '12px',
    labelFontSizeTopSmall: '12px',
  },
  Alert: {
    fontSize: '12px',
    lineHeight: '20px',
    borderRadius: '6px',
    padding: '8px 10px',
  },
  Tabs: {
    tabFontSizeSmall: '12px',
    tabGapSmallLine: '2px',
    tabPaddingSmallLine: '0 8px',
  },
  Select: {
    peers: {
      InternalSelection: {
        fontSizeSmall: '12px',
        heightSmall: '24px',
        borderRadius: '4px',
        paddingSingle: '0 8px',
        fontSizeMedium: '13px',
        heightMedium: '28px',
      },
    },
  },
} as const

/** 暗色覆写。**不要在渲染函数里重建这个对象。** */
export const darkThemeOverrides: GlobalThemeOverrides = {
  common: commonDark,
  ...components,
}

/** 亮色覆写。 */
export const lightThemeOverrides: GlobalThemeOverrides = {
  common: commonLight,
  ...components,
  Button: {
    ...components.Button,
    textColorPrimary: '#ffffff',
    textColorHoverPrimary: '#ffffff',
    textColorPressedPrimary: '#ffffff',
    textColorFocusPrimary: '#ffffff',
    textColorDisabledPrimary: '#ffffff',
  },
}
