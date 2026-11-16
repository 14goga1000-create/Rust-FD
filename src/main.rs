#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cpu_manager;
mod download_engine;
mod fs_watcher;
mod hyper_shield;
mod network_cf;
mod ui_theme;

use eframe::egui;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use download_engine::{
    extract_filename, spawn_smart_download, verify_url_reachability, DownloadItem,
    ModalProbeState, Priority, PROBE_PHRASES,
};
use hyper_shield::GpuScanVerdict;
use network_cf::{CfRouteMode, NetworkSettings};
use ui_theme::*;

const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(145, 155, 180);

#[derive(PartialEq, Clone, Copy)]
enum NavTab {
    All,
    Active,
    Completed,
    Torrents,
}

struct DownloadManagerApp {
    active_tab: NavTab,
    downloads: Vec<DownloadItem>,
    next_download_id: usize,
    global_cpu_load: Arc<Mutex<f32>>,
    net_settings: Arc<Mutex<NetworkSettings>>,
    default_save_dir: PathBuf,
    snail_mode: bool,

    show_settings_window: bool,
    show_add_modal: bool,
    modal_url: String,
    modal_filename: String,
    modal_save_dir: String,
    modal_priority: Priority,
    modal_cf: bool,
    modal_gpu: bool,

    modal_probe_state: Arc<Mutex<ModalProbeState>>,
    modal_probe_ticket: Arc<Mutex<usize>>,
}

impl Default for DownloadManagerApp {
    fn default() -> Self {
        let cpu_monitor = cpu_manager::start_cpu_monitor();
        let net_settings = Arc::new(Mutex::new(NetworkSettings::default()));
        network_cf::run_silent_cf_calibration(Arc::clone(&net_settings));

        let base_dir = std::env::var("USERPROFILE")
            .map(|p| PathBuf::from(p).join("Downloads").join("RustDM"))
            .unwrap_or_else(|_| PathBuf::from("RustDM_Downloads"));
        let _ = fs::create_dir_all(&base_dir);

        Self {
            active_tab: NavTab::All,
            downloads: Vec::new(),
            next_download_id: 1,
            global_cpu_load: cpu_monitor,
            net_settings,
            modal_save_dir: base_dir.display().to_string(),
            default_save_dir: base_dir,
            snail_mode: false,
            show_settings_window: false,
            show_add_modal: false,
            modal_url: String::new(),
            modal_filename: String::new(),
            modal_priority: Priority::Auto,
            modal_cf: true,
            modal_gpu: true,
            modal_probe_state: Arc::new(Mutex::new(ModalProbeState::Idle)),
            modal_probe_ticket: Arc::new(Mutex::new(0)),
        }
    }
}

impl DownloadManagerApp {
    fn open_add_modal(&mut self) {
        self.modal_url.clear();
        self.modal_filename.clear();
        self.modal_save_dir = self.default_save_dir.display().to_string();
        self.modal_priority = Priority::Auto;
        self.modal_cf = true;
        self.modal_gpu = true;
        *self.modal_probe_state.lock().unwrap() = ModalProbeState::Idle;
        *self.modal_probe_ticket.lock().unwrap() += 1;
        self.show_add_modal = true;
    }

