//! 网页收藏的「待入库」元数据：油猴脚本先把 标题/作者/出处/标签/备注 提交到本地桥，
//! 等对应文件真的被扫描进库后，再把元数据挂上去。
//!
//! 这样脚本不需要知道文件落在哪、也不需要写磁盘：它只管提交，入库由影匣负责。

use serde::Serialize;

use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct HookRecord {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub author: String,
    pub site: String,
    pub tags: String,
    pub note: String,
}

/// 归一化标题用于匹配：去掉扩展名、空格、标点，全小写。
/// 文件名可能被浏览器加 (1)、下划线、截断，所以只做“包含”级匹配。
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 提交一条网页元数据；同一 URL 覆盖更新。
/// 返回 `(hook_id, 是否已有相同出处的视频)`——后者就是“防重复”的依据。
pub fn submit_hook(
    conn: &Connection,
    url: &str,
    title: &str,
    author: &str,
    site: &str,
    tags: &str,
    note: &str,
) -> Result<(i64, Option<i64>), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("url 不能为空".into());
    }
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM videos WHERE source_url=?1 AND trashed=0 LIMIT 1",
            params![url],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT INTO web_hooks (url, title, author, site, tags, note, created_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(url) DO UPDATE SET title=?2, author=?3, site=?4, tags=?5, note=?6",
        params![url, title, author, site, tags, note, now_secs()],
    )
    .map_err(|e| e.to_string())?;
    let id: i64 = conn
        .query_row("SELECT id FROM web_hooks WHERE url=?1", params![url], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok((id, existing))
}

/// 把一个 hook 挂到视频上：填出处、追加标签、合并备注。
fn attach(conn: &Connection, hook: &HookRecord, video_id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE videos SET source_url=?2, source_author=?3, source_site=?4,
            note = CASE WHEN note='' THEN ?5
                        WHEN ?5='' THEN note
                        ELSE note || char(10) || ?5 END
         WHERE id=?1",
        params![video_id, hook.url, hook.author, hook.site, hook.note],
    )
    .map_err(|e| e.to_string())?;

    // 标签：逗号/顿号/空格分隔，逐个建好再挂
    for raw in hook.tags.split(['，', ',', '、', ' ', ';', '；']) {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        conn.execute(
            "INSERT INTO tags (name, category, weight, created_at) VALUES (?1, '网页', 0, ?2)
             ON CONFLICT(name) DO NOTHING",
            params![name, now_secs()],
        )
        .map_err(|e| e.to_string())?;
        let tag_id: i64 = conn
            .query_row("SELECT id FROM tags WHERE name=?1", params![name], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT OR IGNORE INTO video_tags (video_id, tag_id) VALUES (?1, ?2)",
            params![video_id, tag_id],
        )
        .map_err(|e| e.to_string())?;
        refresh_tag_text(conn, video_id)?;
    }
    Ok(())
}

/// 扫描/导入之后调用：把还没挂上的 hook 按标题匹配到新入库的视频。
/// 只匹配 `source_url` 还是空的条目，避免覆盖已有出处；返回挂上了几条。
pub fn apply_pending_hooks(conn: &Connection) -> Result<usize, String> {
    let hooks: Vec<HookRecord> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, url, title, author, site, tags, note FROM web_hooks
                 WHERE applied_video_id=0 AND title<>'' ORDER BY id ASC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(HookRecord {
                    id: r.get(0)?,
                    url: r.get(1)?,
                    title: r.get(2)?,
                    author: r.get(3)?,
                    site: r.get(4)?,
                    tags: r.get(5)?,
                    note: r.get(6)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    if hooks.is_empty() {
        return Ok(0);
    }

    // 候选：还没出处、未回收的视频（文件名 → id）。比较时都去扩展名后归一化。
    let candidates: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, file_name FROM videos
                 WHERE trashed=0 AND source_url='' ORDER BY id DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };

    let mut applied = 0usize;
    for hook in &hooks {
        let want = norm(&hook.title);
        if want.len() < 3 {
            continue;
        }
        // 先找完全相等（去扩展名后），再退一步找包含关系，且要求唯一命中，避免张冠李戴
        let mut exact: Option<i64> = None;
        let mut partial: Vec<i64> = Vec::new();
        for (id, name) in &candidates {
            let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
            let got = norm(stem);
            if got == want {
                exact = Some(*id);
                break;
            }
            if got.contains(&want) || want.contains(&got) {
                partial.push(*id);
            }
        }
        let target = match exact {
            Some(id) => Some(id),
            None if partial.len() == 1 => Some(partial[0]),
            None => None,
        };
        if let Some(video_id) = target {
            attach(conn, hook, video_id)?;
            conn.execute(
                "UPDATE web_hooks SET applied_video_id=?2 WHERE id=?1",
                params![hook.id, video_id],
            )
            .map_err(|e| e.to_string())?;
            applied += 1;
        }
    }
    Ok(applied)
}

/// 给脚本用的查重：这个 URL 是否已经在库里。
pub fn find_by_url(conn: &Connection, url: &str) -> Result<Option<VideoItem>, String> {
    let id: Option<i64> = conn
        .query_row(
            "SELECT id FROM videos WHERE source_url=?1 AND trashed=0 LIMIT 1",
            params![url],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    match id {
        Some(id) => get_video(conn, id).map(Some),
        None => Ok(None),
    }
}
