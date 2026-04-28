<div align="center">

<img src="src-tauri/icons/icon.png" width="128" height="128" alt="Logo">

# Douyin Downloader

**抖音视频下载器 · Rust / Tauri 重构版**

[![Rust](https://img.shields.io/badge/Rust-1.70+-orange.svg?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/Tauri-2.0-blue.svg?style=flat-square&logo=tauri&logoColor=white)](https://tauri.app/)
[![CI](https://img.shields.io/github/actions/workflow/status/anYuJia/douyin-downloader-rust/ci.yml?branch=main&style=flat-square&label=CI)](https://github.com/anYuJia/douyin-downloader-rust/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/anYuJia/douyin-downloader-rust?style=flat-square)](https://github.com/anYuJia/douyin-downloader-rust/releases)
[![License](https://img.shields.io/badge/License-MIT-green.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey.svg?style=flat-square)]()

基于 Rust + Tauri 2.0 的跨平台桌面版抖音下载工具，支持用户检索、批量下载、推荐视频浏览、点赞列表获取与实时下载进度。

<p>
  <a href="https://github.com/anYuJia/douyin-downloader-rust/releases/latest"><strong>下载最新版</strong></a>
  ·
  <a href="#快速开始"><strong>快速开始</strong></a>
  ·
  <a href="#从源码构建"><strong>源码运行</strong></a>
  ·
  <a href="#常见问题"><strong>常见问题</strong></a>
</p>

</div>

---

## 项目简介

本项目是 [DY_video_downloader](https://github.com/anYuJia/DY_video_downloader) 的 Rust 重构版本。

相比 Python 打包版，当前版本更适合作为长期维护的桌面应用：

- **更小体积**：打包后体积明显缩小
- **更低内存**：原生应用运行开销更低
- **更快启动**：无需 Python 运行时
- **更易分发**：原生支持 Windows / macOS / Linux

---

## 为什么做这个项目

- 想把原先依赖 Python 运行时的工具，重构成更轻量的桌面应用
- 想保留批量下载、推荐视频、点赞列表这类高频能力
- 想让功能更集中，减少脚本式工具的使用门槛

---

## 功能亮点

- **用户检索**：支持昵称、抖音号、链接搜索用户
- **批量下载**：一键下载用户全部作品
- **推荐视频**：浏览推荐 feed，支持沉浸式预览
- **点赞列表**：获取自己点赞的视频与作者列表
- **多媒体支持**：支持视频、图集、Live Photo 等内容
- **下载质量可选**：最高质量 / 兼容优先 / 最小体积
- **实时进度**：下载任务实时显示进度与状态
- **浏览器登录**：内置登录流程，便于获取可用 Cookie

---

## 快速开始

### 下载安装

从 [Releases](../../releases/latest) 下载对应平台的安装包：

| 平台 | 可用文件 |
|:---|:---|
| Windows | `.exe` 安装包 / 便携版 |
| macOS | `.dmg` / `.app` 便携版 |
| Linux | `.deb` / `.AppImage` |

### 首次使用建议

1. 先在设置中完成 Cookie / 登录配置
2. 再使用搜索、推荐视频或点赞列表功能
3. 最后根据需要进行单个下载或批量下载

> **macOS 用户**
>
> 首次运行若提示“无法验证开发者”，可执行：
>
> ```bash
> sudo xattr -rd com.apple.quarantine /Applications/Douyin\ Downloader.app
> ```

---

## 界面预览

| 首页 | 用户详情 | 播放器 |
|:--:|:--:|:--:|
| <img src="docs/home.png" width="100%" alt="主页"> | <img src="docs/user_detail.png" width="100%" alt="用户详情"> | <img src="docs/playvideo.png" width="100%" alt="视频播放"> |
| 搜索、链接解析、推荐视频入口 | 用户信息与作品入口 | 推荐视频沉浸式预览 |

---

## 当前支持的使用场景

- 搜索用户并查看用户详情
- 批量下载用户作品
- 获取并浏览推荐视频
- 获取自己点赞的视频 / 作者列表
- 下载视频、图集、Live Photo 等内容
- 查看下载历史与本地文件

---

## 从源码构建

### 环境要求

- Rust 1.70+
- Node.js 18+（可选，用于前端开发）
- 系统依赖见 [Tauri 官方文档](https://tauri.app/start/prerequisites/)

### 开发模式运行

```bash
git clone https://github.com/anYuJia/douyin-downloader-rust.git
cd douyin-downloader-rust

cd src-tauri
cargo tauri dev
```

### 构建发布版

```bash
cd src-tauri
cargo tauri build
```

---

## 技术栈

- **后端**：Rust + Tauri 2.0
- **前端**：原生 HTML / CSS / JavaScript + Bootstrap 5
- **通信**：桌面命令调用 + 实时状态推送

---

## 使用说明

- 推荐、点赞列表、批量下载等能力依赖有效登录态
- 部分接口可能因抖音风控、Cookie 失效、网络环境等因素受影响
- 推荐先使用内置登录 / Cookie 配置，再进行大批量操作
- 本项目更适合作为个人学习、研究和桌面工具使用

---

## 常见问题

### 1. 为什么有些功能需要登录？

推荐视频、点赞列表、部分批量下载能力依赖有效 Cookie / 登录态。未登录时，接口可能直接拒绝或返回不完整数据。

### 2. 可以只下载单个视频吗？

可以。除了批量下载，也支持通过粘贴链接解析后进行单个下载。

### 3. 下载文件保存到哪里？

下载目录可以在设置中修改。历史记录页面也支持直接打开文件或定位到文件夹。

### 4. 推荐视频接口为什么有时不稳定？

推荐流、详情、点赞等接口都可能受到平台风控、Cookie 状态和网络环境影响。这类现象属于预期范围。

---

## 已知限制

- 对登录态和 Cookie 有依赖
- 接口可能随抖音策略变化而失效或返回结构变化
- 某些平台首次运行需要额外系统权限或安全确认
- 当前仍以桌面端本地使用为主，不是云服务方案

---

## 贡献与反馈

- 发现问题：欢迎提交 [Issue](https://github.com/anYuJia/douyin-downloader-rust/issues)
- 想改进功能：欢迎发起 Pull Request
- 提交前建议先本地运行：

```bash
cd src-tauri
cargo fmt --check
cargo test
```

---

## 相关项目

- [DY_video_downloader](https://github.com/anYuJia/DY_video_downloader) - Python 原版

---

## License

本项目基于 [MIT License](LICENSE) 开源。

---

## 免责声明

本工具仅供个人学习研究使用，请勿用于商业用途或大规模爬取。因滥用导致的后果，项目贡献者不承担责任。

---

<p align="center">觉得有用？给个 ⭐ Star 支持一下</p>
