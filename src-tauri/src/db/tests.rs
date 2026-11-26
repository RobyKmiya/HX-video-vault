//! db.rs 的 Rust 单测，use super::* 经 mod.rs 的 re-export 拿到全部函数。
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_lib() -> (PathBuf, LibraryContext) {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("vv-test-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let ctx = open_library(&root).expect("临时库应能创建");
        (root, ctx)
    }

    fn rec(path: &str, entry: i64) -> VideoRecord {
        VideoRecord {
            path: path.into(),
            file_name: format!("视频-{entry}"),
            container: "mp4".into(),
            size_bytes: 1024,
            mtime: 100,
            duration_sec: 100.0,
            width: 1920,
            height: 1080,
            codec_video: "h264".into(),
            codec_audio: "aac".into(),
            entry_index: entry,
            entry_count: 3,
        }
    }

    #[test]
    fn v6_migration_normalizes_paths_and_merges_duplicates() {
        let (root, ctx) = temp_lib();
        ctx.conn.execute_batch("PRAGMA user_version = 5;").unwrap();
        let good = format!("{}\\0-199\\dup.mp4", root.to_string_lossy());
        let slashy = good.replace('\\', "/");
        for (path, added) in [
            (good.as_str(), 1_i64),
            (slashy.as_str(), 2_i64),
            ("F:\\outside\\ghost.mp4", 3_i64),
        ] {
            ctx.conn.execute(
                "INSERT INTO videos (path, file_name, container, size_bytes, mtime, added_at) VALUES (?1, 'x.mp4', 'mp4', 1024, 100, ?2)",
                params![path, added],
            ).unwrap();
        }
        ctx.conn.execute("INSERT INTO tags(name) VALUES ('迁移测试')", []).unwrap();
        ctx.conn.execute(
            "INSERT INTO video_tags(video_id, tag_id) SELECT id, 1 FROM videos",
            [],
        ).unwrap();
        drop(ctx);

        let ctx = open_library(&root).unwrap();
        let paths: Vec<String> = {
            let mut stmt = ctx.conn.prepare("SELECT path FROM videos").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
        };
        assert_eq!(paths.len(), 1, "同一文件的两种路径写法应合并，库外残留应清掉");
        assert_eq!(paths[0], good.replace('/', "\\"));
        let tag_rows: i64 = ctx.conn.query_row("SELECT COUNT(*) FROM video_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(tag_rows, 1, "重复行的标签应改挂到保留行");
        let version: i64 = ctx.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn schema_upsert_is_idempotent_per_entry() {
        let (root, ctx) = temp_lib();
        assert_eq!(upsert_video(&ctx.conn, &rec("D:\\a.zip", 0)).unwrap(), true);
        assert_eq!(upsert_video(&ctx.conn, &rec("D:\\a.zip", 1)).unwrap(), true);
        // 同一个 (path, entry_index) 再写一次应算更新
        assert_eq!(upsert_video(&ctx.conn, &rec("D:\\a.zip", 0)).unwrap(), false);
        let page = query_videos(&ctx.conn, &VideoQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.items[0].entry_count, 3);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn stamps_flag_unchanged_entries_and_upsert_keeps_meta() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("D:\\c.mp4", 0)).unwrap();
        let id: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path=?1", params!["D:\\c.mp4"], |r| r.get(0))
            .unwrap();

        let stamps = all_stamps(&ctx.conn).unwrap();
        let st = stamps.get(&("D:\\c.mp4".to_string(), 0)).expect("应有戳记");
        assert!(st.is_fresh(1024, 100), "没变过且元数据齐全 → 不必再探测");
        assert!(!st.is_fresh(2048, 100), "大小变了要重新探测");
        assert!(!st.is_fresh(1024, 200), "mtime 变了要重新探测");

        // 扫描时写回的 stub 没有元数据，不能把已有元数据抹掉
        let mut stub = rec("D:\\c.mp4", 0);
        stub.duration_sec = 0.0;
        stub.width = 0;
        stub.height = 0;
        stub.codec_video = String::new();
        stub.codec_audio = String::new();
        stub.size_bytes = 2048;
        assert!(!upsert_video(&ctx.conn, &stub).unwrap());
        let after = get_video(&ctx.conn, id).unwrap();
        assert_eq!(after.duration_sec, 100.0);
        assert_eq!(after.width, 1920);
        assert_eq!(after.size_bytes, 2048);

        // 从没探测过的条目 → has_meta=false，仍然需要探测
        let mut bare = rec("D:\\d.mp4", 0);
        bare.duration_sec = 0.0;
        bare.width = 0;
        bare.height = 0;
        upsert_video(&ctx.conn, &bare).unwrap();
        let stamps = all_stamps(&ctx.conn).unwrap();
        assert!(!stamps[&("D:\\d.mp4".to_string(), 0)].is_fresh(1024, 100));

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tags_drive_search_and_tag_text() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("D:\\b.mp4", 0)).unwrap();
        let id = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get::<_, i64>(0)).unwrap();
        let tag_id = create_tag(&ctx.conn, "风景", "题材", 2).unwrap();
        set_tag_video(&ctx.conn, id, tag_id, true).unwrap();
        let one = get_video(&ctx.conn, id).unwrap();
        assert_eq!(one.tags, vec!["风景"]);
        assert!(one.tag_text.contains("风景"));
        let q = VideoQuery { search: Some("风景".into()), ..Default::default() };
        assert_eq!(query_videos(&ctx.conn, &q).unwrap().total, 1);
        set_tag_video(&ctx.conn, id, tag_id, false).unwrap();
        assert_eq!(query_videos(&ctx.conn, &q).unwrap().total, 0);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn download_task_crud_and_requeue() {
        let (root, ctx) = temp_lib();
        let id = create_download(&ctx.conn, "https://x/a.mp4", "a.mp4", "x.com", "/tmp/a.mp4", "标签A").unwrap();
        let t = get_download(&ctx.conn, id).unwrap();
        assert_eq!(t.state, "queued");
        assert_eq!(t.tags, "标签A");
        update_download(&ctx.conn, id, "running", 0.5, "", 50, 100, None).unwrap();
        assert_eq!(get_download(&ctx.conn, id).unwrap().progress, 0.5);
        // 中断后重排（模拟进程被关掉）
        assert_eq!(requeue_interrupted(&ctx.conn).unwrap(), 1);
        assert_eq!(get_download(&ctx.conn, id).unwrap().state, "queued");
        delete_download(&ctx.conn, id).unwrap();
        assert!(list_downloads(&ctx.conn, 10).unwrap().is_empty());
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn watch_count_and_progress_are_stable() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("D:\\c.mp4", 0)).unwrap();
        let id = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get::<_, i64>(0)).unwrap();
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 30.0, duration_sec: 100.0, completed: None }).unwrap();
        assert_eq!(get_video(&ctx.conn, id).unwrap().watch_count, 1);
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 60.0, duration_sec: 100.0, completed: None }).unwrap();
        assert_eq!(get_video(&ctx.conn, id).unwrap().watch_count, 1); // 连续看不算新会话
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 2.0, duration_sec: 100.0, completed: None }).unwrap();
        assert_eq!(get_video(&ctx.conn, id).unwrap().watch_count, 2); // 从头看算新会话
        record_watch(&ctx.conn, &WatchUpdate { id, progress_sec: 98.0, duration_sec: 100.0, completed: None }).unwrap();
        assert!(get_video(&ctx.conn, id).unwrap().completed);
        let s = stats(&ctx.conn).unwrap();
        assert_eq!(s.video_count, 1);
        assert_eq!(s.watched_count, 1);
        assert_eq!(s.completed_count, 1);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 压缩包条目入库时时长是 0，等真正解出来再补；补过之后不能被后来的探测覆盖。
    #[test]
    fn backfill_fills_missing_meta_but_never_overwrites() {
        let (root, ctx) = temp_lib();
        let mut archive = rec("D:\\合集.cbz", 0);
        archive.duration_sec = 0.0;
        archive.width = 0;
        archive.height = 0;
        archive.codec_video = String::new();
        archive.codec_audio = String::new();
        upsert_video(&ctx.conn, &archive).unwrap();
        let id = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get::<_, i64>(0)).unwrap();

        // 入库时确实没有元数据
        assert_eq!(get_video(&ctx.conn, id).unwrap().duration_sec, 0.0);
        assert!(ids_without_meta(&ctx.conn, 10).unwrap().contains(&id));

        // 第一次解出来 → 补上
        assert!(backfill_probe(&ctx.conn, id, 1425.5, 1920, 1080, "h264", "aac").unwrap());
        let filled = get_video(&ctx.conn, id).unwrap();
        assert_eq!(filled.duration_sec, 1425.5);
        assert_eq!(filled.width, 1920);
        assert_eq!(filled.codec_video, "h264");
        assert!(!ids_without_meta(&ctx.conn, 10).unwrap().contains(&id));

        // 再来一次探测（值不同）→ 原有数据保持不变
        backfill_probe(&ctx.conn, id, 99.0, 320, 240, "hevc", "opus").unwrap();
        let again = get_video(&ctx.conn, id).unwrap();
        assert_eq!(again.duration_sec, 1425.5);
        assert_eq!(again.width, 1920);
        assert_eq!(again.codec_video, "h264");

        // 探测不到任何东西时不该写库
        assert!(!backfill_probe(&ctx.conn, id, 0.0, 0, 0, "", "").unwrap());
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 目录：分组、软删除、恢复、改名后关联不掉、彻底删除不动视频
    #[test]
    fn collections_support_group_soft_delete_and_restore() {
        let (root, ctx) = temp_lib();
        let id = create_collection(&ctx.conn, "  灵感  ", "创作").expect("建目录");
        assert_eq!(collection_id_by_name(&ctx.conn, "灵感").unwrap(), id);

        upsert_video(&ctx.conn, &rec("K:/a.mp4", 0)).unwrap();
        let vid: i64 = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get(0)).unwrap();
        set_collection_video(&ctx.conn, id, vid, true).unwrap();

        let mine = list_collections(&ctx.conn, false).unwrap().into_iter().find(|c| c.id == id).unwrap();
        assert_eq!(mine.group_name, "创作");
        assert_eq!(mine.count, 1);

        // 软删除：默认列表看不到，回收站看得到
        soft_delete_collection(&ctx.conn, id).unwrap();
        assert!(list_collections(&ctx.conn, false).unwrap().iter().all(|c| c.id != id));
        assert!(list_collections(&ctx.conn, true).unwrap().iter().any(|c| c.id == id && c.deleted_at > 0));
        assert!(get_video(&ctx.conn, vid).unwrap().collections.is_empty(), "已删目录不该出现在视频的目录里");

        // 恢复后关联仍在
        restore_collection(&ctx.conn, id).unwrap();
        assert_eq!(get_video(&ctx.conn, vid).unwrap().collections, vec!["灵感".to_string()]);

        // 内置目录受保护
        let fav = collection_id_by_name(&ctx.conn, "收藏").unwrap();
        assert!(soft_delete_collection(&ctx.conn, fav).is_err());
        assert!(purge_collection(&ctx.conn, fav).is_err());

        // 改名不影响关联（过滤/展示都按 id 走）
        rename_collection(&ctx.conn, id, "灵感库").unwrap();
        assert!(collection_id_by_name(&ctx.conn, "灵感").is_err());
        assert_eq!(collection_id_by_name(&ctx.conn, "灵感库").unwrap(), id);
        assert_eq!(get_video(&ctx.conn, vid).unwrap().collections, vec!["灵感库".to_string()]);

        // 同名冲突要被挡住，不能悄悄建出第二条
        create_collection(&ctx.conn, "另一个", "").unwrap();
        assert!(rename_collection(&ctx.conn, id, "另一个").is_err());

        // 彻底删除：目录没了，视频还在
        purge_collection(&ctx.conn, id).unwrap();
        assert!(list_collections(&ctx.conn, true).unwrap().iter().all(|c| c.id != id));
        assert!(get_video(&ctx.conn, vid).is_ok());

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 同名目录软删后再「新建」应复活原记录，而不是被 UNIQUE 挡掉
    #[test]
    fn creating_same_name_after_delete_revives_it() {
        let (root, ctx) = temp_lib();
        let a = create_collection(&ctx.conn, "素材", "").unwrap();
        soft_delete_collection(&ctx.conn, a).unwrap();
        let b = create_collection(&ctx.conn, "素材", "整理后").unwrap();
        assert_eq!(a, b);
        let c = list_collections(&ctx.conn, false).unwrap().into_iter().find(|c| c.id == a).unwrap();
        assert_eq!(c.group_name, "整理后");
        assert_eq!(c.deleted_at, 0);
        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 老库（user_version=2、collections 没有新列）打开时要自动补列，且数据不丢
    #[test]
    fn v2_library_gains_collection_columns_on_open() {
        let root = std::env::temp_dir().join(format!(
            "vv-mig-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let vault = LibraryContext::vault_dir(&root);
        fs::create_dir_all(&vault).unwrap();
        {
            let conn = Connection::open(vault.join("vault.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE collections (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   name TEXT NOT NULL,
                   kind TEXT NOT NULL DEFAULT 'custom',
                   created_at INTEGER NOT NULL DEFAULT 0,
                   UNIQUE(name, kind));
                 INSERT INTO collections (name, kind) VALUES ('旧目录', 'custom');
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }
        let ctx = open_library(&root).expect("老库应能迁移");
        let cols: Vec<String> = {
            let mut stmt = ctx.conn.prepare("PRAGMA table_info(collections)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.filter_map(Result::ok).collect()
        };
        for want in ["deleted_at", "group_name", "sort_order"] {
            assert!(cols.iter().any(|c| c == want), "迁移后缺少列 {want}");
        }
        let version: i64 = ctx.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let old = list_collections(&ctx.conn, false).unwrap();
        assert!(old.iter().any(|c| c.name == "旧目录"), "迁移不能丢老目录");
        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 预览图：统计、清空（返回待删路径）、只补缺图
    #[test]
    fn sheet_counts_and_clear_round_trip() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("K:/b.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/c.mp4", 0)).unwrap();
        let ids: Vec<i64> = {
            let mut stmt = ctx.conn.prepare("SELECT id FROM videos ORDER BY id").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
            rows.filter_map(Result::ok).collect()
        };
        set_contact_sheet(&ctx.conn, ids[0], "C:/tmp/sheet-1.jpg").unwrap();

        assert_eq!(sheet_counts(&ctx.conn).unwrap(), (2, 1, 1));
        assert_eq!(ids_without_sheet(&ctx.conn, 10).unwrap(), vec![ids[1]]);
        assert_eq!(all_video_ids_for_sheet(&ctx.conn, true).unwrap(), vec![ids[1]]);
        // 全量时缺图的排在前面
        assert_eq!(all_video_ids_for_sheet(&ctx.conn, false).unwrap().first(), Some(&ids[1]));

        let paths = clear_contact_sheets(&ctx.conn, None).unwrap();
        assert_eq!(paths, vec!["C:/tmp/sheet-1.jpg".to_string()]);
        assert_eq!(sheet_counts(&ctx.conn).unwrap(), (2, 0, 2));
        assert_eq!(ids_without_sheet(&ctx.conn, 10).unwrap().len(), 2);

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// 自选封面：登记优先于接触表展示，撤销后回到空（接触表/中间帧兜底由上层决定）
    #[test]
    fn cover_set_clear_and_priority_in_query() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("K:/c.mp4", 0)).unwrap();
        let id: i64 = ctx.conn.query_row("SELECT id FROM videos", [], |r| r.get(0)).unwrap();

        // 初始：无自选封面
        assert_eq!(get_cover_path(&ctx.conn, id).unwrap(), None);
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.cover, "");

        // 重复设置 = 覆盖，查询端直接带到 VideoItem
        set_cover(&ctx.conn, id, "K:/covers/a.jpg").unwrap();
        set_cover(&ctx.conn, id, "K:/covers/b.jpg").unwrap();
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.cover, "K:/covers/b.jpg");
        assert_eq!(get_cover_path(&ctx.conn, id).unwrap().as_deref(), Some("K:/covers/b.jpg"));

        // 撤销后回到空
        clear_cover(&ctx.conn, id).unwrap();
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.cover, "");
        assert_eq!(get_cover_path(&ctx.conn, id).unwrap(), None);

        // contact_sheet 与 cover 互不干扰（预览图统计不受自选封面影响）
        set_contact_sheet(&ctx.conn, id, "K:/sheets/c.jpg").unwrap();
        set_cover(&ctx.conn, id, "K:/covers/c.jpg").unwrap();
        let item = get_video(&ctx.conn, id).unwrap();
        assert_eq!(item.contact_sheet, "K:/sheets/c.jpg");
        assert_eq!(item.cover, "K:/covers/c.jpg");
        assert_eq!(sheet_counts(&ctx.conn).unwrap(), (1, 1, 0));

        drop(ctx);
        let _ = fs::remove_dir_all(root);
    }

    /// v3 老库打开时补建 video_covers 表，查询端（带 cover 联查）照常工作
    #[test]
    fn v3_library_gains_covers_table_on_open() {
        let root = std::env::temp_dir().join(format!(
            "vv-mig4-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let vault = LibraryContext::vault_dir(&root);
        fs::create_dir_all(&vault).unwrap();
        {
            let conn = Connection::open(vault.join("vault.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE videos (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   path TEXT NOT NULL,
                   file_name TEXT NOT NULL,
                   container TEXT NOT NULL DEFAULT '',
                   size_bytes INTEGER NOT NULL DEFAULT 0,
                   mtime INTEGER NOT NULL DEFAULT 0,
                   duration_sec REAL NOT NULL DEFAULT 0,
                   width INTEGER NOT NULL DEFAULT 0,
                   height INTEGER NOT NULL DEFAULT 0,
                   codec_video TEXT NOT NULL DEFAULT '',
                   codec_audio TEXT NOT NULL DEFAULT '',
                   entry_index INTEGER NOT NULL DEFAULT 0,
                   entry_count INTEGER NOT NULL DEFAULT 1,
                   added_at INTEGER NOT NULL DEFAULT 0,
                   last_watched_at INTEGER NOT NULL DEFAULT 0,
                   watch_count INTEGER NOT NULL DEFAULT 0,
                   progress_sec REAL NOT NULL DEFAULT 0,
                   completed INTEGER NOT NULL DEFAULT 0,
                   rating INTEGER NOT NULL DEFAULT 0,
                   note TEXT NOT NULL DEFAULT '',
                   favorite INTEGER NOT NULL DEFAULT 0,
                   trashed INTEGER NOT NULL DEFAULT 0,
                   contact_sheet TEXT NOT NULL DEFAULT '',
                   tag_text TEXT NOT NULL DEFAULT '',
                   UNIQUE(path, entry_index));
                 INSERT INTO videos (path, file_name) VALUES ('K:/old.mp4', 'old.mp4');
                 PRAGMA user_version = 3;",
            )
            .unwrap();
            // v6 会清掉库根外的历史残留，测试数据挪到本库根下
            let old = root.join("old.mp4").to_string_lossy().replace('/', "\\");
            conn.execute("UPDATE videos SET path=?1 WHERE path='K:/old.mp4'", params![old]).unwrap();
        }
        let ctx = open_library(&root).expect("v3 老库应能迁移");
        let version: i64 = ctx.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let item = get_video(&ctx.conn, 1).unwrap();
        assert_eq!(item.file_name, "old.mp4");
        assert_eq!(item.cover, "");
        set_cover(&ctx.conn, 1, "K:/covers/old.jpg").unwrap();
        assert_eq!(get_video(&ctx.conn, 1).unwrap().cover, "K:/covers/old.jpg");

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// v4 老库打开：collection_videos 补 position 列；红心收藏一次性迁入「我的收藏」；
    /// 已有同名夹（哪怕软删）绝不被复活或塞入
    #[test]
    fn v4_library_gains_position_and_migrates_favorites() {
        let root = std::env::temp_dir().join(format!(
            "vv-mig5-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        let vault = LibraryContext::vault_dir(&root);
        fs::create_dir_all(&vault).unwrap();
        {
            let conn = Connection::open(vault.join("vault.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE videos (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   path TEXT NOT NULL, file_name TEXT NOT NULL, container TEXT NOT NULL DEFAULT '',
                   size_bytes INTEGER NOT NULL DEFAULT 0, mtime INTEGER NOT NULL DEFAULT 0,
                   duration_sec REAL NOT NULL DEFAULT 0, width INTEGER NOT NULL DEFAULT 0,
                   height INTEGER NOT NULL DEFAULT 0, codec_video TEXT NOT NULL DEFAULT '',
                   codec_audio TEXT NOT NULL DEFAULT '', entry_index INTEGER NOT NULL DEFAULT 0,
                   entry_count INTEGER NOT NULL DEFAULT 1, added_at INTEGER NOT NULL DEFAULT 0,
                   last_watched_at INTEGER NOT NULL DEFAULT 0, watch_count INTEGER NOT NULL DEFAULT 0,
                   progress_sec REAL NOT NULL DEFAULT 0, completed INTEGER NOT NULL DEFAULT 0,
                   rating INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '',
                   favorite INTEGER NOT NULL DEFAULT 0, trashed INTEGER NOT NULL DEFAULT 0,
                   contact_sheet TEXT NOT NULL DEFAULT '', tag_text TEXT NOT NULL DEFAULT '',
                   UNIQUE(path, entry_index));
                 INSERT INTO videos (path, file_name, favorite) VALUES ('K:/a.mp4','a.mp4',1),('K:/b.mp4','b.mp4',0),('K:/c.mp4','c.mp4',1);
                 CREATE TABLE collections (
                   id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'custom',
                   created_at INTEGER NOT NULL DEFAULT 0, UNIQUE(name, kind));
                 INSERT INTO collections (name, kind) VALUES ('待看','system'),('已有夹','custom');
                 CREATE TABLE collection_videos (
                   collection_id INTEGER NOT NULL, video_id INTEGER NOT NULL,
                   added_at INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (collection_id, video_id));
                 PRAGMA user_version = 4;",
            )
            .unwrap();
            // v6 会清掉库根外的历史残留，测试数据挪到本库根下
            for (old, name) in [("K:/a.mp4", "a.mp4"), ("K:/b.mp4", "b.mp4"), ("K:/c.mp4", "c.mp4")] {
                let p = root.join(name).to_string_lossy().replace('/', "\\");
                conn.execute("UPDATE videos SET path=?1 WHERE path=?2", params![p, old]).unwrap();
            }
        }
        let ctx = open_library(&root).expect("v4 老库应能迁移");
        let version: i64 = ctx.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // 红心两条应已进入「我的收藏」，且带顺序位次；非红心的 b 不在
        let fav_id: i64 = ctx
            .conn
            .query_row("SELECT id FROM collections WHERE name='我的收藏'", [], |r| r.get(0))
            .unwrap();
        let members: Vec<(i64, i64)> = ctx
            .conn
            .prepare("SELECT video_id, position FROM collection_videos WHERE collection_id=?1 ORDER BY position")
            .unwrap()
            .query_map(params![fav_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(members.len(), 2, "favorite=1 的两条应迁入");
        assert_eq!(members[0].1, 0, "迁移位次从 0 开始");
        let names: Vec<String> = members
            .iter()
            .map(|(vid, _)| {
                ctx.conn
                    .query_row("SELECT file_name FROM videos WHERE id=?1", params![vid], |r| r.get(0))
                    .unwrap()
            })
            .collect();
        assert_eq!(names, vec!["a.mp4", "c.mp4"], "按 id 顺序迁移");

        // 再开一次库：不重复迁移（INSERT OR IGNORE 幂等）
        drop(ctx);
        let ctx2 = open_library(&root).unwrap();
        let n: i64 = ctx2
            .conn
            .query_row("SELECT COUNT(*) FROM collection_videos WHERE collection_id=?1", params![fav_id], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2, "重复打开不得再次迁移");

        drop(ctx2);
        let _ = fs::remove_dir_all(&root);
    }

    /// 改名/移动后只动这一条：路径、文件名、mtime 跟着走；auto 镜像夹成员搬到新夹，旧夹空了消失。
    #[test]
    fn apply_path_change_relinks_auto_folder() {
        let (root, ctx) = temp_lib();
        // 起始状态：文件在库根（没有文件夹），像「刚导入还没扫描」那种
        let src = root.join("a.mp4");
        fs::write(&src, b"x").unwrap();
        let mut r = rec(&src.to_string_lossy(), 0);
        r.file_name = "a.mp4".into();
        upsert_video(&ctx.conn, &r).unwrap();
        let id: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path=?1", params![src.to_string_lossy().to_string()], |x| x.get(0))
            .unwrap();

        let folder_of = |id: i64| -> String {
            ctx.conn
                .query_row(
                    "SELECT c.name FROM collections c JOIN collection_videos cv ON cv.collection_id=c.id WHERE cv.video_id=?1",
                    params![id],
                    |x| x.get(0),
                )
                .unwrap_or_default()
        };

        // 移动到 0-199：建夹 + 入成员
        fs::create_dir_all(root.join("0-199")).unwrap();
        let dst0 = root.join("0-199").join("a.mp4");
        fs::rename(&src, &dst0).unwrap();
        apply_path_change(&ctx.conn, &root, id, &dst0).unwrap();
        assert_eq!(folder_of(id), "0-199");

        // 移到 200-399：成员跟着搬，旧夹空了自动消失
        fs::create_dir_all(root.join("200-399")).unwrap();
        let dst = root.join("200-399").join("a.mp4");
        fs::rename(&dst0, &dst).unwrap();
        apply_path_change(&ctx.conn, &root, id, &dst).unwrap();
        assert_eq!(folder_of(id), "200-399");
        let old_left: i64 = ctx
            .conn
            .query_row("SELECT COUNT(*) FROM collections WHERE name='0-199' AND kind='auto'", [], |x| x.get(0))
            .unwrap();
        assert_eq!(old_left, 0, "空掉的镜像夹应删除");

        // 同目录改名：文件名/路径更新，所在夹不变
        let dst2 = root.join("200-399").join("b.mp4");
        fs::rename(&dst, &dst2).unwrap();
        apply_path_change(&ctx.conn, &root, id, &dst2).unwrap();
        let (path, name): (String, String) = ctx
            .conn
            .query_row("SELECT path, file_name FROM videos WHERE id=?1", params![id], |x| Ok((x.get(0)?, x.get(1)?)))
            .unwrap();
        assert!(path.ends_with("b.mp4"));
        assert_eq!(name, "b.mp4");
        assert_eq!(folder_of(id), "200-399");

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 排序方向：无前缀 = 各键既有默认方向（mtime/added 新近在前、title 升序），"-" 前缀取反。
    /// 这条锁住「只加逆序开关、不悄悄改老行为」。
    #[test]
    fn sort_keys_and_reverse_direction() {
        let (root, ctx) = temp_lib();
        let mut a = rec("K:/a.mp4", 0);
        a.file_name = "a.mp4".into();
        a.mtime = 300;
        let mut b = rec("K:/b.mp4", 0);
        b.file_name = "b.mp4".into();
        b.mtime = 100;
        let mut c = rec("K:/c.mp4", 0);
        c.file_name = "c.mp4".into();
        c.mtime = 200;
        upsert_video(&ctx.conn, &a).unwrap();
        upsert_video(&ctx.conn, &b).unwrap();
        upsert_video(&ctx.conn, &c).unwrap();

        let names = |sort: &str| -> Vec<String> {
            query_videos(&ctx.conn, &VideoQuery { sort: Some(sort.into()), ..Default::default() })
                .unwrap()
                .items
                .iter()
                .map(|v| v.file_name.clone())
                .collect()
        };
        assert_eq!(names("mtime"), vec!["a.mp4", "c.mp4", "b.mp4"], "mtime 默认新近在前");
        assert_eq!(names("-mtime"), vec!["b.mp4", "c.mp4", "a.mp4"], "-mtime 取反");
        assert_eq!(names("title"), vec!["a.mp4", "b.mp4", "c.mp4"], "title 默认升序");
        assert_eq!(names("-title"), vec!["c.mp4", "b.mp4", "a.mp4"], "-title 降序");
        // 批量导入时 added_at 整批相同：按 id 倒序（后入库的在前）兜底，
        // 不能用源文件继承来的旧 mtime 当第二键，否则新片会被压到后面。
        assert_eq!(names("added"), vec!["c.mp4", "b.mp4", "a.mp4"], "同秒入库按 id 倒序兜底");
        assert_eq!(names("-added"), vec!["a.mp4", "b.mp4", "c.mp4"], "逆序也稳定");

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 夹内拖拽排序：子集重排 + 未入子集成员整体后移保序；manual 查询按位次返回
    #[test]
    fn reorder_collection_videos_normalizes_positions() {
        let (root, ctx) = temp_lib();
        upsert_video(&ctx.conn, &rec("K:/1.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/2.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/3.mp4", 0)).unwrap();
        upsert_video(&ctx.conn, &rec("K:/4.mp4", 0)).unwrap();
        let ids: Vec<i64> = (1..=4)
            .map(|i| {
                ctx.conn
                    .query_row("SELECT id FROM videos WHERE path=?1", params![format!("K:/{i}.mp4")], |r| r.get(0))
                    .unwrap()
            })
            .collect();
        let coll = create_collection(&ctx.conn, "片单", "").unwrap();
        for vid in &ids {
            set_collection_video(&ctx.conn, coll, *vid, true).unwrap();
        }

        // 初始位次 = 加入顺序 0..3；把 [3号, 1号] 拖到最前
        reorder_collection_videos(&ctx.conn, coll, &[ids[2], ids[0]]).unwrap();
        let order: Vec<i64> = ctx
            .conn
            .prepare(
                "SELECT cv.video_id FROM collection_videos cv WHERE cv.collection_id=?1 ORDER BY cv.position",
            )
            .unwrap()
            .query_map(params![coll], |r| r.get(0))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(order, vec![ids[2], ids[0], ids[1], ids[3]], "子集置顶，未入子集的 2、4 号保序后移");

        // manual 查询按位次返回
        let coll_name = "片单";
        let page = query_videos(
            &ctx.conn,
            &VideoQuery { collection: Some(coll_name.into()), sort: Some("manual".into()), ..Default::default() },
        )
        .unwrap();
        let got: Vec<i64> = page.items.iter().map(|v| v.id).collect();
        assert_eq!(got, vec![ids[2], ids[0], ids[1], ids[3]]);

        // 新追加成员排到夹尾（位次 = MAX+1）
        upsert_video(&ctx.conn, &rec("K:/5.mp4", 0)).unwrap();
        let vid5: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path='K:/5.mp4'", [], |r| r.get(0))
            .unwrap();
        set_collection_video(&ctx.conn, coll, vid5, true).unwrap();
        let pos5: i64 = ctx
            .conn
            .query_row(
                "SELECT position FROM collection_videos WHERE collection_id=?1 AND video_id=?2",
                params![coll, vid5],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pos5, 6, "位次稀疏（子集重排后 2/4 号后移到 3/5），新成员 MAX+1=6 排到末尾");

        // 侧栏收藏夹顺序
        reorder_collections(&ctx.conn, &[coll, 1]).unwrap();
        let so: (i64, i64) = (
            ctx.conn.query_row("SELECT sort_order FROM collections WHERE id=?1", params![coll], |r| r.get(0)).unwrap(),
            ctx.conn.query_row("SELECT sort_order FROM collections WHERE id=1", [], |r| r.get(0)).unwrap(),
        );
        assert_eq!(so, (0, 1));

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 文件夹镜像：嵌套子目录 → auto 收藏夹（名=相对路径 › 连接，分组=首段）；
    /// 成员按目录归位、移动后重链、根目录直放不归类、手选夹不受影响
    #[test]
    fn sync_folder_collections_mirrors_disk_tree() {
        let (root, ctx) = temp_lib();
        for d in ["A", "A/B"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        let paths = vec![
            root.join("A").join("a.mp4"),
            root.join("A").join("B").join("b.mp4"),
            root.join("A").join("合集.zip"),
            root.join("root.mp4"),
        ];
        for p in &paths {
            fs::write(p, b"x").unwrap();
        }
        let ps = |p: &Path| p.to_string_lossy().to_string();
        upsert_video(&ctx.conn, &rec(&ps(&paths[0]), 0)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[1]), 0)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[2]), 0)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[2]), 1)).unwrap();
        upsert_video(&ctx.conn, &rec(&ps(&paths[3]), 0)).unwrap();
        let manual = create_collection(&ctx.conn, "手选", "").unwrap();
        let root_vid: i64 = ctx
            .conn
            .query_row("SELECT id FROM videos WHERE path=?1", params![ps(&paths[3])], |r| r.get(0))
            .unwrap();
        set_collection_video(&ctx.conn, manual, root_vid, true).unwrap();

        let seen: Vec<String> = paths.iter().map(|p| ps(p)).collect();
        let synced = sync_folder_collections(&ctx.conn, &root, &seen).unwrap();
        assert_eq!(synced, 2, "A 与 A/B 两个文件夹镜像");

        let auto_of = |name: &str| -> Option<i64> {
            ctx.conn
                .query_row("SELECT id FROM collections WHERE name=?1 AND kind='auto'", params![name], |r| r.get(0))
                .optional()
                .unwrap()
        };
        let col_a = auto_of("A").expect("应有 auto 夹 A");
        let col_ab = auto_of("A › B").expect("应有 auto 夹 A › B");
        let group_of = |id: i64| -> String {
            ctx.conn
                .query_row("SELECT group_name FROM collections WHERE id=?1", params![id], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(group_of(col_a), "A", "分组 = 首段目录名");
        assert_eq!(group_of(col_ab), "A");

        let member_ids = |cid: i64| -> Vec<i64> {
            ctx.conn
                .prepare("SELECT video_id FROM collection_videos WHERE collection_id=?1 ORDER BY position")
                .unwrap()
                .query_map(params![cid], |r| r.get(0))
                .unwrap()
                .filter_map(Result::ok)
                .collect()
        };
        let vid = |p: &Path| -> i64 {
            ctx.conn
                .query_row("SELECT id FROM videos WHERE path=?1", params![ps(p)], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            member_ids(col_a),
            vec![vid(&paths[0]), vid(&paths[2]), vid(&paths[2]) + 1],
            "A 夹：a.mp4 + 压缩包两集（入库顺序即位次）"
        );
        assert_eq!(member_ids(col_ab), vec![vid(&paths[1])], "A › B 夹：b.mp4");
        assert!(
            !member_ids(col_a).contains(&root_vid) && !member_ids(col_ab).contains(&root_vid),
            "根目录直放的文件不归类"
        );
        assert_eq!(member_ids(manual).len(), 1, "手选夹不受镜像影响");

        // 文件从 A/B 移到 A：重扫后链接应跟着走（旧位置清掉、新位置补链）
        // 注意：路径 UPDATE 后旧路径在 videos 里已不存在，断言一律用移动前捕获的 id
        let id_b = vid(&paths[1]);
        let moved = root.join("A").join("b.mp4");
        fs::rename(root.join("A").join("B").join("b.mp4"), &moved).unwrap();
        let n = ctx
            .conn
            .execute("UPDATE videos SET path=?2 WHERE path=?1", params![ps(&paths[1]), ps(&moved)])
            .unwrap();
        assert_eq!(n, 1);
        let seen2 = vec![ps(&paths[0]), ps(&moved), ps(&paths[2]), ps(&paths[3])];
        sync_folder_collections(&ctx.conn, &root, &seen2).unwrap();
        assert!(auto_of("A › B").is_none(), "B 目录已无视频，镜像夹应整夹消失");
        assert!(!member_ids(col_ab).contains(&id_b), "旧位置链接应被清掉");
        assert!(member_ids(col_a).contains(&id_b), "新位置应补链");

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }

    /// 层级浏览：顶层=单段名夹；子层=前缀匹配；封面只取已有缓存，现场不生成
    #[test]
    fn list_folder_children_returns_layers() {
        let (root, ctx) = temp_lib();
        fs::create_dir_all(root.join("A")).unwrap();
        fs::create_dir_all(root.join("A/B")).unwrap();
        let paths = vec![
            root.join("A").join("a.mp4"),
            root.join("A").join("a2.mp4"),
            root.join("A").join("B").join("b.mp4"),
        ];
        for p in &paths {
            fs::write(p, b"x").unwrap();
        }
        let ps = |p: &Path| p.to_string_lossy().to_string();
        for p in &paths {
            upsert_video(&ctx.conn, &rec(&ps(p), 0)).unwrap();
        }
        let seen: Vec<String> = paths.iter().map(|p| ps(p)).collect();
        sync_folder_collections(&ctx.conn, &root, &seen).unwrap();

        // 顶层：只有 A（单段名），B 在下一层；a/a2 两条成员
        let top = list_folder_children(&ctx.conn, &root, None).unwrap();
        assert_eq!(top.len(), 1, "顶层应只有 A（根目录无视频不生成夹）");
        assert_eq!(top[0].leaf, "A");
        assert_eq!(top[0].name, "A");
        assert_eq!(top[0].count, 2);

        // 子层：A 下有 B；leaf 只剩末段
        let sub = list_folder_children(&ctx.conn, &root, Some("A")).unwrap();
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "A › B");
        assert_eq!(sub[0].leaf, "B");
        assert_eq!(sub[0].count, 1);

        // 封面：无任何缓存 → 空 covers（现场不生成，占位由前端画）
        assert_eq!(top[0].covers.len(), 0, "没有缓存图时 covers 应为空");

        // 造一个成员的中帧缓存后重查 → covers 出现该路径
        let mid = LibraryContext::vault_dir(&root)
            .join("cache")
            .join("cover")
            .join("mid-1.jpg");
        fs::create_dir_all(mid.parent().unwrap()).unwrap();
        fs::write(&mid, b"fake-jpeg").unwrap();
        let top2 = list_folder_children(&ctx.conn, &root, None).unwrap();
        assert_eq!(top2[0].covers.len(), 1, "有缓存图的成员才进拼贴素材");
        assert!(top2[0].covers[0].ends_with("mid-1.jpg"));

        // 空层：B 下没有子文件夹
        let deeper = list_folder_children(&ctx.conn, &root, Some("A › B")).unwrap();
        assert_eq!(deeper.len(), 0);

        drop(ctx);
        let _ = fs::remove_dir_all(&root);
    }
