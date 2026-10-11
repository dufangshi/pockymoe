# Relay 账户安全与密文传输：迁移和恢复

## 登录体验

普通账户在 Account → Security，管理员在 Admin → Security 设置安全因素。邮箱或用户名＋密码登录均支持二步验证。

1. Authenticator app → Set up，使用 Google Authenticator 等应用扫码，输入第一枚六位验证码确认启用。无法扫码时可展开手工密钥。
2. 启用后立即下载或复制恢复码，另存到可信位置。每枚只能使用一次，重新生成会作废所有旧码。
3. 可添加并命名 Passkey，使用浏览器原生 WebAuthn。当前 Passkey 是密码之后的第二因素，不是免用户名／密码的登录入口。取消系统弹窗可继续验证码或恢复码。
4. 新浏览器验证后默认信任 30 天，可取消勾选。信任依赖随机 HttpOnly 凭据和浏览器环境；浏览器版本升级、切换 Wi-Fi／蜂窝网络不会仅因 IP 变化要求重验。清除 Cookie、换浏览器或到期需重新验证。
5. 改密码、修改验证器、撤销信任、轮换设备 token 等安全变更要求 10 分钟内的强验证。普通密码登录和被信任的浏览器不会自动获得近期强验证资格。

可信浏览器和会话可逐项撤销；关联会话随浏览器撤销失效，已打开的 WebSocket 也会断开。恢复码登录不会自动信任浏览器，并撤销已有信任和旧会话。启用／停用因素、修改密码等敏感变更会使其他会话失效。OAuth 登录也遵守该账户的 MFA 设置，不能仅因 OAuth 回调成功就绕过。

## Relay 部署配置与备份

- `POCKYMOE_PUBLIC_BASE_URL` 固定为实际 HTTPS origin，例如 `https://remote.lnz-study.com`，用于 Cookie、Origin 验证、WebAuthn RP 和 OAuth callback。不要使用客户端传入的 forwarded-host 决定信任 origin。
- `POCKYMOE_RELAY_SESSION_SECRET` 至少 32 字符，使用密码学随机值。未配置时，在持久的 `POCKYMOE_RELAY_DATA_DIR/session-secret` 自动创建随机值；Unix 权限 0600。禁止使用临时目录或每次部署重生成。
- 同时备份 relay 数据库 `relay-store.sqlite` 和上述 secret（环境变量形式也必须备份）。SQLite 使用一致性备份，不能只复制正在写入的主文件而漏掉 WAL。
- **secret 同时派生 TOTP 和设备安装凭据的存储加密密钥。丢失／直接替换它会使这些凭据无法解密。** 迁移机器应恢复原 secret；不要把常规改密码当成更换 master secret。数据库和 secret 均按机密材料保存，分离访问权限。
- 首次升级时，如果旧 session secret 不足 32 字符，必须先配置新的随机值，否则启动会明确拒绝；此时尚未登记本版本的 TOTP。启用 MFA 后不再直接替换 master secret，应保留原值迁移。
- 更新后旧的无服务端会话记录 JWT 会要求一次重新登录。新 Cookie 有 Secure（HTTPS）、HttpOnly、SameSite 属性；Web 不再将长期 bearer 保存在 localStorage。原生客户端可使用显式 Authorization bearer，但也受服务端撤销约束。
- 设备凭据继续兼容原值的哈希验证，安装凭据加密保存；设备所有者可从 Device 菜单按需复制安装命令。Replace device token 仍要求强验证，显式替换时旧连接立即失效；将新值配置到对应 supervisor 后重连。该操作不会删除 thread。
- 清空 token 列不是对 SQLite 历史页／备份的安全擦除。若怀疑旧数据库泄露，应轮换设备 token，而不是只依赖字段迁移。

丢失手机时使用恢复码或已登记 Passkey 登录，重新设置因素并生成新恢复码。如果同时丢失全部因素和恢复码，UI 不提供绕过验证的入口；需要可信运维恢复流程。此版本不提供邮件重置 MFA 的捷径，也不应直接删除 session-secret“修复”登录。

## 设备身份与更换

Supervisor 在其数据库同目录保存 `<数据库文件名去扩展名>.transport-identity`，Unix 权限 0600。身份密钥跨重启保持，临时 HPKE 解密密钥每小时轮换，只存在进程内存。备份／迁移 supervisor 时保留身份文件。

设备菜单或 thread 顶部的加密状态图标可查看 SHA-256 指纹。需要独立核对时，在**设备本机**执行：

```sh
pockymoe relay-fingerprint --database /path/to/supervisor.sqlite
```

也可使用与运行 supervisor 相同的 `DATABASE_URL`／配置。该命令只读取身份，不启动服务。首次使用前可能尚未生成身份文件，应先让更新后的 supervisor 建立连接。

浏览器首次使用固定身份公钥。已固定设备身份变化会明确报错，不自动覆盖；确认是自己的设备更换、并通过本机可信渠道核对新指纹后，才能在状态面板重置信任。设备仅重启时身份不变，过期临时密钥的 GET 会重新协商一次；修改类请求不会自动重发，避免重复执行。

## 密文协议与性能约束

