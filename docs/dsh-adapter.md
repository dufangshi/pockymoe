# DSH（DeepSeek Harness）接入：bridge 架构、差异与路线图

适配基线为 npm `@deepseek-ai/dsh@0.2.0-rc.2`，源码对照 `dsh-v0.1.5-rc.1` 与 `0.2.1-alpha.1`。升级 DSH 时至少覆盖当前 latest 与下一个 alpha。

## 结论

DSH 本身是 Cordis 全插件平台，但它**刻意把 ACP 定为“仅自动化”协议**（[设计记录](https://github.com/deepseek-ai/deepseek-harness/blob/dsh-v0.2.0-rc.2/.agents/notes/implemented/simplification/2026-07-23-acp-automation-only-protocol.md)）：只有 model/reasoning 两个 config option、按步提交的整段消息、`kind:"other"` 的通用工具卡。没有流式输出、modes、commands、plan，也没有问答、插件管理或 fork/load。只靠 ACP 永远拿不到 DSH 的主要能力，不能指望上游在 ACP 里补。

可行方案是把“DSH 插件”做成 **adapter 级扩展 + Pockymoe 内置插件**，分四层：

1. DSH 侧 `deepseek-bridge.mjs`：用 DSH 官方 `--patch` 插入 ACP 所在的同一进程。
2. **原生组合**：同一进程再叠加已安装 DSH 自带的 Web bundle（`dsh-web-app` 的 `cordis.patch.yml` 与 `presets/*.patch.yml`）。这样 ACP 会话也有运行模式，并且进程里带着 DSH 自己的 Web 界面（原生控制台）。
3. Rust `acp/deepseek.rs`：负责 bridge 通道、组合发现和语义映射。
4. Pockymoe 内置插件 **DeepSeek Harness**（`remote-codex.deepseek-harness`）：
   - 在插件系统里注册一个 thread panel（kind `harness:deepseek`）。
   - DSH 线程的工作台侧栏（手机为顶栏）出现插件按钮，在工具抽屉里打开 `DshPluginPanel`。
   - 同一面板也出现在 `/harness` 对话框里。
   - 设置 → 插件 可以关闭它；本机没装 `dsh` 时显示为不可用。

bridge 是 DSH 的又一个 carrier：它通过 `ctx.typertGateway.invoke()` 调用 DSH 自己 Web 客户端使用的 `@Remote` 方法（`commands`、`goals`、`permissionPresets`、`pluginManager`、`llm`），通过 `agentPresets` 绑定运行模式，并订阅 DSH 自己的 session projection。它不改 DSH 内核。

插件系统是 Supervisor 的扩展点：

- Supervisor 有内置插件注册表（终端 + DeepSeek Harness），`/api/plugins` 的读取、启停和卸载保护都走这张表。
- thread-ui 的内置模块为每个插件贡献 thread panel。
- `MatterWorkbench` 新增 `toolPanels` 选项，把插件面板放进侧栏和工具抽屉。
- 插件合并时保留设备上报的可用性（例如未安装 harness）。

## DSH 与 Codex / Claude Code 的差异

| 维度 | Codex（ACP + app-server bridge） | Claude Code（ACP adapter） | DSH（`dsh --profile acp`） | 接入要点 |
|---|---|---|---|---|
| 内核 | 固定 Rust 内核 | 固定内核 + SDK | 一切皆 Cordis 插件；profile = bundle 叠加 + 用户 patch | 工具、persona、插件都可能随配置变化，能力必须运行时探测 |
| 协议面 | 完整交互：modes、commands、fork、goal | 交互式：modes、commands、ExitPlanMode | 仅自动化：只有 `set_config_option`（model/reasoning） | 交互能力只能走 bridge 调 DSH 自己的 API |
| 流式 | token 级 chunk | token 级 chunk | 每步提交后一次性整段发送 | bridge 转发 `agent/assistant-stream`，再与提交块对账 |
| 权限 | ACP mode / app-server policy | ACP mode | 进程启动时读 `DSH_PERMISSION_MODE`；会话级 `/permission` | 启动 env + 运行时命令，再读 projection 确认 |
| 计划模式 | mode | mode + ExitPlanMode | `/plan` 命令；`exit_plan_mode` 走 user-questions 审阅 | bridge 下发 `/plan`，并充当问题应答方 |
| 自治工作 | goal 由 Codex 驱动 | 后台任务 | `create_goal` 或 `/goal` 后在 ACP prompt **结束后**继续自动跑轮次 | 回合持续到 DSH 空闲；Stop 时同时暂停 goal |
| 会话 | load/fork（经 bridge） | load | 只有 list/resume/close；无 load/fork/回放；所有 profile 共享 `$DSH_HOME/sessions` | 历史由 Pockymoe 自己保存；fork 由 bridge 用 DSH 自己的种子会话实现 |
| 设置存储 | `CODEX_HOME` config | `CLAUDE_CONFIG_DIR` | volatile 字段整行写进 profile 的 `cordis.patch.yml`；acp 禁用 HMR，需重启；凭据在 `.credentials.yaml`（监听热更新） | 带 revision 写，标注“重连生效”，写前备份 |
| 模型 | 固定目录 | 固定目录 | 运行时 provider 注册（DeepSeek、pi-ai 的 40+ provider、自定义网关） | 可选项以 ACP 的 config options 为准；推理档位按模型逐个解析 |
| 工具展示 | 命令、diff、终端输出 | 富卡片 | 只有 `kind:"other"`、`title` = 工具名、`rawInput`、结果文本 | 尚未做：可由 bridge 转发工具的 presentation |
| 进程 | 每线程一个 | 每线程一个 | 每线程一个，空闲约 170 MB RSS | 进程级 env（权限）正好需要每线程独立进程 |
| 稳定性 | 稳定协议 | 稳定协议 | 公开 API 处于 pre-stable 阶段，0.1.5 → 0.2 已改模型目录和设置存储 | bridge 按服务逐项探测并带协议版本；任一功能缺失只降级该功能 |
| 隐私 | — | — | `session-log-deepseek.enabled` 默认把会话日志随请求上传；OTEL 默认 `FEEDBACK_ONLY` | 面板展示并可修改该开关 |

## bridge 架构

**启动顺序。** Rust 先在 `127.0.0.1:0` 监听并生成一次性 token，然后启动 `dsh --profile acp --patch <bridge>`，并按线程权限设置 `DSH_PERMISSION_MODE`。

原生组合的加载规则：

- 找到与 `dsh` **同一版本**的 `dsh-web-app` 时，先追加它自带的全部 patch。可以用 `POCKYMOE_DSH_WEB_APP` 指定位置。
- 再追加一份覆盖层，把 Web 主机限制为：OS 分配的回环端口、不打印 URL（stdout 是 ACP 帧）、不开浏览器、`trustedHosts` 为空。
- 原生组合启动失败时退回纯 acp profile，失败原因记入 `compositionError`。
- `POCKYMOE_DSH_NATIVE=0` 可以完全关闭原生组合。
- 实测常驻内存与纯 acp 相同（约 169 MB）。bridge 在 `appReady` 之后反向连接，发送 `hello` 快照（协议版本、DSH 版本、profile、各模型推理档位、权限预设、插件/bundle/provider 清单、功能开关）。Rust 收到 hello 之后才发 ACP `initialize`/`session/new`。原因是 ACP 会先于其他插件就绪，过早初始化会拿到不完整的 provider 目录。

**通道协议**（JSON lines，单行上限 2 MB）：

- Rust 调用：
  - `snapshot`：含 `runModes`，以及 `console`/`runModes` 功能位；
  - `session`：projection、全部命令、运行状态、`presetLocked`；
  - `invoke`：白名单 Remote；
  - `command`：会话里注册的任意命令，包括 DSH 插件的命令；未注册的命令报错；
  - `selectPreset`；
  - `console`：回环代理端口 + 带 DSH 启动 token 的路径；
  - `settings`、`updateSetting`。
- DSH 事件：
  - `stream`：attempt 的 start、text/reasoning delta、end，delta 按 40 ms 合并；
  - `status`：idle/running；
  - `projection`：plan/goal/todos/permissions/agentPreset 的 client view，与 DSH Web 同源；
  - `question` / `question-cancelled`。
- Rust 用 `answer` 回复问题。

**语义映射：**

- **模型：** ACP config options 是可选项的真相源。bridge 只补充每个模型自己的推理档位和 provider 名称。切换模型后列表重新计算。
- **上游模型事实：** 应用连接时，Pockymoe 把上游声明的每个模型（名称、上下文窗口、输入模态、推理档位）写进 pi-ai 路由的 `models`；上游说不推理的写 `reasoningEfforts: false`，`none` 写成 `off: none`。上游没声明的事实才用 Codex 模型目录补。规则见 [upstream-connections.md](upstream-connections.md#upstream-metadata)。
- **权限：** 产品的 sandbox 映射为 `read-only` / `workspace-write` / `danger-full-access`。每次应用前读 `permissions` projection，不一致才执行 `/permission`，执行后再读回校验。必须读回，是因为用户的 `permission.defaultPreset` 会覆盖启动 env。
  - profile 没有 permission 插件时，只有启动 env 与要求一致才放行，否则拒绝（fail closed），不会用更宽的沙箱运行。
  - DSH 对未注册的命令返回空结果，一律按错误处理。
- **plan：** `collaborationMode=plan` ↔ `/plan` / `/plan off`，pending 状态按“下一回合翻转”计算。审阅通过后，DSH 先退出 plan；运行时发出 `thread.collaboration.updated`，线程随之改回 default，避免下一回合又进入 plan。
- **问题：** DSH 的 user-questions 与产品现有的 native requestUserInput 结构一致。plan 审阅的正文放进问题文本。用户关闭问题时以错误回复 DSH；没有 active turn 时拒绝回答（fail closed）。
- **流式：** delta 先写入临时段落。提交块内容一致时不再发送；不一致时整段替换；失败的 attempt 标记为 failed，重试写入新段落。工具调用开始前清空待对账状态，保证不会重复。
- **回合：** 回合以 DSH 空闲为结束，而不是以 ACP prompt 返回为结束。
  - bridge 上报的忙碌状态覆盖根会话及其后台子代理：根空闲、子代理仍在跑时也算忙。
  - goal 激活期间的空闲判定窗口为 3 秒。
  - broadcast 丢事件时重新读取状态；bridge 断开视为空闲。
  - interrupt 先 `session/cancel`；有激活 goal 时再尽力执行 `/goal pause`。DSH 可能已自行暂停，那时失败也不影响停止结果。
  - 模式命令在全局会话锁之外执行，超时 20 秒。
- **goal / compact：** 产品的 goal 流程把 `/goal …` 作为一个回合执行；`set_goal` 映射为 edit/pause/clear；compact 走 `/compact`，不再发 prompt。
- **标题：** DSH 原本用第一条 prompt 给会话起名，而这条 prompt 带有 Pockymoe 的上下文前缀，所以控制台的会话列表里全是 `[remoteCodex: …`。
  - 现在每个回合开始前，如果线程标题有变化，bridge 调 `sessionTitle.rename` 把线程标题写给 DSH。
  - 这相当于用户手动命名：标题被固定，DSH 不再自动生成。
  - 同步尽力而为，超时 2 秒，失败不影响回合。

**面板动作：** `POST /api/threads/{id}/harness` 只接受类型化动作，不存在原样转发任意 RPC 的入口：

- `refresh`、`settings`、`updateSetting`（必须带 revision）；
- `setPluginEnabled`、`setBundleEnabled`；
- `stop`、`restart`；
- `selectRunMode`、`console`；
- `command`：`/plan` `/permission` `/goal` `/compact` 由线程控件负责，面板执行会被拒绝，防止状态失配；回合进行中也拒绝。

relay ACL 默认拒绝共享用户访问该路径（有回归测试）。`GET /api/agent-runtimes/acp/harness?agentId=deepseek&cwd=…` 返回新建线程用的运行模式目录，与模型目录一样属于可共享的只读元数据。

`restart` 在线程空闲时关闭该线程的 DSH 进程；下一次请求按已保存的 profile 恢复会话。旧的 `/disconnect` 接口不会重启进程。

**安全约束：**

- 只做白名单式导出：不导出插件 config、环境变量或凭据。
- 设置只接受 JSON 标量，拒绝 `!!` 开头的 YAML tag 和 `__jsExpr` 对象，因为它们会在 DSH 内执行代码。
- 应用级 bundle（base、acp-app、web-app、headless、sdk-app、sdk-minimal）锁定，防止 profile 无法启动。
- 每次写入前把 `cordis.patch.yml` 和 `package.json` 备份到 `profiles/<profile>/.remote-codex/backups/`，保留最近 10 份。

**面板：** `HarnessSettingsFields`（`/harness`）和插件的 `DshPluginPanel`（工作台抽屉）都渲染 `DshHarnessPanel`。插件面板在会话未运行时先发 `refresh` 启动会话；只读查看者不会触发启动。面板包含：

- 会话状态：权限预设、plan、goal 轮次、todos；回合外自治运行时显示 Stop。
- 运行模式：第一轮前可切换，之后锁定并说明原因。名称和说明与 DSH 一致：标准 / PTC / 极简 / 创造模式；自定义模式显示它自己的名字。
- 命令：会话里的全部命令（含 DSH 插件注册的），可带参数运行，并显示结果文本。线程负责的命令标“由线程控件管理”；`/export` 只在 Web 里有意义，标“在原生控制台中使用”。`/feedback` 会把会话历史上传给 DeepSeek，所以显示警告并要求确认。
- 原生控制台：在新标签页打开（见下节）。
- 配置文件设置：精选的 volatile 字段，带 revision 并发保护。
- 插件与 bundle 开关：乐观更新，失败回滚。保存后显示“重连后生效”，并提供“重连以生效”（`restart` 后 `refresh`）。只有新进程确实反映出来的更改才会被清除。
- 设置字段带类型；被拒绝的写入保留错误提示，并恢复为存储值。
- 模型提供方列表。

## 能力覆盖

| DSH 能力 | 状态 | 方式 |
|---|---|---|
| 模型/推理档位（含自定义 provider） | ✓ | ACP + bridge 补充 |
| sandbox/审批预设 | ✓ | env + `/permission` + projection |
| plan 模式与审阅 | ✓ | `/plan` + user-questions 应答方 |
| goal（显示、创建、暂停、清除、回合内自治） | ✓ | goal projection + `/goal` |
| compact | ✓ | `/compact` |
| 流式文本/推理 | ✓ | assistant-stream |
| todos → 产品 plan 面板 | ✓ | todos projection |
| 插件启停、功能 bundle 启停 | ✓（重连生效） | pluginManager |
| profile 精选设置 | ✓（本线程立即生效，其他线程重连后） | settings.mutate |
| provider 列表 | 只读 | llm |
| 运行模式（标准/PTC/极简/创造及自定义） | ✓ 新建线程与面板，首轮后锁定 | 原生组合 + `agentPresets` |
| 按会话的控件可用性 | ✓ | 极简模式没有 plan/goal/compact，线程隐藏对应控件 |
| 任意 DSH 命令（含插件命令） | ✓ | `commands/list` 校验后执行 |
| DSH 插件自带界面、模式编辑器、DSH 设置页等全部 Web 功能 | ✓ 原生控制台（新标签页） | 同进程 Web 主机 + 回环代理 |
| 凭据、provider 配置 | 原生控制台里可用；在 Pockymoe 的「上游」tab 中配置 | 计划只写入、不读取 |
| 插件安装/卸载 | 原生控制台里可用；Pockymoe 面板未做 | pnpm 长任务 |
| 会话标题 | ✓ 线程标题同步到 DSH | `sessionTitle.rename` |
| 工具富展示（diff、位置） | 未做 | bridge 转发 presentation |
| MCP 透传、子代理/后台任务面板 | 未做 | 目前 `mcpServers: []` |
| fork（最新轮次或任一已完成轮次） | ✓ 需原生组合 | bridge 种子会话，见 [acp-slash-fork-and-usage.md](acp-slash-fork-and-usage.md#fork-support) |
| load/回放 | DSH ACP 不支持 | — |

## 运行模式（preset）

DSH 的运行模式就是 agent preset：每种模式是一套工具与 persona 的组合。

| 模式 | 内容 |
|---|---|
| 标准 | 完整编码工具 |
| PTC | 标准工具加程序化工具调用：模型写 TypeScript 批量调用工具 |
| 极简 | 只有一个持久终端 |
| 创造 | 标准工具加 `tool-cordis`：可以编写 DSH 插件、界面和自定义模式 |

**实现。** 不再镜像 web-app 的禁用行，而是直接加载同版本 `dsh-web-app` 自己的 patch，所以不会与 DSH 版本漂移。bridge 在 `agent/created`（串行，会话发布前执行）里绑定模式：

- 新会话：执行 `select(启动 preset 或默认)`；
- 有记录的会话：执行 `recompose(记录值)`；
- 运行模式出现前就已开始的旧会话：执行 `recompose(默认)`；
- 子代理沿用父级的模式。

所选模式由 DSH 写进自己的会话日志，重启或恢复后仍然有效，Pockymoe 不需要另存。

**切换的约束。**

- 切换拿会话的操作锁，回合进行中拒绝，避免第一轮用到切换一半的工具集。
- 目标模式缺少线程正在用的计划模式或目标时（例如开着计划模式切到极简），立即切回原模式并报错，不会悄悄失去计划限制。
- 关闭计划模式时，如果会话没有 `/plan` 命令可以退出，也会报错，不再当作成功。

**锁定。** DSH 只允许在第一轮之前切换（`agent-preset/locked`）：

- `session.presetLocked` 由 turn-boundary projection 计算；
- Pockymoe 开始回合时立即标记为锁定，之后再读回确认。

**新建线程。** 选择 DeepSeek Harness 后显示「运行模式」：

- 列表来自模型探测时缓存的目录，已损坏的 preset 不显示。
- 只对设备 owner 显示；Relay 上的 harness 操作只限 owner。
- 创建线程后立即显式应用所选模式，包括默认模式，因为缓存里的默认值可能已经在原生控制台里被改过。应用失败时提示“线程已创建，但运行模式未生效”，提供重试和“打开线程”，不会重复创建线程。

**实测（真实 DSH 0.2.0-rc.2 + scripted provider）。**

| 模式 | 给模型的工具 |
|---|---|
| 极简 | 只有 `bash` |
| 标准 | `ask_user_question, bash, create_goal, edit, exit_plan_mode, get_goal, glob, grep, …` |

极简会话没有 `/plan`、`/goal`、`/compact`，`patch_capabilities` 按会话命令关闭对应控件。

最理想的上游改动仍是让 DSH ACP 用标准 `configOptions`（category `mode`）公布 preset 和权限预设。

## 原生控制台

**用途。** DSH 插件可以带自己的 Web 界面。DSH 还有很多原生功能没有 Pockymoe 对应物：preset 编辑器、插件管理与安装、provider/凭据设置、会话日志导出等。原生控制台让这些功能都能用。

**做法。** 原生组合让 DSH Web 主机跑在同一个 DSH 进程里，只监听回环地址。bridge 再开一个回环代理：

- 只接受 `localhost` / `127.0.0.1` / `[::1]` 的 Host；DNS rebinding 或公网名一律 403。
- 把 Host 改写成 DSH 的地址；只有同源请求才改写 Origin。跨源页面保留原 Origin，因此仍会被 DSH 自己的 Host/Origin 围栏拒绝。
- 鉴权完全沿用 DSH：打开带一次性启动 token 的地址，DSH 换发绑定 authority 的 `SameSite=Strict` cookie，再 303 跳到 `./`。
- 问题：从别的站点打开控制台时（relay 应用域名、另一个主机名），浏览器在这次 303 跳转上会丢掉 Strict cookie，用户看到“需要认证”。E2E 中实际复现过。
- 处理：代理把这一个登录 303 改成 200 页面，保留 DSH 的 `Set-Cookie`，页面再用 meta refresh 跳到 `./`。这次跳转由页面自己发起，属于同站，cookie 正常带上。其他响应原样转发。

**本地访问。** 控制台使用当前页面的回环主机名。从局域网 IP 访问 Supervisor 时控制台不可达，界面会说明原因。

**远程访问（relay）。** 走现有的仅 owner 端口预览：

1. 设备上的 Supervisor 为控制台端口建映射，标签为 `DSH console <线程前 8 位>`。DSH 进程退出后控制台端口不再监听，这类旧映射在下一次打开时删除，所以不会累积到设备的 32 个映射上限。
2. 用 owner 签名的 `POST /api/port-mappings/{id}/lease` 向设备申请预览租约（已绑定设备只在租约有效期内打开连接，见 [port previews](port-previews.md#bound-devices)；签名的控制台请求本身也返回 `leaseId`），再调用 `POST /relay/devices/{device}/port-mappings/{id}/open?lease=<leaseId> {path}`，在新标签页打开 `p-<id>.<base>`。

relay 会把 Host/Origin 改写成 `127.0.0.1:<端口>`，正好通过代理围栏；DSH 的跳转用相对路径。

已用本地 relay 做端到端验证（`e2e/dsh-console-relay.spec.ts`）：

- 起一个独立 relay，加一个连接它、运行真实 DSH 的 Supervisor。
- 应用页在 `127.0.0.1`，预览在 `p-<id>.preview.localhost`，两者跨站。
- 浏览器从线程页的插件面板打开控制台，经过 launch ticket 和 DSH 登录，加载出 DSH 界面；运行模式数据经预览的 WebSocket 取回。

**为什么不嵌入 iframe。** relay 当前对所有页面设置 `X-Frame-Options: DENY` 和 CSP `frame-ancestors 'none'`、`frame-src 'self' blob:`，主页面无法嵌入 `p-*` 源。要嵌入，需要为预览源放开 `frame-src` 并调整 `frame-ancestors`，这是安全策略决策，留给用户确认。

**注意。** 在控制台里直接对会话发消息，DSH 会照常运行，但这些回合不进入 Pockymoe 线程，面板会显示“DSH 正在回合外运行”。对话应继续在 Pockymoe 里进行。

## 保证不出错的原则

1. **可选项以协议为准，状态读回确认。** 模型以 ACP options 为准；权限、plan、goal 以 DSH projection 为准，不假设命令已生效。
2. **按功能降级。** bridge 对每个服务做存在性探测；hello 带协议版本；只有 hello 失败才拒绝会话，并给出升级提示。
3. **同一 profile 的多进程语义要讲清楚。** acp 没有 HMR，设置只在执行写入的进程立即生效，其他线程需要重连；revision 是进程内计数，写入一律带 CAS。
4. **不在 ACP 进程里开 HMR**：acp 行一旦重载，会断掉所有会话，并在同一 stdio 上挂第二个读者。应用 bundle 锁定，写前备份。
5. **凭据只写不读；永不导出配置。** pi-ai 的 headers 未标记为 secret，不能出现在任何视图里。
6. **外发数据先告知。** DSH 默认遥测模式是 `FEEDBACK_ONLY`：提交 `/feedback` 时会上传会话历史。面板对 `/feedback` 显示警告并二次确认；测试环境一律设置 `DSH_TELEMETRY_DISABLED=1`。
7. **自治工作必须在回合内，或者可见、可停止。**
8. **问答没有 active turn 时 fail closed。**
9. **无密钥回归**：用 scripted provider 驱动真实 DSH 做端到端测试；升级 DSH 时至少覆盖当前 latest 与下一 alpha。

## 路线图

- **P1**
  - 决定是否放开 iframe 嵌入（relay 安全策略）。
  - 设备级 DSH 设置页：用一个短生命周期的“管理 bridge 进程”，不依赖某个线程；重连前用 `--dump-config` 预检；恢复备份。provider/凭据并入「上游」tab。
  - 工具富展示，修正图片能力对应的默认模型，MCP 透传，子代理与后台任务面板。
- **P2**
  - 插件安装/卸载：pnpm 长任务，含进度、registry 选择（DSH 内置 npmmirror 回退）、版本兼容检查和重启编排。
  - 向上游争取：ACP 的 mode/permission config options、流式 chunk、`_meta` 中的工具展示。

## 测试

无需模型密钥：用 scripted provider 驱动真实 DSH（`dsh` 需在 PATH 上），`DSH_HOME` 与工作区都必须是隔离的绝对路径。

```sh
node --test scripts/dsh-bridge.test.mjs
E2E_REAL_DSH=1 E2E_DSH_SCRIPTED=1 E2E_DSH_HOME=/absolute/isolated-dsh-home \
  E2E_API_PORT=18875 E2E_WEB_PORT=15179 E2E_WORKSPACE_ROOT=/absolute/isolated-workspaces \
  pnpm exec playwright test e2e/dsh-bridge.spec.ts e2e/dsh-console-relay.spec.ts --project=desktop-chromium
```

Playwright 配置在真实 DSH 运行时设置 `DSH_TELEMETRY_DISABLED=1`。真实模型的 `e2e/dsh.spec.ts` 需要 DeepSeek 或其他 provider 凭据。
