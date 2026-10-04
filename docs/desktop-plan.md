# 清影（ClearClip）：短视频解析桌面版规划

把本仓库的抖音 / 快手去水印解析接口做成独立运行的桌面程序。本文包含现状评估、15 个同类产品调研、界面参考图说明、技术选型和分阶段实施计划。

界面参考图（可交互的样机）见 [`design-reference.html`](./design-reference.html)，用浏览器直接打开即可。

---

## 实施进度

代码在 [`desktop/`](../desktop/)，使用说明见 [`desktop/README.md`](../desktop/README.md)。

| 阶段 | 状态 | 说明 |
| --- | --- | --- |
| P0 技术验证 | 已完成，有保留 | Tauri 2 + Vue 3 工程可构建运行。抖音改为读取分享页 `_ROUTER_DATA`，快手读取 `INIT_STATE` 并回退旧接口。开发环境无法访问抖音、快手，**线上解析尚未实测**，解析逻辑用按页面结构编写的样本测试 |
| P1 MVP | 已完成 | 两个平台视频 / 图集解析，修复图集丢图、错误提示、SSL 校验等问题；分享文案提取链接；远程 API 模式；解析主页 |
| P2 效率功能 | 已完成 | 下载队列（并发、暂停续传、重试、直链过期重新解析）、图集选择、剪贴板监听、托盘迷你窗、全局快捷键、SQLite 历史与去重、媒体库、设置页、登录窗口取 Cookie |
| P3 打包发布 | 已完成配置 | `desktop-ci.yml`（检查）与 `desktop-release.yml`（三平台打包）；签名需配置 Secrets。自动更新采用“检查 GitHub Releases 并提示下载”，未接入需要签名密钥的静默更新 |
| P4 扩展 | 部分 | 已有：小红书、B站、微博解析；批量导入多条链接；Provider 插件式结构。新平台同样**未经线上实测**。未做：图集合成视频、规则远程下发、B站 DASH 合并 |

### 通用下载器重构（阶段 1–7）

在上面 P0–P4 的基础上，按“通用下载器 + 特殊平台插件”重构，分 7 个阶段在同一分支推进。

| 阶段 | 状态 | 内容 |
| --- | --- | --- |
| 1 加固 | 已完成 | 任务持久化、重启恢复；断点续传校验（ETag / Last-Modified、Content-Range、416）；加密 Cookie 存储（系统钥匙串保存密钥）、多账号、cookies.txt 导入；结构化错误与下一步操作；日志与诊断信息（自动脱敏） |
| 2 通用核心 | 已完成 | 多格式资源模型（清晰度、编码、码率、音视频分轨）；分段并行下载；音视频分轨下载后合并；按网站分流的网络出口（直连 / 系统代理 / 自定义代理）；全局限速、按网站并发；临时目录、重名策略、磁盘空间检查 |
| 3 通用下载能力 | 已完成 | yt-dlp / ffmpeg 组件管理（首次使用时下载、SHA-256 校验、镜像、回退、导入）；yt-dlp 解析上千个网站，直链和 m3u8 由内置引擎下载、DASH 等交给 yt-dlp；m3u8 引擎（AES-128、分片续传、广告过滤，拒绝 DRM 和直播）；网页嗅探兜底；B站 wbi 签名 + DASH 全清晰度；Pixiv 多页原图与动图合成；清晰度预设（优先 H.264）；提取音频；播放列表 / 合集批量下载与剧集命名 |
| 4 体验与手机发链接 | 已完成 | 手机发链接到电脑（局域网网页 + 二维码、令牌与配对确认、iPhone 快捷指令 / Android 说明，见 [phone-send.md](./phone-send.md)）；拖入链接或文本文件导入；剪贴板白名单；媒体库封面缓存、按平台 / 类型 / 时间 / 丢失筛选、网格视图、重新下载；写入标题和封面、信息 JSON、NFO；队列排序、定时开始、全部完成后打开文件夹 / 睡眠 / 关机、任务完成后运行命令；下载时阻止休眠；队列和媒体库虚拟滚动；平台健康检查；Windows 便携版 |
| 5 订阅追更 | 进行中 | |
| 6 直播录制 | 待开始 | |
| 7 生态与分发 | 待开始 | |

阶段 4 端到端验证：手机发送（配对确认后 2 秒内完成下载）、1000 条任务时队列滚动流畅、置顶顺序持久化、文件丢失后重新下载、元数据写入。

