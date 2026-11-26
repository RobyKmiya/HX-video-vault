# 壁纸导出扩展（Wallpaper Engine → 影匣）

在 Wallpaper Engine 的「已安装」列表里右键任意壁纸 → **导出视频到影匣…**：
自动找到这个壁纸的**源地址文件夹**，把文件夹里的视频文件导出来，落到**影匣（VideoVault）数据目录**，并登记入库。

支持单选和多选（Ctrl 点选 / Shift 范围选）。

---

## 插件的正确定位（三段式）

1. **找到壁纸源地址文件夹** —— WE 的 `file` 字段有两种语义，助手会多路验证：
   - 本地项目：`myprojects/<id>_/<视频名>.mp4`（相对 `<WE>/projects`）
   - 创意工坊：`…/workshop/content/431960/<workshopid>/<视频名>.mp4`（绝对路径）
   两条都命中不了时，还���按标题在本地项目库里兜底匹配。
2. **导出该文件夹内的视频文件** —— 递归枚举（深度 4 层），一个文件夹里有几个视频就导几个。
   scene / web 类壁纸不再被前端一刀切掉，它们的项目文件夹里同样可能藏着视频。
3. **落到影匣数据目录** —— 按影匣自己的区间夹规则分配：
   升序找第一个未满的夹（每夹 200 个），漏缺往前补；全满则开新区间。
   落盘后用 `vaultctl import` 登记入库，再后台跑一次全库 `scan` 把文件挂进对应的目录集合。

## 前置说明（重要）

Wallpaper Engine **没有官方插件 API**。官方文档里只有壁纸创作和故障排查，安装目录的 `plugins/led` 是它自家的 LED 灯效 DLL。

所以这个扩展是**改它的 UI 实现**的：WE 的界面是 CEF + AngularJS 渲染的明文代码，本工具往 `ui/dist/index.html` 注入一个 `<script>`，在右键菜单里加一项。

代价很明确：

- **Steam 更新壁纸引擎后会被覆盖**，重新跑一次 `node install.mjs` 即可（原文件有备份，卸载能还原）
- 属于非官方改造，出问题删掉补丁就好

## 怎么用

1. 打开壁纸引擎 → 切到「已安装」
2. 选中一个或多个壁纸（Ctrl 点选 / Shift 范围选）
3. 右键 → **导出视频到影匣…**（多选时用顶部工具条的「导出视频」按钮）
4. 右下角弹通知告诉你导出了几个、落在哪个区间夹

## 组件

```
we-export-injector/
├── inject/export.js     注入到 WE UI 的脚本（加菜单项 + 收集壁纸信息 + 调助手）
├── helper.mjs           本地助手：定位源文件夹 → 枚举视频 → 落影匣 → 登记入库
├── vault.mjs            影匣落点集成（复刻 importer.rs 的分桶与命名规则）
├── supervise.mjs        守护进程：拉起 helper，崩了重拉，卡了重启
├── install.mjs          打 UI 补丁 / 部署助手 / 设置自启
├── config.json          配置
├── 启动助手.vbs          手动启动助手（双击即可）
└── bin/
    ├── start-helper.vbs 隐藏窗口启动守护进程（node 多路径探测）
    └── notify.ps1       完成通知
```

### 为什么需要助手

WE 的 UI 跑在浏览器沙箱里，**没有文件系统权限**，复制文件这件事只能交给外面这个进程。它只监听 `127.0.0.1:51789`。

### 为什么需要守护进程

之前助手是「开机自启拉起的一次性进程」：node 路径一失效、或者进程自己崩了，就永久没人管，
结果就是 WE 里点导出报「**连不上导出助手（127.0.0.1:51789）**」。

现在的结构：

```
注册表 HKCU\...\Run  ─┐
                       ├─→ supervise.mjs ─→ helper.mjs
Startup 文件夹 VBS  ──┘      （崩了重拉 / 卡了重启 / 每 15 秒巡检）
```

- **自启**：注册表 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`（用户级，无需管理员）。
  Startup 文件夹的 VBS 作为兜底，它自带 node 多路径探测。
  VBS 每次启动都会往 `logs\autostart.log` 写一行标记，事后能查「到底有没有触发」。
- **守护**：`supervise.mjs` 盯住 helper，退出就重拉、探 `/ping` 探不到就重启。

## 配置 `config.json`

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `vaultLibrary` | 影匣桌面端 `settings.json` 的 `library_root` | 影匣数据目录（落点根） |
| `vaultctl` | 自动探测 `E:/VideoVault/src-tauri/target/release/vaultctl.exe` | 影匣 CLI，用来登记入库 |
| `autoScan` | `true` | 导出后后台全库 scan，把新文件挂进目录集合 |
| `library` | WE 的 `projects/myprojects` | 本机壁纸库路径 |
| `steamLibrary` | 创意工坊 `431960` 目录 | 订阅壁纸的位置 |
| `weRoot` / `projectsRoot` | 自动探测 | 用于解析本地壁纸的相对路径 |

## 常见问题

**右键没有「导出视频到影匣…」？**
- 补丁没生效：完全退出壁纸引擎（托盘图标 → 退出）后重开
- 确认 `ui/dist/index.html` 里有 `scripts/_wx/export.js` 这一行（跑 `node install.mjs --status` 看）

**点了导出提示「连不上导出助手」？**
- 跑 `node install.mjs --status` 看「助手服务」和「注册表自启」两行
- 手动拉起：`node "%LOCALAPPDATA%\WeExportHelper\app\supervise.mjs"`
- 看 `%LOCALAPPDATA%\WeExportHelper\logs\supervisor-日期.log` 和 `helper-日期.log`

**Steam 更新后菜单消失了？**
正常，重跑 `node install.mjs`。

**导出后中文名变成乱码？**
已修（`fs.cpSync` 在 Windows 上会按 GBK 误解码中文路径，改成 `copyFileSync` 逐个复制）。

**导出的视频影匣里看不到？**
- `vaultctl` 二进制比影匣库版本旧时会报「库版本 6 高于本程序支持的 5」，重编即可：
  `cd E:\VideoVault\src-tauri && cargo build --release --bin vaultctl`
- 目录集合（collection）靠后台 `scan` 同步，全库扫一次约 40 秒，日志在 `logs\vault-scan.log`

**能导出创意工坊的壁纸吗？**
能，导出的是你本地已下载的视频文件。自己备份/迁移没问题，但别二次分发别人有版权的作品。

## 卸载

```bash
node install.mjs --uninstall
```
会把 `index.html` 还原（从 `.wx-backup`）、删掉注入脚本、移除注册表自启和启动文件夹快捷方式。
再删掉本目录和 `%LOCALAPPDATA%\WeExportHelper` 即可。

## 日志

- 助手日志：`%LOCALAPPDATA%\WeExportHelper\logs\日期.log`
- 守护日志：`%LOCALAPPDATA%\WeExportHelper\logs\supervisor-日期.log`
- 自启触发标记：`%LOCALAPPDATA%\WeExportHelper\logs\autostart.log`
- 影匣扫描日志：`%LOCALAPPDATA%\WeExportHelper\logs\vault-scan.log`
- 前端探针/日志：`%LOCALAPPDATA%\WeExportHelper\probes\`
