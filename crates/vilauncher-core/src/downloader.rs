//! Модуль 1: асинхронная дельта-загрузка.
//!
//! Конвейер:
//!   1. `plan_delta`   — параллельно (blocking-пул) сверяет локальные файлы с манифестом
//!                       по размеру и хешу (SHA-256 / SHA-1 / MD5) и строит список того,
//!                       что реально нужно скачать (отсутствует / изменён).
//!   2. `download_all` — качает только этот список: N файлов одновременно (`Semaphore`-подобно
//!                       через `buffer_unordered`), потоковая запись в `*.part`, хеш считается
//!                       «на лету», проверка размера и хеша, атомарный `rename`,
//!                       повторы с экспоненциальной паузой.
//!
//! Ни один файл не попадает на финальный путь, пока не прошёл проверку целостности.

use crate::error::{CoreError, Result};
use crate::fsutil::{safe_join, with_suffix};
use crate::hash::{hash_eq, hash_file, HashAlgo, Hasher};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

// ───────────────────────────── типы ─────────────────────────────

/// Запись манифеста: что скачать и как проверить.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RemoteFile {
    /// Относительный путь внутри каталога клиента (`mods/x.jar`).
    pub path: String,
    /// Прямой URL. Для записей манифеста без `url` заполняется на уровне `sync`.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
    #[serde(default)]
    pub md5: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
}

impl RemoteFile {
    /// Самый сильный из известных хешей: SHA-256 > SHA-1 > MD5.
    pub fn expected_hash(&self) -> Option<(HashAlgo, &str)> {
        fn pick(v: &Option<String>) -> Option<&str> {
            v.as_deref().map(str::trim).filter(|s| !s.is_empty())
        }
        if let Some(h) = pick(&self.sha256) {
            return Some((HashAlgo::Sha256, h));
        }
        if let Some(h) = pick(&self.sha1) {
            return Some((HashAlgo::Sha1, h));
        }
        if let Some(h) = pick(&self.md5) {
            return Some((HashAlgo::Md5, h));
        }
        None
    }
}

/// Политика сверки локального файла.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncPolicy {
    /// Файл должен совпадать с манифестом (размер + хеш, если они известны).
    Strict,
    /// «Ленивая»: качаем только если файла нет. Пользовательские правки не трогаем
    /// (config/, options.txt и т.п.).
    Lazy,
    /// «Вернуть по умолчанию»: файл скачивается всегда, затирая пользовательские правки.
    Force,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadReason {
    Missing,
    SizeMismatch,
    HashMismatch,
    /// Принудительная перезапись (сброс настроек по умолчанию).
    Forced,
}

#[derive(Clone, Debug)]
pub struct PlannedFile {
    pub file: RemoteFile,
    pub dest: PathBuf,
    pub reason: DownloadReason,
    /// Произвольная метка группы (core/mods/defaults) — для разбора ошибок вызывающим кодом.
    pub tag: u32,
}

#[derive(Clone, Debug, Default)]
pub struct DeltaPlan {
    pub to_download: Vec<PlannedFile>,
    /// Сколько файлов уже актуальны и скачиваться не будут.
    pub up_to_date: usize,
}

impl DeltaPlan {
    pub fn total_known_bytes(&self) -> u64 {
        self.to_download.iter().filter_map(|p| p.file.size).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.to_download.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct DownloadOptions {
    /// Сколько файлов качать одновременно.
    pub concurrency: usize,
    /// Сколько повторов после первой неудачной попытки.
    pub retries: u32,
    /// Базовая пауза перед повтором (удваивается каждый раз).
    pub backoff: Duration,
    /// Минимальный интервал между событиями прогресса одного файла.
    pub progress_interval: Duration,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            concurrency: 6,
            retries: 3,
            backoff: Duration::from_millis(500),
            progress_interval: Duration::from_millis(100),
        }
    }
}

