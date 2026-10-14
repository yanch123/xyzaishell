# 10 发布准备与第三方分发证据

本记录对应 2026-10-14 的 M4/M5 开发工作树；不是最终 v0.1 验收结论。最终提交、锁文件或平台变化后必须重跑。项目自身许可保持原状，本记录没有选择或新增项目 LICENSE；是否另行采用开源许可由作者决定，也不将该选择作为本次开发包的额外阻塞项。

## 1. 真实发布门槛

| 待完成事项 | 需要保留的证据 |
| --- | --- |
| 最终版本代码验收 | `06-testing.md` 的平台、SSH、模型、凭据、取消/恢复结果，绑定最终 SHA；不能用旧阶段测试或浏览器 mock 替代 |
| 安装包运行 | 最终 DEB/NSIS 安装、启动、升级/重装、卸载；记录系统版本、桌面会话、WebView/Secret Service 状态。Windows Server 构建不等于 Win10/11 桌面，Xvfb 不等于真实桌面 |
| 第三方分发材料确实入包 | 本文生成器通过；安装后的包中能阅读第三方文本、Rust 标准库告知和对应 MPL 源码。仅有 CI artifact 或开发机生成目录不算入包验证 |
| 内容与来源一致 | tag、源码、安装包、通知/源码资源、校验和、CI SHA 一致；不混入旧产物 |

现有 `.github/workflows/release.yml` 只准备 draft。发布前填入这些结果和已知限制；未完成项不能改写为通过。本记录未创建 tag、合并分支或发布 Release。

## 2. 已落地的通知与源码生成

`docs/v0.1/tools/prepare_third_party.py` 用 Python 3.10+ 标准库生成安装包资源，已接入 `src-tauri/tauri.conf.json` 的 beforeBuildCommand、beforeDevCommand 和 bundle.resources。生成目录 `src-tauri/resources/third-party/` 被 Git 忽略；没有把二进制源码档案或数 MB 生成文本提交进 Git。

```sh
# 从仓库根目录运行；需要 npm ci 已完成、Cargo/Rust 在 PATH。
python docs/v0.1/tools/prepare_third_party.py
# 已有完整锁定缓存时可离线复验。
python docs/v0.1/tools/prepare_third_party.py --offline
# 可单独验证 Windows 材料；这不等于 Windows 二进制或安装验收。
python docs/v0.1/tools/prepare_third_party.py --offline \
  --target x86_64-pc-windows-msvc --output /tmp/xyz-windows-notices
python -m unittest discover -s docs/v0.1/tools -p 'test_*.py' -v
```

构建前先 `cargo fetch --locked`，随后离线生成清单。目标取显式参数、Tauri/Cargo target 环境变量或 `rustc -vV` 的 host。直接运行原生 `cargo check/clippy/test/build` 时，应先运行此生成器；`tauri dev/build` 的 before 命令会自动准备。

输出：

- `THIRD-PARTY-NOTICES.txt`：实际目标 normal dependency tree 的许可、版权和嵌套 NOTICE/来源文本，以及六个 npm 非 dev 包。排除 build/dev/proc-macro-only 路径，不把测试工具和 HTML/CSS 编译工具塞入运行时通知。
- `runtime-inventory.json`：精确版本、锁定 checksum/integrity、原始许可声明、文本 SHA256、补充文本来源、Cargo 包内 VCS 信息、目标和 rustc 版本。
- `sources/option-ext-0.2.0.crate`：本次唯一运行时 MPL-2.0 候选的完整 registry 源码包，校验和必须与 Cargo.lock 相等。通知内给出相对路径和稳定的精确版本下载 URL；`.crate` 可作为 tar.gz 解包。
- `RUST-STANDARD-LIBRARY.html`：从所用 Rust toolchain 复制 library-only 版权/第三方许可告知，记录 hash，不把整个 rustc/Cargo 开发工具清单当成运行时依赖。

