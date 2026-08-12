# 六牙象·连萌 — GitHub Actions 自动构建说明

本文件说明如何用 **GitHub Actions** 自动编译打包六牙象·连萌客户端。
源码已 fork 自 RustDesk 1.4.9（GPLv3），改完后推到 GitHub，由云端 runner 编译，产物直接发到仓库 Releases。

工作流文件：`.github/workflows/build-lianmeng.yml`

---

## 1. 为什么用 GitHub Actions + 必须 public

- **GPLv3 合规**：你改了 RustDesk 源码再分发二进制，依法必须公开对应源码。把 fork 推到 **public** 仓库即满足。
- **免费**：GitHub Actions 对 public 仓库 **不限构建分钟**；对私有仓库每月仅 2000 分钟（RustDesk 全量构建一次就吃不消）。
- **网络不是问题**：runner 在美国机房，拉 `crates.io` / Flutter / Dart 包很快；国内网络慢只发生在你本机。
- **双端并行**：一条流水线两个 job——Windows runner 出教师/学生端 exe，Linux runner 出学生端 APK。

---

## 2. 一次性准备

### 2.1 建 fork 仓库并推送

当前 `upstream/rustdesk` 已是带改动的 `lianmeng` 分支，remote 指向官方。做法（把官方留作 `upstream` 引用，自己的 fork 设为 `origin`）：

```bash
cd upstream/rustdesk

# 保留官方上游
git remote rename origin upstream

# 把你自己的 fork 加为 origin（先在 GitHub 上建一个 public 空仓库）
git remote add origin https://github.com/<你的用户名>/rustdesk-lianmeng.git

# 推送 lianmeng 分支
git push -u origin lianmeng
```

> ⚠️ **子模块**：本仓库用了 `submodules: recursive`。CI 拉取时子模块必须是 **public**，否则 checkout 失败。确认所有子模块指向公开地址。

### 2.2 配置 Android 签名 Secrets（可选但强烈建议）

未配置时 APK 以 **debug 签名** 发出（能装，但商店/部分系统会拦）。配置后自动用 release 签名。

在仓库 `Settings → Secrets and variables → Actions` 新增：

| 名称 | 内容 |
|------|------|
| `ANDROID_SIGNING_KEY` | keystore 文件的 **base64**（见下，PKCS12 格式） |
| `ANDROID_ALIAS` | keystore 别名（本仓库固定 `lianmeng`） |
| `ANDROID_KEY_STORE_PASSWORD` | keystore 密码 |
| `ANDROID_KEY_PASSWORD` | 密钥密码 |

> ⚠️ Secret 名**必须带 `ANDROID_` 前缀**（即 `ANDROID_ALIAS` / `ANDROID_KEY_STORE_PASSWORD` / `ANDROID_KEY_PASSWORD`），工作流按此读取；漏前缀会导致签名步骤被跳过、出 debug 包。

生成 keystore 并转 base64。本机无 JDK/keytool 时，可用 Git Bash 自带的 `openssl` 生成 **PKCS12** keystore（`r0adkll/sign-android-release` 底层用 `apksigner`，原生兼容 PKCS12）：

```bash
# Windows 风格路径（openssl 是原生二进制，只认 C:/...，不认 /c/...）
KEYDIR="C:/Users/Administrator/.lianmeng_keys"
mkdir -p "$KEYDIR"

# 1) 自签名证书 + 私钥（RSA 2048，有效期 10000 天）
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$KEYDIR/key.pem" -out "$KEYDIR/cert.pem" \
  -days 10000 -subj "/CN=liuyaxiang/O=liuyaxiang/C=CN"

# 2) 打包为 PKCS12（alias=lianmeng，导出密码同时作为 store/key 密码）
PASS=$(openssl rand -hex 12)   # 或自定一个强密码
openssl pkcs12 -export \
  -in "$KEYDIR/cert.pem" -inkey "$KEYDIR/key.pem" \
  -name lianmeng -out "$KEYDIR/lianmeng.p12" -passout "pass:$PASS"

# 3) 转 base64（单行）供 GitHub Secret
base64 -w0 "$KEYDIR/lianmeng.p12" > "$KEYDIR/lianmeng.p12.b64"
# 把 .b64 文本内容粘进 ANDROID_SIGNING_KEY；PASS 粘进另两个密码 Secret
```

> 私钥/keystore 绝不提交进仓库（已在根仓库 `.gitignore` 规则内），只走 Secrets。生成的 `lianmeng.p12` 请本地备份，遗失将无法更新已发布 APK。