/// Событие прогресса. Имена полей в camelCase совместимы с прежним
/// IPC-событием `mods-progress` (fileIndex/total/name/filePct/fileDone/fileSize/done).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub file_index: usize,
    pub completed: usize,
    pub total: usize,
    pub name: String,
    pub file_pct: u8,
    pub file_done: u64,
    pub file_size: u64,
    pub overall_done: u64,
    pub overall_total: u64,
    pub done: bool,
}

pub type ProgressFn = Arc<dyn Fn(ProgressEvent) + Send + Sync>;

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct FailedFile {
    pub path: String,
    pub tag: u32,
    pub error: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DownloadSummary {
    pub downloaded: usize,
    pub bytes: u64,
    pub failed: Vec<FailedFile>,
}

// ───────────────────────────── HTTP-клиент ─────────────────────────────

/// Тип HTTP-клиента ядра (реэкспорт, чтобы оболочке не нужна была зависимость от `reqwest`).
pub type HttpClient = reqwest::Client;

pub fn build_client(user_agent: &str) -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()?)
}

/// Скачать небольшой текстовый ресурс (манифест, JSON) с повторами.
pub async fn fetch_text(client: &reqwest::Client, url: &str, retries: u32) -> Result<String> {
    let mut last: Option<CoreError> = None;
    for attempt in 0..=retries {
        let res = async {
            let r = client.get(url).send().await?.error_for_status()?;
            Ok::<String, CoreError>(r.text().await?)
        }
        .await;
        match res {
            Ok(t) => return Ok(t),
            Err(e) => {
                if !is_retryable(&e) {
                    return Err(e);
                }
                last = Some(e);
                if attempt < retries {
                    tokio::time::sleep(Duration::from_millis(400 * (1u64 << attempt))).await;
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| CoreError::other("fetch_text: неизвестная ошибка")))
}

// ───────────────────────────── 1. планирование ─────────────────────────────

async fn check_one(dest: &Path, file: &RemoteFile, policy: SyncPolicy) -> Result<Option<DownloadReason>> {
    let meta = match tokio::fs::metadata(dest).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Some(DownloadReason::Missing)),
        Err(e) => return Err(e.into()),
    };
    if !meta.is_file() {
        return Err(CoreError::other(format!(
            "На месте файла находится каталог: {}",
            dest.display()
        )));
    }
    if policy == SyncPolicy::Lazy {
        return Ok(None);
    }
    if policy == SyncPolicy::Force {
        return Ok(Some(DownloadReason::Forced));
    }
    if let Some(size) = file.size {
        if meta.len() != size {
            return Ok(Some(DownloadReason::SizeMismatch));
        }
    }
    if let Some((algo, expected)) = file.expected_hash() {
        let actual = hash_file(dest.to_path_buf(), algo).await?;
        if !hash_eq(&actual, expected) {
            return Ok(Some(DownloadReason::HashMismatch));
        }
    }
    Ok(None)
}

/// Сравнивает локальные файлы с манифестом и возвращает только то, что надо скачать.
/// Хеширование идёт параллельно (до `hash_concurrency` файлов одновременно) в blocking-пуле.
pub async fn plan_delta(
    base_dir: &Path,
    files: &[RemoteFile],
    policy: SyncPolicy,
    tag: u32,
    hash_concurrency: usize,
) -> Result<DeltaPlan> {
    // Сначала валидируем ВСЕ пути: один небезопасный путь = манифест отвергается целиком.
    let mut dests = Vec::with_capacity(files.len());
    for f in files {
        dests.push(safe_join(base_dir, &f.path)?);
    }

    let sem = Arc::new(Semaphore::new(hash_concurrency.max(1)));
    let mut set: JoinSet<(usize, Result<Option<DownloadReason>>)> = JoinSet::new();
    for (idx, (f, dest)) in files.iter().zip(dests.iter()).enumerate() {
        let f = f.clone();
        let dest = dest.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            (idx, check_one(&dest, &f, policy).await)
        });
    }

    let mut reasons: Vec<Option<DownloadReason>> = vec![None; files.len()];
    while let Some(joined) = set.join_next().await {
        let (idx, res) = joined.map_err(|e| CoreError::other(format!("hash task: {e}")))?;
        reasons[idx] = res?;
    }

    let mut plan = DeltaPlan::default();
    for ((f, dest), reason) in files.iter().zip(dests).zip(reasons) {
        match reason {
            Some(reason) => plan.to_download.push(PlannedFile {
                file: f.clone(),
                dest,
                reason,
                tag,
            }),
            None => plan.up_to_date += 1,
        }
    }
    Ok(plan)
}

