# 自己部署 Relay

[English](self-host-relay.md)

Relay 就是你登录的那个网站。它把浏览器和你的设备连起来，并提供设备的安装命令。Relay 只负责转发：对话、文件和终端内容在浏览器与设备之间端到端加密。绑定到你的账户密钥的设备还会拒绝你的浏览器没有签名的命令，见 [Account key trust](account-key-trust.md)。

需要准备：

- 一台能从公网访问的 Linux 服务器（x64 或 ARM64，glibc 2.28 及以上）；
- 一个指向它的域名，例如 `relay.example.com`；
- 在 Relay 前面加一层 HTTPS。登录 Cookie 带 `Secure` 属性，浏览器只在 HTTPS（或 `localhost`）下接受。

## 1. 下载 Relay

Relay 和设备端是同一个 `pockymoe` 可执行文件，放在一个目录里就能运行：

```sh
mkdir -p ~/pockymoe-relay && cd ~/pockymoe-relay
repo=https://github.com/dufangshi/pockymoe/releases
version=$(curl -fsSL $repo/latest/download/runtime-version.txt)
asset=remote-codex-linux-x64-gnu   # ARM 服务器用 remote-codex-linux-arm64-gnu
curl -fLO $repo/download/v$version/$asset
curl -fLO $repo/download/v$version/remote-codex-web.zip
curl -fLO $repo/download/v$version/SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS
install -m 0755 $asset pockymoe
rm -rf web && mkdir web && (cd web && unzip -q ../remote-codex-web.zip)
```

## 2. 配置

新建 `~/pockymoe-relay/relay.env`，并设为仅自己可读（`chmod 600 relay.env`）：

```sh
# 管理员账号，首次启动时创建。
POCKYMOE_ADMIN_USERNAME=admin
POCKYMOE_ADMIN_PASSWORD=choose-a-long-password
POCKYMOE_ADMIN_EMAIL=you@example.com

# 这个 Relay 实际对外的 HTTPS 地址。
POCKYMOE_PUBLIC_BASE_URL=https://relay.example.com

POCKYMOE_RELAY_HOST=127.0.0.1
POCKYMOE_RELAY_PORT=8788
POCKYMOE_RELAY_DATA_DIR=/home/you/pockymoe-relay/data
POCKYMOE_RELAY_WEB_DIST_DIR=/home/you/pockymoe-relay/web
```

可选配置：

| 变量 | 作用 |
| --- | --- |
| `POCKYMOE_GOOGLE_OAUTH_CLIENT_ID`、`POCKYMOE_GOOGLE_OAUTH_CLIENT_SECRET` | 显示「使用 Google 继续」。在 Google Cloud 创建 OAuth 客户端，回调地址填 `https://relay.example.com/relay/auth/oauth/google/callback`。 |
| `POCKYMOE_GITHUB_OAUTH_CLIENT_ID`、`POCKYMOE_GITHUB_OAUTH_CLIENT_SECRET` | 使用 GitHub 登录，回调地址 `…/relay/auth/oauth/github/callback`。 |
| `POCKYMOE_RELAY_REGISTRATION_ENABLED=false` | 关闭公开注册。默认开放注册。 |
| `POCKYMOE_RELAY_REGISTRATION_PASSWORD` | 注册时需要填写邀请码。 |

注册、审核和登录方式之后也可以由管理员在 `https://relay.example.com/relay-admin` 修改。

## 3. 作为服务运行

```ini
# ~/.config/systemd/user/pockymoe-relay.service
[Unit]
Description=Pockymoe relay
After=network-online.target

[Service]
WorkingDirectory=%h/pockymoe-relay
EnvironmentFile=%h/pockymoe-relay/relay.env
ExecStart=%h/pockymoe-relay/pockymoe relay
Restart=always

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now pockymoe-relay
loginctl enable-linger "$USER"   # 退出登录后继续运行
curl -fsS http://127.0.0.1:8788/healthz
```

## 4. 加上 HTTPS

