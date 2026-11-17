# MCSM-Auto-Deployer

面向 Minecraft 生态的通用自动部署工具：从 GitHub Releases、Modrinth、CurseForge 或 SpigotMC 获取更新，下载并完成 SHA-256 校验，再通过 **MCSManager 的两步上传协议** 直接上传到 Daemon。支持上传前清理旧版、批量部署以及指定时间统一重启。后端使用 Rust（Axum + Tokio），前端是一套嵌入二进制的 **Material Design 3** 可视化控制台。

编译产物是单一静态二进制，部署时不需要 Node.js、数据库或额外运行时。

> GitHub Actions 会在推送到 `main` 或手动触发时自动构建 `x86_64-unknown-linux-musl` 静态二进制，并发布到 GitHub Releases。发布流程见 [Build and Release](.github/workflows/build-release.yml)，下载与安装见「GitHub Actions 自动构建与发布」。

## 功能一览

- 多任务模型：服务端核心、Plugins、Mods、任意其他文件统一配置。
- 多来源：GitHub Releases、Modrinth、CurseForge、SpigotMC（Spiget）。
- 定时轮询；GitHub 支持 **ETag 条件请求**，避免浪费 Rate Limit。
- 流式下载 + 边下边算 SHA-256；GitHub Release 提供 `digest` 时会交叉校验。
- **MCSM 两步上传**：面板只签发一次性凭证，文件字节直传 Daemon，不经过面板 API。
- 文件目标自动重命名：例如上传 `lophine-0.9.2.jar` 后移动为 `/server.jar`。
- 目录目标保留资产原名：`/plugins/`、`/mods/`。
- **上传前清理旧版**：支持按目标目录远程文件名正则清理，即使旧文件被手动重命名也能匹配；未配置正则时回退到本地记录的部署路径。
- 批量部署：一次选择多个任务，逐个更新，结束后只登记一次统一重启（按实例分组）。
- 定时统一重启：指定未来时间，到达时间后执行 `restart` 或 `stop → 等待停止 → 启动`。计划持久化，面板重启后仍会继续。
- 单任务仍支持直接重启、「停止 → 等待停止 → 启动」两种即时策略。
- 配置热更新、任务增删改、立即检查、立即更新、实时 SSE 日志。
- Material Design 3 深色大屏：M3 色调表面、角色色板、形状/动效令牌、状态层、Assist Chip、FAB、Dialog、Snackbar。

## 界面

控制台按 1080p 设计，顶部为系统状态和全局操作，中部为任务卡片，底部为可折叠 SSE 日志终端。任务类型通过不同颜色的 Chip 区分，每张卡片展示：

- 类型（核心 / 插件 / 模组 / 其他）与来源平台
- 目标实例与实例内路径
- 当前版本 → 最新版本
- 检查、下载、校验、旧版清理、上传、重启各阶段状态
- 当前资产名、执行错误或下载进度
- 待统一重启标记
- 「立即检查」「立即更新」「编辑」操作

顶部「批量部署」对话框可选择多个任务、指定统一重启时间，并管理/取消已登记的重启计划。设置对话框可以修改轮询间隔、MCSM 面板地址、API Key、Daemon 协议、重启策略、GitHub Token、CurseForge API Key 等。已配置的密钥不会下发到浏览器；留空提交表示保持原密钥。

## 架构

```
GitHub / Modrinth / CurseForge / Spigot
        │  统一解析为 ResolvedArtifact
        ▼
  SourceManager ──► 临时文件 + SHA-256
        │
        ▼
  DeploymentEngine（每个任务一把异步锁）
        │  1. （可选）列出目标目录并删除旧版
        │  2. 申请上传凭证（面板 API）
        │  3. multipart 直传 Daemon（不经过面板）
        │  4. 文件目标：/api/files/move 重命名
        │  5. 立即重启，或标记为待统一重启
        ▼
  MCSManager 实例
        ▲
        │ 统一重启队列（持久化 + 定时执行）
  Axum API + SSE ──► 嵌入式 M3 Web 控制台
```

