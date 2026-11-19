# 影匣 · Video Vault

本地离线视频库管理器 + 播放器。把散落在硬盘各处的视频与压缩包，收进一个可检索、有记忆、只在本机的私人片库。

## 项目坐标

| 项 | 值 |
| --- | --- |
| 工程目录 | `E:\VideoVault`（独立 Git 仓） |
| 技术栈 | Tauri 2 · Rust 2024 · React 19 · TypeScript · SQLite（rusqlite bundled） |
| 库数据 | `<库根目录>\.vault\vault.db`，默认指向 `E:\`（Remdemption） |
| 视频源 | 库根目录下递归扫描；ZIP / CBZ 内视频逐条登记 |
| 播放器 | 内置播放器（mp4/webm 等） + 本机 VLC 3.0.21 兜底（冷门格式） |
| 协调入口 | **`D:\Qoderwork\docs\施工日志.md`**（画匣 · 影匣统一施工日志，所有 agent 施工前必读） |

> 本项目与画匣（archive-gallery）**不共享代码**、不共享数据库。只共享同一份施工日志的纪律与队列。

## 已落地（v0.1.0 骨架）

- [x] SQLite schema v1：视频条目、标签、目录、观看历史、下载任务预留表
- [x] 递归扫描：视频文件 + ZIP/CBZ 内视频条目，ffprobe 提取时长/分辨率/编码
- [x] 每日一致性快照备份（滚动 3 份）
- [x] 搜索（文件名/路径/备注/标签）、组合筛选、7 种排序、分页加载
- [x] 评分、收藏、备注、完成状态、观看次数与秒级进度记忆
- [x] 标签中心（未分类置顶、分类折叠、权重批量）、批量打标、目录增删
- [x] 统计面板（库容量、观看覆盖、最近入库、标签分布）
- [x] 接触表预览（ffmpeg 4×4 抽帧）、内置播放器（倍速/音量/截图/全屏/续播）
- [x] 外部 VLC 播放（从上次位置续播）
- [x] mpv JSON IPC 内核（字幕/倍速/截图/进度事件，未安装时自动降级）
- [x] 批量接触表生成 + 观看记录报表 + 库容量按文件去重
- [x] 网页链接下载器（curl 解析直链 / 断点续传 / 联网自动续跑 / 下载后自动入库打标）
- [x] `vaultctl` CLI：stats / search / video / tags / collections / scan / import / watch / downloads，全 JSON 输出

## 开发

```bash
npm install
npm run check          # TypeScript
cargo test --offline --manifest-path src-tauri/Cargo.toml --all-targets   # 本机需 --offline
npm run build:cli      # vaultctl 发布版
npm run tauri dev
npm run release        # 独立发版：抬版本 → 检查 → 打包 NSIS → 装到 %LOCALAPPDATA%\影匣
npm run release:dry    # 只预演不发版
```

> **发版隔离**：`npm run release` 只操作本仓库与 `%LOCALAPPDATA%\影匣`，**不碰画匣**（不读画匣库、不杀画匣进程）。打包前请先手动关掉正在运行的影匣，否则 Windows 会锁住 exe 导致链接失败。

ffmpeg / ffprobe 默认在 PATH 和 `D:\MESAVAGE\Tools\ffmpeg` 探测，也可在设置里指定。
下载用系统 curl（Windows 自带）；mpv 可选，未安装时播放器自动降级到内置/VLC。

### vaultctl 示例

```bash
src-tauri/target/debug/vaultctl.exe --library E:\影匣库 stats --compact
src-tauri/target/debug/vaultctl.exe --library E:\影匣库 search --query 风景 --compact
src-tauri/target/debug/vaultctl.exe --library E:\影匣库 import --paths D:\a.mp4 --compact
```

## 路线图

见 [`docs/影匣需求与架构方案.md`](docs/影匣需求与架构方案.md) 与统一施工日志 G 队列。
