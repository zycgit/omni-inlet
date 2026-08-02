# OmniInlet

OmniInlet（全域消息汇流平台）把来自不同聊天渠道的消息和上下文统一转换为可持续消费的事件流。

项目规划包含两类数据入口：

- 官方 API 连接器，用于接入开放接口提供的消息与成员信息。
- 视觉采集代理，用于在专用电脑上被动记录指定软件窗口，后续交给本地视觉模型完成分段、OCR 和身份补全。

统一事件流之上可以继续构建检索、提醒、业务对象生成和自动化处理等应用。

## 当前实现

第一版先实现 `capture-agent/` 视觉采集子系统：

- `omni-inlet`：当前捕获命令总入口，随采集子程序安装到 `app/bin/`。
- `capture-agent`：接收窗口 ID、输出目录和采集配置的无界面采集进程。
- `window-enumerator`：枚举应用窗口并生成窗口快照信息。
- `cargo xtask`：测试、构建和绿色目录打包的统一开发入口。

当前阶段仍是开发预览版。Linux X11、Windows 和 macOS 均已实现原生窗口枚举与捕捉；视频编码使用 `app/lib` 内随包提供的 FFmpeg/OpenH264 动态库，不读取系统 `PATH`，也不要求目标电脑安装 FFmpeg。Windows WGC、macOS SCStream 与 Linux Wayland 后端仍需继续完善。

## 本地验证

```bash
cd capture-agent
cargo xtask doctor
cargo xtask test
cargo xtask demo
cargo xtask package --target current
```

绿色软件生成到：

```text
capture-agent/dist/<version>/<target>/app/
├── bin/
│   ├── omni-inlet
│   ├── capture-agent
│   └── window-enumerator
├── lib/
│   ├── capture-runtime.*
│   └── FFmpeg/OpenH264 动态库
├── resources/
└── licenses/
```

完整桌面版本将由 `launcher/` 生成根目录的 `app/omni-inlet` 图形界面入口。根入口与 `bin/omni-inlet` 可以同名：前者负责 GUI 交互和进程监督，后者负责捕获命令调用。`bin/capture-agent` 和 `bin/window-enumerator` 都是薄启动壳；窗口枚举、采集任务和视频编码等产品实现位于我们自己的 `lib/capture-runtime`，FFmpeg/OpenH264 是同目录下的第三方运行库。

GitHub Actions 在版本标签推送后，分别在 Linux、Windows 和 macOS 原生运行器上构建 ZIP，并上传到对应 GitHub Release。正式 Windows Release 会在压缩前通过 SignPath Foundation 对项目自有的 EXE 和 DLL 执行 Authenticode 签名，并验证所有签名；签名不可用时发布会直接失败，不会降级为未签名附件。普通 CI 构建物仍是未签名的开发预览包。

## Code signing policy

Free code signing is provided by [SignPath.io](https://signpath.io/), certificate by [SignPath Foundation](https://signpath.org/). 项目维护者角色、签名范围、验证方式和隐私边界见[代码签名策略](docs/code-signing-policy.md)。签名服务的一次性配置见 [SignPath 配置说明](docs/signpath-setup.md)。

## 许可证与隐私

OmniInlet 采用 [Apache License 2.0](LICENSE) 开源。当前版本只在用户明确选择后将窗口捕获结果写入本地目录，不主动向联网系统传输捕获内容；完整说明见[隐私策略](docs/privacy.md)。

绿色软件不会写入系统级安装信息。卸载时先停止所有采集任务，再删除解压得到的 `app/` 目录；已经生成的捕获任务目录由操作者根据自己的保留策略单独删除。

## 文档

- [技术架构](docs/technical-architecture.md)
- [引导界面与运行架构](docs/launcher-ui-and-runtime-architecture.md)
- [代码签名策略](docs/code-signing-policy.md)
- [隐私策略](docs/privacy.md)
