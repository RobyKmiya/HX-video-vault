# 影匣（VideoVault）交接单

> 交接：2026-11-27 03:30 ｜ 交接人：workbuddy（深天玥）
> 本文件是**自包含**的：新代理零上下文，从这份 + 第五节开工清单就能接手。
> 接手第一步：`node D:/DSH/agent-dispatch/board.mjs check --project yingxia` 核对登记，
> 再读 `projects/yingxia/state.json` 的 notes（每轮的最新差异都在里面）。
> 细节资料索引见 `docs/handoff/README.md`。

---

## 一、一句话现状

影匣已装 **0.1.27**（FileVersion 0.1.27 / exe mtime 2026-11-27 01:29:28 / 9,489,408 B），
看板登记一致（**源码 HEAD 以 `board.mjs check` 实读为准**，不要信本文里的哈希 —— 文档一提交就过期）。
**管理员 2026-11-27 03:03 实机验收通过**：重复清理面板、网格缩放与 Ctrl+滚轮、去重后交互全部确认可用。

0.1.27 这批交付的是 P0/P1 四项：清幽灵行 201 条（2149 → 1948，账面 113.5 GB 假占用消失）、
源码自动备份、视频网格缩放、**内容查重**（v7 迁移 + blake3 指纹，实测 57 组 / 35 GB，
59 个副本进回收站，剩余重复组 0）。

**没有半截功能。** 剩下的全是"需要人工点一遍"的实机验收项，和几件明确排期外的事（接触表补满、mpv 联调、hanime 插件）。
2026-11-27 03:58 起 `lib.rs` 已从 1893 行拆到 383 行：81 个命令按领域搬到 `src-tauri/src/commands/` 下 12 个子模块；04:30 起 `db.rs` 从 2876 行拆到 272 行：共享类型/helper 留原处，54 个业务函数 + 19 个测试搬到 `src-tauri/src/db/` 下 10 个子模块，`crate::db::xxx` 调用路径不变。两批都是纯结构重构未发版，exe 仍是 0.1.27。

**当前唯一源码 = `D:\VideoVault`**（内置盘）。E 盘源码副本已于 11-27 03:15 全部清空 —— E 盘是外置 USB NVMe，
出过两次 `ERROR_DEVICE_HARDWARE_ERROR(483)`，查明是线材/接触不良，但结论仍是：外置盘只放素材，不放唯一副本。

---

## 二、项目与工具链

| 项 | 值 |
|---|---|
| 源码 | `D:\VideoVault`（master） |
| 安装位置 | `C:\Users\Administrator\AppData\Local\影匣\video-vault.exe` |
| 技术栈 | Tauri 2 + Rust（`src-tauri/src/`）+ rusqlite 0.43 + WebView2 + React 19（`src/`） |
| 媒体库根 | `F:\Hthings\Supervideo`（数据库 `F:\Hthings\Supervideo\.vault\vault.db`） |
| 规模 | 后端 8,830 行 / 13 文件；前端 5,978 行 / 24 文件；Rust 单测 55 个 |
| 日常发版 | `node scripts/release.mjs`（补丁位 +1，自带检查→打包→装机；`--no-test` 只改前端时用，`--reopen` 装机后自动拉起） |
| 类型检查 | `npm run check`（tsc --noEmit） |
| 前端构建 | `npm run build`（tsc + vite build） |
| Rust 单测 | PowerShell 通道跑 `cargo test --manifest-path src-tauri/Cargo.toml --lib` |
| 命令行工具 | `npm run build:cli` → `src-tauri/target/release/vaultctl.exe` |
| 看板核对 | `node D:/DSH/agent-dispatch/board.mjs check --project yingxia` |
| 推水位 | `node D:/DSH/agent-dispatch/hooks/dsh-hook.mjs mark-synced --repo D:/VideoVault` |
| 广播 | `node D:/DSH/agent-dispatch/agentctl.mjs broadcast --as workbuddy --task T-VVxx --status 已完成 --title "…" --text "…"`（管理员要求：每完成一批实质工作**主动**广播，不等催） |
| 收尾三件套 | `projects/yingxia/state.json`（version/head/exeMtime/notes）+ `projects/yingxia/BOARD.md`（头部登记 + 时间线 + 待办）+ `prompts/31-yingxia.md`，然后 `board.mjs check` 必须"与登记状态一致" |