采用 [RFC 9180 HPKE](https://www.rfc-editor.org/rfc/rfc9180)：P-256 DHKEM、HKDF-SHA256、AES-256-GCM；Rust `hpke` 与浏览器 `@hpke/core`／WebCrypto 互操作。身份签名绑定浏览器随机挑战、公钥、key ID、服务端时间和期限，防止重放旧公钥描述。

HTTP 方法、授权路径和资源 ID 是可见且经过 AEAD 绑定的路由元数据；query、私有正文和响应内容加密。HTTP 每请求独立 HPKE 上下文，响应密钥通过 exporter 派生，只用一次；真实 HTTP status 与响应头在认证后的加密包内。请求时间窗和有界 request ID 集合拒绝重放。

WebSocket 一次握手后复用分方向 AES-GCM 密钥，以单调序号构造 nonce，认证 type／thread／shell 路由字段；terminal 按键和 agent 流式事件不重复执行非对称运算。Service Worker 覆盖图片、raw 文件和下载，普通 API fetch 和 WebSocket 使用同一协议。

密文无法被 relay 或 CDN 压缩，因此在加密前协商压缩：浏览器支持 `DecompressionStream` 时在加密 metadata 内声明 `accept-encoding: gzip`；Supervisor 对至少 1 KB 的文本／JSON 响应先 gzip 再加密，并在加密 metadata 中标 `"encoding":"gzip"`，图片、压缩包等二进制类型、分片下载（`streamNext`）和 204／304 不压缩。建立 WebSocket 会话的加密请求同样携带声明；该会话内至少 1 KB 的服务端事件先 gzip，外层 `encrypted.encoding` 设为 `gzip`，并改用 AAD 标签 `rcd-ws-gzip-v1`，relay 去掉或添加该标记都会导致认证失败。旧浏览器不声明、旧 Supervisor 不标记，双方都退回未压缩，互相兼容。

大响应按 1 MiB 拉取分片，最多 16 个活动流；每流最多缓存一个可重试分片，120 秒空闲后释放，文件通过流读取，避免将整个下载读进内存。继续下载 URL 绑定原 thread／workspace 权限，分片同样加密。单连接消息队列 64 条，单设备并行转发 32 个，relay 待响应请求总数有界；超载明确断连／报错，不静默丢消息。AES-GCM 每加密包附 16 字节认证标签；HTTP HPKE encapsulated key 为 65 字节，另有固定头／metadata。内部 JSON 隧道仍有原本的 Base64 编码开销，不能把本机 CPU 测试当成公网吞吐保证。

## 兼容和真实安全边界

- 老 supervisor 可显示未启用加密状态并继续既有兼容路径；一旦浏览器固定该设备的加密身份，就拒绝静默降级。要获得保护，需更新 relay Web 和设备 supervisor 两端。
- 旧原生客户端／显式兼容 API 仍可明文调用，受原有账号／资源授权约束。本版本未删除设备所有者能力、未引入工作区白名单、沙箱或逐命令审批。共享 terminal 的新最小 scope 校验要求更新 supervisor，旧设备失败时应升级，不能放宽越权校验。
- Relay 看得到账户、授权、设备和资源 ID、消息种类、长度与时序。
- 公开分享由浏览器投影用户消息和最终回复后显式发布，公开快照及其图片本来就是公开内容，由 relay 明文保存。分享不再要求 relay 读取整个私有 transcript。HTML 导出保持浏览器渲染流程。
- **本方案不能阻止已经控制 relay 的攻击者替换它提供的网页 JavaScript，也不能阻止仍有设备控制能力的 relay 自行发起获授权命令。** 首次公钥固定也依赖可信首次连接／独立指纹核对。因此这次的加密保护转发内容，不等于把 relay 从设备信任边界中移除。进一步设备权限限制按用户要求暂缓。

## 审计

`relay_security_events` 保留最近 10,000 条安全事件：账号 ID、事件名、资源 ID、时间。记录登录／挑战、安全设置、会话与浏览器撤销、设备 token 轮换，不记录密码、OTP、token、私有正文或原始 User-Agent。审计数据库仍应仅供可信运维读取；不是无限期合规日志。



## 设备安装命令与旧凭据恢复

设备所有者可在 Devices 菜单反复复制 macOS/Linux 或 Windows 安装命令。列表和共享响应不包含 token；点击时通过所有者专用 `POST /relay/devices/:id/setup-token` 获取永久 device token，响应禁止缓存。不会为复制而轮换凭据或关闭设备连接。

安装凭据存于 `relay_device_setup_tokens`，使用 session secret 派生的独立 AES-256-GCM 密钥加密，设备 ID 绑定为认证数据。备份需同时保留数据库和 session secret。认证仍校验 token 哈希。

旧安全迁移错误清空过 `relay_devices.token`。残存明文会迁移到加密存储；原设备重连时可恢复已通过认证的原凭据。若已经只剩哈希且尚未恢复，首次复制会为原设备创建可重复使用的补充凭据，原哈希保持有效，设备 ID、工作区与历史不变。显式 Replace token 会同时撤销原凭据和补充凭据。删除设备也使两者失效。
