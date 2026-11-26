//! Tauri 命令按领域拆分；lib.rs 只保留 AppState、启动注册与单实例守卫。
pub(crate) mod settings;
pub(crate) mod browse;
pub(crate) mod collections;
pub(crate) mod player;
pub(crate) mod sheets;
pub(crate) mod dedupe;
pub(crate) mod downloads;
pub(crate) mod editing;
pub(crate) mod covers;
pub(crate) mod imports;
pub(crate) mod files;
pub(crate) mod window;

pub(crate) mod prelude {
    //! 命令模块共用导入。每个子模块只需 `use super::prelude::*;`。
    pub(crate) use std::fs;
    pub(crate) use std::path::{Path, PathBuf};
    pub(crate) use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    pub(crate) use std::sync::{Arc, Mutex};

    pub(crate) use serde::Serialize;
    pub(crate) use tauri::{AppHandle, Emitter, Manager, State};
    pub(crate) use tauri_plugin_global_shortcut::GlobalShortcutExt;
    pub(crate) use walkdir::WalkDir;

    pub(crate) use crate::db::{self, LibraryContext};
    pub(crate) use crate::models;
    pub(crate) use crate::models::{
        Collection, DownloadTask, LibraryInfo, ResolveReport, ScanOptions, ScanReport, Stats, Tag,
        VideoItem, VideoPage, VideoPatch, VideoQuery, WatchUpdate,
    };
    pub(crate) use crate::player::PlaybackInfo;
pub(crate) use crate::{downloader, edit, fileops, importer, media, mpv, player};
    pub(crate) use crate::{library, write_settings, AppState, Settings};
}