**环境注意**
- `cargo test` 在 Bash 通道会被 SIGTERM → **必须用 PowerShell 通道**。
- `cargo check` 会改 `Cargo.lock`，记得一并提交。
- **打包前必须手动关掉影匣**（Windows 锁住 exe 会让链接失败）。脚本刻意不调 taskkill —— 安全策略会拦。
- **影匣 GUI 开着时 `vaultctl` 会报 `attempt to write a readonly database`**，那不是权限问题，是 `video-vault.exe` 独占着库。跑 CLI 前先确认应用关了。
- 生产编译必须带 `--features tauri/custom-protocol`，漏了会编出连 `localhost:1421` 的开发版 exe。
- zip 去掉 lzma/xz（本机 lib.exe 1114 崩溃，LZMA 条目打不开）。
- 构建用 `CARGO_TARGET_DIR=D:/vv-target-rel`；提交若遇 EPERM，用 `git write-tree` + `commit-tree -F <msg>` + `update-ref`。

---

## 三、代码地图（新代理最容易迷路的模块）

### 后端 `src-tauri/src/`

| 模块 | 行数 | 职责 |
|---|---|---|
| `db.rs` | 272 | 只留 `SCHEMA_VERSION=7` / `VideoRecord` / `LibraryContext` / `VideoStamp` / 共享 helper（`now_secs` `map_video` `escape_like` `rel_dir_of` 家族等）+ `pub(crate) use` re-export。**业务函数已拆到 `src-tauri/src/db/` 下 9 个领域子模块**（schema / dedupe / videos / covers / tags / collections / folder / stats / downloads）+ `tests.rs`，`crate::db::xxx` 路径不变。`schema.rs` 含 `open_library(root)` 唯一入口、`migrate()` 逐版升级、`normalize_and_dedupe_videos()` 路径规范化与重复登记合并 |
| `lib.rs` | 383 | 只留 `Settings` / `AppInner` / `AppState`（`Arc<Mutex<AppInner>>`）+ settings 读写 + `library()` 辅助 + 单实例守卫 + `run()` 注册。**命令已拆到 `src-tauri/src/commands/` 下 12 个领域子模块**（settings / browse / collections / player / sheets / dedupe / downloads / editing / covers / imports / files / window），`mod.rs` 是共用导入 prelude；约 80 个命令，见下方分组 |
| `edit.rs` | 925 | 剪辑：裁剪/拼接/压缩，ffmpeg 前端最小封装。任务全程不持 AppState 锁 |
| `media.rs` | 659 | 探测/缩略图/指纹。`probe()` ffprobe 元数据；`contact_sheet()` 接触表（**要线性解码整片，实测 18 秒/条**）；`fingerprint()` = blake3(大小 + 首 8 MB + 尾 8 MB)，**不含 mtime**，为的是识别跨盘改名副本 |
| `importer.rs` | 524 | 拖拽导入。按 `<导入根>\0-199`、`200-399`… 每夹 200 条顺序装，删除产生的缺口由新导入**往前补**（优先填最小序号未满夹） |
| `downloader.rs` | 329 | 网页链接下载。HTTP 层用系统 curl（自带断点续传/TLS），**通用正则直链抽取**，无站点解析器、无 cookie、无 HLS |
| `models.rs` | 295 | `VideoRecord` 等结构。压缩包里每个视频条目都是独立一条（画匣是"一本=一个文件"，视频不能照搬） |
| `fileops.rs` | 284 | 重命名/移动/资源管理器定位。同盘走 `fs::rename` 元数据操作（毫秒级），跨卷才真复制 |
| `mpv.rs` | 256 | mpv JSON IPC 控制层。mpv 是**独立顶层窗口**（`--force-window`，不是嵌在 webview 里） |
| `player.rs` | 240 | 播放解析。`resolve()` 把条目解析成真实媒体路径（压缩包条目在这里解出来）；`backfill_meta()` 补时长/分辨率 |
| `proc.rs` | 40 | 子进程统一入口。设 `CREATE_NO_WINDOW` 避免 ffprobe/ffmpeg/curl 在屏幕上闪一片黑框 |
| `bin/vaultctl.rs` | 504 | 本地 JSON 接口，供脚本/agent 自动化。子命令见第四节 |

