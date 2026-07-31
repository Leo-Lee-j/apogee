# Apogee 配色系统设计规范

## 一、核心设计理念

**关键词：** 清冷、精准、巅峰、优雅、深空美学

- **暗主题 (Dark):** 冷钛黑背景 + 冰蓝光效，象征无垠深空与高冷科技感
- **亮主题 (Light):** 云母白画布 + 靛蓝精准点缀，保持清爽与专业感

---

## 二、暗主题 (Dark Theme) 配色

### 1. 背景层级 (Surface Hierarchy)

采用 **5-7 层冷调深色阶梯**，每层带微弱蓝色底色，绝不使用纯黑或中性灰。

```css
--canvas: #0C0E12;           /* 画布 - 极深冷钛黑 (3% 亮度) */
--surface: #14171F;          /* 主面板 - 深邃石墨灰 */
--surface-2: #1A1E29;        /* 次级面板 - 微亮一阶 */
--border: #232733;           /* 边框 - 极细冷调分割线 */
```

**设计原则：**
- 最深层 3-6% 亮度，向目标色相倾斜（蓝/青不是纯灰）
- 每层递增 3-5% 亮度，保持微弱的悬浮感
- 边框用半透明叠加，不用实色

---

### 2. 核心强调色 (Accent Colors)

**主色 - 冰蓝巅峰 (Apogee Blue):**
```css
--accent: #4F80FF;           /* 主按钮、选中态 */
--accent-2: #38BDF8;         /* 悬停、次级高亮 */
```
- 用于：Launch 按钮、选中项背景渐变、URL 聚焦环、method 标签 (GET)
- 特性：高饱和、冷色调、穿透力强

**成功 - 极光青 (Aurora Green):**
```css
--success: #34D399;          /* 翡翠绿 - 取代俗气纯绿 */
```
- 用于：200 OK 状态、连接指示灯
- 避免使用鲜艳的 `#00FF00` 类荧光绿

**警告 - 星焰橙红 (Alert Coral):**
```css
--danger: #F87171;           /* 温和低饱和冷红 */
```
- 用于：错误状态、DELETE 方法标签
- 避免刺眼的纯红 `#FF0000`

**辅助 - 紫罗兰 (Violet):**
```css
--violet: #C084FC;           /* POST 方法标签 */
```

---

### 3. 文字层级 (Typography Hierarchy)

```css
--text: #F1F5F9;             /* 主文字 - 冷白 */
--text-soft: #CBD5E1;        /* 次要文字 - 柔和冷白 */
--muted: #94A3B8;            /* 参数名、标签 - 低对比冷灰 */
--dim: #64748B;              /* 占位符、禁用态 */
```

**对比度原则：**
- 主文字与背景对比 ≥ 12:1
- 次要文字对比 ≥ 7:1
- 弱化文字对比 ≥ 4.5:1

---

### 4. 功能性半透明 (Functional Overlays)

```css
/* 毛玻璃面板 */
--topbar-bg: rgba(20,23,31,.55);
--sidebar-bg: rgba(20,23,31,.34);

/* 输入框渐变 */
--input-bg: linear-gradient(180deg, rgba(24,28,37,.9), rgba(18,21,29,.9));

/* 悬停态 */
--item-hover: rgba(255,255,255,.032);  /* 极微弱白光 */
--realm-hover: rgba(255,255,255,.04);

/* 光晕 */
--glow-1: rgba(79,128,255,.10);        /* 冰蓝光晕 - 用于悬停 */
--glow-2: rgba(56,189,248,.055);       /* 次级光晕 - 用于背景辐射 */
```

---

### 5. 特殊视觉效果

**星空粒子色:**
```css
--star-color: rgba(196,222,255,1);     /* 淡冰蓝 - 星点基础色 */
```

**Launch 按钮渐变 (核心视觉焦点):**
```css
background: linear-gradient(135deg, #4F80FF 0%, #38BDF8 100%);
box-shadow: 0 0 0 1px rgba(56,189,248,.32), 
            0 4px 26px -6px rgba(56,189,248,.5);  /* 悬停时 */
```

**状态徽章 (Status Pills):**
```css
/* 200 OK - 翡翠绿 */
background: rgba(52,211,153,.09);
border: 1px solid rgba(52,211,153,.3);
color: #8CF5C8;
```

---

## 三、亮主题 (Light Theme) 配色

### 1. 背景层级 (Surface Hierarchy)

```css
--canvas: #F8FAFB;           /* 画布 - 云母白 (98% 亮度) */
--surface: #FFFFFF;          /* 主面板 - 纯白 */
--surface-2: #F3F5F7;        /* 次级面板 - 微灰一阶 */
--border: #DBE2EA;           /* 边框 - 柔和冷灰蓝 */
```

**设计原则：**
- 避免纯白大面积刺眼，用 98% 亮度的微冷白
- 面板用纯白营造"悬浮卡片"感
- 边框颜色饱和度 <15%，保持优雅克制

---

### 2. 核心强调色 (Accent Colors)

