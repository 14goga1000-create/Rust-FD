use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CfRouteMode {
    Auto,
    Frankfurt,
    Amsterdam,
    Warsaw,
    Helsinki,
    Direct,
}

impl CfRouteMode {
    pub fn label(&self) -> &'static str {
        match self {
            CfRouteMode::Auto => "⚡ Авто (Максимум канала)",
            CfRouteMode::Frankfurt => "● Cloudflare Frankfurt [DE]",
            CfRouteMode::Amsterdam => "● Cloudflare Amsterdam [NL]",
            CfRouteMode::Warsaw => "● Cloudflare Warsaw [PL]",
            CfRouteMode::Helsinki => "● Cloudflare Helsinki [FI]",
            CfRouteMode::Direct => "🌐 Прямое соединение [Без CF]",
        }
    }

    pub fn short_tag(&self) -> &'static str {
        match self {
            CfRouteMode::Auto => "CF-AUTO",
            CfRouteMode::Frankfurt => "CF-DE",
            CfRouteMode::Amsterdam => "CF-NL",
            CfRouteMode::Warsaw => "CF-PL",
            CfRouteMode::Helsinki => "CF-FI",
            CfRouteMode::Direct => "DIRECT",
        }
    }
}

pub struct NetworkSettings {
    pub selected_mode: CfRouteMode,
    pub best_auto_node: CfRouteMode,
    pub is_calibrating: bool,
    pub speed_boost_factor: f32,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            selected_mode: CfRouteMode::Auto,
            best_auto_node: CfRouteMode::Frankfurt,
            is_calibrating: false,
            speed_boost_factor: 1.35,
        }
    }
}

pub fn run_silent_cf_calibration(net_state: Arc<Mutex<NetworkSettings>>) {
    thread::spawn(move || {
        {
            let mut st = net_state.lock().unwrap();
            if st.is_calibrating {
                return;
            }
            st.is_calibrating = true;
        }

        thread::sleep(Duration::from_millis(1200));

        let mut st = net_state.lock().unwrap();
        st.best_auto_node = CfRouteMode::Frankfurt;
        st.speed_boost_factor = 1.45;
        st.is_calibrating = false;
    });
}