// ───────────────────────────── 2. загрузка ─────────────────────────────

struct Shared {
    overall_done: AtomicU64,
    overall_total: u64,
    completed: AtomicUsize,
    total: usize,
    progress: Option<ProgressFn>,
    cancel: CancelToken,
    throttle: Mutex<Vec<Instant>>,
}

impl Shared {
    fn emit(&self, idx: usize, name: &str, file_done: u64, file_size: u64, done: bool) {
        let Some(cb) = &self.progress else { return };
        let file_pct = if done {
            100
        } else if file_size > 0 {
            ((file_done as f64 / file_size as f64) * 100.0).min(100.0) as u8
        } else {
            0
        };
        cb(ProgressEvent {
            file_index: idx,
            completed: self.completed.load(Ordering::Relaxed),
            total: self.total,
            name: name.to_string(),
            file_pct,
            file_done,
            file_size,
            overall_done: self.overall_done.load(Ordering::Relaxed),
            overall_total: self.overall_total,
            done,
        });
    }

    /// true, если для этого файла пора слать очередное событие прогресса.
    fn should_emit(&self, idx: usize, interval: Duration) -> bool {
        let mut slots = self.throttle.lock().unwrap();
        let now = Instant::now();
        if now.duration_since(slots[idx]) >= interval {
            slots[idx] = now;
            true
        } else {
            false
        }
    }
}

fn is_retryable(e: &CoreError) -> bool {
    match e {
        CoreError::Http(err) => match err.status() {
            // 4xx — повтор бессмысленен, кроме таймаута запроса и лимита запросов.
            Some(s) if s.is_client_error() => s.as_u16() == 408 || s.as_u16() == 429,
            _ => true,
        },
        CoreError::Io(_) | CoreError::HashMismatch { .. } | CoreError::SizeMismatch { .. } => true,
        _ => false,
    }
}

async fn download_attempt(
    client: &reqwest::Client,
    pf: &PlannedFile,
    idx: usize,
    shared: &Shared,
    opts: &DownloadOptions,
    got: &mut u64,
) -> Result<u64> {
    if let Some(parent) = pf.dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let part = with_suffix(&pf.dest, ".part");
    let name = pf
        .dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| pf.file.path.clone());

    let resp = client.get(&pf.file.url).send().await?.error_for_status()?;
    let file_size = resp.content_length().or(pf.file.size).unwrap_or(0);

    let mut out = tokio::fs::File::create(&part).await?;
    let expected = pf.file.expected_hash();
    let mut hasher = expected.map(|(algo, _)| Hasher::new(algo));
    let mut stream = resp.bytes_stream();
    let mut file_done = 0u64;

    shared.emit(idx, &name, 0, file_size, false);
    while let Some(chunk) = stream.next().await {
        if shared.cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let chunk = chunk?;
        out.write_all(&chunk).await?;
        if let Some(h) = hasher.as_mut() {
            h.update(&chunk);
        }
        let n = chunk.len() as u64;
        file_done += n;
        *got += n;
        shared.overall_done.fetch_add(n, Ordering::Relaxed);
        if shared.should_emit(idx, opts.progress_interval) {
            shared.emit(idx, &name, file_done, file_size, false);
        }
    }
    out.flush().await?;
    drop(out);

    if let Some(expected_size) = pf.file.size {
        if file_done != expected_size {
            return Err(CoreError::SizeMismatch {
                path: pf.file.path.clone(),
                expected: expected_size,
                actual: file_done,
            });
        }
    }
    if let (Some(h), Some((_, want))) = (hasher, expected) {
        let actual = h.finalize_hex();
        if !hash_eq(&actual, want) {
            return Err(CoreError::HashMismatch {
                path: pf.file.path.clone(),
                expected: want.to_string(),
                actual,
            });
        }
    }

    // Файл проверен — атомарно ставим на место.
    tokio::fs::rename(&part, &pf.dest).await?;
    Ok(file_done)
}

