# 发布前的准备：更新签名、代码签名、扩展上架

这三件事需要用到你自己的账号、证书和付费订阅，只能由你来做。代码和流水线已经准备好了：配好下面的密钥后，重新运行一次 `desktop-release` 工作流，安装包就会带上签名。**不配也能正常发布和使用**，只是安装时系统会弹出“未知发布者”之类的提示。

| 事项 | 作用 | 费用 | 难度 | 建议顺序 |
| --- | --- | --- | --- | --- |
| [A. 更新签名密钥](#a-更新签名密钥) | 程序内“检查更新 → 一键安装” | 免费 | 低（10 分钟） | 先做 |
| [B. 浏览器扩展上架](#b-浏览器扩展上架) | 用户从商店一键安装扩展 | Chrome 一次性注册费（金额以后台为准）；Edge、Firefox 免费 | 中（1–3 天审核） | 第二 |
| [C. macOS 签名和公证](#c-macos-签名和公证) | 去掉“无法验证开发者”的拦截 | Apple Developer Program（年费） | 中 | 第三 |
| [D. Windows 代码签名](#d-windows-代码签名) | 去掉 SmartScreen 的“未知发布者”警告 | 证书或签名服务费用（按年） | 较高 | 最后，可不做 |

> 费用、界面和审核要求会变，下面的步骤以各官方页面为准，文末附了链接。

---

## A. 更新签名密钥

程序内更新用一对密钥校验更新包：**私钥**留在 GitHub 的 Secrets 里，用来给安装包签名；**公钥**编译进程序，用来验证。这和操作系统的代码签名无关，不需要购买证书。

### 步骤

1. 在自己电脑上（需要装 Node.js），进入仓库的 `desktop` 目录，生成密钥：

   ```bash
   cd desktop
   npm install
   npm run tauri signer generate -- -w ~/.tauri/clearclip.key
   ```

   按提示设置一个密码（可以留空，但建议设置）。生成两个文件：`clearclip.key`（私钥）和 `clearclip.key.pub`（公钥）。

2. 打开 GitHub 仓库 → **Settings → Secrets and variables → Actions → New repository secret**，添加三个 Secret：

   | 名称 | 内容 |
   | --- | --- |
   | `TAURI_SIGNING_PRIVATE_KEY` | `clearclip.key` 文件的全部内容 |
   | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | 第 1 步设置的密码（没设就不用添加） |
   | `TAURI_UPDATER_PUBKEY` | `clearclip.key.pub` 文件的全部内容 |

3. 发布新版本时，在 **Actions → desktop-release → Run workflow**（选 `main` 分支）。流水线检测到私钥后会自动：
   - 额外生成更新包（`.sig` 签名文件）和 `latest.json`，上传到 Release；
   - 把公钥编译进程序。

4. 验证：用**这一版及以后**的安装包，在“设置 → 通用 → 关于”点“检查更新”，有新版本时会出现“下载并安装”。

### 注意

- **私钥务必备份**（密码管理器、加密网盘）。丢了私钥，已安装的用户就无法再自动更新，只能手动下载新版安装包。
- **私钥不要提交到仓库**，也不要贴到聊天或 Issue 里。
- 已经发布的 0.1.0 没有编译进公钥，**不能**程序内更新，用户需要手动下载一次新版；之后的版本就可以自动更新了。
- 更新检查地址写在 `desktop/src-tauri/tauri.conf.json` 的 `plugins.updater.endpoints`，指向最新 Release 里的 `latest.json`。

---

## B. 浏览器扩展上架

扩展的打包文件 `ClearClip-extension-<版本>.zip` 会由发布流水线自动放进每个 Release，可以直接上传到各商店。上架材料已经准备好：

- 商店文案和权限说明：[`desktop/extension/store/listing.md`](../desktop/extension/store/listing.md)（复制粘贴即可）
- 隐私政策：[`docs/privacy.md`](privacy.md)，填商店要求的“隐私政策链接”时用  
  `https://github.com/DiXuanAoYi/Short_Video-API/blob/main/docs/privacy.md`

### 要准备的截图

商店都要求截图，我无法替你截。建议 3–5 张，在清影和浏览器里实际操作后截取：

1. 在视频网页上点击扩展图标，弹窗里的“发送当前页面”；
2. 右键菜单里的“发送此页面到清影”；
3. 清影“媒体库 → 收到的链接”里出现这条记录；
4. 扩展里的“同步此网站的登录 Cookie”和自动同步开关。

Chrome 要求 1280×800 或 640×400（PNG 或 JPEG）；Edge、Firefox 对尺寸要求较宽松，用同样的图即可。

### 1. Chrome 网上应用店

1. 打开 [Chrome 开发者控制台](https://chrome.google.com/webstore/devconsole)，用 Google 账号登录，接受开发者协议并缴纳一次性注册费。
2. **添加新内容 → 上传** `ClearClip-extension-<版本>.zip`。
3. 填写“商店列表”（文案见 `listing.md`）、上传图标（`icons/128.png`）和截图。
4. 在“隐私权”页签填写：
   - **单一用途**：见 `listing.md`；
   - **权限理由**：对 `cookies`、`contextMenus`、`storage`、`activeTab`、`notifications` 和主机权限 `<all_urls>` 分别填写理由（`listing.md` 里有现成文字）；
   - **隐私政策链接**：上面的链接；
   - 数据使用披露：勾选“身份验证信息”（Cookie）和“网站内容/浏览历史（用户主动发送的网址）”，并勾选三项合规声明。
5. 提交审核。申请 `<all_urls>` 这类宽范围主机权限的扩展审核会更久，可能被要求补充说明；如果被拒，按邮件意见修改后重新提交。

> 想加快审核、减少被拒的风险，可以以后把 `<all_urls>` 改成“按需授权”（`optional_host_permissions`，同步某个网站的 Cookie 时再请求该网站的权限）。这需要改扩展代码，需要时告诉我。

### 2. Microsoft Edge 加载项

1. 打开 [Partner Center 的 Edge 页面](https://partner.microsoft.com/dashboard/microsoftedge)，用微软账号注册开发者（免费）。
2. **创建新扩展 → 上传** 同一个 zip。
3. 填写属性（类别选“生产力”）、商店列表和隐私政策链接（同上）。
4. 在“Notes for certification”里说明扩展只连接本机 `127.0.0.1`，测试需要先安装清影桌面版，并给出下载链接。
5. 提交，通常几个工作日审核。

### 3. Firefox 附加组件（AMO）

1. 打开 [Firefox 附加组件开发者中心](https://addons.mozilla.org/developers/)，用 Firefox 账号登录（免费）。
2. **提交新附加组件 → 在 Mozilla 上发布**（上架到商店），上传同一个 zip。扩展 ID 已写在 `manifest.json` 的 `browser_specific_settings.gecko.id`，不要改动，否则会被当成新扩展。
3. 源代码：扩展没有打包或压缩，不需要另外提交源代码。
4. 填写简介、分类、隐私政策链接和截图，提交。
5. 审核通过前，用户仍可以按 `desktop/extension/README.md` 手动加载。

### 以后更新扩展

改 `desktop/extension/manifest.json` 里的 `version`（必须比已上架的大），发布流水线会生成新 zip，再到各商店后台上传新版本。

---

## C. macOS 签名和公证

没有签名时，用户首次打开要在“系统设置 → 隐私与安全性”里点“仍要打开”，或执行 `xattr -cr`。签名并公证后就能直接打开。

### 前提

- 加入 [Apple Developer Program](https://developer.apple.com/programs/)（个人或组织，年费）。
- 一台 Mac（导出证书用，不需要长期使用）。

### 步骤

1. **创建证书**：登录 [developer.apple.com → Certificates](https://developer.apple.com/account/resources/certificates/list)，新建 **Developer ID Application** 证书（按页面说明在 Mac 的“钥匙串访问”里生成证书请求文件上传，再下载 `.cer` 双击安装）。
2. **导出 .p12**：钥匙串访问 → 我的证书 → 右键该证书 → 导出，格式选 `.p12`，设一个导出密码。
3. **转成 base64**：

   ```bash
   base64 -i certificate.p12 | pbcopy
   ```

4. **记下签名身份**：终端执行 `security find-identity -v -p codesigning`，复制形如 `Developer ID Application: 你的名字 (TEAMID)` 的整行。
5. **创建应用专用密码**：登录 [appleid.apple.com](https://appleid.apple.com) → 登录与安全 → 应用专用密码，新建一个。
6. **Team ID**：在 [developer.apple.com → Membership details](https://developer.apple.com/account) 里查看。
7. 在 GitHub 仓库 **Settings → Secrets and variables → Actions** 添加：

   | 名称 | 内容 |
   | --- | --- |
   | `APPLE_CERTIFICATE` | 第 3 步复制的 base64 文本 |
   | `APPLE_CERTIFICATE_PASSWORD` | 第 2 步的导出密码 |
   | `APPLE_SIGNING_IDENTITY` | 第 4 步的整行文字 |
   | `APPLE_ID` | Apple ID 邮箱 |
   | `APPLE_PASSWORD` | 第 5 步的应用专用密码（不是 Apple ID 登录密码） |
   | `APPLE_TEAM_ID` | 第 6 步的 Team ID |

8. 重新运行 `desktop-release` 工作流。流水线只会把已配置（非空）的 Secrets 传给打包工具，所以没配的时候不会报错；配好后会自动签名并提交公证（公证需要几分钟到几十分钟）。
9. 验证：下载 `.dmg` 安装后直接双击打开，不再被拦截。可以在终端用 `spctl -a -vv /Applications/ClearClip.app` 检查，显示 `accepted` 和 `source=Notarized Developer ID`。

---

## D. Windows 代码签名

没有签名时，Windows SmartScreen 会提示“Windows 已保护你的电脑”，用户点“更多信息 → 仍要运行”即可。签名后警告会减轻；普通（OV）证书仍需要积累下载量才能完全消除 SmartScreen 的提示，EV 证书或 Microsoft 的签名服务通常更快建立信誉。**这一项成本最高、也不是必须的，建议最后考虑。**

近几年 CA 机构要求代码签名证书的私钥必须放在硬件令牌或云端 HSM 中，所以**不能再简单地把 .pfx 文件放进 GitHub Secrets**。可选的做法：

| 方案 | 特点 |
| --- | --- |
| **Azure Artifact Signing**（原 Trusted Signing） | 微软的云签名服务，按月订阅，用 Azure 账号即可，适合 GitHub Actions；账号类型和地区有限制，申请时需要身份验证 |
| **Azure Key Vault 证书** | 证书存在 Azure Key Vault 的 HSM 里，用 Tauri 的 `signCommand` 调用签名工具 |
| **其他 CA 的云签名**（如 DigiCert KeyLocker、SSL.com eSigner） | 买证书时选云签名方案，按它们的文档接入 CI |
| **SignPath Foundation** | 面向开源项目的免费签名服务，需要申请和审核，项目要满足开源要求 |

### 接入方式（以 Tauri 的自定义签名命令为例）

Tauri 支持在 `desktop/src-tauri/tauri.conf.json` 的 `bundle.windows.signCommand` 里配置一个命令，打包时对每个要签名的文件调用，`%1` 代表文件路径。例如使用 Azure 的签名工具，大致形如：

```json
"bundle": {
  "windows": {
    "signCommand": "<签名工具> <参数…> %1"
  }
}
```

具体的命令、参数和需要在 Secrets 里配置的凭据（如 Azure 的 `AZURE_CLIENT_ID`、`AZURE_TENANT_ID`、`AZURE_CLIENT_SECRET`）取决于你选的方案，请照所选服务的官方文档和 [Tauri 的 Windows 签名文档](https://v2.tauri.app/distribute/sign/windows/) 配置。我没有办法在没有你的账号和凭据的情况下验证这一步，所以代码里没有预先写入 `signCommand`；你选好方案后告诉我，我可以帮你改配置和流水线。

---

## 配好之后怎么发布

1. 修改版本号（`desktop/src-tauri/tauri.conf.json`、`desktop/src-tauri/Cargo.toml`、`desktop/package.json` 三处保持一致），合并到 `main`。
2. 打开 GitHub 的 **Actions → desktop-release → Run workflow**，分支选 `main`。
3. 等四个平台都构建完成（约 15 分钟）。Release 会自动创建，包含各平台安装包、Windows 便携版、浏览器扩展 zip；配置了更新签名时还会有 `latest.json`。

## 官方文档

下面的页面以官方当前内容为准（Tauri 页面可切换到中文）：

- [Tauri：Windows 代码签名](https://v2.tauri.app/distribute/sign/windows/)（更新插件和 macOS 签名在同站的 Distribute 与 Plugins 栏目）
- [Chrome 应用店：用户数据与隐私政策要求](https://developer.chrome.com/webstore/user_data)
- [Chrome 应用店：Limited Use 限制](https://developer.chrome.com/docs/webstore/program-policies/limited-use/)
- [Chrome 应用店：开发者协议](https://developer.chrome.com/docs/webstore/terms/)
- Edge 加载项和 Firefox 附加组件的发布说明，请在各自的开发者中心（上文链接）内查看。
