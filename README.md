# Gemini Rust Gateway

一个不依赖 Python 运行时的原生 Rust Gemini Web 网关，提供 OpenAI 兼容聊天接口、流式输出、图片生成和持久化异步任务。

> [!IMPORTANT]
> 本项目不是 Google 官方 SDK，依赖 Gemini Web 的非公开协议，Google 更新网页协议后可能需要同步适配。请仅在你有权使用的账号和环境中运行，并遵守相关服务条款。

## 特性

- Rust 2024、Axum、Tokio 异步运行时
- OpenAI 风格的 `/v1/chat/completions`
- OpenAI Responses API `/v1/responses`，支持 Codex 风格输入和流式事件
- Gemini API `/v1beta/models/*:generateContent` 与 `streamGenerateContent`
- OpenAI/Gemini function calling 兼容转换
- SSE 流式聊天响应
- 图片生成兼容接口 `/v1/images/generations`
- 图片、视频、音频、音乐、深度研究和 Canvas 异步任务
- 有界队列、并发限制、超时与任务取消
- SQLite WAL 任务持久化、分页查询、旧 JSON 自动迁移和过期产物清理
- Prometheus 文本指标、健康与就绪检查
- 使用 `wreq`/BoringSSL 模拟浏览器 TLS 指纹
- 内嵌 Web 控制台，无需 Node.js，可直接测试聊天、图片和任务
- Gemini Web BL/会话元数据自动刷新、有界指数退避重试
- 多账号隔离、`/u/0`/`/u/1` 路径、健康轮换与故障转移
- Chrome Cookie 同步扩展（支持定时自动同步）和 `HTTPS_PROXY`/`ALL_PROXY` 环境变量
- Windows DPAPI Cookie 加密、独立管理令牌和账号后台健康恢复

## 快速开始

### Windows：下载即用

从 GitHub Releases 下载 `gemini-rust-gateway-windows-x86_64.zip`，解压后运行：

```powershell
.\start.ps1
```

首次启动可隐藏输入 Cookie，也可以直接按 Enter，启动后通过 Chrome 扩展自动同步。程序会生成本地 API Key 并保存配置，后续运行不再重复填写。

也可以直接使用命令：

```powershell
.\gemini-rust-gateway.exe init
.\gemini-rust-gateway.exe doctor --online
.\gemini-rust-gateway.exe serve
```

### Linux：预编译包

下载 `gemini-rust-gateway-linux-x86_64.tar.gz` 并解压：

```bash
./gemini-rust-gateway init
./gemini-rust-gateway doctor --online
./gemini-rust-gateway serve
```

### Docker Compose

先使用预编译二进制或源码执行一次 `init`，然后运行：

```bash
docker compose up -d --build
```

容器部署也支持完全通过环境变量或 Secret Manager 注入配置，不强制要求 `.env` 文件。

### 从源码构建

需要 Rust 1.85 或更高版本。

```bash
cargo run -- init
cargo run --release -- serve
```

`init` 会创建 `.env` 和 `.env.auth.local`。登录凭据只进入被 Git 忽略的 `.env.auth.local`。

Linux：

```bash
sudo apt-get update
sudo apt-get install -y build-essential cmake ninja-build clang libclang-dev perl golang
cargo run --release -- serve
```

Windows 源码构建前可检查依赖：

```powershell
.\scripts\check-build.ps1
.\build.ps1 run -Release init
.\build.ps1 run -Release serve
```

脚本会依次查找 `$env:GEMINI_MSYS2_ROOT`、`$env:MSYS2_ROOT`、用户目录下的 `.cargo\msys64` 和 `C:\msys64`。

默认监听 `http://127.0.0.1:8000`。除健康检查外，请求需要携带：

```text
Authorization: Bearer <GEMINI_API_KEY>
```