仓库与核心目录：

```text
.github/workflows/build-release.yml   GitHub Actions 构建 + Release
scripts/run-windows.ps1                Windows 本地构建辅助脚本
scripts/run-windows.cmd                Windows 一键启动入口

src/
├── main.rs           启动、日志、Axum、优雅退出
├── config.rs         配置模型、校验、归一化
├── state.rs          Arc<AppState>、任务运行时状态、持久化
├── engine.rs         检查 → 下载 → 校验 → 清理 → 上传 → 重启/批量调度
├── api.rs            JSON API、配置热更新、批量部署、重启计划、SSE
├── logs.rs           日志广播与 tracing Layer
├── mcsm/
│   ├── client.rs     两步上传、文件移动、实例重启
│   └── types.rs      MCSM JSON 结构
├── github/
│   └── client.rs     GitHub Releases、ETag、资产选择
├── sources/
│   ├── mod.rs        统一来源抽象、流式下载、SHA-256
│   ├── modrinth.rs   Modrinth v2
│   ├── curseforge.rs CurseForge v1
│   └── spigot.rs     SpigotMC via Spiget
└── web/
    ├── index.html    Material Design 3 单页界面
    └── app.js        原生 JS 交互
```

## 快速开始

### 1. 准备 MCSManager

1. 在面板右上角用户设置中生成 API Key。
2. 获取节点 UUID（Daemon ID）和实例 UUID。
3. 确保运行本程序的机器能同时访问面板端口（默认 `23333`）和 Daemon 端口（默认 `24444`）。

### 2. 编辑配置

复制并编辑仓库根目录的 `config.toml`，至少替换：

- `mcsm.base_url`
- `mcsm.api_key`
- 每个任务的 `mcsm_daemon_id`
- 每个任务的 `mcsm_instance_uuid`

没有 Node/Python 要求，编辑 TOML 即可：

```bash
cp config.toml config.local.toml
chmod 600 config.local.toml
```

### 3. 本地运行

#### Linux / macOS

```bash
cargo run -- --config config.local.toml
# 或 ./mcsm-auto-deployer --config config.local.toml
```

#### Windows 本地测试（推荐）

Windows 直接 `cargo run` 会使用 **MSVC 的 cl.exe** 编译 `ring` 的 C 代码。如果你遇到 `c1.dll ... D8050`，说明本机 MSVC 工具链无法启动 C 编译器；请改用 Zig + GNU 目标，完全绕开 MSVC：

```powershell
# 一次性准备
rustup target add x86_64-pc-windows-gnu
cargo install cargo-zigbuild

# 构建并启动 Windows 本地版本（推荐，自动绕过 PowerShell 执行策略）
.\scripts\run-windows.cmd

# 指定配置 / 发布模式
.\scripts\run-windows.cmd -Config config.local.toml
.\scripts\run-windows.cmd -Config config.local.toml -Release
```