    fn trigger_modal_url_probe(&mut self) {
        let url = self.modal_url.trim().to_string();
        let mut ticket_lock = self.modal_probe_ticket.lock().unwrap();
        *ticket_lock += 1;
        let current_ticket = *ticket_lock;
        drop(ticket_lock);

        if url.is_empty() {
            *self.modal_probe_state.lock().unwrap() = ModalProbeState::Idle;
            return;
        }

        let state_arc = Arc::clone(&self.modal_probe_state);
        let ticket_arc = Arc::clone(&self.modal_probe_ticket);

        thread::spawn(move || {
            let url_for_net = url.clone();
            let net_handle = thread::spawn(move || verify_url_reachability(&url_for_net));

            for phrase in PROBE_PHRASES {
                if *ticket_arc.lock().unwrap() != current_ticket {
                    return;
                }
                *state_arc.lock().unwrap() = ModalProbeState::Probing((*phrase).to_string());
                thread::sleep(Duration::from_millis(480));
                if net_handle.is_finished() && *phrase == PROBE_PHRASES[2] {
                    break;
                }
            }

            if *ticket_arc.lock().unwrap() != current_ticket {
                return;
            }

            match net_handle.join().unwrap_or_else(|_| Err("Сбой проверки".to_string())) {
                Ok(info) => {
                    *state_arc.lock().unwrap() = ModalProbeState::Connected(format!(
                        "✔ Соединение установлено! Можно начинать загрузку! ({})",
                        info
                    ));
                }
                Err(err) => {
                    *state_arc.lock().unwrap() = ModalProbeState::Failed(format!(
                        "✖ Не удалось достучаться до файла: {}",
                        err
                    ));
                }
            }
        });
    }

    fn sync_disk_and_snail_mode(&mut self) {
        self.downloads.retain(|item| {
            let finished = *item.is_finished.lock().unwrap();
            fs_watcher::exists_on_disk(&item.full_path, finished)
        });

        let mut primary_active_found = false;
        for item in &self.downloads {
            let p1 = *item.progress_p1.lock().unwrap();
            let p2 = *item.progress_p2.lock().unwrap();
            let paused = *item.is_paused.lock().unwrap();
            let failed = *item.connection_failed.lock().unwrap();
            let is_active = (p1 + p2) < 2.0 && !paused && !failed;

            if !self.snail_mode {
                *item.snail_throttled.lock().unwrap() = false;
            } else if is_active {
                if !primary_active_found {
                    *item.snail_throttled.lock().unwrap() = false;
                    primary_active_found = true;
                } else {
                    *item.snail_throttled.lock().unwrap() = true;
                }
            } else {
                *item.snail_throttled.lock().unwrap() = false;
            }
        }
    }
}