**主色 - 靛蓝精准 (Precision Indigo):**
```css
--accent: #2563EB;           /* 更深的蓝 - 保持可读性 */
--accent-2: #0EA5E9;         /* 天蓝 - 次级高亮 */
```
- 必须比暗主题深 2-3 个色阶，确保白底可读

**成功 - 翠绿 (Emerald):**
```css
--success: #059669;          /* 比暗主题深的翡翠绿 */
```

**警告 - 赤红 (Crimson):**
```css
--danger: #DC2626;           /* 更饱和的红，白底需要更强对比 */
```

**辅助 - 紫罗兰 (Violet):**
```css
--violet: #9333EA;           /* 比暗主题深的紫 */
```

---

### 3. 文字层级 (Typography Hierarchy)

```css
--text: #0F172A;             /* 主文字 - 深蓝黑 (slate-900) */
--text-soft: #334155;        /* 次要文字 - slate-700 */
--muted: #475569;            /* 参数名 - slate-600 */
--dim: #94A3B8;              /* 占位符 - slate-400 */
```

**反转原则：**
- 亮主题文字必须更深，保持同等对比度
- 避免纯黑 `#000000`，用深蓝黑保持"冷静"感

---

### 4. 功能性半透明 (Functional Overlays)

```css
/* 毛玻璃面板 */
--topbar-bg: rgba(255,255,255,.78);
--sidebar-bg: rgba(248,250,251,.86);

/* 输入框 */
--input-bg: linear-gradient(180deg, #FAFBFC, #F4F6F8);

/* 悬停态 */
--item-hover: rgba(37,99,235,.042);    /* 靛蓝光晕 */
--realm-hover: rgba(37,99,235,.06);

/* 光晕 */
--glow-1: rgba(37,99,235,.055);        /* 靛蓝光晕 */
--glow-2: rgba(14,165,233,.038);       /* 天蓝光晕 */
```

---

### 5. 特殊视觉效果

**星空粒子色 (亮主题弱化):**
```css
--star-color: rgba(148,163,184,1);     /* 温和灰蓝 - 不抢眼 */
```

**Launch 按钮渐变 (亮主题):**
```css
background: linear-gradient(135deg, #2563EB 0%, #0EA5E9 100%);
box-shadow: 0 0 0 1px rgba(14,165,233,.28), 
            0 3px 20px -5px rgba(14,165,233,.42);  /* 悬停时 */
```

**状态徽章 (Status Pills):**
```css
/* 200 OK - 翠绿 */
background: rgba(5,150,105,.08);
border: 1px solid rgba(5,150,105,.28);
color: #047857;
```

---

## 四、方法标签配色 (HTTP Method Tags)

### 暗主题

```css
GET:    background: rgba(56,189,248,.12);  color: #7DD3FC;   /* 冰蓝 */
POST:   background: rgba(192,132,252,.12); color: #E9D5FF;   /* 紫罗兰 */
PATCH:  background: rgba(52,211,153,.12);  color: #6EE7B7;   /* 极光青 */
PUT:    background: rgba(251,191,36,.12);  color: #FCD34D;   /* 琥珀金 */
DELETE: background: rgba(248,113,113,.12); color: #FCA5A5;   /* 星焰橙红 */
```

### 亮主题

```css
GET:    background: rgba(14,165,233,.09);  color: #0369A1;   /* 深天蓝 */
POST:   background: rgba(147,51,234,.09);  color: #7E22CE;   /* 深紫 */
PATCH:  background: rgba(5,150,105,.09);   color: #047857;   /* 深翠绿 */
PUT:    background: rgba(217,119,6,.09);   color: #B45309;   /* 深琥珀 */
DELETE: background: rgba(220,38,38,.09);   color: #B91C1C;   /* 深赤红 */
```

---

## 五、阴影系统 (Shadow System)

### 暗主题

```css
--shadow-sm: 0 1px 3px rgba(0,0,0,.5);
--shadow-md: 0 4px 12px rgba(0,0,0,.65);
--shadow-lg: 0 30px 90px -20px rgba(0,0,0,.85);  /* 命令面板、modal */
```

### 亮主题

```css
--shadow-sm: 0 1px 3px rgba(15,23,42,.08);
--shadow-md: 0 4px 12px rgba(15,23,42,.12);
--shadow-lg: 0 24px 70px -22px rgba(15,23,42,.24);  /* 柔和深色阴影 */
```

---

## 六、渐变与光效 (Gradients & Glows)

### Logo 轨道渐变

```svg
<linearGradient id="g1" x1="2" y1="18" x2="22" y2="5">
  <stop stop-color="#4F80FF" stop-opacity=".25"/>
  <stop offset="1" stop-color="#38BDF8"/>
</linearGradient>
```

### 聚焦环 (Focus Ring)

```css
/* 暗主题 */
box-shadow: 0 0 0 3px rgba(79,128,255,.07);

/* 亮主题 */
box-shadow: 0 0 0 3px rgba(37,99,235,.08);
```

### Launch 按钮粒子爆炸

```css
/* 粒子初始状态 */
background: radial-gradient(circle, rgba(56,189,248,.9) 0%, transparent 70%);
box-shadow: 0 0 8px rgba(56,189,248,.5);

/* 动画：从中心径向散开 14 颗粒子，0.6s 淡出 */
```