启动后直接打开 [http://127.0.0.1:8000](http://127.0.0.1:8000)。首次在控制台填写本地 API Key 后，可选择保存在浏览器中；Google Cookie 不会进入前端。

## 配置命令

```text
gemini-rust-gateway init [--force] [--non-interactive]
gemini-rust-gateway doctor [--online]
gemini-rust-gateway serve
```

- `init`：隐藏输入 Cookie，自动生成 API Key，并持久化本机配置。
- `init --non-interactive`：从环境变量读取凭据，适用于自动部署。
- `doctor`：检查配置、数据目录和监听端口。
- `doctor --online`：额外连接 Gemini，验证 Cookie 是否真实可用。
- 不带子命令与 `serve` 等价，兼容原来的启动方式。

配置优先级为：进程环境变量 > `.env.auth.local` > `.env` > 内置默认值。

常用兼容性配置：

```dotenv
GEMINI_AUTH_USER=0
GEMINI_ADMIN_TOKEN=change-me-admin-token
GEMINI_ACCOUNTS_FILE=gateway_data/accounts.local.json
GEMINI_MAX_ACCOUNT_CONCURRENCY=1
GEMINI_ACCOUNT_COOLDOWN_SECONDS=60
GEMINI_ACCOUNT_PROBE_SECONDS=60
GEMINI_TASK_RETENTION_HOURS=168
GEMINI_RETRY_ATTEMPTS=3
GEMINI_RETRY_DELAY_MS=800
# 六种异步任务默认全部启用；显式设置会覆盖默认值，未列出的类型将被关闭
GEMINI_ENABLED_GENERATIONS=image,video,audio,music,deep_research,canvas
# 未设置时自动读取 HTTPS_PROXY/ALL_PROXY；设为 direct 可明确禁用代理
GEMINI_PROXY=
GEMINI_COOKIE_SYNC_ENABLED=true
```

`GEMINI_RETRY_ATTEMPTS` 只作用于可能因重试而改变结果的失败，例如上游错误、超时和网络故障。参数错误、认证失败以及没有可用账号属于确定性失败，会立即返回，不消耗重试预算。

## API 示例

聊天：

```bash
curl http://127.0.0.1:8000/v1/chat/completions \
  -H "Authorization: Bearer change-me-local-key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gemini-3-flash","messages":[{"role":"user","content":"你好"}]}'
```

流式聊天只需在请求体中添加 `"stream": true`。

Responses API：

```bash
curl http://127.0.0.1:8000/v1/responses \
  -H "Authorization: Bearer change-me-local-key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gemini-3-flash","input":"你好","stream":false}'
```

Gemini 原生 API：

```bash
curl "http://127.0.0.1:8000/v1beta/models/gemini-3-flash:generateContent" \
  -H "x-goog-api-key: change-me-local-key" \
  -H "Content-Type: application/json" \
  -d '{"contents":[{"role":"user","parts":[{"text":"你好"}]}]}'
```

Chat、Responses 和 Gemini `functionDeclarations` 均可传递函数定义。网关使用严格 JSON 提示转换 Gemini Web 文本输出，返回对应的 `tool_calls`、`function_call` 或 `functionCall`；函数由客户端执行，再把结果传回下一轮。

图片生成：

```bash
curl http://127.0.0.1:8000/v1/images/generations \
  -H "Authorization: Bearer change-me-local-key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gemini-3-flash","prompt":"A matte red cube on a white background","n":1}'
```

提交异步任务：

```bash
curl http://127.0.0.1:8000/v1/generations \
  -H "Authorization: Bearer change-me-local-key" \
  -H "Content-Type: application/json" \
  -d '{"type":"image","prompt":"A cinematic orange cat astronaut"}'
```

使用返回的任务 ID 查询：

```bash
curl http://127.0.0.1:8000/v1/tasks/<task_id> \
  -H "Authorization: Bearer change-me-local-key"
```

更多调用方式见 [`examples/client.rs`](examples/client.rs) 和 [`examples/client.ts`](examples/client.ts)。

## 端点

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/` | 内嵌 Web 控制台 |
| `GET` | `/health`、`/healthz` | 健康检查 |
| `GET` | `/readyz` | Gemini 会话就绪检查 |
| `GET` | `/metrics` | Prometheus 文本指标 |
| `GET` | `/v1/models` | 模型列表 |
| `GET` | `/v1/capabilities` | 网关能力列表 |
| `GET` | `/v1/accounts` | 脱敏账号状态和轮换信息 |
| `POST` | `/v1/chat/completions` | 普通或流式聊天 |
| `POST` | `/v1/responses` | OpenAI Responses API |
| `GET` | `/v1beta/models` | Gemini API 模型列表 |
| `POST` | `/v1beta/models/{model}:generateContent` | Gemini API 内容生成 |
| `POST` | `/v1beta/models/{model}:streamGenerateContent` | Gemini API SSE 内容生成 |
| `POST` | `/v1/images/generations` | 同步等待图片生成 |
| `POST` | `/v1/generations` | 提交异步生成任务 |
| `GET` | `/v1/tasks` | 分页查询最近任务，可按状态过滤 |
| `GET`/`DELETE` | `/v1/tasks/{id}` | 查询或取消任务 |
| `GET` | `/files/...` | 访问生成产物 |
| `POST` | `/admin/cookies/sync` | 验证、热更新并持久化本地 Cookie |
| `PATCH`/`DELETE` | `/admin/accounts/{id}` | 调整账号权重、启停或删除账号 |

## 错误语义

错误响应统一为：

```json
{"error": {"message": "...", "type": "not_ready"}}
```

| 状态码 | `type` | 含义 |
| --- | --- | --- |
| `400` | `invalid_request` | 请求参数不合法，例如未知账号 ID、提示词为空或超长 |
| `401` | `authentication_error` | API Key 或管理令牌不匹配 |
| `403` | `feature_disabled` | 该生成类型未在 `GEMINI_ENABLED_GENERATIONS` 中启用 |
| `404` | `not_found` | 任务不存在 |
| `429` | `queue_full` | 任务队列已满 |
| `502` | `provider_error` | Gemini 上游返回了错误或响应无法解析 |
| `503` | `not_ready` | 网关本身无法服务：没有就绪账号、指定账号被停用或正在冷却 |
| `504` | `timeout` | 上游请求超时 |
| `500` | `gateway_error` | 内部错误；详情只写入日志，不返回给调用方 |

`503` 与 `/readyz` 同义，表示本地状态问题，通常同步一次 Cookie 即可恢复；网关会立即返回，不消耗重试预算。`502` 表示上游确实失败，会先按 `GEMINI_RETRY_ATTEMPTS` 退避重试，全部失败后才返回。

## Chrome Cookie 同步

项目自带 [`chrome-extension`](chrome-extension) 扩展。**Cookie 由扩展从浏览器自动读取，不需要手动复制或粘贴**；扩展中要填的只是网关地址和账号归属信息。

开始前请确认两个前置条件，否则同步一定失败：

- **网关正在运行**。扩展会向 `http://127.0.0.1:8000/admin/cookies/sync` 发请求，网关未启动时浏览器只会报 `Failed to fetch`。
- **浏览器已登录目标 Google 账号**，即扩展能在 `google.com` 域下读到 `__Secure-1PSID`。

然后：

1. 使用 `init` 生成的本地配置启动网关；本地初始化默认开启 Cookie 同步。
2. Chrome 打开 `chrome://extensions`，启用开发者模式。
3. 点击“加载已解压的扩展程序”，选择 `chrome-extension` 目录。
4. 登录对应的 `https://gemini.google.com/u/0/app`。
5. 在扩展中填写管理令牌（即 `init` 写入 `.env` 的 `GEMINI_ADMIN_TOKEN`）、稳定的账号名称和账号序号，点击同步。

扩展只允许连接 `127.0.0.1` 或 `localhost`。网关会先在线验证 Cookie，成功后才加入账号池，并原子写入 `GEMINI_ACCOUNTS_FILE`。管理令牌与普通 API Key 分离；未配置 `GEMINI_ADMIN_TOKEN` 的旧环境会暂时回退到 API Key。启用自动同步并允许记住管理令牌后，扩展每 30 分钟刷新所有已保存账号；不开启时仍可手动同步。

每个账号拥有独立 Cookie Jar、HTTP Client、BL/会话元数据、并发限制和健康状态。未指定账号时，网关在健康账号中使用“最少在途请求 + 轮询”；429 会进入冷却，401/403 会下线到重新同步为止。需要固定账号时，在 Chat、Responses、Gemini v1beta 或生成任务请求中添加：

```text
X-Gemini-Account: work
```

异步任务会在入队时绑定账号，任务状态和结果只记录 `account_id`，不会记录 Cookie。已有单账号 `GEMINI_SECURE_1PSID` 配置会自动作为 `default` 账号加载。

Windows 上，账号文件中的 Cookie 使用当前 Windows 用户绑定的 DPAPI 加密；复制到其他用户或机器后无法解密。Linux/macOS 使用权限为 `0600` 的本地文件，容器部署建议通过受保护的数据卷或 Secret Manager 管理。

当已有 Cookie 过期且 Cookie 同步已开启时，网关会以降级模式启动：控制台、健康检查和同步端点仍可访问，模型请求返回未就绪；扩展同步成功后服务立即恢复，不需要重启。

## 构建与测试

建议分三层验证。第一层不连接 Google，适合每次改代码后运行。

Linux/macOS 直接使用 cargo：

```bash
cargo fmt --check
cargo test --locked
cargo build --release --locked
```

Windows 必须通过 `build.ps1`，不能直接调用 cargo：

```powershell
.\build.ps1 fmt
.\build.ps1 clippy
.\build.ps1 test
.\build.ps1 build -Release
```

目标三元组是 `x86_64-pc-windows-gnu`，BoringSSL 需要 MSYS2 UCRT64 的 GCC。`build.ps1` 负责定位 MSYS2 并注入 `ucrt64\bin`、`HOST_CC`、`HOST_CXX`、`LIBCLANG_PATH`。在没有这些环境变量的普通 PowerShell 里直接运行 `cargo test` 会在编译 BoringSSL 时失败：

```text
error occurred in cc-rs: failed to find tool "gcc.exe": program not found
```

出现这个错误说明绕过了 `build.ps1`，不是代码或依赖的问题。`.\build.ps1 fmt` 和 `.\build.ps1 clean` 不需要 MSYS2，其余子命令找不到 UCRT64 GCC 时会直接报错退出。

Windows 的 Release 构建会把所需的 MSYS2 UCRT 运行库复制到 `target/release`；分发时请让这些 DLL 与 EXE 保持在同一目录。GitHub Release 压缩包已经包含它们。

第二层检查 Cookie、网络和 Gemini 登录态：

```powershell
.\build.ps1 run -Release doctor --online
```

第三层在网关启动后执行真实 API 冒烟测试：

```powershell
# 终端 1
.\start.ps1

# 终端 2：前端、健康、认证、模型和真实聊天
.\scripts\smoke-test.ps1

# 再验证 Responses API 与 Gemini v1beta
.\scripts\smoke-test.ps1 -Compatibility

# 再加一次真实图片生成（可能需要几分钟）
.\scripts\smoke-test.ps1 -Image
```

Linux/macOS：

```bash
bash scripts/smoke-test.sh
GEMINI_TEST_COMPAT=1 bash scripts/smoke-test.sh
GEMINI_TEST_IMAGE=1 bash scripts/smoke-test.sh
```

冒烟脚本从进程环境变量或 `.env` 读取 API Key，但不会输出 Key。图片测试通过异步任务接口提交，并验证最终产物确实是图片文件。

`vendor/btls-sys` 包含 Windows MinGW 构建所需的最小上游补丁，因此需要保留在仓库中。

GitHub Release workflow 会生成 Windows x86_64、Linux x86_64 和 Linux ARM64 三种预编译包，同时发布 SPDX SBOM、SHA-256 校验和与 GitHub 构建证明。标签发布还会构建 `linux/amd64`、`linux/arm64` GHCR 镜像。

## 安全

- 不要提交 `.env` 或 `.env.auth.local`。
- Cookie 等同于账号登录凭据；只在本机保存，不要放入 Issue、日志或截图。
- 对外监听前请更换默认 API Key，并使用反向代理、TLS、访问控制和限流。
- 如果 Cookie 曾经公开，请在 Google 账号中刷新会话后重新配置。

## License

[AGPL-3.0-only](LICENSE)。`vendor/btls-sys` 及其 BoringSSL 依赖保留各自的上游许可证。
