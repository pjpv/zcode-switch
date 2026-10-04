# Z·SWITCH (zcode-switch)

**简体中文** ｜ [English](README.en.md)

Tauri 2 桌面工具：在多个 ZCode 账号之间一键切换，自动显示额度。只换登录身份——项目、会话、设置全部共用不动。

![screenshot](docs/screenshot.png)

## 功能

- **保存 / 切换账号**：一键切换登录身份；切换前自动保全当前登录，绝不丢号；设备身份跟账号走，远程控制中继密钥跨切换保活
- **添加账号**：工具内 OAuth 登录新号（BigModel / z.ai 双入口），全程不动当前登录
- **额度展示**：账号行内联显示套餐额度与重置时间，多套餐分组
- **活动领取**：可领套餐一键领取；「自动领取」开关（默认关）定时自动检测并领取，手动操作优先
- **加密导入导出**：`.zsb` 捆绑包，PBKDF2(100k) + AES-256-GCM 口令加密
- **中英双语**：设置里一键切换 中文 / English，主窗、托盘、错误提示、CLI 输出全覆盖；首次运行按系统语言自动选择
- **托盘 / 开机自启 / CLI 自动化**

## 安全设计

- **本地优先**：所有数据在本地，无遥测、无远端存储；额度查询直连官方接口
- **WebView CSP**：`script-src` 基线为 `'self'`；为官方活动的网页组件放行了最小范围的第三方脚本与图片来源，界面事件不依赖动态执行、走白名单式分发
- **防丢号**：切换前自动保全未入库登录；文件写入走临时文件 + 原子 rename
- **路径穿越防护**：账号 id 白名单（`[A-Za-z0-9-]`），删除/读取均不可逃出账号库目录
- **加密导出**：PBKDF2-HMAC-SHA256（100k 迭代）+ AES-256-GCM，随机 salt/nonce；错密码即失败，无明文痕迹
- **凭据只在本地解密**：凭据解密仅用于显示用户名/邮箱；导出文件凭口令加密

## CLI

```
zcode-switch.exe --cli state|list
zcode-switch.exe --cli quota [--id <账号id>]
zcode-switch.exe --cli claim-preview [--id <账号id>]
zcode-switch.exe --cli capture [--name 名称]
zcode-switch.exe --cli switch --id <id> [--force] [--restart|--no-restart] [--hot <bool>|--no-hot]
zcode-switch.exe --cli kill
zcode-switch.exe --cli export --id <id> --out <a.zsb>
zcode-switch.exe --cli export-all --out <all.zsb>
zcode-switch.exe --cli import --file <file.zsb>
zcode-switch.exe --cli rename|delete|update|behavior|setpath|launch
zcode-switch.exe --cli --lang en state              # 英文输出（--lang 空格传值、可置于任意位置；默认跟 GUI 语言/系统语言）
```

CLI 密码（export / import）：优先环境变量 `ZSW_PASSWORD`（不出现在进程列表和命令历史），也可 `--password <密码>`。

## 常见问题

### macOS 弹出「麦克风 / 辅助功能 / 录屏」权限询问？

全部拒绝即可，不影响任何功能。
应用内嵌的登录等网页由系统 WebView 渲染，网页发起的请求会被透传为应用的系统权限询问；应用本体与其内嵌网页均未使用这三项能力。

## 构建

先安装 Node.js 和 Rust 工具链（需要 `cargo`、`rustc`）。macOS 还需要 Xcode Command Line Tools，使用 `xcode-select --install` 安装。

若出现 `cargo metadata ... No such file or directory`，说明 Cargo 未安装或不在 PATH。按 [Rust 官方安装说明](https://rust-lang.org/tools/install/) 安装后，执行 `source "$HOME/.cargo/env"`，再用 `cargo --version` 检查。

```bash
npm install
npm run tauri dev      # 开发（HMR）
npm run tauri build    # macOS：APP + DMG；Windows：NSIS
npm run dist           # 将 macOS DMG 或 Windows 安装包、便携版复制到 release/
```

macOS 构建产物在 `src-tauri/target/release/bundle/macos/` 和 `src-tauri/target/release/bundle/dmg/`。Intel Mac 构建 x64 版本，Apple 芯片 Mac 构建 ARM64 版本。使用前需安装 ZCode；默认探测系统或用户 Applications 目录，也可以在设置中填写 `.app` 或其内部可执行文件路径。

项目构建入口会识别用户目录下的 Cargo；Mac 打包自动跳过 Finder 窗口美化脚本，避免依赖 Finder 自动化权限。

## License

[MIT](./LICENSE)