如果直接运行 `.ps1`，Windows 可能会拦截未签名脚本，请改用：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\run-windows.ps1 -Config config.local.toml
```

手动执行时，等价命令是：

```powershell
cargo zigbuild --target x86_64-pc-windows-gnu
.\target\x86_64-pc-windows-gnu\debug\mcsm-auto-deployer.exe --config config.local.toml
```

脚本会自动安装 Rust 的 GNU 目标、创建 Zig/Cargo 临时目录，并运行生成的 `.exe`。这里构建出来的是 Windows 本地测试程序，不会改变生产目标的 `x86_64-unknown-linux-musl` rustls 静态构建。

浏览器访问 <http://127.0.0.1:8080>。首次启动 4 秒后会执行一次全局检查，之后按 `poll_interval_secs` 轮询。

### Windows 上出现 `ring` / `cl.exe` / `c1.dll` D8050

这不是 Rust 代码错误，而是 MSVC 无法启动 C 编译器（`c1.dll`），发生位置是 `ring` 的 C 源码编译。处理方式：

1. **推荐**：使用上面的 Windows GNU + Zig 方案，完全不调用 MSVC。
2. 如果必须使用 MSVC：从开始菜单打开 **x64 Native Tools Command Prompt for VS 2022**，在同一窗口执行 `cargo build`；若仍报 D8050，请修复或重装 Visual Studio Build Tools 的「使用 C++ 的桌面开发」工作负载，并检查杀毒软件是否拦截 `cl.exe` / `c1.dll`。
3. 单元测试建议在 WSL/Linux 中执行 `cargo test`，避免 Windows C 工具链问题。

## Windows 环境交叉编译静态 Linux 二进制

目标：`x86_64-unknown-linux-musl`，得到不依赖 glibc 的单一可执行文件。

### 前置

1. 安装 Rust（MSVC toolchain 即可）。
2. 安装 [Zig](https://ziglang.org/download/)（Windows 版），把 `zig.exe` 加入 `PATH`。
3. 安装 cargo-zigbuild：

```powershell
rustup target add x86_64-unknown-linux-musl
cargo install cargo-zigbuild
zig version
```

### 编译

```powershell
cd C:\path\to\MCSM-Auto-Deployer

cargo zigbuild --release --target x86_64-unknown-linux-musl
```

产物：

```
target\x86_64-unknown-linux-musl\release\mcsm-auto-deployer
```

在 Linux 上检查静态链接：

```bash
file target/x86_64-unknown-linux-musl/release/mcsm-auto-deployer
# ELF 64-bit LSB executable, x86-64, statically linked, stripped
ldd target/x86_64-unknown-linux-musl/release/mcsm-auto-deployer
# not a dynamic executable
```

### 在 Linux 上直接编译

```bash
sudo apt install musl-tools
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

> `rustls` 默认带 Mozilla WebPKI 根证书；同时启用了系统根证书支持，因此在企业代理 / 自建 CA 环境也能读取本机证书库。

## GitHub Actions 自动构建与发布

工作流文件：[.github/workflows/build-release.yml](.github/workflows/build-release.yml)

触发方式：

- 推送到 `main` 分支；
- 在 Actions 页面手动执行 `workflow_dispatch`。

工作流功能：

1. 从 `Cargo.toml` 读取当前 `version`；
2. 安装 Rust `x86_64-unknown-linux-musl` target、Zig 与 `cargo-zigbuild`；
3. 执行静态构建：`cargo zigbuild --release --target x86_64-unknown-linux-musl`；
4. 将二进制重命名为 `mcsm-auto-deployer-linux-x86_64`，生成 `SHA256SUMS`；
5. 上传 Build Artifact；
6. 创建或更新 GitHub Release `v<version>`，把二进制和校验文件作为 Release 资产，并自动生成 Release Notes。

工作流只使用仓库自带的 `GITHUB_TOKEN`（权限 `contents: write`），不需要额外 Secrets。

### 发布新版本

1. 修改 [Cargo.toml](Cargo.toml) 中的 `version`，例如 `0.2.0`；
2. 提交并推送到 `main`；
3. Actions 完成后，Releases 中会出现或更新 `v0.2.0`。

如果版本号未变化，工作流会使用 `allowUpdates: true` 更新同一个 Release 的资产与说明。

### 从 Releases 安装

在 GitHub Releases 页面下载以下文件：

- `mcsm-auto-deployer-linux-x86_64`
- `SHA256SUMS`

```bash
# 可选：确认下载的是工作流生成的产物
sha256sum -c SHA256SUMS

chmod +x mcsm-auto-deployer-linux-x86_64

# 编辑配置后启动
./mcsm-auto-deployer-linux-x86_64 --config config.toml
```

> Release 构建产物为 Linux x86_64 静态二进制；Windows 本地测试请使用 `scripts/run-windows.cmd`。

## 服务器部署