资源映射到安装目录中的 `third-party/`；具体 DEB 安装路径仍以打包结果为准。生成器只覆盖带自身标记的输出目录，不会覆盖任意用户目录。找不到已审核的许可文本、补充文本 hash/VCS/crate checksum 不符、缺 MPL 源码或 WebView2 loader 与审核版本不符时构建失败，不静默生成不完整通知。更新依赖后应重新核对补充文本和 vendored 组件；不使用最新 main 的 LICENSE 冒充锁定版本。

本次双平台生成、重复生成逐字节一致性、覆盖保护、通知 hash 和 MPL 源码 hash 检查通过。Linux 生成 354 个条目（348 Cargo + 6 npm），Windows 297 个（291 Cargo + 6 npm）。这些是目标正常依赖图；不宣称每个函数都经链接器保留，也不冒充符号级二进制 SBOM。

## 3. 许可证据与本次实际修复

### npm 与普通 Rust 依赖

npm 非 dev 锁定条目为：`@tauri-apps/api 2.12.3`（Apache-2.0 OR MIT）、`@xterm/xterm 5.5.0`、`@xterm/addon-fit 0.10.0`、`react 19.2.0`、`react-dom 19.2.0`、`scheduler 0.27.0`（后五项 MIT）。这六项本地原始许可文本均已收集。