阶段 3 在本地模拟服务上端到端验证过：从 GitHub 安装 yt-dlp（校验通过）→ 解析普通网页、m3u8、DASH 清单、RSS 合集 → 下载、转封装为 MP4、合并 DASH 音视频、提取 MP3、按“列表名 / 第 N 集”归档。YouTube、B站、Pixiv 等线上网站在开发环境无法访问，**线上解析尚未实测**。

在本地用模拟服务（替代远程 API 与 CDN）端到端验证过：剪贴板识别 → 解析 → 视频下载、图集选择下载、重复内容跳过、媒体库记录、迷你窗下载。截图见 [`screenshots/`](./screenshots/)。

---

## 1. 现状评估

仓库实际是"短视频去水印解析"（抖音、快手的视频与图集），不是短剧平台。规划按这个能力来做，解析层设计成每个平台一个独立模块，后续可以按同样方式接入短剧或其他平台。

| 部分 | 现状 |
| --- | --- |
| 后端 | `API.php` 258 行，按域名分发到 `douyin()` / `kuaishou()` |
| 入口 | `jxindex.php?url=` 返回 `{code, message}` JSON |
| 前端 | `index.html`，Vue 2.6 + Element UI + axios，单页 |
| 返回结构 | `nickname / video_url / music / type`，type 为 `movie` 或 `photo` |

### 桌面化前必须处理的问题

| 级别 | 问题 |
| --- | --- |
| 高 | 前端把接口写死成 `https://www.ujrv.cn/api/jxindex.php`，离开这个服务器就无法使用 |
| 高 | 抖音解析依赖 `iesdouyin.com/web/api/v2/aweme/iteminfo`，这个老接口已基本失效，需要换成分享页内嵌数据或带签名的新接口 |
| 高 | 快手解析写死了一个 `did` Cookie，过期后整体失效；需要改为可配置或自动获取 |
| 中 | 所有 curl 请求关闭了 SSL 校验（`CURLOPT_SSL_VERIFYPEER=false`） |
| 中 | 抖音图集循环从下标 1 开始，第 0 张原图被封面替换，会丢一张图 |
| 中 | 出错时 HTTP 状态仍是 200，前端用 `response.status == 200` 判断成功，错误信息显示不出来 |
| 低 | 域名匹配只认 `v.douyin.com` / `v.kuaishou.com`，不支持从整段分享文案里提取链接，也不认 PC 端长链接 |
| 低 | PHP 不适合直接打进桌面安装包（需要捆绑解释器），解析逻辑需要移植 |

---

## 2. 同类产品调研（15 个）

覆盖三类：专做抖音/快手/小红书的开源工具、通用视频下载器（多为 yt-dlp 前端）、商业下载软件。