impl eframe::App for DownloadManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        setup_modern_theme(ctx);
        self.sync_disk_and_snail_mode();

        let current_cpu = *self.global_cpu_load.lock().unwrap();

        egui::SidePanel::left("sidebar_panel")
            .resizable(false)
            .exact_width(220.0)
            .frame(
                egui::Frame::none()
                    .fill(BG_SIDEBAR)
                    .inner_margin(egui::Margin::same(16.0))
                    .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(30, 35, 48))),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 7.0, ACCENT_ORANGE);
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "R",
                        egui::FontId::proportional(18.0),
                        egui::Color32::WHITE,
                    );
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("RustDM").size(19.0).strong().color(egui::Color32::WHITE));
                        ui.label(egui::RichText::new("v0.1.0-alpha.4 [MegaPatch]").size(10.5).color(ACCENT_ORANGE));
                    });
                });

                ui.add_space(24.0);
                ui.label(egui::RichText::new("КАТЕГОРИИ").size(11.0).strong().color(TEXT_MUTED));
                ui.add_space(6.0);

                let nav_items = [
                    (NavTab::All, "▶  Все загрузки"),
                    (NavTab::Active, "⚡  Активные"),
                    (NavTab::Completed, "✔  Завершенные"),
                    (NavTab::Torrents, "♦  Торренты (P2P)"),
                ];

                for (tab, label) in nav_items {
                    let selected = self.active_tab == tab;
                    let bg = if selected {
                        egui::Color32::from_rgb(35, 42, 58)
                    } else {
                        egui::Color32::TRANSPARENT
                    };
                    let fg = if selected { egui::Color32::WHITE } else { TEXT_MUTED };

                    let btn = egui::Button::new(egui::RichText::new(label).size(13.5).color(fg))
                        .fill(bg)
                        .rounding(8.0)
                        .min_size(egui::vec2(188.0, 36.0));

                    if ui.add(btn).clicked() {
                        self.active_tab = tab;
                    }
                    ui.add_space(3.0);
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let settings_btn = egui::Button::new(
                        egui::RichText::new("⚙  Настройки").size(13.5).strong().color(egui::Color32::WHITE),
                    )
                    .fill(if self.show_settings_window {
                        ACCENT_PURPLE
                    } else {
                        egui::Color32::from_rgb(30, 35, 48)
                    })
                    .rounding(8.0)
                    .min_size(egui::vec2(188.0, 38.0));

                    if ui.add(settings_btn).clicked() {
                        self.show_settings_window = !self.show_settings_window;
                    }

                    ui.add_space(10.0);

                    egui::Frame::none()
                        .fill(BG_CARD)
                        .rounding(10.0)
                        .inner_margin(egui::Margin::same(12.0))
                        .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(36, 42, 56)))
                        .show(ui, |ui| {
                            let cpu_color = if current_cpu < 35.0 {
                                ACCENT_GREEN
                            } else if current_cpu < 75.0 {
                                ACCENT_ORANGE
                            } else {
                                ACCENT_RED
                            };
                            draw_modern_bar(ui, current_cpu / 100.0, cpu_color, 6.0, None);
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Нагрузка ЦП:").size(11.5).color(TEXT_MUTED));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(
                                        egui::RichText::new(format!("{:.0}%", current_cpu))
                                            .size(12.0)
                                            .strong()
                                            .color(cpu_color),
                                    );
                                });
                            });
                        });
                });
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(BG_MAIN)
                    .inner_margin(egui::Margin::symmetric(22.0, 18.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let add_btn = egui::Button::new(
                        egui::RichText::new("➕  Новая загрузка")
                            .size(13.5)
                            .strong()
                            .color(egui::Color32::WHITE),
                    )
                    .fill(ACCENT_ORANGE)
                    .rounding(8.0)
                    .min_size(egui::vec2(165.0, 38.0));

                    if ui.add(add_btn).clicked() {
                        self.open_add_modal();
                    }

                    ui.add_space(10.0);

                    let snail_bg = if self.snail_mode {
                        ACCENT_CYAN.gamma_multiply(0.25)
                    } else {
                        BG_CARD
                    };
                    let snail_fg = if self.snail_mode { ACCENT_CYAN } else { TEXT_MUTED };
                    let snail_btn = egui::Button::new(
                        egui::RichText::new(if self.snail_mode {
                            "♦ Режим Улитки: ВКЛ"
                        } else {
                            "♦ Режим Улитки"
                        })
                        .size(13.0)
                        .strong()
                        .color(snail_fg),
                    )
                    .fill(snail_bg)
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if self.snail_mode {
                            ACCENT_CYAN
                        } else {
                            egui::Color32::from_rgb(42, 48, 64)
                        },
                    ))
                    .rounding(8.0)
                    .min_size(egui::vec2(155.0, 38.0));

                    if ui
                        .add(snail_btn)
                        .on_hover_text("Отдает всю скорость первой активной загрузке, переводя остальные в тихий режим")
                        .clicked()
                    {
                        self.snail_mode = !self.snail_mode;
                    }
                });

                ui.add_space(16.0);

                if self.downloads.is_empty() {
                    ui.add_space(90.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Список загрузок пуст")
                                .size(18.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                        );
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new("Нажмите «➕ Новая загрузка», чтобы проверить сервер и запустить 50/50 P-Core конвейер")
                                .size(13.0)
                                .color(TEXT_MUTED),
                        );
                    });
                } else {
                    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                        let mut delete_id: Option<usize> = None;

                        for item in &self.downloads {
                            let p1 = *item.progress_p1.lock().unwrap();
                            let p2 = *item.progress_p2.lock().unwrap();
                            let total = (p1 + p2) / 2.0;
                            let is_done = total >= 1.0;
                            let is_torrent = item.filename.ends_with(".torrent") || item.url.starts_with("magnet:");

                            match self.active_tab {
                                NavTab::All => {}
                                NavTab::Active if is_done => continue,
                                NavTab::Completed if !is_done => continue,
                                NavTab::Torrents if !is_torrent => continue,
                                _ => {}
                            }

                            let status_str = item.status.lock().unwrap().clone();
                            let overlay_opt = item.bar_overlay_text.lock().unwrap().clone();
                            let conn_failed = *item.connection_failed.lock().unwrap();
                            let cores_str = item.assigned_cores.lock().unwrap().clone();
                            let is_paused = *item.is_paused.lock().unwrap();
                            let is_snail_throttled = *item.snail_throttled.lock().unwrap();
                            let verdict = item.gpu_verdict.lock().unwrap().clone();

                            egui::Frame::none()
                                .fill(BG_CARD)
                                .rounding(12.0)
                                .inner_margin(egui::Margin::same(16.0))
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    match verdict {
                                        GpuScanVerdict::Threat { .. } => ACCENT_RED,
                                        _ if conn_failed => ACCENT_RED,
                                        _ if is_done => ACCENT_GREEN.gamma_multiply(0.35),
                                        _ if is_paused => ACCENT_ORANGE.gamma_multiply(0.4),
                                        _ => egui::Color32::from_rgb(34, 40, 56),
                                    },
                                ))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        draw_file_avatar(ui, &item.filename, 44.0);
                                        ui.add_space(10.0);

                                        ui.vertical(|ui| {
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new(&item.filename)
                                                        .size(15.0)
                                                        .strong()
                                                        .color(egui::Color32::WHITE),
                                                );

                                                if let Some(badge_text) = verdict.badge_text() {
                                                    let badge_color = match verdict {
                                                        GpuScanVerdict::Scanning => ACCENT_CYAN,
                                                        GpuScanVerdict::Clean { .. } => ACCENT_GREEN,
                                                        GpuScanVerdict::Threat { .. } => ACCENT_RED,
                                                        _ => TEXT_MUTED,
                                                    };
                                                    draw_badge(
                                                        ui,
                                                        &badge_text,
                                                        badge_color.gamma_multiply(0.22),
                                                        badge_color,
                                                    );
                                                }

                                                if item.cf_shield {
                                                    draw_badge(
                                                        ui,
                                                        "CF WARP",
                                                        ACCENT_ORANGE.gamma_multiply(0.22),
                                                        ACCENT_ORANGE,
                                                    );
                                                }
                                                if is_snail_throttled && !is_done {
                                                    draw_badge(
                                                        ui,
                                                        "♦ УЛИТКА",
                                                        ACCENT_CYAN.gamma_multiply(0.22),
                                                        ACCENT_CYAN,
                                                    );
                                                }
                                            });

                                            ui.add_space(3.0);
                                            ui.label(
                                                egui::RichText::new(format!("⚡ {}   •   {}", cores_str, status_str))
                                                    .size(11.5)
                                                    .color(if conn_failed { ACCENT_RED } else { TEXT_MUTED }),
                                            );
                                        });

                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            let del_btn = egui::Button::new(
                                                egui::RichText::new("✖").size(12.5).strong().color(ACCENT_RED),
                                            )
                                            .fill(egui::Color32::from_rgb(38, 26, 34))
                                            .rounding(6.0);

                                            if ui
                                                .add(del_btn)
                                                .on_hover_text("Удалить из списка (и очистить недокачанный кэш)")
                                                .clicked()
                                            {
                                                delete_id = Some(item.id);
                                            }

                                            ui.add_space(4.0);

                                            let folder_btn = egui::Button::new(
                                                egui::RichText::new("📁 Папка").size(11.5).color(egui::Color32::WHITE),
                                            )
                                            .fill(egui::Color32::from_rgb(35, 42, 58))
                                            .rounding(6.0);

                                            if ui
                                                .add(folder_btn)
                                                .on_hover_text("Показать файл в Проводнике Windows")
                                                .clicked()
                                            {
                                                let dir_str = item.save_dir.display().to_string();
                                                fs_watcher::open_in_windows_explorer(&dir_str, &item.full_path);
                                            }

                                            ui.add_space(4.0);

                                            if !is_done && !conn_failed {
                                                let pause_label = if is_paused { "▶ Пуск" } else { "⏸ Стоп" };
                                                let pause_color = if is_paused { ACCENT_GREEN } else { ACCENT_ORANGE };
                                                let pause_btn = egui::Button::new(
                                                    egui::RichText::new(pause_label).size(11.5).strong().color(pause_color),
                                                )
                                                .fill(egui::Color32::from_rgb(32, 38, 52))
                                                .rounding(6.0);

                                                if ui.add(pause_btn).clicked() {
                                                    let mut p_lock = item.is_paused.lock().unwrap();
                                                    *p_lock = !*p_lock;
                                                }

                                                ui.add_space(4.0);
                                            }

                                            let mut prio = item.priority.lock().unwrap();
                                            egui::ComboBox::from_id_source(format!("prio_{}", item.id))
                                                .selected_text(egui::RichText::new(prio.label()).size(11.5))
                                                .width(130.0)
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut *prio, Priority::Auto, Priority::Auto.label());
                                                    ui.selectable_value(&mut *prio, Priority::Low, Priority::Low.label());
                                                    ui.selectable_value(&mut *prio, Priority::Normal, Priority::Normal.label());
                                                    ui.selectable_value(&mut *prio, Priority::High, Priority::High.label());
                                                });
                                        });
                                    });

                                    ui.add_space(12.0);

                                    let (bar_progress, main_bar_color, bar_text) = if let Some(msg) = overlay_opt {
                                        let col = if conn_failed {
                                            ACCENT_RED
                                        } else if msg.starts_with('✔') {
                                            ACCENT_GREEN
                                        } else {
                                            ACCENT_CYAN
                                        };
                                        let prog = if msg.starts_with('✔') || conn_failed { 1.0 } else { 0.35 };
                                        (prog, col, Some(msg))
                                    } else {
                                        let col = match verdict {
                                            GpuScanVerdict::Threat { .. } => ACCENT_RED,
                                            _ if is_done => ACCENT_GREEN,
                                            _ if is_paused => TEXT_MUTED,
                                            _ => ACCENT_ORANGE,
                                        };
                                        (total, col, Some(format!("{:.0}%", total * 100.0)))
                                    };

                                    draw_modern_bar(ui, bar_progress, main_bar_color, 18.0, bar_text);

                                    ui.add_space(8.0);

                                    ui.columns(2, |cols| {
                                        cols[0].horizontal(|ui| {
                                            ui.label(egui::RichText::new("Поток 1 [0..50%]:").size(11.0).color(TEXT_MUTED));
                                            ui.label(
                                                egui::RichText::new(format!("{:.0}%", p1 * 100.0))
                                                    .size(11.0)
                                                    .strong()
                                                    .color(ACCENT_CYAN),
                                            );
                                        });
                                        cols[0].add_space(2.0);
                                        draw_modern_bar(&mut cols[0], p1, ACCENT_CYAN, 4.0, None);

                                        cols[1].horizontal(|ui| {
                                            ui.label(egui::RichText::new("Поток 2 [50..100%]:").size(11.0).color(TEXT_MUTED));
                                            ui.label(
                                                egui::RichText::new(format!("{:.0}%", p2 * 100.0))
                                                    .size(11.0)
                                                    .strong()
                                                    .color(ACCENT_PURPLE),
                                            );
                                        });
                                        cols[1].add_space(2.0);
                                        draw_modern_bar(&mut cols[1], p2, ACCENT_PURPLE, 4.0, None);
                                    });
                                });

                            ui.add_space(10.0);
                        }

                        if let Some(target_id) = delete_id {
                            if let Some(pos) = self.downloads.iter().position(|d| d.id == target_id) {
                                let removed = self.downloads.remove(pos);
                                *removed.is_cancelled.lock().unwrap() = true;
                                fs_watcher::purge_partial_files(&removed.full_path);
                            }
                        }
                    });
                }
            });

        if self.show_add_modal {
            let mut open = self.show_add_modal;
            let probe_snapshot = self.modal_probe_state.lock().unwrap().clone();

            egui::Window::new("➕ Новая загрузка")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .fixed_size(egui::vec2(530.0, 390.0))
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(BG_SIDEBAR)
                        .rounding(14.0)
                        .stroke(egui::Stroke::new(1.5_f32, ACCENT_ORANGE))
                        .inner_margin(egui::Margin::same(20.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        draw_file_avatar(ui, &self.modal_filename, 46.0);
                        ui.add_space(10.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(if self.modal_filename.is_empty() {
                                    "Введите ссылку на файл"
                                } else {
                                    &self.modal_filename
                                })
                                .size(15.0)
                                .strong()
                                .color(egui::Color32::WHITE),
                            );
                            ui.label(
                                egui::RichText::new("Готов к разделению 50/50 по двум лучшим P-ядрам")
                                    .size(11.5)
                                    .color(ACCENT_GREEN),
                            );
                        });
                    });

                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(8.0);

                    ui.label(egui::RichText::new("Ссылка (HTTP / HTTPS / Magnet):").size(12.0).color(TEXT_MUTED));
                    let url_resp = ui.add(
                        egui::TextEdit::singleline(&mut self.modal_url)
                            .desired_width(f32::INFINITY)
                            .hint_text("Вставьте ссылку (например, https://github.com/archive.zip)"),
                    );
                    if url_resp.changed() {
                        if self.modal_url.trim().is_empty() {
                            self.modal_filename.clear();
                        } else {
                            self.modal_filename = extract_filename(&self.modal_url, self.next_download_id);
                        }
                        self.trigger_modal_url_probe();
                    }

                    ui.add_space(8.0);

                    let (probe_text, probe_col) = match &probe_snapshot {
                        ModalProbeState::Idle => (
                            "• Ожидание ссылки для проверки доступности файла...".to_string(),
                            TEXT_MUTED,
                        ),
                        ModalProbeState::Probing(msg) => (msg.clone(), ACCENT_CYAN),
                        ModalProbeState::Connected(msg) => (msg.clone(), ACCENT_GREEN),
                        ModalProbeState::Failed(msg) => (msg.clone(), ACCENT_RED),
                    };

                    egui::Frame::none()
                        .fill(BG_CARD)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
                        .stroke(egui::Stroke::new(1.0_f32, probe_col.gamma_multiply(0.5)))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(probe_text).size(12.0).strong().color(probe_col));
                        });

                    ui.add_space(10.0);

                    ui.columns(2, |cols| {
                        cols[0].label(egui::RichText::new("Имя файла на диске:").size(12.0).color(TEXT_MUTED));
                        cols[0].add(
                            egui::TextEdit::singleline(&mut self.modal_filename)
                                .desired_width(f32::INFINITY)
                                .hint_text("archive.zip"),
                        );

                        cols[1].label(egui::RichText::new("Приоритет потоков:").size(12.0).color(TEXT_MUTED));
                        egui::ComboBox::from_id_source("modal_prio_box")
                            .selected_text(self.modal_priority.label())
                            .width(220.0)
                            .show_ui(&mut cols[1], |ui| {
                                ui.selectable_value(&mut self.modal_priority, Priority::Auto, Priority::Auto.label());
                                ui.selectable_value(&mut self.modal_priority, Priority::Low, Priority::Low.label());
                                ui.selectable_value(&mut self.modal_priority, Priority::Normal, Priority::Normal.label());
                                ui.selectable_value(&mut self.modal_priority, Priority::High, Priority::High.label());
                            });
                    });

                    ui.add_space(10.0);

                    ui.label(egui::RichText::new("Папка сохранения:").size(12.0).color(TEXT_MUTED));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.modal_save_dir).desired_width(380.0));
                        let browse_btn = egui::Button::new(
                            egui::RichText::new("📁 Обзор...").size(12.5).strong().color(egui::Color32::WHITE),
                        )
                        .fill(egui::Color32::from_rgb(38, 45, 62))
                        .rounding(6.0)
                        .min_size(egui::vec2(95.0, 28.0));

                        if ui.add(browse_btn).clicked() {
                            if let Some(chosen) = fs_watcher::pick_folder_via_explorer(&self.modal_save_dir) {
                                self.modal_save_dir = chosen;
                            }
                        }
                    });

                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.modal_cf, "♦ Маршрутизация Cloudflare");
                        ui.add_space(14.0);
                        ui.checkbox(&mut self.modal_gpu, "⚡ GPU-Антивирус + Хеш 50/50");
                    });

                    ui.add_space(14.0);

                    let is_failed = matches!(probe_snapshot, ModalProbeState::Failed(_));
                    let can_start = !self.modal_url.trim().is_empty() && !is_failed;

                    ui.horizontal(|ui| {
                        let start_btn = egui::Button::new(
                            egui::RichText::new(if is_failed {
                                "✖ Сервер недоступен"
                            } else {
                                "▶ Запустить загрузку (50/50 P-Cores)"
                            })
                            .size(13.5)
                            .strong()
                            .color(egui::Color32::WHITE),
                        )
                        .fill(if can_start {
                            ACCENT_GREEN.gamma_multiply(0.85)
                        } else {
                            egui::Color32::from_rgb(60, 40, 48)
                        })
                        .rounding(8.0)
                        .min_size(egui::vec2(340.0, 38.0));

                        if ui.add_enabled(can_start, start_btn).clicked() {
                            let new_id = self.next_download_id;
                            self.next_download_id += 1;

                            let final_name = if self.modal_filename.trim().is_empty() {
                                extract_filename(&self.modal_url, new_id)
                            } else {
                                self.modal_filename.trim().to_string()
                            };

                            let folder_path = PathBuf::from(self.modal_save_dir.trim());
                            let full_file_path = folder_path.join(&final_name);

                            let cf_label = {
                                let net = self.net_settings.lock().unwrap();
                                match net.selected_mode {
                                    CfRouteMode::Auto => net.best_auto_node.label().to_string(),
                                    other => other.label().to_string(),
                                }
                            };

                            let initial_verdict = if self.modal_gpu {
                                GpuScanVerdict::Waiting
                            } else {
                                GpuScanVerdict::Disabled
                            };

                            let already_connected = matches!(probe_snapshot, ModalProbeState::Connected(_));

                            let new_item = DownloadItem {
                                id: new_id,
                                url: self.modal_url.trim().to_string(),
                                filename: final_name,
                                save_dir: folder_path,
                                full_path: full_file_path,
                                progress_p1: Arc::new(Mutex::new(0.0)),
                                progress_p2: Arc::new(Mutex::new(0.0)),
                                status: Arc::new(Mutex::new("Подключение к серверу...".to_string())),
                                bar_overlay_text: Arc::new(Mutex::new(Some(
                                    "⚡ Пробуем достучаться до файла...".to_string(),
                                ))),
                                connection_failed: Arc::new(Mutex::new(false)),
                                assigned_cores: Arc::new(Mutex::new("Калибровка...".to_string())),
                                priority: Arc::new(Mutex::new(self.modal_priority)),
                                is_paused: Arc::new(Mutex::new(false)),
                                is_cancelled: Arc::new(Mutex::new(false)),
                                snail_throttled: Arc::new(Mutex::new(false)),
                                is_finished: Arc::new(Mutex::new(false)),
                                cf_shield: self.modal_cf,
                                gpu_verify: self.modal_gpu,
                                gpu_verdict: Arc::new(Mutex::new(initial_verdict)),
                            };

                            spawn_smart_download(
                                &new_item,
                                Arc::clone(&self.global_cpu_load),
                                cf_label,
                                already_connected,
                            );
                            self.downloads.insert(0, new_item);
                            self.show_add_modal = false;
                        }

                        if ui
                            .add(
                                egui::Button::new("Отмена")
                                    .fill(egui::Color32::from_rgb(40, 45, 60))
                                    .rounding(8.0)
                                    .min_size(egui::vec2(120.0, 38.0)),
                            )
                            .clicked()
                        {
                            self.show_add_modal = false;
                        }
                    });
                });

            if !open {
                self.show_add_modal = false;
            }
        }

        if self.show_settings_window {
            let mut open = self.show_settings_window;
            egui::Window::new("⚙ Настройки RustDM")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(450.0)
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(BG_SIDEBAR)
                        .rounding(12.0)
                        .stroke(egui::Stroke::new(1.0_f32, ACCENT_PURPLE))
                        .inner_margin(egui::Margin::same(18.0)),
                )
                .show(ctx, |ui| {
                    ui.label(
                        egui::RichText::new("Файловая система")
                            .size(14.5)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Папка по умолчанию для новых загрузок:").size(12.0).color(TEXT_MUTED));
                    ui.horizontal(|ui| {
                        let mut dir_str = self.default_save_dir.display().to_string();
                        ui.add(egui::TextEdit::singleline(&mut dir_str).desired_width(310.0));
                        if ui.button("📁 Выбрать...").clicked() {
                            if let Some(picked) = fs_watcher::pick_folder_via_explorer(&dir_str) {
                                self.default_save_dir = PathBuf::from(picked);
                            }
                        }
                    });

                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(10.0);

                    ui.label(
                        egui::RichText::new("Маршрутизация Cloudflare Edge")
                            .size(14.5)
                            .strong()
                            .color(egui::Color32::WHITE),
                    );
                    ui.add_space(8.0);

                    let mut net = self.net_settings.lock().unwrap();

                    egui::Frame::none()
                        .fill(BG_CARD)
                        .rounding(8.0)
                        .inner_margin(egui::Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Статус туннеля:").color(TEXT_MUTED));
                                let col = if !net.is_calibrating { ACCENT_GREEN } else { ACCENT_ORANGE };
                                let badge_txt = if net.is_calibrating {
                                    "Калибровка узлов..."
                                } else {
                                    "Активен (WARP Ready)"
                                };
                                ui.label(egui::RichText::new(badge_txt).strong().color(col));
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Оптимальный узел:").color(TEXT_MUTED));
                                ui.label(egui::RichText::new(net.best_auto_node.label()).strong().color(ACCENT_CYAN));
                            });
                        });

                    ui.add_space(12.0);
                    ui.label(egui::RichText::new("Режим маршрутизации:").size(12.5).color(TEXT_MUTED));
                    ui.add_space(6.0);

                    let modes = [
                        CfRouteMode::Auto,
                        CfRouteMode::Frankfurt,
                        CfRouteMode::Amsterdam,
                        CfRouteMode::Warsaw,
                        CfRouteMode::Helsinki,
                        CfRouteMode::Direct,
                    ];

                    for mode in modes {
                        ui.radio_value(&mut net.selected_mode, mode, mode.label());
                        ui.add_space(2.0);
                    }

                    ui.add_space(12.0);
                    if ui.button("⚡ Перезапустить калибровку").clicked() {
                        drop(net);
                        network_cf::run_silent_cf_calibration(Arc::clone(&self.net_settings));
                    }
                });
            self.show_settings_window = open;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(80));
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1080.0, 620.0])
            .with_min_inner_size([900.0, 520.0])
            .with_title("RustDM — Smart P-Core & HyperShield Downloader"),
        ..Default::default()
    };

    eframe::run_native(
        "RustDM",
        options,
        Box::new(|_cc| Box::new(DownloadManagerApp::default())),
    )
}