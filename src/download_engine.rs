use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::cpu_manager;
use crate::fs_watcher;
use crate::hyper_shield::{self, GpuScanVerdict};

/// Фразы, которые крутятся при реальной проверке файла на сервере
pub const PROBE_PHRASES: &[&str] = &[
    "⚡ Пробуем достучаться до файла...",
    "⚡ Пробуем получить секретный доступ...",
    "⚡ Взламываем Пентагон...",
    "⚡ Проверяем заголовки и целостность файла...",
    "⚙ Синхронизируем фазовые решетки P-ядер...",
];

#[derive(Clone, Debug, PartialEq)]
pub enum ModalProbeState {
    Idle,
    Probing(String),
    Connected(String),
    Failed(String),
}

/// Информация о реально найденном на сервере файле
#[derive(Clone, Debug)]
pub struct RemoteFileMeta {
    pub normalized_url: String,
    pub total_bytes: Option<u64>,
    pub supports_range: bool,
}

/// Автоматически добавляет https://, если пользователь ввел просто hamam.com/file.txt
pub fn normalize_url(raw_url: &str) -> Result<String, String> {
    let trimmed = raw_url.trim();
    if trimmed.is_empty() {
        return Err("Пустая ссылка".to_string());
    }
    if trimmed.starts_with("magnet:?xt=") {
        return Ok(trimmed.to_string());
    }
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return Ok(trimmed.to_string());
    }
    if trimmed.contains('.') && !trimmed.contains(' ') {
        return Ok(format!("https://{}", trimmed));
    }
    Err("Некорректный адрес сервера".to_string())
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} МБ", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} КБ", bytes as f64 / 1024.0)
    } else {
        format!("{} Б", bytes)
    }
}

/// Внутренняя проверка HTTP-ответа, кода 200/206, размера и защита от HTML-страниц вместо файлов
pub fn probe_remote_file(raw_url: &str) -> Result<RemoteFileMeta, String> {
    let url = normalize_url(raw_url)?;

    if url.starts_with("magnet:?xt=") {
        if url.len() > 20 {
            return Ok(RemoteFileMeta {
                normalized_url: url,
                total_bytes: None,
                supports_range: false,
            });
        } else {
            return Err("Некорректный Magnet-хеш".to_string());
        }
    }

    // Проверяем, что в ссылке есть путь к файлу, а не просто голый домен сайта
    let without_proto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(&url);

    let (host, path_part) = match without_proto.split_once('/') {
        Some((h, p)) => (h, p),
        None => return Err("Это адрес сайта, а не ссылка на файл".to_string()),
    };

    if host.is_empty() || !host.contains('.') || host.starts_with('.') || host.ends_with('.') {
        return Err(format!("Домен '{}' не существует", host));
    }

    let clean_path = path_part.split(&['?', '#'][..]).next().unwrap_or("").trim();
    if clean_path.is_empty() {
        return Err("В ссылке не указан файл для скачивания".to_string());
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(3500))
        .timeout_read(Duration::from_millis(5000))
        .build();

    // Запрашиваем первые 512 байт файла для проверки заголовков и сигнатуры
    let response = agent
        .get(&url)
        .set("User-Agent", "RustDM/0.1.0-MegaPatch")
        .set("Range", "bytes=0-511")
        .call();

    match response {
        Ok(resp) => {
            let status = resp.status();
            if status != 200 && status != 206 {
                return Err(format!("Сервер отклонил запрос (HTTP {})", status));
            }

            let content_type = resp.content_type().to_lowercase();
            let supports_range = status == 206
                || resp
                    .header("Accept-Ranges")
                    .map(|v| v.contains("bytes"))
                    .unwrap_or(false);

            // Вычисляем точный размер файла из Content-Range или Content-Length
            let total_bytes = if let Some(cr) = resp.header("Content-Range") {
                cr.rsplit('/').next().and_then(|s| s.parse::<u64>().ok())
            } else {
                resp.header("Content-Length").and_then(|s| s.parse::<u64>().ok())
            };

            // Читаем первые байты ответа, чтобы убедиться, что это реальный файл, а не HTML-заглушка
            let mut peek_buf = [0u8; 512];
            let mut reader = resp.into_reader();
            let n = reader.read(&mut peek_buf).unwrap_or(0);

            if n == 0 && total_bytes == Some(0) {
                return Err("Файл на сервере пуст (0 байт)".to_string());
            }

            let peek_text = String::from_utf8_lossy(&peek_buf[..n]).to_lowercase();
            let lower_path = clean_path.to_lowercase();
            let user_asked_for_html =
                lower_path.ends_with(".html") || lower_path.ends_with(".htm");

            // Если пользователь просил файл (.txt, .zip, .exe и т.д.), а сайт вернул HTML-страницу!
            if !user_asked_for_html
                && (content_type.contains("text/html")
                    || peek_text.contains("<!doctype html")
                    || peek_text.contains("<html"))
            {
                return Err(
                    "Такого файла нет на сайте (сервер вернул веб-страницу вместо файла)"
                        .to_string(),
                );
            }

            Ok(RemoteFileMeta {
                normalized_url: url,
                total_bytes,
                supports_range,
            })
        }
        Err(ureq::Error::Status(code, _)) => {
            if code == 404 {
                Err("Такого файла не существует на сервере (HTTP 404 Not Found)".to_string())
            } else if code == 403 || code == 401 {
                Err(format!("Доступ к файлу запрещен сервером (HTTP {})", code))
            } else {
                Err(format!("Сервер вернул ошибку HTTP {}", code))
            }
        }
        Err(_) => Err(format!("Сервер '{}' недоступен или не существует", host)),
    }
}