| # | 产品 | 形态 / 技术栈 | 平台范围 | 授权 | 可借鉴点 |
| --- | --- | --- | --- | --- | --- |
| 01 | [Douyin_TikTok_Download_API](https://github.com/Evil0ctal/Douyin_TikTok_Download_API)（Evil0ctal） | FastAPI + HTTPX，PyWebIO 网页，另有 CLI、MCP | 抖音、TikTok、B站 | 开源 | 解析层与界面分离；批量解析；配套 Cookie 抓取浏览器扩展 |
| 02 | [douyin-downloader](https://github.com/jiji262/douyin-downloader)（jiji262） | Python CLI | 抖音作品、图集、合集、音乐、主页批量 | 开源 | 进度显示、失败重试、SQLite 去重、浏览器兜底 |
| 03 | [TikTokDownloader](https://github.com/JoeanAmier/TikTokDownloader)（JoeanAmier） | Python，终端交互 + Web API | 抖音/TikTok 作品、喜欢、合集、直播 | 开源 | 文件命名模板、账号批量采集、可配置目录结构 |
| 04 | [XHS-Downloader](https://github.com/JoeanAmier/XHS-Downloader)（JoeanAmier） | Python，Textual 图形界面 + CLI | 小红书图文、视频、LivePhoto | 开源 | 剪贴板监听自动解析；提供 Win/macOS 打包产物 |
| 05 | [douyin-downloader](https://github.com/lecepin/douyin-downloader)（lecepin） | Rust + Tauri | 抖音 | 开源 | 与本方案技术路线一致；安装包很小 |
| 06 | [dYm](https://github.com/IronnMan/dYm)（IronnMan） | Electron + TypeScript | 抖音 | 开源 | 下载管理 + AI 内容分析，媒体库思路 |
| 07 | [TikDown](https://cdn.jsdelivr.net/npm/tiktokdien@1.1.4/README.md) | Electron，npm 分发 | 抖音、TikTok | 开源 | 复制分享链接即自动下载，零操作流程 |
| 08 | [Open Video Downloader](https://www.linuxlinks.com/open-video-downloader-cross-platform-yt-dlp-gui/) | Vue 3 + Tauri/Rust，调用 yt-dlp | yt-dlp 支持的站点 | 开源 | 和现有 Vue 前端最接近的参考架构 |
| 09 | [yt-dlp-gui](https://github.com/imsyy/yt-dlp-gui)（imsyy） | Tauri 2 + Rust | yt-dlp 支持的站点 | 开源 | 现代视觉风格，Tauri 2 工程组织 |
| 10 | [Parabolic](https://ubuntuhandbook.org/index.php/2026/04/parabolic-video-downloader-released-2026-4-0-with-macos-app/) | GTK4/libadwaita（Linux、macOS），Qt（Windows） | yt-dlp 支持的站点 | 开源 | 多任务并行、限速、账号凭据存系统钥匙串 |
| 11 | [Stacher](https://alternativeto.net/software/stacher/about) | 闭源 yt-dlp 图形前端 | yt-dlp 支持的站点 | 免费 + 订阅 | 免费版与付费功能的边界划分 |
| 12 | [4K Video Downloader Plus](https://www.softwareadvice.com/product/540736-4K-Video-Downloader-Plus/) | 商业桌面软件 | 主流视频站 | 付费 | 极简"粘贴链接"主界面、清晰度选择弹窗 |
| 13 | [Downie](https://mac.softpedia.com/get/Internet-Utilities/Downie.shtml) | macOS 原生 | 上千站点 | 付费 | 拖拽链接下载、浏览器扩展、后处理选项 |
| 14 | [Cobalt](https://alternativeto.net/software/cobalt-co-wukko-me-/about) | Web，自托管 | 20+ 平台 | 开源 | 无广告、无追踪；单输入框的极简交互 |
| 15 | [Seal](https://github.com/junkfood02/Seal) | Android，Kotlin + Compose，Material 3 | yt-dlp 支持的站点 | 开源 | 下载卡片、格式选择面板和动态主题的视觉参考 |

### 调研结论

- **主流程只有一步。** 4K Video Downloader、Cobalt、TikDown 都把"粘贴"做成唯一动作。我们也支持剪贴板监听，复制分享文案后自动识别。
- **Tauri 已是这一类工具的常见选择。** 05、08、09 都用 Tauri，安装包体积远小于 Electron（[对比](https://noqta.tn/en/blog/tauri-2-desktop-apps-rust-web-technologies-2026)）。
- **队列和历史是标配。** 02、10 都有并行下载、重试、去重。
- **Cookie 管理决定可用性。** 01、04 都为 Cookie 单独做了工具。桌面端可以内置登录窗口获取 Cookie。
- **解析层要可插拔。** 平台接口变动频繁，每个平台写成独立模块，方便单独修复。
- **合规提示要放在显眼处。** 首次启动展示免责声明，只下载用户有权保存的内容。

---

## 3. 设计参考图

视觉方向：深色工具风格，暖橙色作为唯一强调色。左侧固定导航，主区域上方始终保留粘贴框。共 5 张，见 [`design-reference.html`](./design-reference.html)：

| 编号 | 页面 | 要点 |
| --- | --- | --- |
| A | 解析主页 | 粘贴整段分享文案即可，自动提取短链；结果卡片列出视频 / 音乐 / 封面，默认勾选视频；下方是最近解析 |
| B | 下载队列 | 进行中 / 等待 / 完成 / 失败统计与总速度；失败项给出原因，直链过期一键重新解析 |
| C | 图集选择 | 图集默认全选，点击切换，可同时下载背景音乐 |
| D | 设置 | 下载目录、命名模板、并发数；剪贴板监听、自动下载；本地解析 / 远程 API；Cookie；外观、托盘、更新 |
| E | 托盘迷你窗 | 主窗口关闭后驻留托盘；复制到链接时弹出迷你窗直接下载；全局快捷键 `Ctrl+Shift+D` |

---

## 4. 技术选型

| 方案 | 安装包 | 复用现有代码 | 解析逻辑放哪 | 结论 |
| --- | --- | --- | --- | --- |
| **Tauri 2 + Vue 3 + Rust** | 约 10 MB | 前端交互迁移到 Vue 3；PHP 逻辑移植到 Rust | Rust（reqwest），不受跨域限制 | **推荐** |
| Electron + Vue 3 + Node | 100 MB 以上 | 同上，解析移植到 TypeScript | Node 主进程 | 可行，体积和内存偏大 |
| Tauri + PHP 侧车 | 30 MB 以上 | PHP 原样保留 | 捆绑 php-cli 子进程 | 不推荐：跨平台打包麻烦，原接口本身也要重写 |
| Wails（Go）/ Flutter | 10–25 MB | 前端需重写 | Go / Dart | 无相关积累时不划算 |

### 推荐技术栈

- **壳**：Tauri 2，插件 `clipboard-manager`、`notification`、`global-shortcut`、`dialog`、`opener`，系统托盘。
- **前端**：Vue 3 + TypeScript + Vite + Pinia + Element Plus（与现有 Element UI 用法相近）。
- **解析层**：Rust，统一的 `Provider` trait（`matches(url)` / `resolve(url) → MediaInfo`），每个平台一个文件；输出结构兼容现有 `nickname / video_url / music / type`，补充封面、作者、时长、分辨率。
- **下载器**：Rust 流式下载，带 Referer/UA，进度以事件推给前端；并发数、暂停、失败重试、直链过期自动重新解析。
- **存储**：SQLite（rusqlite）保存历史与去重；设置存应用配置目录下的 JSON。

### 目录规划（新增 `desktop/`，原 PHP 保留不动）

```
Short_Video-API/
├── API.php / jxindex.php / index.html   # 原网页版，保留
└── desktop/
    ├── src/                             # Vue 3 前端
    │   ├── views/ ParseView QueueView LibraryView SettingsView MiniView
    │   ├── stores/ queue settings history
    │   └── api/                         # 封装 Tauri 命令
    └── src-tauri/
        ├── src/
        │   ├── lib.rs commands.rs tray.rs clipboard.rs
        │   ├── providers/ mod.rs douyin.rs kuaishou.rs remote.rs
        │   ├── download.rs              # 队列与任务
        │   ├── db.rs                    # 历史与去重
        │   └── settings.rs
        └── tauri.conf.json
```

---

## 5. 实施计划

按 1 名全栈开发估算，共约 6 周。

### P0 · 第 1 周 · 技术验证
- 搭 Tauri 2 + Vue 3 空壳，三平台本地能跑起来
- 验证抖音、快手当前可用的取数方式（分享页内嵌 JSON / 新接口及签名），确定是否需要 Cookie
- 用 Rust 写出最小的抖音解析并通过命令返回给前端

完成标准：粘贴一条抖音链接，桌面窗口里拿到无水印直链。若取数方式不可行，在这里决定是否改为"远程 API 为主"。

### P1 · 第 2–3 周 · MVP
- 移植抖音、快手的视频与图集解析，修复现状评估里的图集丢图、错误码等问题
- 解析主页（参考图 A）、单任务下载、预览
- 从分享文案中提取链接；支持远程 API 模式
- 解析逻辑的单元测试（用录制的响应样本，不依赖线上）

完成标准：两个平台四种内容都能解析并下载到本地。

### P2 · 第 4–5 周 · 效率功能
- 下载队列（参考图 B）：并发、暂停、重试、直链过期自动重新解析
- 图集选择（参考图 C）、音乐/封面单独下载
- 剪贴板监听、托盘迷你窗、全局快捷键（参考图 E）
- SQLite 历史记录与去重、媒体库页；设置页（参考图 D）
- 内置登录窗口获取 Cookie

完成标准：连续复制 10 条链接，全部自动进队列并下载完成。

### P3 · 第 6 周 · 打包与发布
- GitHub Actions 矩阵构建 Windows（msi/nsis）、macOS（dmg，Intel + Apple Silicon）、Linux（AppImage/deb）
- 代码签名与 macOS 公证；自动更新
- 首次启动免责声明、README 更新

完成标准：从 Releases 下载安装包，三平台安装即用。

### P4 · 之后 · 扩展
- 新增平台：小红书、B站、微博等（依赖 Cookie 的平台走内置登录）
- 解析规则远程下发，平台接口变化时不必发新版
- 图集合成视频（ffmpeg）、批量导入链接文件

---

## 6. 风险与对策

| 风险 | 影响 | 对策 |
| --- | --- | --- |
| 平台接口和签名频繁变化 | 解析失效，用户直接感知 | Provider 独立模块 + 远程 API 备用通道；用样本测试快速定位 |
| 风控（需要 Cookie、验证码） | 部分内容无法获取 | 内置登录窗口；请求频率限制；失败时给出明确提示 |
| 直链有时效 | 排队后下载 403 | 下载前检查，过期自动重新解析 |
| 版权与合规 | 分发渠道下架、法律风险 | 首次启动声明，仅用于个人有权保存的内容；批量采集他人主页等功能默认关闭 |
| macOS 签名/公证成本 | 未签名包被系统拦截 | P3 预留 Apple 开发者账号；早期可先发 Windows 版 |
