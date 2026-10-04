# 项目进度

## 进行中

- Windows 和 Apple 芯片 Mac 尚未完成本次改动的运行回归；Intel Mac 的具体业务测试项未逐项记录。

## 最近完成

- 2026-10-04 12:08：Lite 套餐界面隐藏每月工具调用额度，保留 5 小时和每周用量；过滤仅作用于 Lite 的月度时长项，其他套餐和额度展示保持原样。
- 2026-10-04 11:33：修复新版 BigModel 账号因登录提供方判断而漏查 Start 的问题；使用已有 Start 凭据查询并与付费套餐合并，保留无效凭据回退及接口部分失败时的付费额度展示。
- 2026-10-04 09:12：完成 macOS 构建适配：Mac 专用 APP/DMG 配置、Cargo 路径识别、非交互 DMG 打包、`.app` 路径支持、客户端真实版本读取及 DMG 分发。使用入口见 [README.md](README.md)。

## 最近验证

- 2026-10-04 12:10：用户确认套餐合并及 Lite 月工具额度隐藏均正常；提交前 JavaScript 语法、国际化和补丁空白检查通过。本次提交沿用已通过的 Rust 回归、真实账号查询、Mac 构建及 DMG 校验证据。
- 2026-10-04 12:08：直接执行实际渲染函数，确认 Lite 月工具额度隐藏，5 小时、每周及其他套餐额度保留，空输入正常；前端构建、国际化及补丁空白检查通过。`npm run dist` 成功生成新版 Intel APP/DMG，`hdiutil verify` 校验通过。
- 2026-10-04 11:33：新增回归用例在修改前因缺少 Start 通道失败，修复后全部 5 项 Rust 测试通过；覆盖 Lite 与多个 Start 合并、仅 Start、接口失败和无效凭据回退。国际化及补丁空白检查通过。
- 2026-10-04 11:33：`npm run dist` 成功生成修复版 Intel APP/DMG，安装包校验通过；新程序只读查询已保存账号，付费账号返回 Lite 与有效 Start 套餐，非付费账号仅返回 Start，未修改账号数据。
- 2026-10-04 10:50：用户确认当前 macOS 13.7.8 Intel 环境可正常使用；提交前重新检查国际化键、Node 脚本语法和补丁空白，均通过；生成产物已被 Git 忽略。
- 2026-10-04 09:12：在 macOS 13.7.8 Intel 上运行 `npm run dist`（包含 `npm run tauri build`）成功，生成 APP 和 x64 DMG；`hdiutil verify` 校验通过，打包临时磁盘已全部卸载。
- 2026-10-04 09:12：`cargo test --release --lib --locked --manifest-path src-tauri/Cargo.toml` 通过 1 项路径回归测试；隔离目录中的 CLI `state` 启动成功并识别本机 ZCode 路径；脚本语法及 `git diff --check` 通过。未验证 GUI 交互与真实账号业务流程。
- 2026-10-04 08:32：前端 `npm run build`、`npm run check:i18n`、分发脚本语法检查及 `git diff --check` 通过；系统 PlistBuddy 成功读取已安装 ZCode 版本 `3.14.4`。
