//! vaultctl — 影匣的本地 JSON 接口，供 AI agent / 脚本自动化使用。
//! 与画匣 galleryctl 同一风格：所有输出都是 JSON，--compact 可压缩。

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use video_vault_lib::db::{self, LibraryContext};
use video_vault_lib::media;
use video_vault_lib::models::{VideoPatch, VideoQuery, WatchUpdate};

#[derive(Debug, Parser)]
#[command(
    name = "vaultctl",
    version,
    about = "Local JSON interface for AI agents managing a video-vault library"
)]
struct Cli {
    #[arg(long, global = true, env = "VIDEO_VAULT_LIBRARY")]
    library: Option<PathBuf>,
    #[arg(long, global = true)]
    compact: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 初始化或打开一个库（默认当前目录）
    Init {
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// 库概览统计
    Stats,
    /// 搜索视频
    Search {
        #[arg(long)]
        query: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        view: Option<String>,
        #[arg(long, default_value_t = 1)]
        page: u32,
        #[arg(long, default_value_t = 48)]
        limit: u32,
        #[arg(long, default_value = "added")]
        sort: String,
    },
    /// 单条视频详情 / 修改
    Video {
        #[command(subcommand)]
        command: VideoCommand,
    },
    /// 标签
    Tags {
        #[command(subcommand)]
        command: TagCommand,
    },
    /// 目录
    Collections,
    /// 把目录加入索引（扫描视频与压缩包内条目）
    Scan {
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long, default_value_t = 60)]
        entry_limit: usize,
    },
    /// 导入单个视频文件或压缩包
    Import {
        #[arg(long, required = true)]
        paths: Vec<PathBuf>,
        #[arg(long, default_value_t = 60)]
        entry_limit: usize,
    },
    /// 观看记录
    Watch {
        #[command(subcommand)]
        command: WatchCommand,
    },
    /// 下载任务
    Downloads {
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// 补齐缺失的时长/分辨率（压缩包条目扫描时不计元数据，需要解出来才知道）
    Probe {
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// 内容查重：算指纹 → 列重复组 → 可选把副本送进回收站
    Dedupe {
        /// 只算 N 条指纹就停（0 = 全算完）
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// 真的把重复副本标记进回收站（不加只报告）
        #[arg(long)]
        apply: bool,
        /// 只清理回收站里文件已不在的幽灵行
        #[arg(long)]
        purge: bool,
    },
    /// 整理库：按 200/夹 把视频收进 1、2、3… 号文件夹（不改文件名）
    Organize,
    /// 批量补预览图（接触表）：只补缺的，已有跳过
    Sheets {
        /// 本轮最多做 N 条（0 = 全做完）
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// 进度打印间隔（每 N 条一次）
        #[arg(long, default_value_t = 50)]
        verbose_every: usize,
    },
}

#[derive(Debug, Subcommand)]
enum VideoCommand {
    Get { id: i64 },
    Patch {
        id: i64,
        #[arg(long)]
        rating: Option<i64>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        favorite: Option<bool>,
        #[arg(long)]
        completed: Option<bool>,
    },
    Trash { id: i64, #[arg(long, default_value_t = true)] trashed: bool },
}

#[derive(Debug, Subcommand)]
enum TagCommand {
    List,
    Create { name: String, #[arg(long, default_value = "")] category: String, #[arg(long, default_value_t = 0)] weight: i64 },
    /// 批量给若干视频打标签
    Attach { video_ids: Vec<i64>, tag_id: i64, #[arg(long, default_value_t = true)] add: bool },
}

#[derive(Debug, Subcommand)]
enum WatchCommand {
    /// 记录一次观看进度
    Record { id: i64, #[arg(long)] progress: f64, #[arg(long, default_value_t = 0.0)] duration: f64 },
    /// 最近观看记录
    List { #[arg(long, default_value_t = 50)] limit: usize },
}

fn open(cli: &Cli) -> Result<LibraryContext, String> {
    let root = cli
        .library
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    if !root.is_dir() {
        return Err(format!("库目录不存在: {}", root.to_string_lossy()));
    }
    db::open_library(&root)
}

fn output(v: &Value, compact: bool) {
    if compact {
        println!("{}", serde_json::to_string(v).unwrap_or_else(|_| "{}".into()));
    } else {
        println!("{}", serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into()));
    }
}

fn main() {
    let cli = Cli::parse();
    let result = run(&cli);
    match result {
        Ok(v) => output(&v, cli.compact),
        Err(e) => {
            output(&json!({ "ok": false, "error": e }), cli.compact);
            std::process::exit(1);
        }
    }
}

fn run(cli: &Cli) -> Result<Value, String> {
    match &cli.command {
        Command::Init { path } => {
            // 全局 --library 优先，其次 --path，最后当前目录
            let root = cli.library.clone().or_else(|| path.clone())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            let ctx = db::open_library(&root)?;
            let info = db::library_info(&ctx)?;
            Ok(json!({ "ok": true, "action": "init", "library": info }))
        }
        Command::Stats => {
            let ctx = open(cli)?;
            Ok(json!({ "ok": true, "action": "stats", "stats": db::stats(&ctx.conn)? }))
        }
        Command::Search { query, tag, view, page, limit, sort } => {
            let ctx = open(cli)?;
            let q = VideoQuery {
                search: query.clone(), tag: tag.clone(), view: view.clone(), sort: Some(sort.clone()),
                page: Some(*page as usize), page_size: Some(*limit as usize), collection: None,
            };
            let page_result = db::query_videos(&ctx.conn, &q)?;
            Ok(json!({ "ok": true, "action": "search", "page": page_result }))
        }
        Command::Video { command } => {
            let ctx = open(cli)?;
            match command {
                VideoCommand::Get { id } => Ok(json!({ "ok": true, "video": db::get_video(&ctx.conn, *id)? })),
                VideoCommand::Patch { id, rating, note, favorite, completed } => {
                    let patch = VideoPatch {
                        id: *id, rating: *rating, note: note.clone(), favorite: *favorite, completed: *completed,
                        progress_sec: None,
                    };
                    db::patch_video(&ctx.conn, &patch)?;
                    Ok(json!({ "ok": true, "action": "patch", "video": db::get_video(&ctx.conn, *id)? }))
                }
                VideoCommand::Trash { id, trashed } => {
                    db::set_trashed(&ctx.conn, *id, *trashed)?;
                    Ok(json!({ "ok": true, "action": "trash", "id": id, "trashed": trashed }))
                }
            }
        }
        Command::Tags { command } => {
            let ctx = open(cli)?;
            match command {
                TagCommand::List => Ok(json!({ "ok": true, "tags": db::list_tags(&ctx.conn)? })),
                TagCommand::Create { name, category, weight } => {
                    let id = db::create_tag(&ctx.conn, name, category, *weight)?;
                    Ok(json!({ "ok": true, "action": "create-tag", "id": id, "name": name }))
                }
                TagCommand::Attach { video_ids, tag_id, add } => {
                    let n = db::bulk_tag(&ctx.conn, video_ids, &[*tag_id], *add)?;
                    Ok(json!({ "ok": true, "action": "attach-tag", "changed": n }))
                }
            }
        }
        Command::Collections => {
            let ctx = open(cli)?;
            Ok(json!({ "ok": true, "collections": db::list_collections(&ctx.conn, false)? }))
        }
        Command::Scan { root, entry_limit } => {
            let ctx = open(cli)?;
            let target = root.clone().unwrap_or_else(|| ctx.root.clone());
            // 与 GUI 扫描一致：文件没变且元数据齐全的条目不再跑 ffprobe
            let known = db::all_stamps(&ctx.conn)?;
            let mut found = 0usize;
            let mut added = 0usize;
            let mut updated = 0usize;
            let mut probed = 0usize;
            let mut reused = 0usize;
            let mut errors = Vec::new();
            let mut seen: Vec<String> = Vec::new();
            for entry in walkdir::WalkDir::new(&target).max_depth(12).into_iter().filter_map(Result::ok) {
                let p = entry.path();
                // 与 GUI 扫描同一套规则：跳过链接/快捷方式，路径统一反斜杠
                if entry.file_type().is_symlink() { continue; }
                if !p.is_file() || p.to_string_lossy().contains(".vault") { continue; }
                if p.extension().and_then(|s| s.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk")) { continue; }
                let key_path = media::path_key(p);
                let recs = if media::is_video(p) {
                    let (size, mtime) = media::file_stamp(p);
                    let fresh = known
                        .get(&(key_path.clone(), 0))
                        .is_some_and(|st| st.is_fresh(size, mtime));
                    if fresh {
                        reused += 1;
                        vec![media::record_stub(p)]
                    } else {
                        probed += 1;
                        vec![media::record_for_file(p)]
                    }
                } else if media::is_supported_archive(p) {
                    let (size, mtime) = media::file_stamp(p);
                    let unchanged = known
                        .get(&(key_path.clone(), 0))
                        .is_some_and(|st| st.size_bytes == size && st.mtime == mtime && st.entry_count > 0);
                    if unchanged {
                        reused += 1;
                        Vec::new()
                    } else {
                        media::archive_records(p, *entry_limit).unwrap_or_else(|e| {
                            errors.push(format!("{}: {e}", p.to_string_lossy()));
                            Vec::new()
                        })
                    }
                } else { Vec::new() };
                if p.is_file() && (media::is_video(p) || media::is_supported_archive(p)) {
                    seen.push(key_path);
                }
                found += recs.len();
                for rec in recs {
                    match db::upsert_video(&ctx.conn, &rec) {
                        Ok(true) => added += 1,
                        Ok(false) => updated += 1,
                        Err(e) => errors.push(e),
                    }
                }
            }
            let missing = db::mark_missing(&ctx.conn, &seen)?;
            let folders = db::sync_folder_collections(&ctx.conn, &target, &seen)?;
            Ok(json!({ "ok": true, "action": "scan", "found": found, "added": added, "updated": updated,
                "probed": probed, "reused": reused, "missing": missing, "folders": folders, "errors": errors }))
        }
        Command::Import { paths, entry_limit } => {
            let ctx = open(cli)?;
            let mut added = 0usize;
            let mut errors = Vec::new();
            for p in paths {
                let recs = if media::is_video(p) {
                    vec![media::record_for_file(p)]
                } else if media::is_supported_archive(p) {
                    match media::archive_records(p, *entry_limit) {
                        Ok(r) => r,
                        Err(e) => { errors.push(format!("{}: {e}", p.to_string_lossy())); continue; }
                    }
                } else {
                    errors.push(format!("不支持的文件: {}", p.to_string_lossy()));
                    continue;
                };
                for rec in recs {
                    match db::upsert_video(&ctx.conn, &rec) {
                        Ok(true) => added += 1,
                        Ok(false) => added += 1,
                        Err(e) => errors.push(e),
                    }
                }
            }
            Ok(json!({ "ok": true, "action": "import", "added": added, "errors": errors }))
        }
        Command::Watch { command } => {
            let ctx = open(cli)?;
            match command {
                WatchCommand::Record { id, progress, duration } => {
                    db::record_watch(&ctx.conn, &WatchUpdate {
                        id: *id, progress_sec: *progress, duration_sec: *duration, completed: None,
                    })?;
                    Ok(json!({ "ok": true, "action": "watch-record", "video": db::get_video(&ctx.conn, *id)? }))
                }
                WatchCommand::List { limit } => Ok(json!({ "ok": true, "history": db::watch_history(&ctx.conn, *limit)? })),
            }
        }
        Command::Downloads { limit } => {
            let ctx = open(cli)?;
            Ok(json!({ "ok": true, "downloads": db::list_downloads(&ctx.conn, *limit)? }))
        }
        Command::Probe { limit } => {
            let ctx = open(cli)?;
            let ids = db::ids_without_meta(&ctx.conn, *limit)?;
            let mut filled = 0;
            let mut failed: Vec<String> = Vec::new();
            for id in &ids {
                // resolve 会把压缩包条目按需解出来，backfill_meta 顺手把元数据写回
                match video_vault_lib::player::resolve(&ctx, *id) {
                    Ok(info) => {
                        let before = db::get_video(&ctx.conn, *id).ok().map(|v| v.duration_sec).unwrap_or(0.0);
                        video_vault_lib::player::backfill_meta(&ctx, *id, std::path::Path::new(&info.media_path));
                        let after = db::get_video(&ctx.conn, *id).ok().map(|v| v.duration_sec).unwrap_or(0.0);
                        if after > 0.0 && after != before {
                            filled += 1;
                        } else {
                            failed.push(format!("#{id}: 取不到时长"));
                        }
                    }
                    Err(e) => failed.push(format!("#{id}: {e}")),
                }
            }
            Ok(json!({
                "ok": true,
                "action": "probe",
                "candidates": ids.len(),
                "filled": filled,
                "failed": failed,
            }))
        }
        Command::Dedupe { limit, apply, purge } => {
            let ctx = open(cli)?;
            if *purge {
                let removed = db::purge_missing_trashed(&ctx.conn)?;
                return Ok(json!({ "ok": true, "action": "purge", "removed": removed }));
            }
            // 1) 算指纹：按大小倒序批量取，大文件先出结果
            let mut hashed = 0usize;
            loop {
                let batch = db::videos_needing_fingerprint(&ctx.conn, 24)?;
                if batch.is_empty() {
                    break;
                }
                let mut hit_limit = false;
                for (id, path, size) in &batch {
                    let fp = media::fingerprint(std::path::Path::new(path), *size as u64)
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "!missing".to_string());
                    db::set_fingerprint(&ctx.conn, *id, &fp)?;
                    hashed += 1;
                    if *limit > 0 && hashed >= *limit {
                        hit_limit = true;
                        break;
                    }
                }
                if hit_limit {
                    break;
                }
            }
            let (done, total) = db::fingerprint_progress(&ctx.conn)?;
            // 2) 列重复组
            let groups = db::duplicate_groups(&ctx.conn)?;
            let wasted: i64 = groups.iter().map(|g| g.wasted_bytes).sum();
            // 3) 可选：把副本标记进回收站（不删文件）
            let mut applied = 0usize;
            if *apply {
                for g in &groups {
                    let drops: Vec<i64> = g.others.iter().map(|i| i.id).collect();
                    applied += db::mark_duplicates(&ctx.conn, g.keep.id, &drops)?;
                }
            }
            Ok(json!({
                "ok": true,
                "action": "dedupe",
                "hashed": hashed,
                "progress": { "done": done, "total": total },
                "groups": groups.len(),
                "wasted_bytes": wasted,
                "applied": applied,
                "detail": groups.iter().take(20).map(|g| json!({
                    "keep": g.keep.path,
                    "size": g.size_bytes,
                    "others": g.others.iter().map(|o| &o.path).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            }))
        }
        Command::Organize => {
            let ctx = open(cli)?;
            let report = db::organize_numbered(&ctx, db::FOLDER_CAP)?;
            Ok(json!({ "ok": true, "action": "organize", "report": report }))
        }
        Command::Sheets { limit, verbose_every } => {
            let ctx = open(cli)?;
            let mut ids = db::all_video_ids_for_sheet(&ctx.conn, true)?;
            if *limit > 0 {
                ids.truncate(*limit);
            }
            let cache = db::LibraryContext::vault_dir(&ctx.root).join("cache").join("contacts");
            let total = ids.len();
            // 抽帧是纯 CPU 活，单个 ffmpeg 要把整片线性解码一遍，串行做 1000 条得几小时。
            // 这里开 3 个 worker，DB 访问用 Mutex 串起来（rusqlite 的 Connection 不是 Sync）。
            let lib = std::sync::Mutex::new(ctx);
            let cursor = std::sync::atomic::AtomicUsize::new(0);
            let ok_count = std::sync::atomic::AtomicUsize::new(0);
            let failed: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
            std::thread::scope(|scope| {
                for _ in 0..3 {
                    scope.spawn(|| loop {
                        use std::sync::atomic::Ordering;
                        let i = cursor.fetch_add(1, Ordering::SeqCst);
                        if i >= ids.len() {
                            break;
                        }
                        let id = ids[i];
                        // 1) 解析出真实媒体路径（压缩包条目会在这里解出来）
                        let source = {
                            let Ok(guard) = lib.lock() else { break };
                            match video_vault_lib::player::resolve(&guard, id) {
                                Ok(info) => std::path::PathBuf::from(info.media_path),
                                Err(e) => {
                                    if let Ok(mut f) = failed.lock() {
                                        f.push(format!("#{id}: {e}"));
                                    }
                                    continue;
                                }
                            }
                        };
                        if !source.is_file() {
                            if let Ok(mut f) = failed.lock() {
                                f.push(format!("#{id}: 文件不存在"));
                            }
                            continue;
                        }
                        // 2) ffmpeg 抽帧（不占锁，三个 worker 真并行）
                        let out = cache.join(format!("{id}.jpg"));
                        if let Err(e) = media::contact_sheet(&source, &out) {
                            if let Ok(mut f) = failed.lock() {
                                f.push(format!("#{id}: {e}"));
                            }
                            continue;
                        }
                        // 3) 写回元数据与预览图路径
                        if let Ok(guard) = lib.lock() {
                            video_vault_lib::player::backfill_meta(&guard, id, &source);
                            let _ = db::set_contact_sheet(&guard.conn, id, &out.to_string_lossy());
                        }
                        let n = ok_count.fetch_add(1, Ordering::SeqCst) + 1;
                        if *verbose_every > 0 && n % *verbose_every == 0 {
                            eprintln!("预览图进度 {n}/{total}");
                        }
                    });
                }
            });
            let done = ok_count.load(std::sync::atomic::Ordering::SeqCst);
            let failed = failed.into_inner().unwrap_or_default();
            Ok(json!({
                "ok": true,
                "action": "sheets",
                "total": total,
                "done": done,
                "failed": failed.len(),
                "failures": failed.iter().take(20).collect::<Vec<_>>(),
            }))
        }
    }
}