async fn download_with_retries(
    client: &reqwest::Client,
    pf: &PlannedFile,
    idx: usize,
    opts: &DownloadOptions,
    shared: &Shared,
) -> Result<u64> {
    let part = with_suffix(&pf.dest, ".part");
    let mut attempt = 0u32;
    loop {
        if shared.cancel.is_cancelled() {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(CoreError::Cancelled);
        }
        let mut got = 0u64;
        match download_attempt(client, pf, idx, shared, opts, &mut got).await {
            Ok(bytes) => {
                shared.completed.fetch_add(1, Ordering::Relaxed);
                let name = pf.dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                shared.emit(idx, &name, bytes, bytes, true);
                return Ok(bytes);
            }
            Err(e) => {
                // Откатываем учтённые байты неудачной попытки и убираем огрызок.
                shared.overall_done.fetch_sub(got, Ordering::Relaxed);
                let _ = tokio::fs::remove_file(&part).await;
                if matches!(e, CoreError::Cancelled) || attempt >= opts.retries || !is_retryable(&e) {
                    return Err(e);
                }
                log::warn!("[download] {} — попытка {} не удалась: {e}", pf.file.path, attempt + 1);
                tokio::time::sleep(opts.backoff * (1u32 << attempt.min(6))).await;
                attempt += 1;
            }
        }
    }
}

/// Параллельно скачивает всё из плана. Ошибки отдельных файлов не останавливают остальные —
/// они собираются в `DownloadSummary::failed`, а решение (фатально это или нет) принимает
/// вызывающий код. Исключение — отмена: возвращает `Err(Cancelled)`.
pub async fn download_all(
    client: &reqwest::Client,
    plan: &DeltaPlan,
    opts: &DownloadOptions,
    cancel: &CancelToken,
    progress: Option<ProgressFn>,
) -> Result<DownloadSummary> {
    let total = plan.to_download.len();
    let mut summary = DownloadSummary::default();
    if total == 0 {
        return Ok(summary);
    }

    let shared = Arc::new(Shared {
        overall_done: AtomicU64::new(0),
        overall_total: plan.total_known_bytes(),
        completed: AtomicUsize::new(0),
        total,
        progress,
        cancel: cancel.clone(),
        throttle: Mutex::new(vec![Instant::now() - Duration::from_secs(60); total]),
    });

    // Каждая загрузка — отдельная задача с владеющими данными (`'static + Send`);
    // одновременность ограничивает семафор. Если вызывающий код бросит это future
    // (отмена), `JoinSet` оборвёт все задачи.
    let sem = Arc::new(Semaphore::new(opts.concurrency.max(1)));
    let mut set: JoinSet<(usize, Result<u64>)> = JoinSet::new();
    for (idx, pf) in plan.to_download.iter().cloned().enumerate() {
        let (shared, client, opts, sem) = (shared.clone(), client.clone(), opts.clone(), sem.clone());
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            (idx, download_with_retries(&client, &pf, idx, &opts, &shared).await)
        });
    }
    let mut outcomes: Vec<(usize, Result<u64>)> = Vec::with_capacity(total);
    while let Some(joined) = set.join_next().await {
        outcomes.push(joined.map_err(|e| CoreError::other(format!("download task: {e}")))?);
    }

    for (idx, res) in outcomes {
        let pf = &plan.to_download[idx];
        match res {
            Ok(bytes) => {
                summary.downloaded += 1;
                summary.bytes += bytes;
            }
            Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
            Err(e) => summary.failed.push(FailedFile {
                path: pf.file.path.clone(),
                tag: pf.tag,
                error: e.to_string(),
            }),
        }
    }
    Ok(summary)
}
