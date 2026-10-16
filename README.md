<div align="center">
  <img src="assets/LOGO_notext.png" width="96" alt="Clippi 图标">

  # Clippi

  **简洁 · 轻量 · 开源** 的原生剪贴板管理器<br>
  Rust + GPUI · Windows / macOS

  <p>
    <a href="https://clippi.rains-ailurus.cn/"><img src="https://img.shields.io/badge/官网-访问网站-2EA44F?style=for-the-badge" alt="官网"></a>
    <a href="https://clippi.rains-ailurus.cn/docs"><img src="https://img.shields.io/badge/文档-使用指南-0969DA?style=for-the-badge" alt="文档"></a>
    <a href="https://github.com/Ruszero01/clippi/releases"><img src="https://img.shields.io/github/downloads/Ruszero01/clippi/total?style=for-the-badge&amp;label=%E4%B8%8B%E8%BD%BD&amp;color=F97316&amp;logo=github&amp;logoColor=white" alt="下载"></a>
  </p>

  <p>
    <img src="https://img.shields.io/badge/Rust-2021-000000?style=flat&amp;logo=rust&amp;logoColor=white" alt="Rust 2021">
    <img src="https://img.shields.io/badge/GPUI-0.2.2-237a55?style=flat" alt="GPUI 0.2.2">
    <img src="https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-007EC6?style=flat" alt="Windows / macOS">
  </p>

  <strong>中文</strong> · <a href="README_EN.md">English</a>
</div>

![Clippi 主界面](docs/images/UI.png)

## 核心功能

- **记录与整理**：保存文本、富文本、图片、文件等复制内容，支持搜索、标签、收藏、备注和批量操作。
- **快速粘贴**：全局快捷键呼出，快速窗口不获取系统焦点；中文支持全拼和首字母搜索，不接管系统输入法；按住 Alt 查看并使用切换快捷键。
- **内容识别**：图片 OCR、二维码识别，快速提取和复用内容。
- **跨设备同步**：通过 OneDrive、iCloud 文件夹或 WebDAV 同步文本条目与标签，支持多个后端。
- **实用工具**：条目编辑、表情与符号面板、文件中转站。
- **个性与隐私**：明暗主题、字体与主题色设置，敏感内容预览脱敏、本地备份与恢复。

## 下载与使用

从 [官网](https://clippi.rains-ailurus.cn/) 或 [GitHub Releases](https://github.com/Ruszero01/clippi/releases) 下载安装，默认使用 `Alt+V` 呼出主窗口。更多操作见 [使用文档](https://clippi.rains-ailurus.cn/docs)。

- **macOS**：应用尚未签名，首次打开或更新后需在「系统设置 → 隐私与安全性」中允许打开，并授予「辅助功能」权限以使用快速粘贴。
- **Windows**：向管理员权限的应用粘贴时，Clippi 也需以管理员权限运行；可在「设置 → 通用 → 启动」中切换管理员模式，重启后生效。

## 从源码运行

安装 [Rust](https://www.rust-lang.org/tools/install) 及对应平台的编译工具后：

```bash
git clone https://github.com/Ruszero01/clippi.git
cd clippi
cargo run --release
```

## 贡献墙

感谢每一位贡献者！

<a href="https://github.com/Ruszero01/clippi/graphs/contributors">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/contributors-dark.svg">
    <img src="docs/images/contributors.svg" alt="Clippi 贡献者" width="840">
  </picture>
</a>

<p align="center">
  <a href="https://github.com/Ruszero01/clippi/issues"><img src="https://img.shields.io/badge/贡献与反馈-237a55?style=flat&amp;logo=github&amp;logoColor=white" alt="贡献与反馈" height="24"></a>
  <a href="RELEASES.md"><img src="https://img.shields.io/badge/更新日志-237a55?style=flat&amp;logo=readthedocs&amp;logoColor=white" alt="更新日志" height="24"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/MIT%20License-237a55?style=flat&amp;logo=opensourceinitiative&amp;logoColor=white" alt="MIT License" height="24"></a>
</p>
