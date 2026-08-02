# Capture Agent

第一版被动窗口采集器。Rust 负责窗口取帧、job 生命周期和输出协议，并通过 `app/lib` 内的 FFmpeg 动态库与 OpenH264 编码器生成 H.264/MKV 视频分片。

当前已经实现：

- `test-pattern`：在无图形环境中验证完整视频链路；
- Linux X11、Windows 和 macOS 原生窗口枚举与被动捕捉；
- 默认 10 FPS、每 5 秒一个 MKV 分片；
- job 目录中的 `meta.json`、`events.json` 和 `videos/`；
- 同一个 job 重新启动后从下一个视频序号继续，不覆盖旧分片；
- `cargo xtask` 统一开发、测试和打包入口。

## 当前边界

版本 `0.2.0` 是跨平台开发预览版，还没有实现：

- Linux Wayland Portal + PipeWire；
- Windows Graphics Capture 流式后端（当前使用 `PrintWindow`）；
- macOS ScreenCaptureKit 流式后端（当前使用窗口截图接口）；
- 后续长图、OCR 和消息事件处理器。

采集器不持久化 PNG 帧。帧仅作为内存中的编码输入，落盘原始数据是 MKV 视频。

## 环境

开发构建需要 Rust 稳定工具链、vcpkg 和各平台 GUI/窗口 API 的构建环境。`cargo xtask package` 会把产品采集实现编译为 `app/lib/capture-runtime`，并把它依赖的 FFmpeg/OpenH264 动态库收集到同一目录。`bin/capture-agent` 与 `bin/window-enumerator` 只是加载该运行时的启动壳。运行时只解析采集器自身相邻的 `../lib`，不会搜索系统 `PATH`；目标电脑不需要安装 Python、GStreamer 或 FFmpeg。

检查环境：

```bash
cargo xtask doctor
```

X11 后端使用纯 Rust `x11rb`，不要求链接 `libX11` 开发包。FFmpeg 和 OpenH264 由固定 vcpkg manifest 构建为动态库，许可证文件随包放入 `app/licenses`。

## 统一入口

```bash
cargo xtask doctor
cargo xtask test
cargo xtask build
cargo xtask package --target current
```

发布目录：

```text
dist/{version}/{target}/
└── app/
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

`app/` 是解压即用的权威产物，安装器只能重新封装它。`xtask` 不允许在一个操作系统上生成另一个操作系统的完整包。

## GitHub 三平台发布

`.github/workflows/release-portable.yml` 会在一次工作流中并行启动三个原生构建：

```text
ubuntu-latest   -> linux-x64
windows-latest  -> windows-x64
macos-26        -> macos-arm64
```

每个平台都执行测试、生成同一结构的 `app/` 并压缩为 ZIP。三个 ZIP 会保留为 14 天的 Actions Artifacts；全部成功后，再上传到同一 GitHub Release。

发布标签必须与 Cargo 版本一致，例如当前版本使用：

```bash
git tag v0.2.0
git push origin v0.2.0
```

也可以在 GitHub Actions 页面手动运行 `Build portable apps`，并输入 `release_tag=v0.1.0`。同名 Release 已存在时会覆盖更新其中的三个 ZIP 附件。

## 无图形环境演示

确定性测试画面可以验证真实 H.264/MKV 输出：

```bash
cargo xtask demo \
  --segment-seconds 5 \
  --segments 1 \
  --output capture-data
```

`xtask` 会在输出根目录下生成唯一的 `demo-<unix_ms>` job 目录。快速测试可以把 `--segment-seconds` 改为 `1`。

## X11 窗口捕捉

```bash
cargo run -p capture-agent --bin capture-agent -- doctor

cargo run -p capture-agent --bin window-enumerator -- \
  snapshot \
  --source x11

cargo run -p capture-agent --bin capture-agent -- \
  run \
  --source x11 \
  --window-id 0x4600007 \
  --job-id 01J4JOB \
  --segment-seconds 5 \
  --fps 10 \
  --video-bitrate-kbps 2048 \
  --segments 0 \
  --output "$HOME/Videos/OmniInlet/2026-08-02/01J4JOB"
```

`--output` 是最终 job 目录，不是其父目录；未传 `--job-id` 时，采集器使用输出目录最后一级名称。`--segments 0` 表示持续捕捉，按 `Ctrl+C` 后安全封装当前分片并停止。

X11 后端不能捕捉原生 Wayland 窗口。Wayland 后端需要 XDG Desktop Portal 和 PipeWire。

## 数据目录

```text
Videos/OmniInlet/
└── 2026-08-02/
    └── 01J4JOB/
        ├── meta.json
        ├── events.json
        └── videos/
            ├── 00000001.mkv
            ├── 00000002.mkv
            └── 00000003.mkv
```

- `meta.json`：标准 JSON，固化窗口信息、捕捉及编码分辨率、帧率、容器、编码格式、实际编码器、像素格式、码率、关键帧间隔和分片时长。H.264 要求偶数尺寸时，只在右侧或底部补一个像素，并记录实际编码尺寸。
- `events.json`：标准 JSON，`events` 数组记录 job 启停、恢复、采集间断和每个已提交视频分片的路径、时间、帧数与字节数。
- `videos/`：默认每 5 秒一个 H.264/MKV 文件。分片先写临时文件，编码完成后原子改名。

OCR 聊天消息不写入 `events.json`，而由后续处理系统从视频分片产生。

## 验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask demo --segment-seconds 1 --segments 1
cargo xtask package --target current
```