---

## 3. 触发构建

**打 tag 即构建并发布 Release**（推荐）：

```bash
cd upstream/rustdesk
git tag v1.4.9-lianmeng.1
git push origin v1.4.9-lianmeng.1
```

- tag 形如 `v*`（如 `v1.4.9-lianmeng.1`、`v1.4.9-lianmeng.2`）。
- 推送后 Actions 自动跑：`generate-bridge` → `build-windows` + `build-android`（并行）。
- 完成后在仓库 **Releases** 页出现对应 tag，含三个文件。

**手动调试构建**（只上传 artifact，不发布 Release）：
仓库 `Actions → Build Lianmeng → Run workflow`。

---

## 4. 产物

| 文件 | 平台 / 说明 |
|------|------|
| `Lianmeng-1.4.9-windows-x86_64.zip` | Windows 教师端 + 学生端（解压即用的绿色软件，含 `Lianmeng.exe` + `librustdesk.dll`） |
| `Lianmeng-1.4.9-aarch64.apk` | Android 学生端（arm64，主流手机） |
| `Lianmeng-1.4.9-armv7.apk` | Android 学生端（32 位 arm） |
| `Lianmeng-1.4.9-x86_64.apk` | Android 学生端（x64 模拟器/平板） |

> Windows 端**未做代码签名**（绿色软件解压即用）。用户首次运行会遇 SmartScreen 提示，点"仍要运行"即可；如需消除需另购代码签名证书并在工作流加签名步骤。

---

## 5. 工作流做了什么（对照本地已验证命令）

- **`generate-bridge`**：生成 `flutter/lib/generated_bridge.dart`（该文件 gitignore，未入库，必须生成）。
- **Windows job**：
  - 安装 LLVM 15.0.6 / Flutter 3.24.5 / Rust 1.75 / vcpkg（ffmpeg 等依赖）。
  - 替换 RustDesk 自编译 Flutter engine（hwcodec/纹理需要，从 `rustdesk/engine` 发布下载）。
  - 构建：`cargo build --release --features flutter,hwcodec,vram` → `flutter build windows --release`（由 `BINARY_NAME=Lianmeng` 直接产出 `Lianmeng.exe`）。
- **Android job**（三架构并行）：
  - 安装 NDK r28c / cargo-ndk 3.1.2 / Flutter 3.24.5 / vcpkg 安卓依赖。
  - 编译 `librustdesk.so`（`flutter/ndk_*.sh`）→ `flutter build apk --split-per-abi`。
  - 有签名 Secrets 则自动签名并发布签名包，否则发布 debug 包。

---

## 6. 已知注意 / 风险

1. **Windows 构建耗时**：GitHub Windows runner 仅 2 vCPU，全量 RustDesk release 构建加上 vcpkg 安装，预计 **30–60 分钟**（rust-cache 命中后可降到 15–30 分钟）。第一次最慢。
2. **自定义 engine 下载依赖**：Windows 步骤会从 `github.com/rustdesk/engine` 下载 `windows-x64-release.zip`。若该发布不可用，Windows 构建会失败——届时去掉"Replace engine"步骤试试（可能损失部分 hwcodec 能力）。
3. **TopMostWindow 已省略**：官方那段第三方构建在本 fork 中无引用，已砍掉以提速降风险；若后续"正在被查看"悬浮窗需要它，再加回 `build-RustDeskTempMostMostWindow` 依赖。
4. **i686（x86 32 位安卓）已省略**：仅保留 arm64/armv7/x86_64 三个主流架构。
5. **版本号集中管理**：`env.VERSION` 在 `build-lianmeng.yml` 顶部，发版前如需改版本改这里（目前 `1.4.9`）。

---

## 7. 本地复现（如需在本机手动构建）

```bash
# Windows
rustup target add x86_64-pc-windows-msvc
cargo build --release --features flutter,hwcodec,vram
flutter build windows --release
# 产物：flutter/build/windows/x64/runner/Release/Lianmeng.exe + librustdesk.dll

# Android（某架构）
rustup target add aarch64-linux-android
cargo install cargo-ndk --version 3.1.2 --locked
./flutter/ndk_arm64.sh
# 再把 so 拷进 flutter/android/app/src/main/jniLibs/arm64-v8a/，然后：
flutter build apk --release --target-platform android-arm64 --split-per-abi
```