核心 `russh 0.64.1` 为 Apache-2.0，`rig-core 0.44.0`、`rig-reqwest 0.44.0` 为 MIT。`rusqlite 0.37.0` / `libsqlite3-sys 0.35.0` 是 MIT wrapper，启用 `bundled`；SQLite 自身为 public domain。crate 内存在 `sqlcipher/LICENSE` 不代表启用了 SQLCipher，保守保留上游文本也不作此声明。[SQLite 官方说明](https://www.sqlite.org/copyright.html)

`ring 0.17.14` 声明 Apache-2.0 AND ISC，必须保留顶层 LICENSE 指向的多个来源文本；`aws-lc-sys 0.46.0` 有复合许可及内嵌第三方代码。生成器收集嵌套许可/NOTICE，不将 AND 简化成任选 MIT。Unicode-3.0 数据文本也随适用运行时依赖保留。

### 已补齐的缺少独立 LICENSE 包

补充材料在 `docs/v0.1/tools/license-overrides/`，`sources.json` 记录 exact-version crate checksum、包内 VCS commit、原始下载 URL、原文 hash，不猜测版权人：

- `rig-http 0.44.0`、`rig-reqwest 0.44.0`：Rig commit `2d1071e0e0c602f994e9d6cfd8cf5faea6f53650` 的原始 MIT LICENSE。
- `alloc-stdlib 0.3.0`：commit `0a81fd6928ea3b33c8cd484aa4575d50ffb98012` 的原始 BSD LICENSE。
- `dlopen2 0.8.2`：commit `cc80e4a0a90d499b677fdf7743699b4b3a43a989` 的原始 MIT LICENSE。
- `webview2-com 0.39.1` / `webview2-com-sys 0.39.1`：commit `edc2caf886175ccaebe86078c9cfe1ae2a187328` 的 wrapper 原始 MIT LICENSE。
- `russh 0.64.1`、`russh-cryptovec 0.62.0`、`pageant 0.2.4`：精确包遗漏独立文本，从各自 VCS commit 的仓库根 `LICENSE.txt` 补齐 Apache-2.0 原文。`russh-util 0.52.0` 的包/对应仓库根未取得独立文本，按精确包的 Apache-2.0 声明补入 Apache 官方原文。保留实际源文件中存在的版权声明，未凭 authors 字段编造新的 copyright。[Apache 2.0 原文及第 4 条](https://www.apache.org/licenses/LICENSE-2.0)

原始声明仍原样保留，不替作者替换第三方许可选择。缺少独立文本的 build-only 包没有被误判为应用分发阻塞。

### MPL 范围

全锁图含 `cssparser`、`cssparser-macros`、`dtoa-short`、`selectors`、`option-ext` 五个 MPL 包，但 `cargo tree -p xyzaishell --target … --edges normal,no-proc-macro` 将前四个排除为编译路径；只将运行时 `option-ext 0.2.0` 的文本/对应源码入包。这避免把全 workspace feature 联合图误当成最终运行时图。MPL 对应源码获取说明随包提供，并不意味着整个 xyzaishell 项目采用 MPL。[Mozilla 分发说明](https://www.mozilla.org/en-US/MPL/2.0/FAQ/)

### Microsoft WebView2 loader

Windows wrapper 内实际附带微软 loader，只有 wrapper 的 MIT 文本不完整。本次额外核实：

- exact wrapper commit 的更新脚本指定 Microsoft.Web.WebView2 **1.0.3800.47**。
- 从微软 NuGet 精确版本包获取 `LICENSE.txt`（微软 BSD 条款）与 `NOTICE.txt`，保留原文。
- crate 内 x64 的 `WebView2Loader.dll`、`WebView2Loader.dll.lib`、`WebView2LoaderStatic.lib` 与该 NuGet 包对应文件逐字节一致。
- `webview2-sdk.json` 记录 SDK 包 hash、三种 loader hash、来源脚本 URL 和通知 hash；Windows 生成时再次检查，不只凭包名推断许可。

这些 SDK loader 文本并不代替浏览器 WebView2 Runtime 的分发条款；当前应用安装器按 bootstrapper 路径获取 runtime，没有把整个固定 runtime 当作自己的内嵌依赖。

## 4. 额外审计材料（不直接入安装包）

```sh
python docs/v0.1/tools/dependency_inventory.py --output /tmp/xyz-license-evidence
```

输出路径须不存在；工具离线生成全 workspace 原始证据：617 个 Cargo 外部包 + 6 个 npm 非 dev 包、两份锁文件、许可文本及 hash、normal target 分类、VCS 信息、原始 MPL 源码包。全图 46 项缺少标准文件名文本只是审计提醒，不等于 46 项违规，更不直接混入运行时包。最终分发由 `prepare_third_party.py` 过滤并补齐。

扫描仍有边界：源码内非标准版权头、链接器实际裁剪和将来增加的 native vendored 成分需随升级检查；系统动态库由目标系统包管理器提供，不把构建机所有 GTK/WebKit/测试包重打包。依赖及工具链升级后复跑现有测试并核查产物；已实现具体当前缺口，不宣称永久自动完成法律判断。

## 5. Windows 签名与运行前提

- 当前配置/工作流无 Authenticode 证书。可以提供明确标注的未签名测试包；不能称已签名、已建立可信发布者或保证没有 SmartScreen。SHA256 校验和不等于代码签名。签名身份/服务由作者决定，凭据不进代码/日志。[Tauri 签名指引](https://v2.tauri.app/distribute/sign/windows/)
- `webviewInstallMode` 未显式覆盖，按 Tauri 当前默认下载 bootstrapper 流程验收。缺 runtime 的机器可能需要网络；至少记录已有 runtime、缺 runtime 在线、无网络三种实际结果。不擅自改成固定 runtime 分发。[Tauri Windows installer](https://v2.tauri.app/distribute/windows-installer/)
- 若以后附带固定 runtime，应重新核查微软 runtime 分发要求、更新责任及随包文本。[Microsoft WebView2 分发说明](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution)

## 6. Linux 的具体运行前提

- DEB 在 Ubuntu 22.04 构建，分别验证 Ubuntu 22.04/24.04 实际桌面，不泛称所有 Linux 支持。检查生成包 `Depends`、安装后 `ldd` 无缺库。Tauri 使用 GTK3/WebKitGTK 4.1 runtime，构建机装了 `*-dev` 不等于目标机运行完整。[Tauri Debian 指引](https://v2.tauri.app/distribute/debian/)
- M4 Secret Service 需要桌面用户 D-Bus 会话和可用 provider。无 provider、钥匙串锁定、用户拒绝解锁应给可恢复错误且不回退明文；普通配置/历史按设计单独验收。
- `.deb` 按实际链接/功能补运行依赖，不仅因 Rust crate 名称就添加 `libdbus` 或强制某个钥匙串实现。隔离 provider 自动测试不替代实际桌面会话验收。