```bash
sudo useradd --system --home /opt/mcsm-auto-deployer --shell /usr/sbin/nologin mcsmdeploy
sudo mkdir -p /opt/mcsm-auto-deployer/data
sudo cp mcsm-auto-deployer-linux-x86_64 config.toml /opt/mcsm-auto-deployer/
sudo chown -R mcsmdeploy:mcsmdeploy /opt/mcsm-auto-deployer
sudo chmod 600 /opt/mcsm-auto-deployer/config.toml
```

`/etc/systemd/system/mcsm-auto-deployer.service`：

```ini
[Unit]
Description=MCSM-Auto-Deployer
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=mcsmdeploy
Group=mcsmdeploy
WorkingDirectory=/opt/mcsm-auto-deployer
ExecStart=/opt/mcsm-auto-deployer/mcsm-auto-deployer-linux-x86_64 --config /opt/mcsm-auto-deployer/config.toml
Restart=on-failure
RestartSec=5
Environment=RUST_LOG=info,mcsm_auto_deployer=debug
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/mcsm-auto-deployer
UMask=0077

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now mcsm-auto-deployer
sudo journalctl -u mcsm-auto-deployer -f
```

### Nginx 反向代理 / SSE

```nginx
server {
    listen 443 ssl http2;
    server_name deploy.example.com;

    # 建议额外加 auth_basic，见“安全”
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # SSE /api/logs/stream 必须关闭缓冲
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 3600s;
    }
}
```

## 配置参考

```toml
[server]
bind = "0.0.0.0:8080"           # 改这个需要重启进程
poll_interval_secs = 600         # 30–86400；最小 30 防止滥用 GitHub 限额
state_file = "data/state.json"   # 已部署版本持久化
log_history = 500                # 控制台保留日志条数

[mcsm]
base_url = "http://127.0.0.1:23333"
api_key = "YOUR_MCSM_API_KEY"
timeout_secs = 180
restart_mode = "restart"         # restart | stop_then_open
# daemon_scheme = "http"         # 面板 HTTPS、节点 HTTP 时显式指定
accept_invalid_certs = false     # 仅自签名内网建议开启

[github]
token = ""                       # 可留空；建议配置 PAT 提高限额
api_base = "https://api.github.com"
user_agent = "MCSM-Auto-Deployer/0.1"

[providers]
modrinth_base = "https://api.modrinth.com/v2"
curseforge_base = "https://api.curseforge.com/v1"
curseforge_api_key = ""          # CurseForge 来源必填
spiget_base = "https://api.spiget.org/v2"
user_agent = "MCSM-Auto-Deployer/0.1"

[[tasks]]
id = "lophine-core"              # 可省略，程序会从 name 生成稳定 slug
name = "Lophine 服务端核心"
category = "core"                # core | plugin | mod | other；省略时按路径推断
enabled = true
source_type = "github"           # github | modrinth | curseforge | spigot
source = "LophineLabs/Lophine"   # 仓库 / 项目 slug / 项目 ID / 资源 ID
asset_regex = '^lophine-.*\.jar$'
include_prerelease = false
mcsm_daemon_id = "DAEMON_UUID"
mcsm_instance_uuid = "INSTANCE_UUID"
mcsm_target_path = "/server.jar" # 文件目标：上传后重命名
restart = true
cleanup_old = false

[[tasks]]
id = "essentialsx-plugin"
name = "EssentialsX 基础插件"
category = "plugin"
enabled = true
source_type = "github"
source = "EssentialsX/Essentials"
asset_regex = '^EssentialsX-.*\.jar$'
mcsm_daemon_id = "DAEMON_UUID"
mcsm_instance_uuid = "INSTANCE_UUID"
mcsm_target_path = "/plugins/"    # 目录目标：保留资产文件名
restart = true
cleanup_old = true
cleanup_regex = '^EssentialsX-.*\.jar$'
```

### 目标路径语义

| `mcsm_target_path` | MCSM `upload_dir` | 最终文件 |
| --- | --- | --- |
| `/server.jar` | `/` | 上传 `资产名.jar` 后移动成 `/server.jar` |
| `/plugins/` | `/plugins` | `/plugins/资产名.jar` |
| `/mods/` | `/mods` | `/mods/资产名.jar` |
| `/config/extra.yml` | `/config` | 上传后移动成 `/config/extra.yml` |

