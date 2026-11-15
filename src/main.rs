#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod cpu_manager;

use cpu_manager::CoreSelection;
use eframe::egui;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const APP_VERSION: &str = "v0.1.0-alpha.2";

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 520.0]) // Сделали окно просторнее для списка
            .with_title(format!("RustDM [{}] - Smart P-Core Downloader", APP_VERSION)),
        ..Default::default()
    };

    eframe::run_native(
        "RustDM",
        options,
        Box::new(|cc| {
            // Включаем стильную темную тему по умолчанию
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            Box::new(DownloadManagerApp::default())
        }),
    )
}

// Уровни приоритета загрузки
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    Low,
    Normal,
    High,
}

impl Priority {
    fn label(&self) -> &'static str {
        match self {
            Priority::Low => "🐢 Низкий",
            Priority::Normal => "⚡ Обычный",
            Priority::High => "🔥 Высокий",
        }
    }

    // Задержка потока (чем выше приоритет, тем быстрее ядра молотят пакеты)
    fn delay_ms(&self) -> u64 {
        match self {
            Priority::Low => 110,
            Priority::Normal => 55,
            Priority::High => 20,
        }
    }
}

// Карточка одной конкретной загрузки в списке
struct DownloadItem {
    id: usize,
    file_name: String,
    url: String,
    priority: Priority,
    is_downloading: bool,
    is_finished: bool,
    progress_core_1: f32,
    progress_core_2: f32,
    cores_info: CoreSelection,
    status_text: String,
}

struct DownloadManagerApp {
    url_input: String,
    new_priority: Priority,
    downloads: Vec<Arc<Mutex<DownloadItem>>>, // Наш список всех загрузок (история + активные)
    next_id: usize,
}

impl Default for DownloadManagerApp {
    fn default() -> Self {
        Self {
            url_input: String::from("https://speed.hetzner.de/10GB_Game_Archive.zip"),
            new_priority: Priority::Normal,
            downloads: Vec::new(),
            next_id: 1,
        }
    }
}

impl DownloadManagerApp {
    // Вытаскиваем имя файла из URL
    fn extract_filename(url: &str, id: usize) -> String {
        url.split('/')
            .last()
            .filter(|s| !s.is_empty())
            .unwrap_or(&format!("file_{}.bin", id))
            .to_string()
    }

    fn add_and_start_download(&mut self) {
        let id = self.next_id;
        self.next_id += 1;

        let file_name = Self::extract_filename(&self.url_input, id);

        let new_item = Arc::new(Mutex::new(DownloadItem {
            id,
            file_name,
            url: self.url_input.clone(),
            priority: self.new_priority,
            is_downloading: true,
            is_finished: false,
            progress_core_1: 0.0,
            progress_core_2: 0.0,
            cores_info: CoreSelection::default(),
            status_text: String::from("🔍 Поиск свободных P-ядер..."),
        }));

        // Добавляем в начало списка (чтобы новые были сверху)
        self.downloads.insert(0, Arc::clone(&new_item));

        // Запускаем фоновый менеджер для этой конкретной загрузки
        thread::spawn(move || {
            let selected = cpu_manager::find_best_p_cores();

            {
                let mut item = new_item.lock().unwrap();
                item.cores_info = selected.clone();
                item.status_text = format!(
                    "Качаем 50/50 на P-Ядрах #{} и #{}",
                    selected.core1_id, selected.core2_id
                );
            }

            // Поток 1 (Первые 50% файла на первом P-ядре)
            let item_c1 = Arc::clone(&new_item);
            let c1_id = selected.core1_id;
            let t1 = thread::spawn(move || {
                cpu_manager::pin_current_thread_to_core(c1_id);
                for _ in 0..50 {
                    // Читаем приоритет на каждом шаге (можно менять на лету!)
                    let delay = item_c1.lock().unwrap().priority.delay_ms();
                    thread::sleep(Duration::from_millis(delay));

                    let mut it = item_c1.lock().unwrap();
                    it.progress_core_1 += 0.01;
                }
            });

            // Поток 2 (Вторые 50% файла на втором P-ядре)
            let item_c2 = Arc::clone(&new_item);
            let c2_id = selected.core2_id;
            let t2 = thread::spawn(move || {
                cpu_manager::pin_current_thread_to_core(c2_id);
                for _ in 0..50 {
                    let delay = item_c2.lock().unwrap().priority.delay_ms();
                    thread::sleep(Duration::from_millis(delay + 8)); // Чуть-чуть разная скорость

                    let mut it = item_c2.lock().unwrap();
                    it.progress_core_2 += 0.01;
                }
            });

            t1.join().unwrap();
            t2.join().unwrap();

            let mut item = new_item.lock().unwrap();
            item.is_downloading = false;
            item.is_finished = true;
            item.status_text = String::from("✅ Завершено (100%)");
        });
    }
}

