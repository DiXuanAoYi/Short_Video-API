# 清影 ClearClip（桌面版）

把本仓库的抖音 / 快手去水印解析做成的桌面程序。基于 Tauri 2 + Vue 3 + Rust，支持 Windows、macOS、Linux。

规划与设计参考见 [`docs/desktop-plan.md`](../docs/desktop-plan.md)。

| 解析 | 图集选择 |
| --- | --- |
| ![解析](../docs/screenshots/parse.png) | ![图集](../docs/screenshots/album.png) |
| **下载队列** | **设置** |
| ![队列](../docs/screenshots/queue.png) | ![设置](../docs/screenshots/settings.png) |

## 功能

- 粘贴整段分享文案即可解析，自动提取链接；一次粘贴多条链接可批量下载
- 抖音、快手的视频和图集；可单独下载背景音乐、封面
- 下载队列：并发数可调、暂停 / 继续（断点续传）、网络错误自动重试、直链过期自动重新解析
- 剪贴板监听：复制链接后自动识别；主窗口不在前台时，右下角弹出迷你窗
- 全局快捷键（默认 `Ctrl+Shift+D`）解析剪贴板；关闭窗口后驻留系统托盘
- 媒体库：已下载文件和解析历史，支持搜索；已下载过的内容自动跳过
- 文件命名模板：`{author}` `{title}` `{date}` `{id}` `{platform}`
- 内置登录窗口获取 Cookie，也可手动填写
- 远程 API 模式：可继续使用已部署的旧版 `jxindex.php`，也可作为本地解析失败时的备用
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
    ├── providers/             # 解析器：douyin.rs kuaishou.rs remote.rs
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

- 抖音解析读取移动端分享页里的 `window._ROUTER_DATA`，快手读取 `window.INIT_STATE`，并回退到旧版使用的 `rest/wd/photo/info` 接口。平台调整页面结构后需要更新对应解析器；可以先切换到远程 API 模式。
- 单元测试使用按页面结构编写的样本数据（`src-tauri/tests/fixtures/`），不访问线上。
- 尚未实现：图集合成视频、解析规则远程下发、小红书 / B站等更多平台。

## 免责声明

仅用于下载你有权保存的内容。本项目只为学习研究，如涉及侵权请联系删除。
