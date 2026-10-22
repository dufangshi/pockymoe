# 核心测试地图

套件已按用户要求收口：只保留高影响行为。不要根据历史 release 文档恢复已删除的大矩阵、真实模型验收脚本或细节 UI 回归。新增测试围绕明确的高影响失败机制，优先复用现有入口。

## 浏览器：6 条核心链路

默认只跑 desktop-chromium。Relay spec 自建隔离 Relay 和 fake Supervisor，使用已构建的 apps/supervisor-web/dist；不需要真实模型凭据。

| 风险 | 文件 | 场景 |
| --- | --- | --- |
| 创建 workspace/thread、发送首条 prompt | e2e/phase2.spec.ts | receive a hello response |
| ACP 状态修复、队列跨刷新只执行一次；断线/后台挂起后运行中的回合补齐漏掉的步骤 | e2e/session-state-recovery.spec.ts | queued composer input survives reload；working turn shows the steps it missed |
| 两台设备、两个浏览器分别验证加密 | e2e/device-encryption-status.spec.ts | device locks are verified independently |
| 附件、WS、Origin、会话撤销 | e2e/relay-security.spec.ts | 文件内唯一场景 |
| 离线禁止降级、真实加密、多分块、防重放、公开快照 | e2e/relay-encryption.spec.ts | 两个场景按风险选择 |
| Relay 管理控制台：导航、分页、详情抽屉、断开隧道、管理操作日志 | e2e/relay-admin.spec.ts | 文件内唯一场景；用 `-c playwright.account-key.config.ts`，不启动本地 Supervisor/Vite |

从项目根目录运行：

```sh
E2E_API_PORT=19887 E2E_WEB_PORT=16173 \
  E2E_DATABASE_URL=.local/core-e2e.sqlite E2E_WORKSPACE_ROOT=.local/core-e2e \
  pnpm exec playwright test e2e/session-state-recovery.spec.ts --project=desktop-chromium

# 仅在 Relay/加密边界需要时运行
pnpm test:e2e:relay
```

pnpm test:e2e 现在默认选择 desktop。平时仍按文件/场景选择；验证整个保留集时才用该入口。需要触摸验证时用 pnpm exec playwright 加显式 mobile-chromium project，不重复所有纯 API 场景。

## 更便宜的检查

- Rust：cargo test --workspace。保留迁移/备份/原子回滚、鉴权加密、ACP 执行故障、更新恢复、存储排他锁、历史持久化、线程 CLI 投递。
- Web：pnpm --filter @pockymoe/supervisor-web test。仅设备加密、设备作用域管理权限、投递结果确认。
- Launcher/更新：pnpm npm:publish:test。下载校验、CLI 参数、数据库身份、不可变发布、升级失败回滚。
- Docker CLI：e2e/thread-interaction/inbox.py 保留 CLI/HTTP/存储串联；启动方式见同目录 README。真实模型历史验收不属于默认测试。

## 执行约束

保持主 skill 的隔离要求：覆盖高优先级数据库/工作目录环境变量，清除正式 Relay 配置，不对活动 Supervisor 运行恢复测试。纯测试删减不需要重建产品；产品代码变化才准备对应的新 binary/dist。

缺少必要场景时明确覆盖缺口，不用大量无关测试代替，也不把跳过或仅列出测试称为通过。