impl eframe::App for DownloadManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            // --- ШАПКА ПРОГРАММЫ ---
            ui.horizontal(|ui| {
                ui.heading("🚀 RustDM");
                ui.colored_label(egui::Color32::from_rgb(255, 165, 0), format!("[{}]", APP_VERSION));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!("Всего загрузок: {}", self.downloads.len()));
                });
            });
            ui.add_space(8.0);

            // --- ПАНЕЛЬ ДОБАВЛЕНИЯ НОВОЙ ЗАГРУЗКИ ---
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("🔗 URL:");
                    ui.add(egui::TextEdit::singleline(&mut self.url_input).desired_width(320.0));

                    // Выбор стартового приоритета
                    egui::ComboBox::from_id_source("new_prio")
                        .selected_text(self.new_priority.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.new_priority, Priority::Low, Priority::Low.label());
                            ui.selectable_value(&mut self.new_priority, Priority::Normal, Priority::Normal.label());
                            ui.selectable_value(&mut self.new_priority, Priority::High, Priority::High.label());
                        });

                    if ui.button("➕ Скачать").clicked() {
                        self.add_and_start_download();
                    }
                });
            });

            ui.add_space(12.0);
            ui.heading("📋 Список загрузок:");
            ui.separator();
            ui.add_space(5.0);

            // --- ПРОКРУЧИВАЕМЫЙ СПИСОК ЗАГРУЗОК (КАК В FDM) ---
            if self.downloads.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.label("Список загрузок пуст. Добавьте первую ссылку сверху!");
                });
            } else {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for dl_arc in &self.downloads {
                        let mut dl = dl_arc.lock().unwrap();
                        let total_percent = (dl.progress_core_1 + dl.progress_core_2).clamp(0.0, 1.0);

                        // Рисуем красивую карточку для каждого файла
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            // Верхняя строка карточки: Имя файла, Статус и Смена Приоритета
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("📄 {}", dl.file_name)).strong().size(15.0));
                                ui.colored_label(
                                    if dl.is_finished { egui::Color32::LIGHT_GREEN } else { egui::Color32::LIGHT_BLUE },
                                    &dl.status_text,
                                );

                                // Правый угол карточки: смена приоритета прямо на лету!
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.add_enabled_ui(!dl.is_finished, |ui| {
                                        egui::ComboBox::from_id_source(format!("prio_{}", dl.id))
                                            .selected_text(dl.priority.label())
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut dl.priority, Priority::Low, Priority::Low.label());
                                                ui.selectable_value(&mut dl.priority, Priority::Normal, Priority::Normal.label());
                                                ui.selectable_value(&mut dl.priority, Priority::High, Priority::High.label());
                                            });
                                        ui.label("Приоритет:");
                                    });
                                });
                            });

                            ui.add_space(6.0);

                            // Главный прогресс-бар файла
                            ui.add(
                                egui::ProgressBar::new(total_percent)
                                    .text(format!("Общий прогресс: {:.0}%", total_percent * 100.0))
                                    .animate(dl.is_downloading),
                            );

                            // Детализация по P-ядрам (50 на 50)
                            ui.add_space(4.0);
                            ui.columns(2, |columns| {
                                columns[0].label(format!(
                                    "⚙️ P-Ядро #{} ({} МГц): {:.0}%",
                                    dl.cores_info.core1_id,
                                    dl.cores_info.core1_freq,
                                    (dl.progress_core_1 * 200.0).clamp(0.0, 100.0)
                                ));
                                columns[0].add(egui::ProgressBar::new(dl.progress_core_1 * 2.0));

                                columns[1].label(format!(
                                    "⚙️ P-Ядро #{} ({} МГц): {:.0}%",
                                    dl.cores_info.core2_id,
                                    dl.cores_info.core2_freq,
                                    (dl.progress_core_2 * 200.0).clamp(0.0, 100.0)
                                ));
                                columns[1].add(egui::ProgressBar::new(dl.progress_core_2 * 2.0));
                            });
                        });

                        ui.add_space(6.0);
                    }
                });
            }
        });

        ctx.request_repaint();
    }
}