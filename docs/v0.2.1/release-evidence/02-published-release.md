# 02 — v0.2.1 发布核验

2026-10-17 UTC。**发布完成，七个公开资产已下载并独立核验。**

- [Release v0.2.1](https://github.com/yanch123/xyzaishell/releases/tag/v0.2.1)，Release ID 415692392，发布时间 2026-10-17 09:06:45 UTC。
- 状态：公开、draft=false、prerelease=true、非 Latest；未配置代码签名。
- main／dev 已合入激活提交 `6eb80a4fc4f28e4af403ac28a1f965320c03f04f`。本发布记录提交只更新文档，不改变发布标签或资产。
- 标签 `v0.2.1` 精确指向 `a1a060c9bc7cff25437ae03d75ec3a37f64b29dd`，tree `4724bfa43e803165aa504d67fdfddc018482e4b4`。
- [发布运行 39018580188](https://github.com/yanch123/xyzaishell/actions/runs/39018580188)，attempt 1：预检查、12 个完整候选 CI 作业、发布作业均成功。

## 最终资产

以下为公开 Release 实际下载文件摘要；与 GitHub 资产元数据、六个有效载荷的 SHA256SUMS 记录逐项一致。不得使用早期开发／候选构建的摘要替代。

| 文件 | 字节数 | SHA-256 |
| --- | ---: | --- |
| [build-linux-x64.json](https://github.com/yanch123/xyzaishell/releases/download/v0.2.1/build-linux-x64.json) | 1107 | `a73e895bc42c99ddf18723af8b5e6ea84796887403824dd9adf4b71f388fe867` |
| [build-windows-x64.json](https://github.com/yanch123/xyzaishell/releases/download/v0.2.1/build-windows-x64.json) | 1132 | `dc1253bf97c71c4c60b69fb3446f27a42871238650fc009f4ceae33706bc48b7` |
| [SHA256SUMS.txt](https://github.com/yanch123/xyzaishell/releases/download/v0.2.1/SHA256SUMS.txt) | 563 | `e46c336e30f41f3d628ddc186516626319c63686c554890b4d1898797085018f` |
| [xyzaishell-0.2.1-linux-x64.deb](https://github.com/yanch123/xyzaishell/releases/download/v0.2.1/xyzaishell-0.2.1-linux-x64.deb) | 12759000 | `7f6a5a26a5147976e3f19fb65f89ed218344d418ed232ad378138dad0f27bf18` |
| [xyzaishell-0.2.1-source.tar.gz](https://github.com/yanch123/xyzaishell/releases/download/v0.2.1/xyzaishell-0.2.1-source.tar.gz) | 1680261 | `84ca9f8f0e3960ce0d9232c00bfa238cf2327ff0e804f53125e78df86b7c9c96` |
| [xyzaishell-0.2.1-source.zip](https://github.com/yanch123/xyzaishell/releases/download/v0.2.1/xyzaishell-0.2.1-source.zip) | 1958856 | `d35b58389f35d4a45ab6d58f79ba41e1b99ab4997dd4d1d25df4a880582d6a89` |
| [xyzaishell-0.2.1-windows-x64.exe](https://github.com/yanch123/xyzaishell/releases/download/v0.2.1/xyzaishell-0.2.1-windows-x64.exe) | 7002733 | `f6baadecbc1c88f93b0d180258d9eaf31efd1ee93de990499210f369346ebbaa` |

## 来源与原生证据

两平台 build JSON 的 source_commit、版本、run／attempt 与锁文件摘要均核对一致。两种源码归档中的 377 个 Git 文件逐字节和执行位核对通过，包含被测 README 和应用源码；不混入后续 activation 的证据文档与 intent 内容。

三平台原生 ZIP 均核对下载摘要，15 份生命周期 result.json 全部通过。首次安装验证五类／协议对账、LLM 取消重试与缩减、LLM→聊天→Quick→聊天及完整摘要保持；三平台实际 v0.1.4→0.2.1 升级保留数据，恢复模型请求为 0。

- [Ubuntu 22.04 原生证据](https://github.com/yanch123/xyzaishell/actions/runs/39018580188/artifacts/12095758459)
- [Ubuntu 24.04 原生证据](https://github.com/yanch123/xyzaishell/actions/runs/39018580188/artifacts/12096665401)
- [Windows Server 2022 原生证据](https://github.com/yanch123/xyzaishell/actions/runs/39018580188/artifacts/12096341831)

## 保留限制

未签名预发布，不设 Latest。Windows 10／11 实机与完整用户桌面仍 pending；合成模型与原生 fixture 不证明真实供应商摘要语义质量、安装后 GUI 外部 SSH、输入法／剪贴板／Wayland、长期压力或原生多页归档覆盖。详见 Release 说明及 [候选验收记录](01-candidate-acceptance.md)。公开后没有覆盖旧标签或旧资产。