**命令分组（`lib.rs` 的 `generate_handler!`）**
- 库与设置：`app_settings` `set_library` `update_settings` `pick_directory` `scan` `stats`
- 浏览：`query_videos` `get_video` `patch_video` `list_folder_children` `video_cover`
- 标签/收藏夹：`list_tags` `create_tag` `update_tag` `set_tag_video` `bulk_tag` `list_collections` `create_collection` `rename_collection` `set_collection_group` `delete_collection` `restore_collection` `set_collection_video` `reorder_collections` `reorder_collection_videos`
- 播放：`playback_info` `play_external` `record_watch` `watch_history` `player_engines` `mpv_open` `mpv_command` `mpv_close`
- 封面/接触表：`make_contact_sheet(s)` `cancel_contact_sheets` `clear_contact_sheets` `sheet_status` `reset_folder_covers` `set_cover_image` `set_cover_from_time` `set_cover_from_mpv` `mpv_cover_path` `clear_cover`
- 文件编辑：`rename_video` `move_videos` `move_targets` `reveal_video` `trash_video` `import_videos` `import_status` `cancel_import`
- 剪辑：`trim_video` `concat_video` `compress_videos` `edit_status` `cancel_edit` `probe_media` `pick_video_files`
- 下载：`resolve_link` `list_downloads` `add_download` `remove_download` `start_downloads`
- 窗口：`set_immersive` `set_fullscreen` `hide_main_window` `get_boss_recall` `set_boss_recall`
- 查重（v7 新增）：`dedupe_scan` `dedupe_status` `dedupe_groups` `dedupe_apply` `dedupe_apply_selected` `purge_missing_trashed`

### 前端 `src/`

| 文件 | 行数 | 职责 |
|---|---|---|
| `App.tsx` | 1951 | 视图状态机 + 侧栏 + 网格 + 工具栏。**28 个 useEffect，是全仓最挤的文件，动之前先看清依赖** |
| `components/Player.tsx` | 798 | 播放页。mpv 独立窗口 + 内嵌详情面板三态（384 常驻 / 780 加宽 / 58 折叠） |
| `components/DetailDrawer.tsx` | 548 | 详情抽屉。内联重命名、标签、封面、移动 |
| `types.ts` | 323 | 前后端共享类型 |
| `components/EditPanel.tsx` | 294 | 剪辑面板（三入口：侧栏/详情/批量栏） |
| `components/Sidebar.tsx` | 227 | 侧栏（收藏夹 + 磁盘文件夹镜像双区） |
| `components/DedupePanel.tsx` | 176 | 重复清理面板（进度条/分组明细/逐条勾选/一键清理） |
| `components/DownloadPanel.tsx` | 165 | 下载队列 |
| `api.ts` | 159 | 所有 `invoke` 的类型化封装，**加命令先来这里** |
| `components/VideoCard.tsx` | 137 | 卡片。进视口 600 px 内才请求中帧封面 |
| `shortcuts.ts` | 131 | 可绑定快捷键 |
| `cover.ts` | 116 | 封面加载与缓存键 |
| `components/ContextMenu.tsx` | 109 | 通用右键菜单（贴边回弹、Esc 不误退播放） |
| `components/Splitter.tsx` | 97 | 可拖分界线 |
| `layout.ts` | 77 | 布局尺寸持久化 |
| `gestures.ts` | 77 | 鼠标手势槽位（双击 + Ctrl+滚轮） |
| `components/Toast.tsx` | 39 | 分级轻提示（**onClose 不要传内联箭头函数**，会让自动消失的定时器反复续期） |

---

## 四、数据模型与目录约定