---

## 七、字体配置 (Typography)

### 字体族

```css
--sans: 'Inter', -apple-system, BlinkMacSystemFont, system-ui, sans-serif;
--mono: 'JetBrains Mono', 'Fira Code', 'Cascadia Code', Consolas, monospace;
```

### 字重

```css
--fw-normal: 400;
--fw-medium: 500;
--fw-semibold: 600;
```

### 字号比例 (Fluid Scale)

```css
--text-xs:   clamp(10px, 0.625rem, 11px);
--text-sm:   clamp(11.5px, 0.75rem, 13px);
--text-base: clamp(13px, 0.875rem, 14px);
--text-lg:   clamp(15px, 1rem, 17px);
--text-xl:   clamp(18px, 1.25rem, 22px);
```

---

## 八、圆角系统 (Border Radius)

```css
--radius-sm: 6px;    /* 小元素：checkbox, tag */
--radius-md: 9px;    /* 中等：input, card */
--radius-lg: 14px;   /* 大面板：modal, command palette */
--radius-full: 999px; /* 胶囊：button, pill badge */
```

---

## 九、间距系统 (Spacing Scale)

```css
--space-1: 4px;
--space-2: 8px;
--space-3: 12px;
--space-4: 16px;
--space-5: 20px;
--space-6: 24px;
--space-8: 32px;
--space-10: 40px;
```

---

## 十、动效时序 (Transitions)

```css
--duration-fast: 0.18s;     /* 悬停、点击反馈 */
--duration-base: 0.3s;      /* 主题切换、面板展开 */
--duration-slow: 0.6s;      /* 粒子爆炸、loading beam */

--ease-out: cubic-bezier(0.4, 0, 0.2, 1);  /* 标准缓出 */
--ease-spring: cubic-bezier(0.34, 1.56, 0.64, 1);  /* 弹性 */
```

---

## 十一、应用规则 (Application Rules)

### 1. 色彩使用优先级

1. **主强调色 (Accent):** 只用于核心交互 (Launch 按钮、选中态、method GET)
2. **成功/错误色:** 只用于状态反馈，不用于装饰
3. **中性色:** 80% 界面用中性色阶梯，强调色只占 5-10%

### 2. 对比度要求

- **关键按钮:** 背景与前景对比 ≥ 4.5:1
- **正文:** 文字与背景对比 ≥ 7:1
- **装饰元素:** 对比 ≥ 3:1

### 3. 避免的设计

❌ **绝对禁止:**
- 纯黑 `#000000` 背景（用冷钛黑 `#0C0E12`）
- 纯白 `#FFFFFF` 大面积画布（用云母白 `#F8FAFB`）
- 中性灰（必须带色相倾向，蓝/青/紫）
- 荧光绿 `#00FF00`（用翡翠绿 `#34D399`）
- 紫色渐变（Apogee 不用紫色渐变，只用冰蓝渐变）

❌ **避免:**
- 平面白底（亮主题也要微层级）
- 过多强调色（一屏不超过 3 处高饱和色）
- 硬边框（用半透明叠加或微阴影代替）

---

## 十二、快速参考 CSS 变量速查

```css
/* ===== DARK THEME ===== */
:root, [data-theme="dark"] {
  /* Surfaces */
  --canvas: #0C0E12;
  --surface: #14171F;
  --border: #232733;
  
  /* Accents */
  --accent: #4F80FF;
  --accent-2: #38BDF8;
  --success: #34D399;
  --danger: #F87171;
  --violet: #C084FC;
  
  /* Text */
  --text: #F1F5F9;
  --muted: #94A3B8;
  --dim: #64748B;
  
  /* Effects */
  --glow-1: rgba(79,128,255,.10);
  --shadow-lg: 0 30px 90px -20px rgba(0,0,0,.85);
}

/* ===== LIGHT THEME ===== */
[data-theme="light"] {
  /* Surfaces */
  --canvas: #F8FAFB;
  --surface: #FFFFFF;
  --border: #DBE2EA;
  
  /* Accents */
  --accent: #2563EB;
  --accent-2: #0EA5E9;
  --success: #059669;
  --danger: #DC2626;
  --violet: #9333EA;
  
  /* Text */
  --text: #0F172A;
  --muted: #475569;
  --dim: #94A3B8;
  
  /* Effects */
  --glow-1: rgba(37,99,235,.055);
  --shadow-lg: 0 24px 70px -22px rgba(15,23,42,.24);
}
```

---

## 最后的核心原则

1. **冷色调统治** - 蓝、青、灰为主，避免暖色（除状态色）
2. **极简几何** - 圆角统一（6/9/14/999px），网格优先
3. **微弱光晕** - 强调色用半透明叠加，不用实色块
4. **精确对比** - 文字对比度严格符合 WCAG AA/AAA
5. **航天隐喻** - 术语、图标、动效均服务"深空巅峰"叙事

---

**配色设计完成** ✓  
适用场景：API 客户端、开发者工具、数据平台、技术中台  
设计师：基于 Apogee 远地点美学提炼  
版本：1.0 | 2026-07-30
