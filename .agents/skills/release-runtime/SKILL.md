---
name: release-runtime
description: 排查 Pockymoe 原生安装与更新，固定提交并验证、发布不可变 GitHub runtime release；Windows Device Manager 独立发布。
---

# Pockymoe 原生更新与发布

GitHub Releases 是 runtime 的权威版本。不要发布新的 runtime/npm 包；npm 目录只保留旧版本兼容和历史测试。参考 [安装与迁移](../../../docs/github-runtime.md)。

## 排查安装与更新

先比较实际 CLI 路径、`pockymoe version`、设备管理 API 的 installed/running/latest 版本及 job/log。Unix 用 `type -a pockymoe` 和 `~/.local/bin/pockymoe version`；Windows 用 `Get-Command pockymoe -All`。CLI 版本不等于后台正在运行的版本。

旧 npm 安装需通过相同设备 setup 命令完成一次迁移，保留配置、SQLite 和设备身份。运行中的设备优先使用 Settings 对应的 device-scoped Check/Update API；更新 worker 必须独立运行、暂停和恢复受本次维护影响的线程。API 不可用或不能完成迁移时才手动分离恢复，不盲目 kill 或降级已迁移数据库。

## 固定内容与检查

记录 runtime 候选提交、稳定版本和共享 UI 的完整已发布 SHA。共享 UI origin 必须为 `dufangshi/pockymoe-thread-ui-rust`。若别人还在修改 checkout，用独立 worktree，不能把未授权的其它改动顺手发布。

核对 GitHub 已有版本，使用 `node scripts/set-version.mjs VERSION` 更新 root/Cargo 版本并同步 Cargo.lock。所有四个平台必须同一不可变版本；不要改 Windows Device Manager 版本或 seed。它的独立 release 必须 `--latest=false`，保留 runtime latest alias。

按最终 diff 一次选择相关 crate/测试名、fmt/check、SH/bootstrap/打包脚本及 Web 检查。Web E2E 遵循 focused-e2e，显式 spec/project、隔离数据库和环境。工作流用 actionlint。普通修复不跑全 workspace/全浏览器/跨平台矩阵；已授权的真实 runtime 发布才执行其 release gates。已经通过且源码未变的检查不重复跑。检查通过后在各受影响仓库提交相关文件。

## 发布与部署

共享 UI 先 push，runtime ref 必须已指向记录的提交：

```sh
gh workflow run runtime-release.yml --ref PUSHED_REF \
  -f channel=latest -f thread_ui_sha=FULL_PUBLISHED_UI_SHA
gh run watch RUN_ID --interval 30 --exit-status
```

`dry-run` 只在用户明确要求或已授权发布需要验证 job/artifact 链路时调用。保存唯一 run ID/headSha，只保留一个 watch 进程，工具单次等待不超过 60 秒。失败先读日志，不盲目重跑或覆盖同版本资产。

verify、四平台 native、固定 UI 的 Web 构建并行，package 依赖全部成功后才能 release。核对 GitHub 版本、四平台资产、Web ZIP、setup.sh/setup.ps1、runtime-version.txt 和 SHA256SUMS，下载代表平台验证真实版本。GitHub 查询失败不等于尚未发布。没有 npm publish job。

公开网页及 setup 脚本是 GitHub Pages 静态站点（https://app.lnz-study.com）：main 上 Web 路径、setup 脚本或 `thread-ui.sha` 的推送（或手动 dispatch）触发 `pages-deploy.yml`，用 `thread-ui.sha` 中的共享 UI 完整 SHA 构建并部署。共享 UI 提交须先推送到其 main；Release 资产可用后，确认主仓 main 的 `thread-ui.sha` 是同一完整 SHA，不是则提交并推送。Relay（https://remote.lnz-study.com）只提供 API；Relay 代码有变更时从 main dispatch `relay-deploy.yml`，它只部署 Relay，没有 `thread_ui_sha` 输入。重启设备 Supervisor 不会更新公开网页。按用户已有授权执行发布/部署，技能不额外扩大授权。

需要当前设备更新时通过设备管理 API，或旧版迁移所需的 setup 兼容桥。核对 job completed、运行版本/PID/Relay 重连和受维护线程恢复。交付提交、版本、run 链接、相关验证及真实限制；不要把安装成功等同于后台已更新。
