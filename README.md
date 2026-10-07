# mihomo-server-desktop

[mihomo-server](https://github.com/oodzchen/mihomo-server) 的轻量桌面客户端（Tauri 2）。它独立于服务安装和发布，不装也不影响服务使用。

- 自动发现本机当前用户的 mihomo-server 实例，在窗口中打开管理页面并自动登录；
- 本机还没有安装服务时可以在客户端里一键安装（管理员授权通过系统的 polkit 对话框完成），已安装但未运行时也能一键启动；
- 关闭窗口后驻留托盘：左键单击打开管理界面，右键菜单可以切换代理模式、开关 TUN、选择节点并测速、切换订阅，以及启动、停止或重启服务；「退出客户端」只关闭桌面端，服务继续运行；
- 登录时启动和界面语言都在 Web 管理界面的「设置」页中统一调整，托盘菜单会随之切换语言。

目前支持 Linux x86_64，只管理本机实例。

## 安装

从[最新 Release 页面](https://github.com/oodzchen/mihomo-server-desktop/releases/latest)下载对应的安装包（v0.2.20 及更早的版本发布在 [mihomo-server 的 Release 页面](https://github.com/oodzchen/mihomo-server/releases)）：

```sh
sudo dnf install ./mihomo-server-desktop-vX.Y.Z-x86_64.rpm     # Fedora/RHEL
sudo apt install ./mihomo-server-desktop-vX.Y.Z-x86_64.deb     # Debian/Ubuntu
chmod +x mihomo-server-desktop-vX.Y.Z-x86_64.AppImage          # 其他发行版
```

安装包在 Ubuntu 24.04 上构建，需要 glibc 2.39 及以上，并依赖 WebKitGTK 4.1、libayatana-appindicator 和 polkit。GNOME 需要安装 AppIndicator 扩展才能显示托盘图标；KDE 等桌面原生支持。

### NixOS

在系统 flake 中同时引入服务和客户端，并让客户端跟随同一个服务输入：

```nix
inputs.mihomo-server = {
  url = "github:oodzchen/mihomo-server";
  inputs.nixpkgs.follows = "nixpkgs";
};
inputs.mihomo-server-desktop = {
  url = "github:oodzchen/mihomo-server-desktop";
  inputs.nixpkgs.follows = "nixpkgs";
  inputs.mihomo-server.follows = "mihomo-server";
};
```

在主机的 `modules` 中加入 `mihomo-server.nixosModules.default` 和 `mihomo-server-desktop.nixosModules.default`，然后：

```nix
services.mihomo-server = { enable = true; users = [ "colin" ]; };
programs.mihomo-server-desktop.enable = true;
```

默认包 `packages.x86_64-linux.default`（`desktop-bin`）使用 CI 发布的预编译版本；`desktop-source` 从锁定的源码构建。Nix 安装的客户端通过 NixOS 模块管理服务，不会运行安装脚本。服务端模块的说明见 mihomo-server 的 [NIXOS.md](https://github.com/oodzchen/mihomo-server/blob/main/docs/NIXOS.md)。

## 开发

构建需要 Rust（版本由 `rust-toolchain.toml` 固定）以及 WebKitGTK 4.1、ayatana appindicator 和 librsvg 开发包：

```sh
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev   # Debian/Ubuntu
sudo dnf install webkit2gtk4.1-devel libayatana-appindicator-gtk3-devel librsvg2-devel libxdo-devel  # Fedora

cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo install tauri-cli --version 2.12.1 --locked
cargo tauri build --bundles deb,rpm,appimage
```

也可以用 `nix develop` 进入带全部依赖的开发环境。

管理 API 客户端 `management-client` 来自 mihomo-server 仓库，在 `Cargo.toml` 中按服务的发布 tag 固定。需要同时修改两个仓库时，可以临时让 Cargo 使用本地检出（不要提交）：

```toml
# .cargo/config.toml
[patch."https://github.com/oodzchen/mihomo-server"]
management-client = { path = "../mihomo-server/crates/management-client" }
```

调试时可用的环境变量：

| 变量 | 作用 |
| --- | --- |
| `MIHOMO_SERVER_API`、`MIHOMO_SERVER_TOKEN_FILE` | 连接指定的前台服务，而不是从 systemd 查找本机实例 |
| `MIHOMO_SERVER_HELPER` | 替换启动、停止服务所用的 `mihomo-server-user` 帮助程序 |
| `MIHOMO_SERVER_INSTALLER` | 用本地路径或 URL 替换发布的安装脚本 |

## 发布

推送 `v*` tag 后，CI 先运行全部检查，再构建 `.deb`、`.rpm`、AppImage 和供 Nix 使用的 `.tar.gz`（均附 SHA-256 文件），发布到 GitHub Release，并在 main 上更新 `flake.nix` 中的版本和哈希。发布版本号取自 tag。

设计说明见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)。

## 许可证

GPL-3.0-only，见 [LICENSE](LICENSE)。`vendor/ksni` 是打过补丁的 [ksni](https://crates.io/crates/ksni) 0.3.6（Unlicense），说明见 `vendor/ksni/PATCHED.md`。