### 来源平台字段

| `source_type` | `source` 含义 | 必需配置 |
| --- | --- | --- |
| `github` | `owner/repo` | 可选 GitHub Token（提高限额） |
| `modrinth` | 项目 slug 或 project ID，如 `fabric-api` | 无 |
| `curseforge` | 数字项目 ID，如 `306612` | `providers.curseforge_api_key` |
| `spigot` | 数字 Spigot 资源 ID，如 `9089` | 无；通过 Spiget 访问 |

- Modrinth / CurseForge 可使用 `game_versions = ["1.21.1"]` 和 `loaders = ["fabric"]` 过滤版本；字段留空表示不筛选。
- CurseForge 文件若作者禁用了第三方下载，API 的 `downloadUrl` 会为空；程序会明确报错，无法绕过。
- Spigot 资源若为“外部托管”（Spiget 只返回外部页面 URL），无法保证直接下载文件；请改用该项目的 GitHub 或 Modrinth 来源。
- 旧版配置字段 `github_repo` 仍然兼容（会映射到 `source`），但保存后统一写为 `source`。

### 旧版文件清理（`cleanup_old`）

启用后，在 SHA-256 校验通过之后、上传新版之前：

1. 调用 MCSM `GET /api/files/list` 列出目标目录；
2. 用 `cleanup_regex` 匹配**服务器上的远程文件名**，而不是 GitHub Asset 名；
3. 排除即将上传的新文件名后，调用 `DELETE /api/files` 删除旧文件；
4. 上传新版。

因此即使旧文件在服务器上被管理员重命名，只要名称仍符合 `cleanup_regex`，也能被清理。若未填写正则，则仅尝试删除本地状态记录的上次部署路径（文件被重命名时可能找不到）。为避免误删，`cleanup_old` 只允许目录目标，且不会删除目录或未匹配文件。

### 资产正则

`asset_regex` 匹配候选资产的文件名，而不是 Release Tag / 版本号。TOML 字面量字符串（单引号）中 `\.` 只写一次即可：

```toml
asset_regex = '^fabric-api-.*\.jar$'
```

- GitHub：从最新 Release 向旧遍历，返回第一个匹配文件；必填。
- Modrinth / CurseForge：在同一版本的文件列表中选 primary 或第一个匹配项；可留空表示匹配任意文件。
- Spigot：使用资源名生成稳定 `.jar` 文件名，`asset_regex` 不参与选择。
- `include_prerelease = true` 时 GitHub 接受 prerelease，Modrinth 接受 beta/alpha，CurseForge 接受 beta/alpha 文件。

## 内容平台凭据

