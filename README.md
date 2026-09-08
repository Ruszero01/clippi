<div align="center">
  <p>
    <img src="assets/LOGO_notext.png" width="120" alt="Clippi 图标">
  </p>

  # Clippi

  **简洁 · 轻量 · 开源** 的原生剪贴板管理器<br>
  基于 Rust + GPUI 构建，支持 Windows 和 macOS

  <p>
    <a href="https://clippi.rains-ailurus.cn/">🌐 官方网站</a> ·
    <a href="https://github.com/Ruszero01/clippi/releases">⬇️ 下载</a> ·
    <a href="https://github.com/Ruszero01/clippi">GitHub</a>
  </p>

  <p>
    <a href="README.md">中文</a> · <a href="README_EN.md">English</a>
  </p>

  <p>
    <a href="https://github.com/Ruszero01/clippi/issues">反馈问题</a> ·
    <a href="https://github.com/Ruszero01/clippi/releases">更新日志</a>
  </p>

  <p>
    <a href="LICENSE"><img src="https://img.shields.io/badge/License-MIT-yellow.svg" alt="License"></a>
    <img src="https://img.shields.io/badge/Rust-2021-%23000000?logo=rust" alt="Rust">
    <img src="https://img.shields.io/badge/GPUI-0.2-%23555555?logo=rust" alt="GPUI">
    <img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-blue" alt="Platform">
    <a href="https://deepwiki.com/Ruszero01/clippi"><img src="https://deepwiki.com/badge.svg" alt="Ask DeepWiki"></a>
  </p>
</div>

![UI](./docs/images/UI.png)

---

> Clippi 是一款轻量、开源的剪贴板历史管理工具：它默默记录你的复制内容，支持搜索、标签筛选、图片 OCR、二维码识别、批量操作和跨设备同步，让复制内容真正成为可整理、可复用、可搜索的资产。

---

## 为什么选择 Clippi？

- **无 WebView 依赖**：使用 Rust + GPUI 构建原生界面，静默时内存占用低，性能与美观兼得
- **多标签系统**：支持为条目添加多个标签，并提供多种标签筛选模式
- **双窗口模式**：无焦点快速粘贴窗口不打断当前输入状态，分工明确
- **多后端同步架构**：支持 OneDrive / iCloud / WebDAV 等同步方式
- **隐私保护**：对邮箱、手机号等敏感内容进行脱敏预览，减少屏幕上的隐私暴露

## 功能亮点

### 剪贴板监控

![clipboard](./docs/images/clipboard.png)

- 多格式内容检测：纯文本、富文本、文件、图片、链接、路径、颜色、电话、邮箱
- 内容哈希去重：相同内容重复复制时更新时间戳，不重复记录
- 颜色归一化去重：`#FF8000` ≡ `rgb(255,128,0)`，避免重复
- 图片 OCR：关键词搜索与内容粘贴
- 二维码识别：识别二维码，支持一键跳转
- 快捷键黑名单：指定应用中禁用全局热键
- 纯文本复制模式：开启后丢弃富文本格式，仅保留纯文本

### 内容管理

![content1](./docs/images/content.png)

- 双击卡片快速粘贴
- 多类型条目编辑
- 收藏与备注：收藏常用片段，并为条目添加备注
- 多选批量操作：批量粘贴（换行分隔）、批量收藏、批量删除、批量标签
- 类型组合筛选：自由搭配多个筛选规则
- 关键词搜索 — 同时匹配文本内容和标签名
- 标签筛选 — 多标签 AND/OR 逻辑可切换
- 排序：按创建时间 / 按最后使用时间
- 敏感信息预览脱敏：邮箱仅显示前两位 + 域名，手机号仅显示前三位 + 后四位

### 标签系统

![tags](./docs/images/tags.png)

- 创建/编辑/删除标签、12 种预设颜色
- 标签关联到剪贴板条目（多对多）
- 侧边标签栏：筛选标签固定到窗口左侧，支持展开/折叠动画和固定
- 标签筛选面板 + 标签选择器面板（均支持标签 CRUD）
- 单条目/批量标签分配与移除
- 跨设备标签同步（含颜色冲突解决）

### 窗口与交互

![hotkey](./docs/images/hotkey.png)

- 全局快捷键呼出（默认 `Alt+V`，支持录制自定义）
- 窗口置顶（固定）模式
- 失焦自动隐藏
- 多显示器支持（光标所在屏幕）
- 三种窗口呼出位置：居中 / 跟随鼠标 / 记住位置
- 深色/亮色主题，自动检测系统深色模式

### 显示选项

![display](./docs/images/display.png)

- 来源应用信息显示（剪贴板来源程序名称和图标）
- 卡片高度模式：高 / 中 / 低 / 自适应
- 悬停显示原始内容（有备注时）
- 纯文本复制

### 云同步

![sync](./docs/images/sync.png)

- 多后端架构：支持同时配置多个同步服务，每个后端独立开关与独立同步间隔
- 本地文件夹后端：通过 OneDrive / iCloud 云盘同步
- WebDAV 后端：支持 WebDAV 服务器，ETag 缓存 + Basic Auth
- 自动检测 OneDrive (Windows + macOS) 和 iCloud (macOS) 预设路径
- 跨设备删除与取消收藏传播（墓碑机制，30 天窗口）
- 最后写入者胜出 (LWW) 冲突解决
- 语义哈希比较，跳过无变更推送（避免同步循环）
- 冲突文件自动合并与清理
- 可配同步间隔（30秒 / 1分 / 10分 / 30分）+ 手动即时同步
- 仅收藏条目同步模式
- 异步连接测试

## 快速开始

### 下载安装

- 访问 [官方网站](https://clippi.rains-ailurus.cn/) 或 [GitHub Releases](https://github.com/Ruszero01/clippi/releases) 下载最新版本
- 支持 Windows 与 macOS 安装包 / dmg

### 从源码构建

```bash
git clone https://github.com/Ruszero01/clippi.git
cd clippi
cargo build
cargo run
```

---

## macOS 用户须知

Clippi 未经过 Apple 开发者签名（未加入 Apple Developer Program），首次打开或每次更新后，macOS Gatekeeper 会阻止应用运行。请按以下步骤操作：

### 首次安装 / 更新后打开

1. 下载 `.dmg` 文件后，将 Clippi 拖入 `应用程序` 文件夹
2. 双击 Clippi 打开，弹出安全性弹窗后选择 **"完成"**
3. 前往 **系统设置 → 隐私与安全性**，在页面底部点击 **"仍要打开"**（每次更新后需重新执行此步骤）

### 授予辅助功能权限（快速粘贴功能必需）

Clippi 的快速粘贴功能需要辅助功能权限来模拟按键输入：

1. 打开 **系统设置 → 隐私与安全性 → 辅助功能**
2. 在列表中找到 **Clippi** 并开启开关
3. 如果 Clippi 不在列表中，点击 `+` 按钮手动添加，路径为 `/Applications/Clippi.app`

> 未授予辅助功能权限时，快速粘贴（双击卡片/Enter 键粘贴）将无法工作，但你仍然可以使用右键菜单手动复制粘贴。

---

## 相关链接

- [官方网站](https://clippi.rains-ailurus.cn/)
- [官方文档](https://clippi.rains-ailurus.cn/docs.html)
- [GitHub 仓库](https://github.com/Ruszero01/clippi)
- [GitHub Releases](https://github.com/Ruszero01/clippi/releases)
- [问题反馈](https://github.com/Ruszero01/clippi/issues)

## License

[MIT](LICENSE)
