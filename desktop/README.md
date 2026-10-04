# 清影 ClearClip（桌面版）

把本仓库的短视频去水印解析做成的桌面程序，支持抖音、快手、小红书、B站、微博。基于 Tauri 2 + Vue 3 + Rust，支持 Windows、macOS、Linux。

规划与设计参考见 [`docs/desktop-plan.md`](../docs/desktop-plan.md)。

| 解析 | 图集选择 |
| --- | --- |
| ![解析](../docs/screenshots/parse.png) | ![图集](../docs/screenshots/album.png) |
| **下载队列** | **设置** |
| ![队列](../docs/screenshots/queue.png) | ![设置](../docs/screenshots/settings.png) |

## 功能

- 粘贴整段分享文案即可解析，自动提取链接；一次粘贴多条链接可批量下载
- 支持平台：

  | 平台 | 内容 | 解析方式 |
  | --- | --- | --- |
  | 抖音 | 视频、图集、背景音乐 | 分享页 `window._ROUTER_DATA` |
  | 快手 | 视频、图集、背景音乐 | 分享页 `window.INIT_STATE`，回退 `rest/wd/photo/info` |
  | 小红书 | 视频笔记、图文笔记（无水印原图） | 笔记页 `window.__INITIAL_STATE__` |
  | B站 | 视频（含多 P，按链接 `?p=` 选择） | `x/web-interface/view` + `x/player/playurl`（html5 MP4） |
  | 微博 | 视频、图片（转发微博取原微博媒体） | `m.weibo.cn/statuses/show` |

- 可单独下载背景音乐、封面
- 下载队列：并发数可调、暂停 / 继续（断点续传）、网络错误自动重试、直链过期自动重新解析
- 剪贴板监听：复制链接后自动识别；主窗口不在前台时，右下角弹出迷你窗
- 全局快捷键（默认 `Ctrl+Shift+D`）解析剪贴板；关闭窗口后驻留系统托盘
- 媒体库：已下载文件和解析历史，支持搜索；已下载过的内容自动跳过
- 文件命名模板：`{author}` `{title}` `{date}` `{id}` `{platform}`
- 内置登录窗口获取 Cookie，也可手动填写
- 远程 API 模式：可继续使用已部署的旧版 `jxindex.php`（仅抖音、快手），也可作为本地解析失败时的备用
- 首次启动免责声明；通过 GitHub Releases 检查新版本

## 开发

需要 Node.js 20+、Rust 1.80+，以及 [Tauri 的系统依赖](https://tauri.app/start/prerequisites/)（Linux 需要 `libwebkit2gtk-4.1-dev` 等）。

```bash
cd desktop
npm install
npm run tauri dev      # 开发模式
npm run tauri build    # 打包安装包，产物在 src-tauri/target/release/bundle/
```

检查：

```bash
npm run build                                   # 前端类型检查 + 构建
cd src-tauri
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test                                      # 解析器、下载器、数据库、命名规则的单元测试
```

## 发布

推送 `desktop-v*` 标签（例如 `desktop-v0.1.0`）后，`.github/workflows/desktop-release.yml` 会构建 Windows（msi / nsis）、macOS（Apple Silicon 与 Intel 的 dmg）、Linux（AppImage / deb），并创建草稿 Release。

macOS 签名和公证需要在仓库 Secrets 中配置 `APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、`APPLE_SIGNING_IDENTITY`、`APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`。不配置时生成未签名的安装包。

## 目录结构

```
desktop/
├── src/                       # Vue 3 前端
│   ├── App.vue                # 主窗口：侧边导航 + 四个页面
│   ├── MiniApp.vue            # 托盘迷你窗（index.html#mini）
│   ├── views/                 # 解析 / 下载队列 / 媒体库 / 设置
│   ├── stores/app.ts          # Pinia：设置、队列、解析状态
│   └── api/index.ts           # Tauri 命令与事件封装
└── src-tauri/src/
    ├── providers/             # 解析器：douyin kuaishou xiaohongshu bilibili weibo remote
    ├── download.rs            # 下载队列
    ├── db.rs                  # SQLite：历史与媒体库
    ├── naming.rs              # 文件命名
    ├── settings.rs            # 设置（JSON）
    ├── clipboard.rs           # 剪贴板监听
    ├── tray.rs                # 托盘、迷你窗
    └── commands.rs            # 前端可调用的命令
```

新增平台：在 `providers/` 下新建文件实现 `Provider` trait（`matches` / `resolve` / `referer`），并在 `providers::all()` 中注册。

## 数据位置

设置、数据库保存在系统的应用数据目录（标识 `com.dixuanaoyi.clearclip`），例如：

- Windows：`%APPDATA%\com.dixuanaoyi.clearclip\`
- macOS：`~/Library/Application Support/com.dixuanaoyi.clearclip/`
- Linux：`~/.config/com.dixuanaoyi.clearclip/` 与 `~/.local/share/com.dixuanaoyi.clearclip/`

默认下载到系统“下载”文件夹下的 `ClearClip` 目录。

## 已知限制

- 各平台解析依赖公开页面或接口，平台调整后需要更新对应解析器（抖音、快手可先切换到远程 API 模式）。
- 小红书网页链接通常需要带 `xsec_token`，部分笔记、微博需要先在设置中登录。
- B站使用 html5 平台的合一 MP4：未登录一般为 480P / 720P，登录后请求 1080P；只有 DASH 分轨的视频、番剧和付费内容暂不支持。
- 单元测试使用按页面结构编写的样本数据（`src-tauri/tests/fixtures/`），不访问线上。
- 尚未实现：图集合成视频、解析规则远程下发、B站 DASH 音视频合并、小红书实况照片。

## 免责声明

仅用于下载你有权保存的内容。本项目只为学习研究，如涉及侵权请联系删除。