**库根**：`F:\Hthings\Supervideo`（settings.json 的 `library_root` 与 `import_root` 都是它）
- `.vault\vault.db` 数据库（WAL 模式），`.vault\cache\contacts\` 接触表缓存，`.vault\backups\` 每日库备份
- 视频按区间夹存放：`0-199`、`200-399`…每夹 200 条，`pick_target` 挑升序第一个未满夹

**schema v7 七张表**

| 表 | 关键字段 |
|---|---|
| `videos` | `path` `file_name` `size_bytes` `mtime` `duration_sec` `width/height` `codec_*` `entry_index/entry_count` `added_at` `last_watched_at` `watch_count` `progress_sec` `completed` `rating` `note` `favorite` `trashed` `contact_sheet` `tag_text` + **v7 新增 `fingerprint`（空=未算）与 `dedup_of`（0=自己是保留项）**；唯一键 `(path, entry_index)` |
| `tags` / `video_tags` | 标签与多对多关联 |
| `collections` / `collection_videos` | `kind` 区分 `custom`（收藏夹，可编辑）与 `auto`（磁盘文件夹镜像，只读）；`group_name` `sort_order` `position` |
| `watch_history` | 观看流水（`from_sec`/`to_sec`/`finished`） |
| `video_covers` | 自定义封面，按 video_id 缓存 |
| `download_tasks` | 下载队列（`state` `progress` `bytes_done/total` `error`） |

**两条容易踩的约定**
1. **`entry_index > 0` 的记录数 = 0** —— 压缩包多集条目模型实际未启用，全是散文件。去重按整文件内容哈希即可，不用管条目边界。
2. **收藏夹 = custom（可编辑/拖拽/多夹并存），文件夹 = auto（磁盘结构镜像，只读）**。两者全链路解耦：详情 pills、管理面板、批量下拉都不该出现 auto。

**vaultctl 子命令**：`init` `stats` `search` `video` `tags` `collections` `scan` `import` `watch` `downloads` `probe` `dedupe`（`--limit/--apply/--purge`）`sheets`（3 worker 并发补接触表）
- ⚠️ **绝不能对区间夹子目录跑 `vaultctl scan --root <子目录>`**：`mark_missing` 是全局的（`path NOT IN _seen`），会把库内其它视频全标失效。只能 `import --paths`，或对库根做全量 scan。
- ⚠️ **vaultctl.exe 必须与库 schema 齐平**。库升 v7 后，只支持到 v6 的旧二进制会让 import/scan 全部失败。重编：`npm run build:cli`。

---

## 五、开工清单

**A. 需要管理员配合的（我做不了）**

1. **上传 GitHub**（管理员已定：后续云端备份走 GitHub）——仓库已就绪：91 个跟踪文件 / 1.7 MB，`.gitignore` 挡住 `target`/`node_modules`/`dist`/`*.db`/`*.db-shm`/`*.db-wal`/`tools/we-export/config.json`。建库后 `git remote add origin <url>` + `git push -u origin master`。**传成功之前，容灾只剩本地 `D:\VideoVault-backups` 一份（2.2 MB），别无副本。**
2. **接触表补满**：还缺 1,204 条（683/1,887）。单个 `contact_sheet` 要线性解码整片（18 秒/条），全量约 6 小时；`vaultctl sheets` 3 worker 可压到 2 小时。**需要先关掉影匣**（GUI 独占库）。跑完大夹首次打开的抽帧排队会明显缓解。
3. **历史 P0 实机验收**（第七节清单），全部只差人工点一遍。

**B. 待排期功能**

4. **hanime 插件**（管理员点名，当前全仓 `hanime` 零匹配）：抓视频直链、自动下载最高画质、采集 tag/作者/原链接进详情。建议抽 `SiteResolver` trait（`matches`/`resolve`），通用正则作 fallback，新站点只加 impl 不动主干；凭据走 Windows 凭据管理器或 DPAPI，**绝不进代码/明文库/日志/广播**。
5. **mpv 真机联调**：本机**还没装 mpv**，`mpv.rs` 只有 IPC 协议层 + 自测，装完要跑一次真实回传。
6. **拆巨头文件**：`App.tsx`(1951) / `db.rs`(2876) / `lib.rs`(1893) 三个吃 45% 体量，需单独批次做，别混在功能里。

**C. 架构方向（等画匣 1.2.0）**

7. **影匣 ↔ 画匣合并为"画影匣"**：已调研（`docs/handoff/03-接入画匣-调研与导入方案-20261126.md`），结论走**启动器**路线——双库分离、界面独立、播放器独立、收藏夹仅记名。**当前不动**，等画匣 1.2.0（网络浏览器一体化）落地后再评估。最大技术风险是 mpv 独立顶层窗口在合并后的窗口管理。

---

## 六、踩过的坑（每一条都真实翻过车）

**数据层**
1. **幽灵行**：回收站里的记录文件早没了，账面仍占 113.5 GB。`purge_missing_trashed` 清掉 201 条。统计"占用"永远别只看 DB 的 `size_bytes` 求和。
2. **v6 迁移把索引建在初始 batch 里会炸**：老库 `videos` 表已存在但没 `fingerprint` 列，批次里 `CREATE INDEX` 直接报 `no such column`。**索引必须放进 v7 迁移、补完列之后。**
3. **库版本单向**：库升到 v7 后，只认 v6 的 0.1.26 会打不开库（GUI 直接起不来）。改 schema 必须同步发版，别让旧版本碰到新库。
4. **`UNIQUE(path, entry_index)` 拦不住伪重复**：同一部片子在不同盘/改名/快捷方式引用会成多条。真重复要靠 `fingerprint`（blake3 大小+首尾各 8 MB，**不含 mtime**）。
5. 扫描要跳过符号链接与 `.lnk`，否则快捷方式指向的文件会被当成库内独立条目重复登记。

**性能**
6. **"分文件夹防卡顿"是失效假设**：扫描是递归全库，物理分夹对扫描零收益。真正的卡顿来自四层：全库递归、接触表缺失率高（现场抽帧）、文件夹 2×2 拼贴的 N+1 查询、重复条目放大工作量。
7. **接触表极其昂贵**：`contact_sheet` 要线性解码整片，18 秒/条。任何批量任务都要并发 + 可暂停 + 可续跑，别串行硬等。
8. 网格必须窗口化渲染（`calcGridMetrics` + `--card-min`/`--grid-gap` 两个 CSS 变量），卡片进视口 600 px 内才请求中帧封面，否则 1900 条一次性渲染直接卡死。

**前端**
9. **React #310**：`App.tsx` 里任何 hook 都必须放在 `if (!ready) return` **早退之前**。侧栏宽度的 `usePersistentSize` 曾放在早退之后，启动白屏（0.1.26 踩过）。
10. **Toast 的 `onClose` 不要传内联箭头函数**，每次渲染都是新引用，会让自动消失的定时器反复续期、永不消失。
11. 模态让位总线只接了设置面板，其它模态都漏了 —— 加新模态时检查一遍。

**窗口/播放器**
12. **mpv 是独立顶层窗口**（`--force-window`，前端不传 `wid`）：藏主窗时画面会残留（`hide_main_window` 已先 `mpv.stop()`）；改主窗尺寸也**不会**带动 mpv 窗口位置。
13. 沉浸（`set_immersive`）仍动系统窗口（不同于画匣 1.1.3 起的纯 CSS 方案），因为双击必须一步到位变无边框。晃动已用"最大化状态下取消最大化→切装饰→再最大化"压到最小。
14. 全局热键（老板键）注册可能失败：`set_boss_recall` 是**注册成功才落盘**，失败回滚设置面板的绑定并给中文原因。

**工具链**
15. `PowerShell 5.1` 读**无 BOM** 的 UTF-8 脚本会按 GBK 解析，中文注释里的引号会破坏语法 → 脚本必须存 UTF-8 with BOM。
16. `robocopy` 的 `/XD` 传相对路径不生效，必须 `Join-Path` 拼绝对路径，否则 `target`（31 GB）照样被复制。
17. **本环境回收站通道不可用**：`Add-Type` 与 `[Reflection.Assembly]::LoadWithPartialName` 都被安全策略拦，拿不到 `Microsoft.VisualBasic.FileIO.FileSystem`；`.vbs` 被归为 LOLBin 不许写。删除只能 `Remove-Item -Recurse -Force`，**删前必须自己确认活体 + 备份双份在手**。
18. `Get-PSDrive` 的 `Free` 有缓存，删完大目录要用 `Get-Volume` 或 `fsutil volume diskfree` 重读。
19. **改源码位置时，备份三件套必须一起改**：脚本默认参数、自启项、automation 的 prompt + `cwds`。11-27 就因为三处全指着旧路径（E 盘），每日备份静默断流一整轮——脚本报错被 `-Quiet` 吞掉，automation 因 `cwds` 不存在连启动都不启动，两条路都不报警。

---

## 七、待管理员实机验收（全部已装机，只差人工点）

- [ ] 剪辑：面板三入口、裁剪秒出与精确模式、混合参数拼接重编码、取消与进度条、成品入库（0.1.10）
- [ ] 封面：卡片中帧铺满观感、播放页"设为封面"（内置 + mpv 双链路）、恢复默认、详情"内容走向"（0.1.11/0.1.12）
- [ ] 收藏夹：侧栏拖拽排序、夹内卡片拖拽、红心→我的收藏联动、详情内联新建（0.1.14）
- [ ] 文件夹镜像：扫描后侧栏出现磁盘子目录树、移动/删除磁盘文件夹后重扫自动同步（0.1.15）
- [ ] 启动屏：暗色品牌动画、阶段文案与真实进度、淡出转场（0.1.17）
- [ ] 层级浏览：主页文件夹识图卡、面包屑、进夹自动切手动排序（0.1.18）
- [ ] 导航历史：侧键返回/前进跨层级回退、播放页侧键返回（0.1.19）
- [ ] 0.1.24 界面：16:9 对齐网格、文件夹模式层层进入、双击播放、沉浸无边框（Esc 先退沉浸）、右键多选、合集画廊、快捷键录制
- [ ] 文件编辑：详情内联重命名、MoveDialog 批量移动、资源管理器定位、18 GB 同盘移动手感
- [ ] 0.1.25：最近入库是否跨夹按最新在前、进夹排序保持、文件夹卡"最新入库"排序、网格滚动手感、首屏预热不卡
- [ ] 0.1.26：双击画面进沉浸、顶部拖动热区、右下角还原点、**播放器右键菜单各项**、侧栏拖宽记忆、老板键藏窗与 Ctrl+Shift+H 呼回
- [ ] 鼠标手势（滚轮音量 / 横滚 ±5s / 侧键切条）——`T-VV04` 明确写了"侧键在 WebView2 能否收到 = NOT TESTED"
- [ ] 播放页内嵌详情面板三态手感（384 / 780 / 58 + 拖拽调宽）
- [ ] WE 导出插件：壁纸引擎里右键"导出视频到影匣…"，看是否真落到 `F:\Hthings\Supervideo\<区间夹>`
- [ ] 旧接触表重算：预览图面板点"全部重做"，让全库接触表变成自适应格数

---

## 八、版本史速查（0.1.24 → 0.1.27，更早见 BOARD 时间线）

| 版本 | 一句话 |
|---|---|
| 0.1.24 | 文件编辑/排序重构/16:9 网格（T-VV18） |
| 0.1.25 | 最近入库视图 + 网格窗口化 + 进视口才抽帧 + v6 路径规范化去重；**库根收敛到 `F:\Hthings\Supervideo`** |
| 0.1.26 | 画匣特性迁移：真无边框沉浸 / 播放器右键菜单 / 侧栏可拖宽 / 老板键 / 手势槽位 / Toast 分级；修 React #310 白屏 |
| 0.1.27 | 清幽灵行 201 条 / 源码自动备份 / 网格缩放 / **内容查重（v7 + blake3）**；**管理员验收通过** |

---

## 九、新窗口开场

> 「读 `D:\VideoVault\HANDOFF.md` 交接单，先跑 `board.mjs check --project yingxia` 核对，
> 然后从第五节开工清单挑一件干。管理员说：验收通过的那批已完成，剩下的等排期。」

管理员习惯：喜欢当天闭环、每轮可验；发版后**不自动拉起应用**，留他手动检查；
反馈常带截图；重要架构决策会自己拍板（如"库根只选 `F:\Hthings\Supervideo`"、"先不合并，等画匣 1.2.0"）。
工作语言中文，讲究"干活不报流水账，做完留一句人话"。

---

## 十、凭据与安全

交接文本、代码、日志、广播里**都不写 cookie / token / 密码**。
下载器的登录态设计是走 Windows 凭据管理器或 DPAPI 加密配置，**不入库明文、不落日志**。
（管理员曾贴过一份 hanime cookie 进对话，已提醒重登使其失效；任何新凭据一律按上述规则处理。）