- **GitHub**：`[github].token` 可选；匿名 API 限额较低。
- **Modrinth**：无需 Key，但必须提供可识别的 `providers.user_agent`。
- **CurseForge**：需要 [CurseForge for Studios](https://console.curseforge.com/) 的 API Key，填入 `[providers].curseforge_api_key` 或设置对话框。程序使用 `GET /v1/mods/{id}/files` 获取文件，并读取 `downloadUrl`。
- **SpigotMC**：通过 Spiget 社区 API；无需 Key。Spiget 只能保证 Spigot 托管文件的直链；外部托管或需要登录的资源可能失败，见“已知边界”。

## 网页使用说明

1. **新增任务**：右下角 FAB。
2. **选择来源平台**：GitHub / Modrinth / CurseForge / SpigotMC。来源标识会随平台切换提示（`owner/repo`、项目 slug、数字项目 ID 或 Spigot 资源 ID）。
3. **选择任务类型**：核心 / 插件 / 模组 / 其他。选择「自动识别」时：
   - 目标以 `.jar` 结尾且不在 `/plugins/`、`/mods/` 下 → 核心；
   - 路径含 `/plugins/` → 插件；
   - 路径含 `/mods/` → 模组；
   - 其余 → 其他。
4. **平台筛选**：Modrinth / CurseForge 可填写逗号分隔的 Minecraft 版本与加载器，例如 `1.21.1` 和 `fabric, neoforge`。
5. **资产正则**：GitHub 必填；Modrinth / CurseForge 可选；Spigot 会隐藏此字段。
6. **Daemon / 实例 UUID**：从 MCSManager 面板 URL 或实例详情中获取。
7. **目标路径**：以 `/` 结尾或最后一段没有扩展名表示目录；否则是文件路径。
8. **旧版清理**：目录任务可开启。填写清理正则时会按**远程文件名**删除旧版，适合旧文件已被重命名的情况；留空时只按本地记录的上次部署路径清理。
9. **立即检查**：只解析来源并更新「最新版本」，不下载文件。
10. **立即更新**：完整执行下载 → 校验 → （可选清理）→ 上传 → 重启。按钮进入 loading，状态通过 2.5 秒轮询与 SSE 实时更新。
11. **批量部署**：顶部「批量部署」按钮，勾选多个任务并指定统一重启时间。批量期间不逐个重启；结束后按实例登记一条计划重启。
12. **统一重启计划**：批量部署对话框中可查看所有待执行/最近完成的重启计划并取消 pending 计划；任务卡片会显示「待统一重启」。
13. **筛选器**：全部 / 有更新 / 执行中 / 异常。
14. **设置**：修改轮询间隔、MCSM 凭据、GitHub Token、CurseForge API Key 等。密钥字段留空表示保持原值。
15. **日志面板**：底部可折叠，SSE 断线会自动重连。

## HTTP API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/api/health` | 健康检查 |
| GET | `/api/status` | 系统信息 + 所有任务运行时状态 |
| GET | `/api/config` | 配置（密钥脱敏为 `""`，并带 `api_key_set` / `token_set` / `curseforge_api_key_set`） |
| POST | `/api/config` | 热更新配置，写回 TOML；空密钥表示保留原密钥 |
| POST | `/api/check-all` | 全局检查（异步，202 语义） |
| POST | `/api/batch/update` | 批量部署，body 必含 `task_ids`、`restart_at`（Unix ms），可选 `restart_mode` |
| GET | `/api/restarts` | 查看统一重启计划 |
| POST | `/api/restarts` | 单独登记统一重启 |
| DELETE | `/api/restarts/{id}` | 取消待执行的重启计划 |
| POST | `/api/tasks/{id}/check` | 立即检查单个任务 |
| POST | `/api/tasks/{id}/update` | 立即完整部署单个任务 |
| GET | `/api/logs/stream` | SSE 日志流 |

所有耗时接口立即返回「已受理」，实际进度通过状态接口/SSE 观察；这保证关闭浏览器不会取消部署。

## MCSM 两步上传边界（实现说明）

第一步，面板：

```
POST {panel}/api/files/upload?apikey=...&upload_dir=/plugins&daemonId=...&uuid=...
X-Requested-With: XMLHttpRequest
```

返回 `password` + `addr`（新版）或 `uploadAddress`（旧版）。

第二步，Daemon：

```
POST {daemon}/upload/{password}?overwrite=true&apikey=...
X-Requested-With: XMLHttpRequest
Content-Type: multipart/form-data

file=@artifact.jar
```

文件字节**绝不**提交到面板的 `/api/files/upload`。

- **文件目标重命名**：上传后通过面板 `PUT /api/files/move` 将资产移动到目标路径，例如 `/lophine-0.9.2.jar` → `/server.jar`。该能力始终启用，不需要额外开关。
- **旧版清理**：如果任务开启 `cleanup_old`，在下载和 SHA-256 校验完成后、上传前，通过 `GET /api/files/list` 列出目录，再通过 `DELETE /api/files` 删除匹配的旧文件。删除前会排除即将上传的新文件名。
- **重启**：单任务立即执行 `/api/protected_instance/restart`（或 stop → 等待 status=0 → open）；批量任务先跳过重启，全部结束后把统一重启计划持久化，由独立调度器在指定时间执行一次。

核心实现带详细边界注释，见 `src/mcsm/client.rs` 与 `src/engine.rs`。

## 安全建议

- 控制台 **没有内置登录**，且 `POST /api/config` 能修改任务和访问凭据。生产环境请：
  - 只监听 `127.0.0.1`，通过 Nginx / Caddy 加 Basic Auth 或 SSO；
  - 防火墙限制 8080；
  - 不把面板 API Key 暴露给公网。
- `config.toml` 含明文 MCSM API Key / GitHub Token，保持 `chmod 600`。
- Web UI 从不接收真实密钥；GET 配置时密钥永远为空。
- 建议使用最小权限的 MCSM 子账号 API Key，只授予目标实例的文件/实例权限。
- 开启 `accept_invalid_certs` 会关闭 TLS 校验，仅限自签名内网。

## 已知边界

- GitHub/Modrinth/CurseForge 每个版本只取第一个匹配资产；不做多文件/多附件安装。
- 目录任务默认保留资产原名；启用 `cleanup_old` 可清理旧版，但清理正则由用户负责，过宽的正则会误删文件。
- 清理发生在新版上传之前：若随后上传失败，旧文件不会自动恢复；重要实例请先备份并先在测试实例验证清理正则。
- 文件任务会覆盖目标文件，不提供回滚/备份。重要服务器建议配合世界与配置备份。
- CurseForge 中作者禁用第三方分发的文件没有 `downloadUrl`，无法自动下载。
- Spigot 通过 Spiget 社区 API 访问，仅保证 Spigot 托管文件可下载；外部托管资源会给出明确错误。部分资源可能触发 Cloudflare/登录限制，建议优先使用项目的 GitHub 或 Modrinth 来源。
- 统一重启每个实例同一时间只保留一条 pending 计划；重复登记会合并。计划时间早于批量任务完成时会自动顺延至少 5 秒。
- 轮询间隔硬下限为 30 秒，避免无意义的 Rate Limit 消耗。
- 页面样式依赖 Google Fonts、Tailwind CSS 4 Browser、DaisyUI 5 的 CDN；核心布局同时有内联 fallback CSS，CDN 不可用时仍可操作，但字体和少数 DaisyUI 细节会退化。

## 开发

```bash
cargo fmt
cargo check
cargo test
cargo zigbuild --release --target x86_64-unknown-linux-musl
```

> 当前 GitHub Actions 只负责构建静态二进制并发布 Release，不执行测试；提交前请自行运行 `cargo fmt --check`、`cargo test`，或在单独的 CI 中增加测试作业。

仓库的 `.gitignore` 已忽略 `target/`、`dist/`、`data/`、本地配置（`config.*.toml`，但保留示例 `config.toml`）、日志、IDE 配置和本地浏览器测试目录。

在 Windows 上，`cargo check` / `cargo test` 同样会触发 `ring` 的 MSVC C 编译；如遇到 D8050，可先执行 `cargo zigbuild --target x86_64-pc-windows-gnu` 验证编译，或使用 WSL/Linux 运行完整测试。只启动应用请用 `scripts/run-windows.cmd`。

环境变量：

- `MCSM_CONFIG`：配置文件路径，默认 `config.toml`。
- `RUST_LOG`：日志过滤，默认 `info,mcsm_auto_deployer=debug`。

单测位于各模块底部，覆盖：

- 目标路径归一化、类型推断与旧文件清理限制；
- 文件 / 目录上传目标解析与上传后重命名；
- 新旧上传凭证 URL 构造；
- GitHub Release 资产匹配与预发布跳过；
- Modrinth 游戏版本/加载器筛选；
- CurseForge 加载器映射；
- Spigot 外部托管检测；
- 配置 slug 行为。

集成验证建议使用本地 mock（避免真实下载和重启生产实例），覆盖以下链路：四种来源解析 → 下载 SHA-256 → 旧文件列表/删除 → 两步上传 → 文件重命名 → 批量部署 → 统一重启。

## License

MIT
