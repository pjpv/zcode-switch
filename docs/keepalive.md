# Windows 保活 / Windows keepalive

保活是默认关闭的 Windows 功能。它在 **Z·SWITCH 保持运行（可以缩到托盘）** 时观察 ZCode 新产生的验证码失败，在同一个账号下冷启动 ZCode，并向符合条件的原会话发送一次 `继续`。

在工具栏打开 **保活**，阅读首次启用说明并确认即可。使用设置中探测或指定的 ZCode 路径以及现有数据目录，不需要安装插件、Python、Node.js 或修改 ZCode 安装包。其他操作系统暂不显示此开关。

## 行为

- 从开启时的日志末尾开始观察。只匹配结构化 `turn.failed` 的已知验证码错误链，并要求会话 ID、轮次 ID 和有效时间；普通停顿、网络错误或聊天里提到“验证码”不会触发。
- 首次发现故障立即进入恢复。重复验证码失败持续恢复，间隔从 15 秒逐步增加，最多 5 分钟，不限制总尝试次数。
- 只恢复本轮捕获的故障会话，以及开启后观察到正在执行、重启前索引仍为 running 的同实例会话。不会遍历历史错误会话批量发送消息。
- 在发送前重新检查原会话和工作区。已完成、已暂停、等待回答或权限批准、已切到新轮次的任务不自动发送。
- 发送使用当前桌面连接的客户端标识和稳定命令 ID；丢失回执时先观察会话，避免直接重复发送。只有新轮次实际产生进展或完成才增加“已恢复”计数。
- 关闭开关取消后续续跑。已经停止 ZCode 的重启会以退避间隔继续尝试完成启动，并明确显示状态；也可手动启动。通过切换器手动换号、启停或更换路径会关闭保活并取消待执行的启动补偿，优先尊重这些操作。
- 未知版本、未知状态、多个匹配实例、远程工作区或不明确的进程归属不会被猜测为可恢复。状态栏会显示等待或需要处理。

## 兼容性与本机控制

已在 Windows 的 **ZCode 3.14.4.7912** 上验证原会话冷恢复。会话控制复用 ZCode renderer 内部服务，因此不是 ZCode 承诺兼容的公开 API。实现按连接协议、服务、会话身份和状态字段检查兼容性；升级后若检查失败会停止续跑，需要更新适配后再使用。

恢复启动附加 `--remote-debugging-address=127.0.0.1 --remote-debugging-port=0`。系统选择端口；切换器验证监听进程、PID/启动时间、可执行路径、Windows 用户及桌面 renderer URL，然后才使用该连接。不会开放公网监听。**此接口没有额外身份认证，本机其他程序也可能控制该 ZCode 实例**，因此首次启用需明确确认。关闭保活不强行重启工作中的 ZCode；调试接口在下一次普通启动后关闭。

会话恢复集合仅保存在当前 Z·SWITCH 进程中。退出后不自动重放旧恢复命令；再次启动从新的观察边界开始。启用偏好会保存，手动生命周期操作会保存为关闭。保活负责故障恢复，不能保证服务端以后永远不再要求人工验证码，也不会自动解滑块或更换账号。

## 验证

```sh
npm ci --ignore-scripts
npm run check:i18n
npm run test:keepalive
npm run build
cd src-tauri
cargo test --lib
```

测试覆盖历史日志排除、轮转/截断/部分写入、错误归因、资源上限、精确恢复目标、手动续跑覆盖自动恢复、启动失败后的重试、关闭期间完成已提交启动、回执与执行进展的区别、请求去重及等待用户状态。

本次运行验收另外使用了隔离数据目录与隐藏桌面的实际 ZCode：本地模拟模型制造失败 → 原进程退出 → 冷启动 → 原 session ID 恢复 → `继续` → 新轮次完成；重复命令未增加模型请求。也通过 Rust 桥实际核实了 Windows 进程归属、随机端口发现和 renderer 控制连接。真实阿里云再次发出挑战后的完整自动恢复尚未作为本次验收复现；不能把模拟模型测试表述为该外部服务验收。

## English

Keepalive is an opt-in Windows toolbar switch. While Z·SWITCH runs, it detects **new structured CAPTCHA turn failures**, restarts the selected local ZCode instance with the same account, and resumes only captured affected conversations. Repeated failures back off from 15 seconds to 5 minutes without a total attempt limit. Completed, manually paused, user-input and permission-waiting conversations are excluded; ACK alone is not counted as recovery.

No ZCode installation patch, plugin, Python, or Node.js runtime is required by the packaged feature. Installation/data paths are discovered through existing settings. Manual account/lifecycle/path actions disable it. Recovery intents are in memory and are not replayed after the switcher exits.

The adapter was exercised against ZCode 3.14.4.7912, including a real isolated cold restart and a local mock model. It uses internal desktop services with explicit capability/schema checks, not a supported public ZCode API. Other versions may require an adapter update. The random loopback debugging port is accessible to other local software; enablement requires consent. Disable stops automation, and a later normal ZCode restart closes that debugging interface. Real Aliyun challenge recovery remains a separate external acceptance boundary.
