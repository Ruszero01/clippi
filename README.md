# Clippi

一个轻量的 Windows（暂时，未来扩展跨平台） 剪贴板管理器，使用 Rust + Slint 构建。

## 目前功能

- 自动监听剪贴板变化，记录历史
- 单击复制，双击快速粘贴
- 暗色 / 亮色主题切换
- 系统托盘后台运行
- SQLite 本地持久化
- 基础设置页面
- 开机自启
- 基础快捷键功能和快捷键配置

## 技术栈

| 组件 | 技术 |
|------|------|
| UI | [Slint](https://slint.dev/) 1.16 |
| 剪贴板监听 | clipboard-rs |
| 数据存储 | rusqlite (bundled SQLite) |
| 系统托盘 | tray-icon |
| 键盘模拟 | windows-sys |
| 图标字体 | iconfont (运行时加载) |

## 构建

```bash
cargo build
cargo run
```

## 状态

> **早期项目雏形**，功能尚不完善，仅作为基础框架验证。后续会持续迭代完善。
