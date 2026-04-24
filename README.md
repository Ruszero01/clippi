# Clippi

一个轻量的 Windows（暂时，未来扩展跨平台） 剪贴板管理器，使用 Rust + Slint 构建。

## 功能

- 自动监听剪贴板变化，记录历史
- 单击复制，双击快速粘贴
- 暗色 / 亮色主题切换
- 系统托盘后台运行
- SQLite 本地持久化

## 技术栈

| 组件 | 技术 |
|------|------|
| UI | [Slint](https://slint.dev/) 1.16 |
| 剪贴板监听 | clipboard-rs |
| 数据存储 | rusqlite (bundled SQLite) |
| 系统托盘 | tray-icon |
| 键盘模拟 | windows-sys |
| 图标字体 | iconfont (运行时加载) |

## 图标

类型指示图标使用 iconfont 字体，位于 `assets/fonts/iconfont.ttf`。如需添加新图标：

1. 在 [iconfont](https://www.iconfont.cn/) 上传/创建图标项目
2. 下载字体文件并替换 `assets/fonts/iconfont.ttf`
3. 在 `ClipboardList.slint` 中使用对应 unicode（格式：`\u{xxxx}`）

## 构建

```bash
cargo build
cargo run
```

## 状态

> **早期项目雏形**，功能尚不完善，仅作为基础框架验证。后续会持续迭代完善。
