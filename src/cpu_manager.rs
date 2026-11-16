use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use sysinfo::System;

#[derive(Clone, Default)]
pub struct CoreSelection {
    pub core1_id: usize,
    pub core1_freq: u64,
    pub core1_load: f32,
    pub core2_id: usize,
    pub core2_freq: u64,
    pub core2_load: f32,
}

/// Запускает фоновый мониторинг общей нагрузки процессора для Авто-Приоритета
pub fn start_cpu_monitor() -> Arc<Mutex<f32>> {
    let cpu_load = Arc::new(Mutex::new(0.0_f32));
    let load_clone = Arc::clone(&cpu_load);

    thread::spawn(move || {
        let mut sys = System::new_all();
        loop {
            sys.refresh_cpu();
            thread::sleep(Duration::from_millis(1500));
            sys.refresh_cpu();

            let global_load = sys.global_cpu_info().cpu_usage();
            if let Ok(mut load) = load_clone.lock() {
                *load = global_load;
            }
        }
    });

    cpu_load
}

/// Ищет два самых быстрых (P-ядра) и наименее загруженных ядра в системе
pub fn find_best_p_cores() -> CoreSelection {
    let mut sys = System::new_all();
    sys.refresh_cpu();
    thread::sleep(Duration::from_millis(150));
    sys.refresh_cpu();

    let cpus = sys.cpus();

    let mut core_stats: Vec<(usize, u64, f32)> = cpus
        .iter()
        .enumerate()
        .map(|(i, cpu)| (i, cpu.frequency(), cpu.cpu_usage()))
        .collect();

    core_stats.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
    });

    let c1 = core_stats.get(0).copied().unwrap_or((0, 0, 0.0));
    let c2 = core_stats.get(1).copied().unwrap_or((1, 0, 0.0));

    CoreSelection {
        core1_id: c1.0,
        core1_freq: c1.1,
        core1_load: c1.2,
        core2_id: c2.0,
        core2_freq: c2.1,
        core2_load: c2.2,
    }
}

/// Жестко привязывает текущий рабочий поток к конкретному ядру процессора
pub fn pin_current_thread_to_core(core_id: usize) {
    if let Some(core_ids) = core_affinity::get_core_ids() {
        if let Some(target_core) = core_ids.get(core_id) {
            core_affinity::set_for_current(*target_core);
        }
    }
}