/// Проверка ссылки для окна «Новая загрузка»
pub fn verify_url_reachability(raw_url: &str) -> Result<String, String> {
    let meta = probe_remote_file(raw_url)?;
    match meta.total_bytes {
        Some(bytes) => Ok(format!("Файл доступен • Размер: {}", format_bytes(bytes))),
        None => Ok("Файл доступен • Потоковый канал готов".to_string()),
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Priority {
    Auto,
    Low,
    Normal,
    High,
}

impl Priority {
    pub fn label(&self) -> &'static str {
        match self {
            Priority::Auto => "⚡ Авто (Под CPU)",
            Priority::Low => "Тихий (Low)",
            Priority::Normal => "Обычный (Norm)",
            Priority::High => "Максимум (High)",
        }
    }

    pub fn effective_delay_ms(&self, current_cpu_load: f32, snail_throttled: bool) -> (u64, &'static str) {
        if snail_throttled {
            return (140, "Улитка (Фоновый)");
        }
        match self {
            Priority::Low => (110, "Ручной: Тихий"),
            Priority::Normal => (45, "Ручной: Обычный"),
            Priority::High => (12, "Ручной: Макс."),
            Priority::Auto => {
                if current_cpu_load < 35.0 {
                    (12, "Авто: Буст (CPU свободен)")
                } else if current_cpu_load < 75.0 {
                    (45, "Авто: Баланс")
                } else {
                    (125, "Авто: Игровой (CPU занят)")
                }
            }
        }
    }
}

pub struct DownloadItem {
    pub id: usize,
    pub url: String,
    pub filename: String,
    pub save_dir: PathBuf,
    pub full_path: PathBuf,
    pub progress_p1: Arc<Mutex<f32>>,
    pub progress_p2: Arc<Mutex<f32>>,
    pub status: Arc<Mutex<String>>,
    pub bar_overlay_text: Arc<Mutex<Option<String>>>,
    pub connection_failed: Arc<Mutex<bool>>,
    pub assigned_cores: Arc<Mutex<String>>,
    pub priority: Arc<Mutex<Priority>>,
    pub is_paused: Arc<Mutex<bool>>,
    pub is_cancelled: Arc<Mutex<bool>>,
    pub snail_throttled: Arc<Mutex<bool>>,
    pub is_finished: Arc<Mutex<bool>>,
    pub cf_shield: bool,
    pub gpu_verify: bool,
    pub gpu_verdict: Arc<Mutex<GpuScanVerdict>>,
}

pub fn extract_filename(url: &str, id: usize) -> String {
    let clean = url.split(&['?', '#'][..]).next().unwrap_or(url).trim_end_matches('/');
    if let Some(last) = clean.rsplit('/').next() {
        if !last.is_empty() && last.contains('.') && !last.starts_with("http") {
            return last.to_string();
        }
    }
    format!("downloaded_file_{}.bin", id)
}

pub fn spawn_smart_download(
    item: &DownloadItem,
    global_cpu_load: Arc<Mutex<f32>>,
    cf_node_label: String,
    already_verified_in_modal: bool,
) {
    let p1 = Arc::clone(&item.progress_p1);
    let p2 = Arc::clone(&item.progress_p2);
    let status = Arc::clone(&item.status);
    let bar_overlay = Arc::clone(&item.bar_overlay_text);
    let conn_failed = Arc::clone(&item.connection_failed);
    let cores_label = Arc::clone(&item.assigned_cores);
    let priority = Arc::clone(&item.priority);
    let is_paused = Arc::clone(&item.is_paused);
    let is_cancelled = Arc::clone(&item.is_cancelled);
    let snail_throttled = Arc::clone(&item.snail_throttled);
    let is_finished = Arc::clone(&item.is_finished);
    let gpu_verdict = Arc::clone(&item.gpu_verdict);

    let cf_enabled = item.cf_shield;
    let gpu_enabled = item.gpu_verify;
    let save_dir = item.save_dir.clone();
    let final_path = item.full_path.clone();
    let filename = item.filename.clone();
    let raw_url = item.url.clone();

    thread::spawn(move || {
        let selection = cpu_manager::find_best_p_cores();
        let cores_info = format!(
            "P-Core #{} ({} МГц) + P-Core #{} ({} МГц)",
            selection.core1_id, selection.core1_freq, selection.core2_id, selection.core2_freq
        );
        *cores_label.lock().unwrap() = cores_info.clone();

        // =====================================================================
        // ЭТАП 1: РЕАЛЬНАЯ ПРОВЕРКА ФАЙЛА НА СЕРВЕРЕ
        // =====================================================================
        let url_for_check = raw_url.clone();
        let check_handle = thread::spawn(move || probe_remote_file(&url_for_check));

        if !already_verified_in_modal {
            for phrase in PROBE_PHRASES.iter().take(4) {
                if *is_cancelled.lock().unwrap() {
                    return;
                }
                *status.lock().unwrap() = (*phrase).to_string();
                *bar_overlay.lock().unwrap() = Some((*phrase).to_string());
                thread::sleep(Duration::from_millis(480));
            }
        }

        let meta = match check_handle.join().unwrap_or_else(|_| Err("Сбой потока сети".to_string())) {
            Ok(m) => {
                *status.lock().unwrap() = "✔ Соединение установлено! Начинаем скачивание...".to_string();
                *bar_overlay.lock().unwrap() = Some("✔ Соединение установлено!".to_string());
                thread::sleep(Duration::from_millis(650));
                m
            }
            Err(err_msg) => {
                *conn_failed.lock().unwrap() = true;
                *status.lock().unwrap() = format!("✖ Ошибка: {}", err_msg);
                *bar_overlay.lock().unwrap() = Some(format!("✖ Ошибка: {}", err_msg));
                return;
            }
        };

        // Убираем надпись подключения — включаем живые % на полоске
        *bar_overlay.lock().unwrap() = None;

        // =====================================================================
        // ЭТАП 2: РЕАЛЬНОЕ СКАЧИВАНИЕ БАЙТОВ ФАЙЛА (50/50 ПО ДВУМ P-ЯДРАМ)
        // =====================================================================
        let _ = fs::create_dir_all(&save_dir);
        let part_path = fs_watcher::get_part_path(&final_path);
        let _ = File::create(&part_path);

        let route_tag = if cf_enabled {
            format!("CF: {}", cf_node_label)
        } else {
            "Напрямую".to_string()
        };

        // Скачиваем полный поток файла с сервера для распределения по 2 P-ядрам
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(5000))
            .build();

        // Стримим ответ в память порциями, чтобы пауза/отмена работали уже во время передачи,
        // а не только после того, как файл целиком окажется в RAM.
        let full_bytes: Vec<u8> = match agent
            .get(&meta.normalized_url)
            .set("User-Agent", "RustDM/0.1.0-MegaPatch")
            .call()
        {
            Ok(resp) => {
                let mut buf = Vec::new();
                let mut reader = resp.into_reader();
                let mut chunk = [0u8; 64 * 1024];

                loop {
                    if *is_cancelled.lock().unwrap() {
                        let _ = fs::remove_file(&part_path);
                        return;
                    }
                    if *is_paused.lock().unwrap() {
                        *status.lock().unwrap() = "⏸ На паузе (соединение удерживается)".to_string();
                        *bar_overlay.lock().unwrap() = Some("⏸ Пауза".to_string());
                        thread::sleep(Duration::from_millis(120));
                        continue;
                    }

                    match reader.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            *bar_overlay.lock().unwrap() =
                                Some(format!("⚡ Приём потока: {}", format_bytes(buf.len() as u64)));
                        }
                        Err(_) => break,
                    }
                }

                if buf.is_empty() {
                    *conn_failed.lock().unwrap() = true;
                    *status.lock().unwrap() = "✖ Сервер закрыл соединение, файл не получен".to_string();
                    *bar_overlay.lock().unwrap() = Some("✖ Обрыв соединения".to_string());
                    let _ = fs::remove_file(&part_path);
                    return;
                }
                buf
            }
            Err(_) => {
                *conn_failed.lock().unwrap() = true;
                *status.lock().unwrap() = "✖ Обрыв соединения при передаче файла".to_string();
                *bar_overlay.lock().unwrap() = Some("✖ Обрыв соединения".to_string());
                let _ = fs::remove_file(&part_path);
                return;
            }
        };

        // Поток принят полностью — снова отдаём полоску под живой прогресс 50/50
        *bar_overlay.lock().unwrap() = None;

        let total_len = full_bytes.len();
        let mid = total_len / 2;
        let first_half_data = Arc::new(full_bytes[..mid].to_vec());
        let second_half_data = Arc::new(full_bytes[mid..].to_vec());

        let out_part1: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::with_capacity(mid)));
        let out_part2: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::with_capacity(total_len - mid)));

        let p1_clone = Arc::clone(&p1);
        let prio_1 = Arc::clone(&priority);
        let cpu_1 = Arc::clone(&global_cpu_load);
        let status_1 = Arc::clone(&status);
        let pause_1 = Arc::clone(&is_paused);
        let cancel_1 = Arc::clone(&is_cancelled);
        let snail_1 = Arc::clone(&snail_throttled);
        let core_1 = selection.core1_id;
        let route_1 = route_tag.clone();
        let data_1 = Arc::clone(&first_half_data);
        let out_1 = Arc::clone(&out_part1);

        // Поток 1 (P-Core #1): обрабатывает и записывает первые 50% реальных байтов файла
        let handle_1 = thread::spawn(move || {
            cpu_manager::pin_current_thread_to_core(core_1);
            let total_steps = 50usize;
            let chunk_size = (data_1.len() / total_steps).max(1);

            for step in 0..total_steps {
                loop {
                    if *cancel_1.lock().unwrap() {
                        return;
                    }
                    if *pause_1.lock().unwrap() {
                        *status_1.lock().unwrap() = "⏸ На паузе (потоки заморожены)".to_string();
                        thread::sleep(Duration::from_millis(120));
                    } else {
                        break;
                    }
                }

                let p_mode = *prio_1.lock().unwrap();
                let cpu_now = *cpu_1.lock().unwrap();
                let is_snail = *snail_1.lock().unwrap();
                let (delay, mode_text) = p_mode.effective_delay_ms(cpu_now, is_snail);

                *status_1.lock().unwrap() = format!(
                    "{}  |  {}  |  {}",
                    route_1,
                    mode_text,
                    format_bytes(total_len as u64)
                );

                let start = step * chunk_size;
                if start < data_1.len() {
                    let end = if step + 1 == total_steps {
                        data_1.len()
                    } else {
                        ((step + 1) * chunk_size).min(data_1.len())
                    };
                    out_1.lock().unwrap().extend_from_slice(&data_1[start..end]);
                }

                thread::sleep(Duration::from_millis(delay));
                *p1_clone.lock().unwrap() = ((step + 1) as f32 / total_steps as f32).min(1.0);
            }
        });

        let p2_clone = Arc::clone(&p2);
        let prio_2 = Arc::clone(&priority);
        let cpu_2 = Arc::clone(&global_cpu_load);
        let pause_2 = Arc::clone(&is_paused);
        let cancel_2 = Arc::clone(&is_cancelled);
        let snail_2 = Arc::clone(&snail_throttled);
        let core_2 = selection.core2_id;
        let data_2 = Arc::clone(&second_half_data);
        let out_2 = Arc::clone(&out_part2);

        // Поток 2 (P-Core #2): обрабатывает и записывает вторые 50% реальных байтов файла
        let handle_2 = thread::spawn(move || {
            cpu_manager::pin_current_thread_to_core(core_2);
            let total_steps = 50usize;
            let chunk_size = (data_2.len() / total_steps).max(1);

            for step in 0..total_steps {
                loop {
                    if *cancel_2.lock().unwrap() {
                        return;
                    }
                    if *pause_2.lock().unwrap() {
                        thread::sleep(Duration::from_millis(120));
                    } else {
                        break;
                    }
                }

                let p_mode = *prio_2.lock().unwrap();
                let cpu_now = *cpu_2.lock().unwrap();
                let is_snail = *snail_2.lock().unwrap();
                let (delay, _) = p_mode.effective_delay_ms(cpu_now, is_snail);

                let start = step * chunk_size;
                if start < data_2.len() {
                    let end = if step + 1 == total_steps {
                        data_2.len()
                    } else {
                        ((step + 1) * chunk_size).min(data_2.len())
                    };
                    out_2.lock().unwrap().extend_from_slice(&data_2[start..end]);
                }

                thread::sleep(Duration::from_millis(delay + 5));
                *p2_clone.lock().unwrap() = ((step + 1) as f32 / total_steps as f32).min(1.0);
            }
        });

        let _ = handle_1.join();
        let _ = handle_2.join();

        if *is_cancelled.lock().unwrap() {
            let _ = fs::remove_file(&part_path);
            return;
        }

        // Склеиваем обе реальные половинки файла на диске!
        if let Ok(mut file) = File::create(&final_path) {
            let _ = file.write_all(&out_part1.lock().unwrap());
            let _ = file.write_all(&out_part2.lock().unwrap());
        }
        let _ = fs::remove_file(&part_path);

        // =====================================================================
        // ЭТАП 3: ГИПЕР-БИБЛИОТЕКА GPU (ХЕШ РЕАЛЬНЫХ БАЙТОВ + АНТИВИРУС)
        // =====================================================================
        if gpu_enabled {
            *gpu_verdict.lock().unwrap() = GpuScanVerdict::Scanning;
            *status.lock().unwrap() = "⚡ GPU Shield: Матричная сверка 50/50 и антивирус...".to_string();

            let verdict = hyper_shield::execute_hyper_gpu_pipeline(&filename, &meta.normalized_url, &final_path);
            match &verdict {
                GpuScanVerdict::Clean { short_hash } => {
                    *status.lock().unwrap() = format!(
                        "✔ Проверено GPU [{}] ({})  |  {}",
                        short_hash,
                        format_bytes(total_len as u64),
                        final_path.display()
                    );
                }
                GpuScanVerdict::Threat { reason } => {
                    *status.lock().unwrap() = format!(
                        "! ВНИМАНИЕ: Обнаружена угроза [{}] !",
                        reason
                    );
                }
                _ => {}
            }
            *gpu_verdict.lock().unwrap() = verdict;
        } else {
            *status.lock().unwrap() = format!(
                "✔ Сохранено ({}): {}",
                format_bytes(total_len as u64),
                final_path.display()
            );
        }

        *is_finished.lock().unwrap() = true;
    });
}