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

打开 `https://relay.example.com`，注册账号，然后按[快速上手](../README.zh-CN.md#快速上手)继续。

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

用新版本重复第 1 步，然后重启服务。设备在网页的设置里自行更新；Relay 提供的是与它自身版本一致的安装命令。

### 旧账户和未绑定的设备

还没有账户密钥的账户（旧账户）照常可用。它用密码登录一次后，必须完成「Secure your account」：设置一个至少 12 个字符的密码并保存恢复码。完成之前，Relay 只允许会话、退出登录和设备相关的请求。使用 Google 或 GitHub 登录的账户改为设置一个解锁密码。Relay 管理员账户用配置的密码登录，没有账户密钥。

未绑定的设备照常可用，但不受保护（「Not protected · Relay trusted」），直到所有者点「Protect this device」并在设备上运行显示的 `pockymoe bind` 命令。设备需要运行支持绑定的版本。

## 重置密码

Relay 只把每个用户的账户密钥保存在用其密码和恢复码加密的 vault 里。任何能访问服务器的人（包括你）都无法恢复它。

在管理面板重置用户密码，会设置一个至少 12 个字符的临时密码。它只能使用一次，用户若设置了二步验证仍需通过，并且旧密码不能再登录。用户在登录页选择「Use an admin reset password」，然后输入恢复码和新密码。没有恢复码时只能用新账户密钥重新开始，并在每台设备本机重新绑定（先 `pockymoe bind --reset`，再「Protect this device」）。对尚未完成保护的账户，重置设置的是普通密码，用户随后用它完成账户保护。

## 备份

备份整个数据目录，里面有 `relay-store.sqlite` 和 `session-secret`。这个 secret 还用来加密已保存的二步验证和设备凭据，丢失或替换后这些数据就无法解密。SQLite 请用一致性备份，不要直接复制正在写入的数据库。

数据库里还有加密的账户 vault。数据库丢失后，用户必须重新开始，并重新绑定所有设备。恢复的备份可能包含较旧的 vault，已经见过更新 vault 的浏览器会拒绝它（「The relay served an older account vault」）。更多说明见 [Relay 安全运维](relay-security-operations.zh.md)。

## 独立的 Web origin

默认情况下，Relay 在 `POCKYMOE_PUBLIC_BASE_URL` 上同时提供网页和 API。提供网页的 Relay 也能修改网页代码，而这段代码会处理你的密码和账户密钥。要去掉这个能力，可以把网页作为静态文件放到另一个 origin，并让 Relay 只提供 API：

```sh
POCKYMOE_PUBLIC_BASE_URL=https://relay.example.com   # API、OAuth 回调、设备连接
POCKYMOE_WEB_ORIGIN=https://app.example.com          # 静态网页
```

`POCKYMOE_WEB_ORIGIN` 必须是不带路径的 HTTPS origin（只有 `localhost` 可以用 HTTP），默认等于 `POCKYMOE_PUBLIC_BASE_URL` 的 origin。设置后 Relay 会：

- 用 Web origin 做 WebAuthn：登录 passkey 的 RP ID 是它的主机名。passkey 绑定 RP ID，所以如果用户已经在当前主机上注册过 passkey，应让网页留在这个主机、把 API 移到新主机；
- 对修改类请求和 WebSocket 升级的 `Origin` 检查只接受 Web origin；
- 在 `/relay/*` 上只对这个 Web origin 返回 CORS，允许携带凭据并带 `Vary: Origin`。预检允许网页使用的方法和请求头（`content-type`、`accept`、`range`、`authorization`、`if-none-match`、`x-remote-codex-auth-realm`、`x-rcd-*`），响应暴露网页需要读取的头。其他 origin 得不到 CORS 头，预检返回 403；
- OAuth 登录完成后把浏览器重定向回 Web origin；OAuth 回调地址仍在 `POCKYMOE_PUBLIC_BASE_URL` 上；
- 两个 origin 不同时不再提供任何 HTML 或脚本：只保留 `/relay/*`、`/healthz`、`/supervisor/tunnel` 和端口预览的数据通道 `/supervisor/port-stream/*`，其他 `GET`、`HEAD` 请求（包括 `/setup.sh`）重定向到 Web origin 的同一路径，其他方法返回 404，也不需要 `POCKYMOE_RELAY_WEB_DIST_DIR`。

Relay 的 Cookie 仍然只属于 Relay 主机（host-only），带 `Secure`、`HttpOnly` 和 `SameSite=Lax`。只有两个 origin 属于同一站点（同一个可注册域名，例如 `app.example.com` 和 `relay.example.com`）时，浏览器才会在网页发出的请求中带上它们。开发时可以用 `localhost` 的不同端口。

浏览器密钥和每个浏览器已核对过的账户数据保存在 Web origin 的存储里。把网页移到另一个 origin 后，每个浏览器都要作为新浏览器重新登录。

### 构建静态网页

在按 [development](development.md) 准备好的源码 checkout 中（共享 UI 位于 `pockymoe-thread-ui/`）：

```sh
VITE_POCKYMOE_RELAY_ORIGIN=https://relay.example.com \
  pnpm --filter @pockymoe/supervisor-web build:static
```

这个构建在编译时选定 relay 模式，不需要内联脚本。`apps/supervisor-web/dist/` 中除了网页本身还有：

- `_headers`：CSP 的 `connect-src` 只允许 Web origin、Relay origin 及其 `wss:` 主机；`/assets/*` 带 `Service-Worker-Allowed: /`，`index.html` 带 `no-cache`；
- `_redirects`：SPA 回退规则 `/* /index.html 200`；
- `setup.sh` 和 `setup.ps1`，因为设备安装命令从 Web origin 下载它们；
- `STATIC_HOSTING.txt`，上述要求的摘要。

Cloudflare Pages 等托管服务会直接应用 `_headers` 和 `_redirects`；其他托管服务需要换成对应的响应头和重写规则。请通过 HTTPS 在 Web origin 上提供这个目录。不设置 `VITE_POCKYMOE_RELAY_ORIGIN` 时，构建结果访问自身 origin。

### 已安装的设备

设备会连接安装时保存的 Relay 地址上的 `/supervisor/tunnel`。如果网页接管了设备安装时使用的主机，需要在该主机上把这个路径继续转发到 Relay（例如用代理或 CDN 规则），否则这些设备会离线。Devices 页面复制的安装命令使用 Relay origin，并通过 `--web-origin` 传入 Web origin。