任何支持 WebSocket 的反向代理都可以。用会自动申请证书的 [Caddy](https://caddyserver.com)：

```caddyfile
relay.example.com {
    reverse_proxy 127.0.0.1:8788
}
```

打开 `https://relay.example.com`，注册账号，然后按[快速上手](../README.zh-CN.md#快速上手)继续。这样部署的 Relay 自己提供网页；如果不想让 Relay 掌握网页代码，就把网页单独托管（见[独立的 Web origin](#独立的-web-origin)）。

## Docker

在本仓库的 checkout 里，`Dockerfile.relay` 会从源码构建 Relay 和网页：

```sh
docker build -f Dockerfile.relay -t pockymoe-relay .
docker run -d --name pockymoe-relay --restart unless-stopped \
  -p 127.0.0.1:8788:8788 -v pockymoe-relay:/var/lib/remote-codex-relay \
  --env-file relay.env pockymoe-relay
```

容器里不需要 `POCKYMOE_RELAY_HOST`、`POCKYMOE_RELAY_DATA_DIR` 和 `POCKYMOE_RELAY_WEB_DIST_DIR`，镜像已经设置好。

## 更新

用新版本重复第 1 步，然后重启服务。使用独立的 Web origin 时，还要用同一版本重新构建并发布静态网页。设备在网页的设置里自行更新；安装脚本安装的是最新发布的版本。

### 尚无账户密钥的账户和未绑定的设备

账户、二步验证、设备、共享和 thread 都会保留。还没有账户密钥的账户第一次普通登录会失败，这时选择「Continue with this password」（这是密码唯一一次发给 Relay），随后在「Secure your account」设置一个至少 12 个字符的密码并保存恢复码。完成之前，Relay 只允许会话、退出登录和设备相关的请求；升级前已有的会话也受同样限制，并会被要求重新登录。创建 vault 时 Relay 会删除旧的密码哈希。使用 Google 或 GitHub 登录的账户照常登录，然后设置一个解锁密码。Relay 管理员账户在 `/relay-admin` 用配置的密码登录，没有账户密钥。

未绑定的设备照常可用，但不受保护（「未保护」，信任 Relay），直到所有者保护它：在 Devices 页面点「Protect all devices」，或点「Protect this device」并在设备上运行显示的 `pockymoe bind` 命令，或重新运行它的安装命令。设备需要运行支持绑定的版本。逐步说明见 [Migration](account-key-trust.md#migration)。

发布的 `remote-codex-web.zip` 提供这个升级。所有账户都完成保护后，设置 `VITE_POCKYMOE_LEGACY_UPGRADE=0` 从源码构建网页（例如下面的[静态构建](#构建静态网页)）即可去掉它。此后仍没有 vault 的账户需要先[重置密码](#重置密码)才能登录并完成保护。

## 重置密码

Relay 只把每个用户的账户密钥保存在用其密码和恢复码加密的 vault 里。任何能访问服务器的人（包括你）都无法恢复它。

在管理面板重置用户密码，会设置一个至少 12 个字符的临时密码。它只能使用一次，用户若设置了二步验证仍需通过（在受信任的浏览器上也一样），并且旧密码不能再登录。用户在登录页选择「Forgot password?」→「Admin password」，然后输入恢复码和新密码。没有恢复码时只能用新账户密钥重新开始，并重新绑定每台设备（先在设备上运行 `pockymoe bind --reset`，再「Protect this device」）。对尚未完成保护的账户，同样的登录会直接进入「Secure your account」；去掉了安全升级的网页构建也可以这样做。

## 备份

备份整个数据目录，里面有 `relay-store.sqlite` 和 `session-secret`。这个 secret 还用来加密已保存的二步验证和设备凭据，丢失或替换后这些数据就无法解密。SQLite 请用一致性备份，不要直接复制正在写入的数据库。

数据库里还有加密的账户 vault。数据库丢失后，用户必须重新开始，并重新绑定所有设备。恢复的备份可能包含较旧的 vault，已经见过更新 vault 的浏览器会拒绝它（「The relay served an older account vault」）。更多说明见 [Relay 安全运维](relay-security-operations.zh.md)。

## 独立的 Web origin

Relay 有两种运行方式：

- **单一 origin**（默认）：Relay 从 `POCKYMOE_RELAY_WEB_DIST_DIR` 提供网页，并和 API 一起放在 `POCKYMOE_PUBLIC_BASE_URL` 上。这种方式最简单，但 Relay 因此也能修改网页代码，而这段代码会处理你的密码和账户密钥（见[威胁模型](account-key-trust.md#threat-model)）。
- **独立的 Web origin**：网页是另一个 origin 上的静态站点，Relay 只提供 API。公共 Pockymoe 服务就是这样运行的：网页在 GitHub Pages 上（`https://app.lnz-study.com`），Relay 在 `https://remote.lnz-study.com`。

第二种方式要在 Relay 上同时设置两个 origin：

```sh
POCKYMOE_PUBLIC_BASE_URL=https://relay.example.com   # API、OAuth 回调、设备连接
POCKYMOE_WEB_ORIGIN=https://app.example.com          # 静态网页
```

`POCKYMOE_WEB_ORIGIN`（也接受旧名称 `REMOTE_CODEX_WEB_ORIGIN`）必须是不带路径的 HTTPS origin（只有 `localhost` 可以用 HTTP），默认等于 `POCKYMOE_PUBLIC_BASE_URL` 的 origin。设置后 Relay 会：

- 用 Web origin 做 WebAuthn：登录 passkey 的 RP ID 是它的主机名。passkey 绑定 RP ID，所以在其他主机上注册的 passkey 将不能再用；
- 对修改类请求和 WebSocket 升级的 `Origin` 检查只接受 Web origin；
- 在 `/relay/*` 上只对这个 Web origin 返回 CORS，允许携带凭据并带 `Vary: Origin`。预检允许网页使用的方法和请求头（`content-type`、`accept`、`range`、`authorization`、`if-none-match`、`x-remote-codex-auth-realm`、`x-rcd-*`），响应暴露网页需要读取的头。其他 origin 得不到 CORS 头，预检返回 403；
- OAuth 登录完成后把浏览器重定向回 Web origin；OAuth 回调地址仍在 `POCKYMOE_PUBLIC_BASE_URL` 上；
- 两个 origin 不同时不再提供任何 HTML 或应用脚本：只保留 `/relay/*`、`/healthz`、`/supervisor/tunnel` 和端口预览的数据通道 `/supervisor/port-stream/*`，其他 `GET`、`HEAD` 请求（包括 `/setup.sh`）重定向到 Web origin 的同一路径，其他方法返回 404，也不需要 `POCKYMOE_RELAY_WEB_DIST_DIR`。它唯一还会返回的脚本，是用来注销 Relay 以前提供的网页所注册的 service worker 的小 worker（见[旧的 Web origin](account-key-trust.md#old-web-origin)）。

Relay 的 Cookie 仍然只属于 Relay 主机（host-only），带 `Secure`、`HttpOnly` 和 `SameSite=Lax`。只有两个 origin 属于同一站点（同一个可注册域名，例如 `app.example.com` 和 `relay.example.com`）时，浏览器才会在网页发出的请求中带上它们。开发时可以用 `localhost` 的不同端口。

设备上的文件经 Web origin 的 service worker 到达页面，它只加密页面自己发出的请求：`fetch` 调用和子资源，例如 Markdown 文件里的图片、聊天照片和文件浏览器的图片预览。打开 Relay 地址的框架或标签页属于 Relay origin 上的导航，这个 worker 看不到，Relay 也拒绝被嵌入框架。因此网页对浏览器本会直接导航打开的预览（例如 PDF）改为经加密 API 读取，再用 `blob:` 地址显示（`frame-src 'self' blob:`）；框架里没有 PDF 查看器的浏览器（例如 Android 版 Chrome）改为显示**打开**和**下载**按钮。

浏览器密钥和每个浏览器已核对过的账户数据保存在 Web origin 的存储里。把网页移到另一个 origin 后，每个浏览器都要作为新浏览器重新登录。已有用户和设备的 Relay 要迁移，请按[迁移到独立的 Web origin](account-key-trust.md#moving-to-a-separate-web-origin)操作。

### 构建静态网页

在按 [development](development.md) 准备好的源码 checkout 中（共享 UI 位于 `pockymoe-thread-ui/`）：

```sh
VITE_POCKYMOE_RELAY_ORIGIN=https://relay.example.com POCKYMOE_WEB_DOMAIN=app.example.com \
  pnpm --filter @pockymoe/supervisor-web build:static
node scripts/check-static-web.mjs   # 设置同样的两个变量
```

`POCKYMOE_WEB_DOMAIN` 是网页的主机名，必须设置。这个构建在编译时选定 relay 模式，不需要内联脚本。加上 `VITE_POCKYMOE_LEGACY_UPGRADE=0` 可去掉安全升级（「Continue with this password」）。`apps/supervisor-web/dist/` 中除了网页本身还有：

- `index.html` 里以 `<meta http-equiv>` 标签写入的内容安全策略（CSP），包含响应头版本中除 `frame-ancestors` 以外的全部指令，以及 `no-referrer` 的 referrer 策略；
- `404.html`：`index.html` 的副本，在 GitHub Pages 等托管服务上作为 SPA 回退；
- 位于站点根目录的 `relayServiceWorker.js`，因此作用域是 `/`，不需要 `Service-Worker-Allowed` 响应头；
- `_headers`：CSP 的 `connect-src` 只允许 Web origin、Relay origin 及其 `wss:` 主机，`script-src` 只额外允许 `'wasm-unsafe-eval'`（密码派生用 WebAssembly 运行 Argon2id），并设置 `frame-ancestors 'none'`；另有 `X-Content-Type-Options: nosniff`、`Referrer-Policy: no-referrer`、`X-Frame-Options: DENY`，以及 `index.html` 和 service worker 的 `no-cache`；
- `_redirects`：SPA 回退规则 `/* /index.html 200`；
- `CNAME`：GitHub Pages 的自定义域名；
- `setup.sh` 和 `setup.ps1`，因为设备安装命令从 Web origin 下载它们；
- `STATIC_HOSTING.txt`，上述要求的摘要。

Cloudflare Pages 等托管服务会直接应用 `_headers` 和 `_redirects`；其他托管服务需要换成对应的响应头和重写规则。GitHub Pages 两者都不应用：它使用 `CNAME` 和 `404.html`，页面只靠 meta 标签和应用自身的 frame 检查保护（缺少哪些保护见[静态托管](account-key-trust.md#static-hosting)）。请通过 HTTPS 在 Web origin 上提供这个目录。不设置 `VITE_POCKYMOE_RELAY_ORIGIN` 时，构建结果访问自身 origin。

本仓库用 `.github/workflows/pages-deploy.yml` 发布公共网页。fork 可以沿用它：设置仓库变量 `POCKYMOE_RELAY_ORIGIN`，修改工作流中的 `POCKYMOE_WEB_DOMAIN`，在 `thread-ui.sha` 中固定共享 UI 的提交，并把 Pages 的发布来源设为 GitHub Actions。每次发布会保留之前构建的资源 72 小时（`scripts/carry-web-assets.mjs`），发布前已打开的页面因此仍能正常加载。

### 已安装的设备

设备会连接安装时保存的 Relay 地址上的 `/supervisor/tunnel`。如果网页接管了设备安装时使用的主机，需要在该主机上把这个路径继续转发到 Relay（例如用代理或 CDN 规则），否则这些设备会离线。Devices 页面复制的安装命令使用 Relay origin，并通过 `--web-origin` 传入 Web origin。
