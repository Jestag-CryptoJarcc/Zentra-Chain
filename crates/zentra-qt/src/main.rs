#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::process::{Command, Child};
use std::net::TcpStream;
use serde_json::json;

// ─── CPU info (read once at startup) ─────────────────────────────────────────

struct CpuInfo {
    brand: String,
    physical_cores: usize,
    logical_cores: usize,
    freq_ghz: f64,
}

impl CpuInfo {
    fn detect() -> Self {
        use sysinfo::System;
        let mut sys = System::new();
        sys.refresh_cpu_all();
        let cpus = sys.cpus();
        let brand = cpus.first()
            .map(|c| c.brand().trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Unknown CPU".to_string());
        let logical_cores = cpus.len().max(1);
        let physical_cores = sys.physical_core_count().unwrap_or(logical_cores / 2).max(1);
        let freq_mhz = cpus.iter().map(|c| c.frequency()).sum::<u64>() as f64
            / logical_cores as f64;
        CpuInfo { brand, physical_cores, logical_cores, freq_ghz: freq_mhz / 1000.0 }
    }
}

// ─── Theme system ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeTab { Presets, Customize }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Harmony { Complementary, Triadic, Analogous, SplitComplementary, Tetradic }

impl Harmony {
    fn label(self) -> &'static str {
        match self {
            Harmony::Complementary => "Complementary",
            Harmony::Triadic => "Triadic",
            Harmony::Analogous => "Analogous",
            Harmony::SplitComplementary => "Split-Complementary",
            Harmony::Tetradic => "Tetradic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThemeDensity { Comfortable, Compact, Cozy }

impl ThemeDensity {
    fn label(self) -> &'static str {
        match self { Self::Comfortable => "Comfortable", Self::Compact => "Compact", Self::Cozy => "Cozy" }
    }
    fn item_spacing(self) -> egui::Vec2 {
        match self { Self::Compact => egui::vec2(6.0, 3.0), Self::Cozy => egui::vec2(10.0, 7.0), _ => egui::vec2(8.0, 4.0) }
    }
    fn btn_padding(self) -> egui::Vec2 {
        match self { Self::Compact => egui::vec2(7.0, 4.0), Self::Cozy => egui::vec2(13.0, 8.0), _ => egui::vec2(10.0, 5.0) }
    }
}

#[derive(Debug, Clone)]
struct AppTheme {
    name:    String,
    bg:      [u8; 3],
    toolbar: [u8; 3],
    surface: [u8; 3],
    surface2:[u8; 3],
    border:  [u8; 3],
    text:    [u8; 3],
    muted:   [u8; 3],
    accent:  [u8; 3],
    green:   [u8; 3],
    density: ThemeDensity,
}

impl AppTheme {
    fn col(&self, c: [u8;3]) -> egui::Color32 { egui::Color32::from_rgb(c[0],c[1],c[2]) }
    fn col_a(&self, c: [u8;3], a: u8) -> egui::Color32 { egui::Color32::from_rgba_unmultiplied(c[0],c[1],c[2],a) }

    fn load() -> Self {
        let path = data_dir().join("wallet_theme.json");
        if let Ok(s) = std::fs::read_to_string(&path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                fn arr(v: &serde_json::Value, k: &str) -> Option<[u8;3]> {
                    let a = v.get(k)?.as_array()?;
                    Some([a.get(0)?.as_u64()? as u8, a.get(1)?.as_u64()? as u8, a.get(2)?.as_u64()? as u8])
                }
                let density = match v.get("density").and_then(|d| d.as_str()) {
                    Some("Compact") => ThemeDensity::Compact,
                    Some("Cozy")    => ThemeDensity::Cozy,
                    _               => ThemeDensity::Comfortable,
                };
                if let (Some(bg), Some(toolbar), Some(surface), Some(surface2),
                        Some(border), Some(text), Some(muted), Some(accent), Some(green)) =
                    (arr(&v,"bg"), arr(&v,"toolbar"), arr(&v,"surface"), arr(&v,"surface2"),
                     arr(&v,"border"), arr(&v,"text"), arr(&v,"muted"), arr(&v,"accent"), arr(&v,"green"))
                {
                    return AppTheme {
                        name: v.get("name").and_then(|n| n.as_str()).unwrap_or("Custom").to_string(),
                        bg, toolbar, surface, surface2, border, text, muted, accent, green, density,
                    };
                }
            }
        }
        AppTheme::preset_zentra()
    }

    fn save(&self) {
        let path = data_dir().join("wallet_theme.json");
        let v = serde_json::json!({
            "name": self.name,
            "bg": self.bg, "toolbar": self.toolbar,
            "surface": self.surface, "surface2": self.surface2,
            "border": self.border, "text": self.text,
            "muted": self.muted, "accent": self.accent, "green": self.green,
            "density": self.density.label(),
        });
        let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
    }

    fn to_json(&self) -> String {
        let v = serde_json::json!({
            "name": self.name, "bg": self.bg, "toolbar": self.toolbar,
            "surface": self.surface, "surface2": self.surface2,
            "border": self.border, "text": self.text,
            "muted": self.muted, "accent": self.accent, "green": self.green,
            "density": self.density.label(),
        });
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }

    // ── Presets ───────────────────────────────────────────────────────────────
    fn preset_zentra() -> Self {
        Self { name:"Zentra".into(), bg:[22,22,22], toolbar:[32,32,32], surface:[44,44,44],
               surface2:[55,55,55], border:[70,70,70], text:[228,228,228], muted:[148,148,148],
               accent:[99,102,241], green:[80,195,138], density:ThemeDensity::Comfortable }
    }
    fn preset_midnight() -> Self {
        Self { name:"Midnight".into(), bg:[5,5,10], toolbar:[10,10,20], surface:[16,16,30],
               surface2:[22,22,40], border:[35,35,65], text:[220,225,255], muted:[100,105,155],
               accent:[88,148,212], green:[52,211,153], density:ThemeDensity::Comfortable }
    }
    fn preset_ocean() -> Self {
        Self { name:"Ocean".into(), bg:[5,12,22], toolbar:[8,18,34], surface:[12,26,48],
               surface2:[16,34,62], border:[25,55,90], text:[200,230,255], muted:[80,140,190],
               accent:[34,211,238], green:[52,211,153], density:ThemeDensity::Comfortable }
    }
    fn preset_forest() -> Self {
        Self { name:"Forest".into(), bg:[8,14,8], toolbar:[12,20,12], surface:[18,30,18],
               surface2:[25,40,25], border:[40,65,40], text:[210,240,210], muted:[100,155,100],
               accent:[74,222,128], green:[74,222,128], density:ThemeDensity::Comfortable }
    }
    fn preset_cyberpunk() -> Self {
        Self { name:"Cyberpunk".into(), bg:[5,5,8], toolbar:[10,10,16], surface:[16,16,25],
               surface2:[22,22,35], border:[0,255,200], text:[240,240,255], muted:[120,120,180],
               accent:[0,255,200], green:[0,220,150], density:ThemeDensity::Compact }
    }
    fn preset_copper() -> Self {
        Self { name:"Copper".into(), bg:[14,9,5], toolbar:[22,14,8], surface:[32,20,12],
               surface2:[42,28,16], border:[90,55,25], text:[255,230,200], muted:[160,120,80],
               accent:[200,130,60], green:[80,195,138], density:ThemeDensity::Comfortable }
    }
    fn preset_lavender() -> Self {
        Self { name:"Lavender".into(), bg:[12,10,18], toolbar:[18,15,28], surface:[26,22,40],
               surface2:[34,30,52], border:[65,55,100], text:[230,225,255], muted:[140,130,185],
               accent:[167,139,250], green:[134,239,172], density:ThemeDensity::Comfortable }
    }
    fn preset_light() -> Self {
        Self { name:"Light".into(), bg:[245,246,250], toolbar:[235,237,245], surface:[255,255,255],
               surface2:[228,230,240], border:[200,205,220], text:[25,25,35], muted:[100,105,130],
               accent:[99,102,241], green:[22,163,74], density:ThemeDensity::Comfortable }
    }

    fn all_presets() -> Vec<AppTheme> {
        vec![Self::preset_zentra(), Self::preset_midnight(), Self::preset_ocean(),
             Self::preset_forest(), Self::preset_cyberpunk(), Self::preset_copper(),
             Self::preset_lavender(), Self::preset_light()]
    }
}

// ─── Color harmony ────────────────────────────────────────────────────────────

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r,g,b) = (r as f32/255.0, g as f32/255.0, b as f32/255.0);
    let max = r.max(g).max(b); let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 { return (0.0, 0.0, l); }
    let d = max - min;
    let s = if l > 0.5 { d/(2.0-max-min) } else { d/(max+min) };
    let h = if (max-r).abs() < 1e-6 { (g-b)/d + if g<b{6.0}else{0.0} }
            else if (max-g).abs() < 1e-6 { (b-r)/d + 2.0 }
            else { (r-g)/d + 4.0 };
    (h/6.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8;3] {
    fn h2r(p:f32,q:f32,mut t:f32)->f32{if t<0.0{t+=1.0}if t>1.0{t-=1.0}if t<1.0/6.0{return p+(q-p)*6.0*t}if t<0.5{return q}if t<2.0/3.0{return p+(q-p)*(2.0/3.0-t)*6.0}p}
    if s < 1e-6 { let v = (l*255.0) as u8; return [v,v,v]; }
    let q = if l<0.5{l*(1.0+s)}else{l+s-l*s}; let p = 2.0*l-q;
    [(h2r(p,q,h+1.0/3.0)*255.0) as u8, (h2r(p,q,h)*255.0) as u8, (h2r(p,q,h-1.0/3.0)*255.0) as u8]
}

fn generate_theme_from_harmony(accent: [u8;3], harmony: Harmony, dark_mode: bool) -> AppTheme {
    let (h, s, _l) = rgb_to_hsl(accent[0], accent[1], accent[2]);
    let bg_l   = if dark_mode { 0.07 } else { 0.95 };
    let sat    = (s * 0.5).max(0.05);
    let bg      = hsl_to_rgb(h, sat,        bg_l);
    let toolbar = hsl_to_rgb(h, sat,        if dark_mode { 0.10 } else { 0.90 });
    let surface = hsl_to_rgb(h, sat,        if dark_mode { 0.15 } else { 0.86 });
    let surface2= hsl_to_rgb(h, sat,        if dark_mode { 0.19 } else { 0.82 });
    let border  = hsl_to_rgb(h, sat*0.8,    if dark_mode { 0.24 } else { 0.72 });
    let text    = hsl_to_rgb(h, 0.08,       if dark_mode { 0.90 } else { 0.10 });
    let muted   = hsl_to_rgb(h, 0.08,       0.55);
    let acc_s   = s.max(0.6);
    let acc_l   = if dark_mode { 0.60 } else { 0.45 };
    // harmony partner hue
    let h2 = match harmony {
        Harmony::Complementary       => (h + 0.5)  % 1.0,
        Harmony::Triadic             => (h + 1.0/3.0) % 1.0,
        Harmony::Analogous           => (h + 1.0/12.0) % 1.0,
        Harmony::SplitComplementary  => (h + 5.0/12.0) % 1.0,
        Harmony::Tetradic            => (h + 0.25) % 1.0,
    };
    let green = hsl_to_rgb(h2, acc_s, acc_l);
    AppTheme {
        name: "Generated".into(), bg, toolbar, surface, surface2, border, text, muted,
        accent: hsl_to_rgb(h, acc_s, acc_l), green,
        density: ThemeDensity::Comfortable,
    }
}

// ─── Tab enum ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Send,
    Receive,
    Transactions,
    Mining,
    Console,
    Network,
    Theme,
}

// ─── Polled node state ────────────────────────────────────────────────────────

struct NodeState {
    connected: bool,
    /// Consecutive failed polls — we only show "Not connected" after a few in a
    /// row, so a single transient RPC hiccup (e.g. the node briefly busy mining)
    /// doesn't flicker the status while we're clearly synced.
    poll_failures: u32,
    blue_score: u64,
    tips_count: usize,
    selected_tip: String,
    ztr_balance: f64,
    recent_blocks: Vec<serde_json::Value>,
    is_mining: bool,
    mining_lane: u8,
    mining_address: String,
    mining_hashrate: f64,
    mining_hashes: u64,
    mined_blocks: u64,
    mining_difficulty: f64,
    target_block_time_ms: u64,
    amm_reserve_ztr: f64,
    amm_reserve_zusd: f64,
    amm_lp_burned: f64,
    mempool_size: usize,
    peers: Vec<serde_json::Value>,
    network_hashrate: f64,
    network_name: String,
    protocol_version: u64,
    mining_threads_daemon: u32,
    max_mining_threads: u32,
    block_reward: f64,
    blocks_until_halving: u64,
    days_until_halving: f64,
    wallet_address: String,
    pending_txs: Vec<serde_json::Value>,
    // Pool
    pool_mode: bool,
    pool_address: String,
    pool_pending_zents: u64,
    pool_active_miners: usize,
    pool_ms_until_payout: u64,
    pool_my_paid_zents: u64,
    pool_my_share_pct: f64,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            connected: false,
            poll_failures: 0,
            blue_score: 0,
            tips_count: 0,
            selected_tip: "—".into(),
            ztr_balance: 0.0,
            recent_blocks: vec![],
            is_mining: false,
            mining_lane: 0,
            mining_address: String::new(),
            mining_hashrate: 0.0,
            mining_hashes: 0,
            mined_blocks: 0,
            mining_difficulty: 1.0,
            target_block_time_ms: 60_000,
            amm_reserve_ztr: 0.0,
            amm_reserve_zusd: 0.0,
            amm_lp_burned: 0.0,
            mempool_size: 0,
            peers: vec![],
            network_hashrate: 0.0,
            network_name: "devnet".into(),
            protocol_version: 1,
            mining_threads_daemon: 0,
            max_mining_threads: 8,
            block_reward: 50.0,
            blocks_until_halving: 0,
            days_until_halving: 0.0,
            wallet_address: String::new(),
            pending_txs: vec![],
            pool_mode: false,
            pool_address: String::new(),
            pool_pending_zents: 0,
            pool_active_miners: 0,
            pool_ms_until_payout: 0,
            pool_my_paid_zents: 0,
            pool_my_share_pct: 0.0,
        }
    }
}

// ─── Transaction detail (click-to-preview) ─────────────────────────────────────
#[derive(Clone)]
struct TxDetail {
    direction: String,
    amount: f64,
    txid: String,
    counterparty: String,
    block_height: u64,
    confirmations: i64,
}

// ─── App struct ───────────────────────────────────────────────────────────────

struct ZentraApp {
    state: Arc<Mutex<NodeState>>,
    daemon: Arc<Mutex<Option<Child>>>,
    current_tab: Tab,

    mnemonic: String,
    address: String,

    send_to: String,
    send_amount: String,
    send_label: String,
    send_asset: String,

    restore_input: String,

    // Thread bug fix: persists user's choice between frames, only init once from daemon
    mining_threads_local: u32,
    mining_threads_set: bool,
    /// false = solo mining (rewards to own address), true = pool mining
    pool_mining: bool,

    console_input: String,
    console_history: Vec<(String, String)>,

    notification: Option<(String, bool, f64)>,
    show_about: bool,
    cpu_info: CpuInfo,

    // Auto-update
    update_available: Option<String>,  // Some("v0.1.5") when a newer release exists
    update_checked: bool,
    update_status: Option<String>,     // progress text while downloading an update
    update_progress: Arc<Mutex<String>>,
    update_ready: Arc<std::sync::atomic::AtomicBool>,
    update_launching: bool,

    // Transaction detail popup (click a row to preview everything)
    tx_detail: Option<TxDetail>,

    // Wallet security (optional PIN/password encryption of the seed)
    wallet_locked: bool,            // true = waiting for password to unlock
    encryption_enabled: bool,       // true = seed is stored encrypted
    unlock_input: String,
    unlock_error: String,
    show_encrypt_dialog: bool,
    encrypt_pw1: String,
    encrypt_pw2: String,
    encrypt_error: String,

    // Dialogs / menu state
    show_seed_dialog: bool,
    seed_revealed: bool,
    show_sign_dialog: bool,
    sign_message: String,
    sign_result: String,
    show_verify_dialog: bool,
    verify_message: String,
    verify_sig: String,
    verify_addr: String,
    verify_result: String,
    show_addresses_dialog: bool,
    show_options_dialog: bool,
    show_cmdline_dialog: bool,
    logo_texture: Option<egui::TextureHandle>,

    // Send fee priority: 0=low,1=normal,2=high
    fee_priority: u8,
    // Network: add-peer input
    peer_input: String,

    // Startup sync gate (shown every launch until the node is ready)
    sync_complete: bool,
    sync_started: std::time::Instant,
    sync_ready_since: Option<std::time::Instant>,

    // Theme system
    theme: AppTheme,
    theme_tab: ThemeTab,
    theme_custom_name: String,
    theme_harmony: Harmony,
    theme_harmony_accent: [u8; 3],
    theme_dark_mode: bool,
    theme_export_buf: String,
    theme_import_buf: String,
}

impl ZentraApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_visuals(&cc.egui_ctx);

        let state: Arc<Mutex<NodeState>> = Arc::new(Mutex::new(NodeState::default()));
        let child = spawn_daemon();
        let daemon = Arc::new(Mutex::new(child));

        let data_dir = data_dir();
        std::fs::create_dir_all(&data_dir).ok();
        let mut mnemonic = String::new();
        let mut address = String::new();

        // Detect optional seed encryption. If an encrypted seed exists we start
        // LOCKED and require the password before deriving the address.
        let enc_path = data_dir.join("wallet_mnemonic.enc");
        let encryption_enabled = enc_path.exists();
        let wallet_locked = encryption_enabled;

        if !encryption_enabled {
        if let Ok(cached) = std::fs::read_to_string(data_dir.join("wallet_mnemonic.txt")) {
            let phrase = cached.trim().to_string();
            if !phrase.is_empty() {
                mnemonic = phrase.clone();
                let tmp_path = data_dir.join("derived_address.tmp");
                let tmp_path2 = tmp_path.clone();
                std::thread::spawn(move || {
                    for _ in 0..20 {
                        std::thread::sleep(Duration::from_millis(500));
                        if let Ok(v) = call_rpc("deriveAddress", json!([phrase.clone()])) {
                            let addr = v.as_str().unwrap_or("").to_string();
                            std::fs::write(&tmp_path, &addr).ok();
                            break;
                        }
                    }
                });
                if let Ok(a) = std::fs::read_to_string(&tmp_path2) {
                    address = a.trim().to_string();
                }
                if !address.is_empty() {
                    state.lock().unwrap().wallet_address = address.clone();
                }
            }
        }
        } // end if !encryption_enabled

        let sc = Arc::clone(&state);
        std::thread::spawn(move || loop {
            poll_node(&sc);
            std::thread::sleep(Duration::from_millis(1000));
        });

        Self {
            state,
            daemon,
            current_tab: Tab::Overview,
            mnemonic,
            address,
            send_to: String::new(),
            send_amount: "1.0".into(),
            send_label: String::new(),
            send_asset: "ZTR".into(),
            restore_input: String::new(),
            mining_threads_local: 2,
            mining_threads_set: false,
            pool_mining: true, // Pool mining is the standard/default mode
            console_input: String::new(),
            console_history: vec![],
            notification: None,
            show_about: false,
            cpu_info: CpuInfo::detect(),
            update_available: None,
            update_checked: false,
            update_status: None,
            update_progress: Arc::new(Mutex::new(String::new())),
            update_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_launching: false,
            tx_detail: None,
            wallet_locked,
            encryption_enabled,
            unlock_input: String::new(),
            unlock_error: String::new(),
            show_encrypt_dialog: false,
            encrypt_pw1: String::new(),
            encrypt_pw2: String::new(),
            encrypt_error: String::new(),
            show_seed_dialog: false,
            seed_revealed: false,
            show_sign_dialog: false,
            sign_message: String::new(),
            sign_result: String::new(),
            show_verify_dialog: false,
            verify_message: String::new(),
            verify_sig: String::new(),
            verify_addr: String::new(),
            verify_result: String::new(),
            show_addresses_dialog: false,
            show_options_dialog: false,
            show_cmdline_dialog: false,
            logo_texture: None,
            fee_priority: 1,
            peer_input: String::new(),
            sync_complete: false,
            sync_started: std::time::Instant::now(),
            sync_ready_since: None,
            theme: AppTheme::load(),
            theme_tab: ThemeTab::Presets,
            theme_custom_name: String::new(),
            theme_harmony: Harmony::Complementary,
            theme_harmony_accent: [99, 102, 241],
            theme_dark_mode: true,
            theme_export_buf: String::new(),
            theme_import_buf: String::new(),
        }
    }

    fn notify(&mut self, msg: impl Into<String>, err: bool) {
        self.notification = Some((msg.into(), err, 7.0));
    }

    fn tick_notification(&mut self, dt: f64) {
        if let Some((_, _, ref mut ttl)) = self.notification {
            *ttl -= dt;
            if *ttl <= 0.0 { self.notification = None; }
        }
    }

    fn check_for_update(&mut self) {
        const CURRENT: &str = concat!("v", env!("CARGO_PKG_VERSION"));
        let tmp = data_dir().join("update_check.tmp");
        // Kick off the GitHub query once.
        if !self.update_checked {
            self.update_checked = true;
            let tmpw = tmp.clone();
            std::thread::spawn(move || {
                if let Ok(resp) = ureq::get("https://api.github.com/repos/Jestag-CryptoJarcc/Zentra-Chain/releases/latest")
                    .set("User-Agent", "ZentraCoreWallet")
                    .timeout(Duration::from_secs(15))
                    .call()
                {
                    if let Ok(body) = resp.into_string() {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                            if let Some(tag) = json["tag_name"].as_str() {
                                let _ = std::fs::write(&tmpw, tag);
                            }
                        }
                    }
                }
            });
        }
        // Poll each tick until the worker writes the latest tag.
        if self.update_available.is_none() {
            if let Ok(tag) = std::fs::read_to_string(&tmp) {
                let tag = tag.trim().to_string();
                let _ = std::fs::remove_file(&tmp);
                // Only offer when the remote tag is a strictly-higher semver than
                // ours — never on an equal tag, a downgrade, or a malformed tag.
                if !tag.is_empty() && tag != CURRENT && semver_gt(&tag, CURRENT) {
                    self.update_available = Some(tag);
                }
            }
        }
    }

    fn try_derive_address(&mut self) {
        if !self.address.is_empty() { return; }
        let tmp = data_dir().join("derived_address.tmp");
        if let Ok(a) = std::fs::read_to_string(&tmp) {
            let addr = a.trim().to_string();
            if !addr.is_empty() {
                self.address = addr.clone();
                self.state.lock().unwrap().wallet_address = addr;
                let _ = std::fs::remove_file(tmp);
            }
        }
    }

    /// Kick off async address derivation for the current mnemonic (writes the
    /// result to derived_address.tmp, which try_derive_address() then reads).
    fn start_derive(&self) {
        let phrase = self.mnemonic.clone();
        if phrase.is_empty() { return; }
        let tmp = data_dir().join("derived_address.tmp");
        std::thread::spawn(move || {
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(400));
                if let Ok(v) = call_rpc("deriveAddress", json!([phrase.clone()])) {
                    let addr = v.as_str().unwrap_or("").to_string();
                    if !addr.is_empty() { let _ = std::fs::write(&tmp, &addr); break; }
                }
            }
        });
    }

    /// Attempt to unlock an encrypted wallet using the entered password.
    fn try_unlock(&mut self) {
        let enc_path = data_dir().join("wallet_mnemonic.enc");
        let blob = std::fs::read_to_string(&enc_path).unwrap_or_default();
        match decrypt_seed(&blob, &self.unlock_input) {
            Some(seed) => {
                self.mnemonic = seed;
                self.wallet_locked = false;
                self.unlock_input.clear();
                self.unlock_error.clear();
                self.start_derive();
            }
            None => { self.unlock_error = "Incorrect password — please try again.".into(); }
        }
    }

    /// Download the latest Windows release zip and stage a PowerShell updater
    /// that swaps the executables once this process exits, then relaunches.
    fn start_self_update(&self) {
        let prog = Arc::clone(&self.update_progress);
        let ready = Arc::clone(&self.update_ready);
        let exe_dir = std::env::current_exe().ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let pid = std::process::id();
        std::thread::spawn(move || {
            use std::sync::atomic::Ordering;
            *prog.lock().unwrap() = "Downloading update…".into();
            let url = "https://github.com/Jestag-CryptoJarcc/Zentra-Chain/releases/latest/download/zentra-windows-x64.zip";
            let zip_path = exe_dir.join("zentra-update.zip");
            match ureq::get(url).timeout(Duration::from_secs(180)).call() {
                Ok(resp) => {
                    use std::io::Read;
                    let mut reader = resp.into_reader();
                    let mut buf = Vec::new();
                    if reader.read_to_end(&mut buf).is_err() {
                        *prog.lock().unwrap() = "Update failed: download was interrupted.".into();
                        return;
                    }
                    // INTEGRITY CHECK: fetch the release's checksums.txt and confirm
                    // the downloaded zip's SHA-256 matches the published value BEFORE
                    // we write or extract anything. This blocks a corrupted download
                    // or a CDN/mirror swapping the binary out from under us. (Full
                    // authenticity still wants a signature — tracked separately.)
                    *prog.lock().unwrap() = "Verifying download…".into();
                    let sums_url = "https://github.com/Jestag-CryptoJarcc/Zentra-Chain/releases/latest/download/checksums.txt";
                    let want = ureq::get(sums_url).timeout(Duration::from_secs(60)).call()
                        .ok().and_then(|r| r.into_string().ok());
                    let computed = {
                        use sha2::{Sha256, Digest};
                        let mut h = Sha256::new();
                        h.update(&buf);
                        hex::encode(h.finalize())
                    };
                    let verified = want.as_ref().map(|txt| txt.lines().any(|line| {
                        let l = line.to_ascii_lowercase();
                        l.contains("zentra-windows-x64.zip") && l.contains(&computed)
                    })).unwrap_or(false);
                    if !verified {
                        *prog.lock().unwrap() =
                            "Update aborted: checksum did not match the published release. Download manually from GitHub.".into();
                        return;
                    }
                    if std::fs::write(&zip_path, &buf).is_ok() {
                        *prog.lock().unwrap() = "Preparing installer…".into();
                        let dir_str = exe_dir.to_string_lossy().replace('\'', "''");
                        let script = SELF_UPDATE_PS1
                            .replace("__PID__", &pid.to_string())
                            .replace("__DIR__", &dir_str);
                        let script_path = exe_dir.join("zentra-updater.ps1");
                        if std::fs::write(&script_path, script).is_ok() {
                            ready.store(true, Ordering::SeqCst);
                            *prog.lock().unwrap() = "Ready — restarting to install…".into();
                            return;
                        }
                    }
                    *prog.lock().unwrap() = "Update failed: could not save the download.".into();
                }
                Err(e) => { *prog.lock().unwrap() = format!("Update failed: {}", e); }
            }
        });
    }
}

// PowerShell updater: waits for the wallet to exit, extracts the new zip over
// the install directory, then relaunches. {PID}/{DIR} filled in at runtime.
const SELF_UPDATE_PS1: &str = r#"$ErrorActionPreference = 'SilentlyContinue'
$wpid = __PID__
while (Get-Process -Id $wpid -ErrorAction SilentlyContinue) { Start-Sleep -Milliseconds 400 }
$dir = '__DIR__'
$zip = Join-Path $dir 'zentra-update.zip'
$tmp = Join-Path $dir 'zentra-update-tmp'
if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
Expand-Archive -Path $zip -DestinationPath $tmp -Force
Get-ChildItem -Path $tmp -Recurse -Filter *.exe | ForEach-Object { Copy-Item $_.FullName -Destination (Join-Path $dir $_.Name) -Force }
Remove-Item -Recurse -Force $tmp
Remove-Item -Force $zip
Start-Process -FilePath (Join-Path $dir 'zentra-qt.exe')
"#;

// ─── Seed encryption (self-contained, SHA-256 based) ────────────────────────────
// Derives a 32-byte key from the password with heavy SHA-256 stretching, then
// XORs the seed with a counter-mode SHA-256 keystream. A verification tag lets
// us detect an incorrect password without leaking the plaintext.
fn derive_key(password: &str, salt: &[u8]) -> [u8; 32] {
    use sha2::{Sha256, Digest};
    let mut h = Sha256::new();
    h.update(b"zentra-seed-v1");
    h.update(salt);
    h.update(password.as_bytes());
    let mut key = h.finalize_reset();
    for _ in 0..150_000 {
        h.update(key);
        h.update(salt);
        key = h.finalize_reset();
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&key);
    out
}

fn keystream_xor(key: &[u8; 32], data: &mut [u8]) {
    use sha2::{Sha256, Digest};
    let mut counter: u64 = 0;
    let mut offset = 0;
    while offset < data.len() {
        let mut h = Sha256::new();
        h.update(key);
        h.update(counter.to_le_bytes());
        let block = h.finalize();
        for (i, b) in block.iter().enumerate() {
            if offset + i >= data.len() { break; }
            data[offset + i] ^= b;
        }
        offset += 32;
        counter += 1;
    }
}

fn encrypt_seed(mnemonic: &str, password: &str) -> String {
    use sha2::{Sha256, Digest};
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos()).unwrap_or(0);
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&t.to_le_bytes());
    let key = derive_key(password, &salt);
    let mut data = mnemonic.as_bytes().to_vec();
    keystream_xor(&key, &mut data);
    let mut h = Sha256::new(); h.update(key); h.update(b"verify");
    let tag = h.finalize();
    format!("{}:{}:{}", hex::encode(salt), hex::encode(tag), hex::encode(data))
}

fn decrypt_seed(blob: &str, password: &str) -> Option<String> {
    use sha2::{Sha256, Digest};
    let parts: Vec<&str> = blob.trim().split(':').collect();
    if parts.len() != 3 { return None; }
    let salt = hex::decode(parts[0]).ok()?;
    let tag = hex::decode(parts[1]).ok()?;
    let mut data = hex::decode(parts[2]).ok()?;
    let key = derive_key(password, &salt);
    let mut h = Sha256::new(); h.update(key); h.update(b"verify");
    let expect = h.finalize();
    if expect.as_slice() != tag.as_slice() { return None; }
    keystream_xor(&key, &mut data);
    String::from_utf8(data).ok()
}

impl eframe::App for ZentraApp {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        kill_daemon(&self.daemon);
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(800));
        self.try_derive_address();
        // Keep the shared node state's wallet address in lock-step with the UI
        // address. setup_screen (create/restore) sets self.address directly, so
        // without this the balance poller would never learn the address and the
        // balance would read 0 forever on a freshly created wallet.
        if !self.address.is_empty() {
            let mut s = self.state.lock().unwrap();
            if s.wallet_address != self.address { s.wallet_address = self.address.clone(); }
        }
        self.tick_notification(0.8);
        self.check_for_update();

        // Self-update: surface worker progress + launch the staged installer.
        {
            let p = self.update_progress.lock().unwrap().clone();
            if !p.is_empty() { self.update_status = Some(p); }
        }
        if self.update_ready.load(std::sync::atomic::Ordering::SeqCst) && !self.update_launching {
            self.update_launching = true;
            let exe_dir = std::env::current_exe().ok()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                .unwrap_or_else(|| std::path::PathBuf::from("."));
            let script = exe_dir.join("zentra-updater.ps1");
            kill_daemon(&self.daemon);
            #[cfg(target_os = "windows")]
            { use std::os::windows::process::CommandExt;
              let _ = Command::new("powershell")
                .args(["-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script)
                .creation_flags(0x08000000).spawn(); }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // ── Snapshot node state ──
        let ns = {
            let s = self.state.lock().unwrap();
            (
                s.connected, s.blue_score, s.tips_count, s.selected_tip.clone(),
                s.ztr_balance, s.recent_blocks.clone(),
                s.is_mining, s.mining_lane, s.mining_address.clone(),
                s.mining_hashrate, s.mining_hashes, s.mined_blocks,
                s.mining_difficulty, s.target_block_time_ms,
                s.amm_reserve_ztr, s.amm_reserve_zusd, s.amm_lp_burned,
                s.mempool_size, s.peers.clone(), s.network_hashrate,
                s.network_name.clone(), s.protocol_version,
                s.mining_threads_daemon, s.max_mining_threads,
                s.block_reward, s.blocks_until_halving, s.days_until_halving,
                s.pending_txs.clone(),
            )
        };
        let (connected, height, tips, tip_hash, ztr_bal, recent_blocks,
             is_mining, mining_lane, mining_addr, hashrate, hashes, mined_blks,
             difficulty, target_ms, amm_ztr, amm_zusd, amm_lp_burned,
             mempool, peers, net_hash, net_name, proto_ver,
             threads_daemon, max_threads, block_reward,
             halving_blocks, halving_days, pending_txs) = ns;

        // Init thread slider once from daemon
        if !self.mining_threads_set && threads_daemon > 0 {
            self.mining_threads_local = threads_daemon;
            self.mining_threads_set = true;
        }

        // Pool snapshot + push UI pool mode into shared state for the heartbeat thread
        let (pool_addr, pool_pending, pool_miners, pool_payout_ms, pool_paid, pool_share) = {
            let mut s = self.state.lock().unwrap();
            s.pool_mode = self.pool_mining;
            (s.pool_address.clone(), s.pool_pending_zents, s.pool_active_miners,
             s.pool_ms_until_payout, s.pool_my_paid_zents, s.pool_my_share_pct)
        };

        let wallet_ready = !self.address.is_empty();

        // ── Palette derived from active theme ────────────────────────────────
        apply_theme_visuals(ctx, &self.theme);
        let col_bg      = self.theme.col(self.theme.bg);
        let col_toolbar = self.theme.col(self.theme.toolbar);
        let col_surface = self.theme.col(self.theme.surface);
        let col_surface2= self.theme.col(self.theme.surface2);
        let col_border  = self.theme.col(self.theme.border);
        let col_text    = self.theme.col(self.theme.text);
        let col_muted   = self.theme.col(self.theme.muted);
        let col_faint   = egui::Color32::from_rgb(90,  90,  90);
        let col_green   = self.theme.col(self.theme.green);
        let col_amber   = egui::Color32::from_rgb(210, 162, 62);
        let col_red     = egui::Color32::from_rgb(190, 68,  68);
        let col_blue    = egui::Color32::from_rgb(88,  148, 212);
        let col_purple  = egui::Color32::from_rgb(150, 120, 200);
        let col_accent  = self.theme.col(self.theme.accent);

        // ── Wallet lock screen (shown when an encrypted seed needs a password) ─
        if self.wallet_locked {
            let logo = self.logo_texture.get_or_insert_with(|| load_logo_texture(ctx)).clone();
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(col_bg))
                .show(ctx, |ui| {
                    ui.allocate_ui_at_rect(
                        egui::Rect::from_center_size(ui.max_rect().center(), egui::vec2(360.0, 340.0)),
                        |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add_space(10.0);
                                ui.image((logo.id(), egui::vec2(74.0, 74.0)));
                                ui.add_space(16.0);
                                ui.label(egui::RichText::new("🔒  Wallet Locked").size(22.0).strong().color(col_text));
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new("Enter your password to unlock your wallet.").size(12.0).color(col_muted));
                                ui.add_space(22.0);
                                let resp = ui.add(egui::TextEdit::singleline(&mut self.unlock_input)
                                    .password(true).desired_width(280.0).hint_text("Password"));
                                resp.request_focus();
                                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                ui.add_space(14.0);
                                let click = ui.add_sized(egui::vec2(280.0, 36.0),
                                    egui::Button::new(egui::RichText::new("Unlock").size(13.5).color(egui::Color32::WHITE).strong())
                                        .fill(col_accent).rounding(8.0)).clicked();
                                if enter || click { self.try_unlock(); }
                                if !self.unlock_error.is_empty() {
                                    ui.add_space(12.0);
                                    ui.label(egui::RichText::new(&self.unlock_error).size(11.5).color(col_red));
                                }
                            });
                        });
                });
            ctx.request_repaint_after(Duration::from_millis(80));
            return;
        }

        // ── Startup sync gate ────────────────────────────────────────────────
        // We just need the node CONNECTED (RPC responding). Height 0 is a valid
        // synced state for a fresh chain — don't require height > 0 or a brand
        // new network would hang here forever.
        if connected {
            if self.sync_ready_since.is_none() {
                self.sync_ready_since = Some(std::time::Instant::now());
            }
            // Brief stable-connection window so the splash is visible.
            if self.sync_ready_since.map(|t| t.elapsed().as_secs_f32() > 2.0).unwrap_or(false) {
                self.sync_complete = true;
            }
        } else {
            self.sync_ready_since = None;
        }

        if wallet_ready && !self.sync_complete {
            let logo = self.logo_texture.get_or_insert_with(|| load_logo_texture(ctx)).clone();
            let elapsed = self.sync_started.elapsed().as_secs_f32();
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(col_bg))
                .show(ctx, |ui| {
                    let avail = ui.available_size();
                    ui.allocate_ui_at_rect(
                        egui::Rect::from_center_size(
                            ui.max_rect().center(),
                            egui::vec2(420.0, 360.0)),
                        |ui| {
                            ui.vertical_centered(|ui| {
                                ui.add_space(20.0);
                                // Pulsing coin logo
                                let pulse = 0.85 + 0.15 * (elapsed * 2.0).sin();
                                ui.image((logo.id(), egui::vec2(96.0 * pulse, 96.0 * pulse)));
                                ui.add_space(18.0);
                                ui.label(egui::RichText::new("Zentra Core Wallet").size(22.0).strong().color(col_text));
                                ui.add_space(4.0);
                                ui.label(egui::RichText::new(format!("Network: {}", net_name)).size(12.0).color(col_muted));
                                ui.add_space(28.0);

                                // Status line
                                let (status, stcol) = if !connected {
                                    ("Connecting to the Zentra network…", col_amber)
                                } else if height == 0 {
                                    ("Loading blockchain…", col_amber)
                                } else {
                                    ("Synchronizing blockchain…", col_green)
                                };
                                ui.label(egui::RichText::new(status).size(14.0).color(stcol).strong());
                                ui.add_space(10.0);

                                // Progress bar (indeterminate sweep while connecting,
                                // fills as the node reports a height).
                                let bar_w = 320.0_f32;
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(bar_w, 8.0), egui::Sense::hover());
                                let p = ui.painter();
                                p.rect_filled(rect, egui::Rounding::same(4.0), col_surface2);
                                if connected && height > 0 {
                                    // Ramp to full over the stable-connection window.
                                    let frac = self.sync_ready_since
                                        .map(|t| (t.elapsed().as_secs_f32() / 2.2).clamp(0.05, 1.0))
                                        .unwrap_or(0.05);
                                    let fill = egui::Rect::from_min_size(rect.min, egui::vec2(bar_w * frac, 8.0));
                                    p.rect_filled(fill, egui::Rounding::same(4.0), col_green);
                                } else {
                                    // Indeterminate sweeping segment
                                    let seg = 90.0_f32;
                                    let x = (elapsed * 160.0) % (bar_w + seg) - seg;
                                    let sweep = egui::Rect::from_min_size(
                                        egui::pos2(rect.min.x + x.max(0.0), rect.min.y),
                                        egui::vec2(seg.min(bar_w - x.max(0.0)).max(0.0), 8.0));
                                    p.rect_filled(sweep, egui::Rounding::same(4.0), col_accent);
                                }
                                ui.add_space(14.0);

                                // Detail line
                                ui.label(egui::RichText::new(
                                    if connected { format!("Block height: {}   ·   {} peers", height, peers.len()) }
                                    else { "Starting local node…".to_string() }
                                ).size(11.5).color(col_muted));
                                ui.add_space(20.0);
                                ui.label(egui::RichText::new("The Zentra blockchain keeps running while your wallet is closed.\nThis quick sync makes sure you have the latest blocks.").size(10.5).color(col_muted));
                            });
                        });
                    let _ = avail;
                });
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        }

        // ── About dialog ──────────────────────────────────────────────────────
        if self.show_about {
            egui::Window::new("About Zentra Core Wallet")
                .collapsible(false)
                .resizable(false)
                .default_pos(egui::pos2(300.0, 200.0))
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Zentra Core").size(20.0).strong().color(col_text));
                        ui.label(egui::RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION"))).size(12.0).color(col_muted));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new("BlockDAG Wallet — GhostDAG Consensus").size(12.5).color(col_muted));
                        ui.label(egui::RichText::new("Multi-lane PoW  ·  AMM  ·  Omni-Vault TSS").size(11.0).color(col_muted));
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("Max Supply: 50,000,000 ZTR").size(11.0).color(col_muted));
                        ui.label(egui::RichText::new("Block Reward: 47.56468797 ZTR").size(11.0).color(col_muted));
                        ui.label(egui::RichText::new("Halving: every 525,600 blocks (~1 year)").size(11.0).color(col_muted));
                        ui.add_space(14.0);
                        if ui.button("  Close  ").clicked() { self.show_about = false; }
                        ui.add_space(8.0);
                    });
                });
        }

        // ── Transaction detail popup (click any tx row) ────────────────────────
        if let Some(d) = self.tx_detail.clone() {
            let mut open = true;
            egui::Window::new("Transaction Details")
                .collapsible(false).resizable(false)
                .open(&mut open)
                .default_pos(egui::pos2(280.0, 150.0))
                .show(ctx, |ui| {
                    ui.set_max_width(500.0);
                    ui.add_space(6.0);
                    let (sign, ac) = match d.direction.as_str() {
                        "Sent" | "Sent to" => ("-", col_red),
                        "Mined"            => ("+", col_amber),
                        _                  => ("+", col_green),
                    };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&d.direction).size(13.0).color(col_muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(egui::RichText::new(format!("{}{} ZTR", sign, fmt_amount(d.amount)))
                                .size(22.0).strong().color(ac));
                        });
                    });
                    ui.add_space(12.0); ui.separator(); ui.add_space(12.0);

                    let status = if d.confirmations <= 0 { "Unconfirmed (in mempool)".to_string() }
                        else if d.confirmations < 6 { format!("{} confirmation(s)", d.confirmations) }
                        else { "Confirmed".to_string() };
                    let st_col = if d.confirmations <= 0 { col_amber } else if d.confirmations < 6 { col_amber } else { col_green };

                    egui::Grid::new("txd_grid").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                        ui.label(egui::RichText::new("Status").size(11.5).color(col_muted));
                        ui.label(egui::RichText::new(&status).size(11.5).strong().color(st_col));
                        ui.end_row();
                        ui.label(egui::RichText::new("Block").size(11.5).color(col_muted));
                        ui.label(egui::RichText::new(format!("#{}", d.block_height)).size(11.5).color(col_text));
                        ui.end_row();
                        if !d.counterparty.is_empty() {
                            ui.label(egui::RichText::new("Address").size(11.5).color(col_muted));
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&d.counterparty).size(11.0).monospace().color(col_text));
                                copy_btn(ui, &d.counterparty, col_muted, col_accent);
                            });
                            ui.end_row();
                        }
                        if !d.txid.is_empty() {
                            ui.label(egui::RichText::new("Transaction ID").size(11.5).color(col_muted));
                            ui.horizontal(|ui| {
                                let short = if d.txid.len() > 40 { format!("{}…{}", &d.txid[..24], &d.txid[d.txid.len()-12..]) } else { d.txid.clone() };
                                ui.label(egui::RichText::new(short).size(11.0).monospace().color(col_text));
                                copy_btn(ui, &d.txid, col_muted, col_accent);
                            });
                            ui.end_row();
                        }
                    });
                    ui.add_space(6.0);
                });
            if !open { self.tx_detail = None; }
        }

        // ── Encrypt wallet dialog (set / change / remove password) ─────────────
        if self.show_encrypt_dialog {
            egui::Window::new(if self.encryption_enabled { "Wallet Security" } else { "Encrypt Wallet" })
                .collapsible(false).resizable(false)
                .default_pos(egui::pos2(300.0, 180.0))
                .show(ctx, |ui| {
                    ui.set_max_width(430.0);
                    ui.add_space(4.0);
                    egui::Frame::none()
                        .fill(self.theme.col_a(self.theme.accent, 24))
                        .stroke(egui::Stroke::new(1.0, col_accent)).rounding(8.0)
                        .inner_margin(egui::vec2(12.0, 10.0)).show(ui, |ui| {
                            ui.label(egui::RichText::new("Encrypt your seed phrase on disk with a password. You'll enter it each time you open the wallet. If you forget it you can still recover with your 24-word seed.").size(11.5).color(col_text));
                        });
                    ui.add_space(14.0);
                    ui.label(egui::RichText::new("New password").size(11.5).color(col_text));
                    ui.add_space(3.0);
                    ui.add(egui::TextEdit::singleline(&mut self.encrypt_pw1).password(true)
                        .desired_width(f32::INFINITY).hint_text("At least 6 characters"));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Confirm password").size(11.5).color(col_text));
                    ui.add_space(3.0);
                    ui.add(egui::TextEdit::singleline(&mut self.encrypt_pw2).password(true)
                        .desired_width(f32::INFINITY).hint_text("Repeat password"));
                    if !self.encrypt_error.is_empty() {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(&self.encrypt_error).size(11.0).color(col_red));
                    }
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        let lbl = if self.encryption_enabled { "  Update Password  " } else { "  Encrypt Wallet  " };
                        if ui.add(egui::Button::new(egui::RichText::new(lbl).color(egui::Color32::WHITE).strong()).fill(col_accent)).clicked() {
                            if self.encrypt_pw1.len() < 6 {
                                self.encrypt_error = "Password must be at least 6 characters.".into();
                            } else if self.encrypt_pw1 != self.encrypt_pw2 {
                                self.encrypt_error = "Passwords do not match.".into();
                            } else if self.mnemonic.is_empty() {
                                self.encrypt_error = "No wallet seed is loaded to encrypt.".into();
                            } else {
                                let blob = encrypt_seed(&self.mnemonic, &self.encrypt_pw1);
                                let dd = data_dir();
                                let _ = std::fs::write(dd.join("wallet_mnemonic.enc"), blob);
                                let _ = std::fs::remove_file(dd.join("wallet_mnemonic.txt"));
                                self.encryption_enabled = true;
                                self.show_encrypt_dialog = false;
                                self.encrypt_pw1.clear(); self.encrypt_pw2.clear(); self.encrypt_error.clear();
                                self.notification = Some(("Wallet encrypted — you'll need this password next launch.".into(), false, 7.0));
                            }
                        }
                        if ui.button("  Cancel  ").clicked() {
                            self.show_encrypt_dialog = false;
                            self.encrypt_pw1.clear(); self.encrypt_pw2.clear(); self.encrypt_error.clear();
                        }
                        if self.encryption_enabled {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(egui::Button::new(egui::RichText::new(" Remove Encryption ").color(col_red))
                                    .stroke(egui::Stroke::new(1.0, col_red))).clicked() {
                                    let dd = data_dir();
                                    let _ = std::fs::write(dd.join("wallet_mnemonic.txt"), &self.mnemonic);
                                    let _ = std::fs::remove_file(dd.join("wallet_mnemonic.enc"));
                                    self.encryption_enabled = false;
                                    self.show_encrypt_dialog = false;
                                    self.notification = Some(("Encryption removed — your seed is stored unencrypted.".into(), false, 6.0));
                                }
                            });
                        }
                    });
                    ui.add_space(6.0);
                });
        }

        // ── Seed phrase dialog (warning → reveal → copy) ───────────────────────
        if self.show_seed_dialog {
            egui::Window::new("Backup Seed Phrase")
                .collapsible(false).resizable(false)
                .default_pos(egui::pos2(260.0, 150.0))
                .show(ctx, |ui| {
                    ui.set_max_width(460.0);
                    ui.add_space(6.0);
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgba_unmultiplied(190, 68, 68, 30))
                        .stroke(egui::Stroke::new(1.0, col_red))
                        .rounding(8.0).inner_margin(egui::vec2(14.0, 12.0))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("⚠  NEVER share these 24 words with anyone.").size(13.0).strong().color(col_red));
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new("Anyone with your seed phrase has FULL control of your funds. Zentra staff will never ask for it. Store it offline, on paper, somewhere safe. Phishing sites and fake 'support' will try to steal it.").size(11.5).color(col_text));
                        });
                    ui.add_space(12.0);
                    if !self.seed_revealed {
                        ui.vertical_centered(|ui| {
                            if ui.add(egui::Button::new(egui::RichText::new("  I understand — Reveal Seed Phrase  ").color(egui::Color32::WHITE))
                                .fill(col_red).rounding(8.0)).clicked() {
                                self.seed_revealed = true;
                            }
                        });
                    } else {
                        egui::Frame::none()
                            .fill(col_surface2)
                            .stroke(egui::Stroke::new(1.0, col_border))
                            .rounding(8.0).inner_margin(egui::vec2(14.0, 12.0))
                            .show(ui, |ui| {
                                let words: Vec<&str> = self.mnemonic.split_whitespace().collect();
                                egui::Grid::new("seed_dialog_grid").num_columns(4).spacing([14.0, 7.0]).show(ui, |ui| {
                                    for (i, w) in words.iter().enumerate() {
                                        ui.horizontal(|ui| {
                                            ui.label(egui::RichText::new(format!("{:2}.", i+1)).size(10.0).color(col_muted));
                                            ui.label(egui::RichText::new(*w).size(12.0).monospace().color(col_text).strong());
                                        });
                                        if (i+1) % 4 == 0 { ui.end_row(); }
                                    }
                                });
                            });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(egui::RichText::new("  ⧉ Copy to Clipboard  ").color(egui::Color32::WHITE)).fill(col_accent)).clicked() {
                                ui.output_mut(|o| o.copied_text = self.mnemonic.clone());
                                self.notification = Some(("Seed phrase copied — store it safely, then clear your clipboard.".into(), false, 6.0));
                            }
                            if ui.button("  Save to File…  ").clicked() {
                                if let Some(path) = rfd::FileDialog::new()
                                    .set_file_name("zentra-seed-phrase.txt")
                                    .add_filter("Text", &["txt"]).save_file() {
                                    let _ = std::fs::write(&path, &self.mnemonic);
                                    self.notification = Some(("Seed phrase saved to file.".into(), false, 5.0));
                                }
                            }
                        });
                    }
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(6.0);
                    if ui.button("  Close  ").clicked() { self.show_seed_dialog = false; self.seed_revealed = false; }
                    ui.add_space(4.0);
                });
        }

        // ── Sign Message dialog ────────────────────────────────────────────────
        if self.show_sign_dialog {
            egui::Window::new("Sign Message")
                .collapsible(false).resizable(true)
                .default_pos(egui::pos2(280.0, 160.0))
                .show(ctx, |ui| {
                    ui.set_max_width(480.0);
                    ui.label(egui::RichText::new("Sign a message to prove you control this wallet's address.").size(12.0).color(col_muted));
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Your Address").size(11.0).color(col_muted));
                    let mut addr = self.address.clone();
                    ui.add(egui::TextEdit::singleline(&mut addr).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Message").size(11.0).color(col_muted));
                    ui.add(egui::TextEdit::multiline(&mut self.sign_message).desired_rows(3).desired_width(f32::INFINITY).hint_text("Type the message to sign…"));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("  Sign  ").color(egui::Color32::WHITE)).fill(col_accent)).clicked() {
                            self.sign_result = sign_message_with_wallet(&self.mnemonic, &self.sign_message);
                        }
                        if ui.button("  Copy Signature  ").clicked() {
                            ui.output_mut(|o| o.copied_text = self.sign_result.clone());
                            self.notification = Some(("Signature copied.".into(), false, 3.0));
                        }
                    });
                    if !self.sign_result.is_empty() {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Signature (hex)").size(11.0).color(col_muted));
                        let mut sig = self.sign_result.clone();
                        ui.add(egui::TextEdit::multiline(&mut sig).desired_rows(2).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                    }
                    ui.add_space(10.0);
                    if ui.button("  Close  ").clicked() { self.show_sign_dialog = false; }
                });
        }

        // ── Verify Message dialog ──────────────────────────────────────────────
        if self.show_verify_dialog {
            egui::Window::new("Verify Message")
                .collapsible(false).resizable(true)
                .default_pos(egui::pos2(280.0, 160.0))
                .show(ctx, |ui| {
                    ui.set_max_width(480.0);
                    ui.label(egui::RichText::new("Verify a message was signed by the owner of an address.").size(12.0).color(col_muted));
                    ui.add_space(10.0);
                    ui.label(egui::RichText::new("Signing Address").size(11.0).color(col_muted));
                    ui.add(egui::TextEdit::singleline(&mut self.verify_addr).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace).hint_text("zentra…"));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Message").size(11.0).color(col_muted));
                    ui.add(egui::TextEdit::multiline(&mut self.verify_message).desired_rows(2).desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Signature (hex)").size(11.0).color(col_muted));
                    ui.add(egui::TextEdit::multiline(&mut self.verify_sig).desired_rows(2).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                    ui.add_space(10.0);
                    if ui.add(egui::Button::new(egui::RichText::new("  Verify  ").color(egui::Color32::WHITE)).fill(col_accent)).clicked() {
                        self.verify_result = verify_message_sig(&self.verify_addr, &self.verify_message, &self.verify_sig);
                    }
                    if !self.verify_result.is_empty() {
                        ui.add_space(8.0);
                        let ok = self.verify_result.starts_with("✓");
                        ui.label(egui::RichText::new(&self.verify_result).size(12.5).strong().color(if ok { col_green } else { col_red }));
                    }
                    ui.add_space(10.0);
                    if ui.button("  Close  ").clicked() { self.show_verify_dialog = false; self.verify_result.clear(); }
                });
        }

        // ── Receiving Addresses dialog ─────────────────────────────────────────
        if self.show_addresses_dialog {
            egui::Window::new("Receiving Addresses")
                .collapsible(false).resizable(false)
                .default_pos(egui::pos2(280.0, 180.0))
                .show(ctx, |ui| {
                    ui.set_max_width(480.0);
                    ui.label(egui::RichText::new("Your wallet's receiving address. Share it to receive ZTR.").size(12.0).color(col_muted));
                    ui.add_space(10.0);
                    egui::Frame::none().fill(egui::Color32::from_rgb(20,20,20)).stroke(egui::Stroke::new(1.0, col_green))
                        .rounding(6.0).inner_margin(egui::vec2(12.0, 10.0)).show(ui, |ui| {
                            let mut a = self.address.clone();
                            ui.add(egui::TextEdit::singleline(&mut a).font(egui::TextStyle::Monospace).desired_width(f32::INFINITY).frame(false));
                        });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(egui::RichText::new("  ⧉ Copy  ").color(egui::Color32::WHITE)).fill(col_green)).clicked() {
                            ui.output_mut(|o| o.copied_text = self.address.clone());
                            self.notification = Some(("Address copied.".into(), false, 3.0));
                        }
                        if ui.button("  Close  ").clicked() { self.show_addresses_dialog = false; }
                    });
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Note: Zentra v1 uses a single deterministic address per wallet (derived from your seed). HD multi-address support is on the roadmap.").size(10.0).color(col_muted));
                });
        }

        // ── Options dialog ─────────────────────────────────────────────────────
        if self.show_options_dialog {
            egui::Window::new("Options")
                .collapsible(false).resizable(false)
                .default_pos(egui::pos2(300.0, 180.0))
                .show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(egui::RichText::new("Node & Wallet").size(13.0).strong().color(col_text));
                    ui.add_space(8.0);
                    kv(ui, "Network", &net_name, col_muted, col_text);
                    kv(ui, "RPC endpoint", &node_rpc_hostport(), col_muted, col_text);
                    kv(ui, "Web dashboard", "zentrachain.xyz", col_muted, col_text);
                    kv(ui, "Data directory", &data_dir().display().to_string(), col_muted, col_text);
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Appearance").size(13.0).strong().color(col_text));
                    ui.add_space(6.0);
                    if ui.button("  Open Theme Customizer  ").clicked() {
                        self.current_tab = Tab::Theme;
                        self.show_options_dialog = false;
                    }
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("  Open Data Folder  ").clicked() {
                            let d = data_dir();
                            #[cfg(target_os = "windows")]
                            { let _ = std::process::Command::new("explorer").arg(d).spawn(); }
                        }
                        if ui.button("  Close  ").clicked() { self.show_options_dialog = false; }
                    });
                });
        }

        // ── Command-line options dialog ────────────────────────────────────────
        if self.show_cmdline_dialog {
            egui::Window::new("Command-line Options")
                .collapsible(false).resizable(true)
                .default_pos(egui::pos2(260.0, 160.0))
                .show(ctx, |ui| {
                    ui.set_max_width(520.0);
                    ui.label(egui::RichText::new("zentrad — Zentra L1 node daemon").size(12.5).strong().color(col_text));
                    ui.add_space(8.0);
                    let opts = [
                        ("--network <net>", "mainnet | testnet | devnet"),
                        ("--mine", "Enable mining on this node"),
                        ("--pool", "Run as a mining pool operator"),
                        ("--lane <0-4>", "Mining lane: 0=CPU 1=GPU 2=BTC 3=LTC 4=FPGA"),
                        ("--rpc-port <port>", "JSON-RPC port (default 16111)"),
                        ("--p2p-port <port>", "P2P port (default 16110)"),
                        ("--data-dir <path>", "Data directory location"),
                        ("--wallet", "Enable wallet mode"),
                    ];
                    for (flag, desc) in opts {
                        ui.horizontal(|ui| {
                            ui.add_sized(egui::vec2(150.0, 18.0), egui::Label::new(egui::RichText::new(flag).size(11.5).monospace().color(col_accent)));
                            ui.label(egui::RichText::new(desc).size(11.5).color(col_muted));
                        });
                    }
                    ui.add_space(10.0);
                    if ui.button("  Close  ").clicked() { self.show_cmdline_dialog = false; }
                });
        }

        // ── Menu bar ──────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("menubar")
            .frame(egui::Frame::none()
                .fill(col_toolbar)
                .stroke(egui::Stroke::new(1.0, col_border)))
            .show(ctx, |ui| {
                egui::menu::bar(ui, |ui| {
                    // ZTR coin logo (loaded PNG texture)
                    let tex = self.logo_texture.get_or_insert_with(|| {
                        load_logo_texture(ctx)
                    });
                    ui.image((tex.id(), egui::vec2(30.0, 30.0)));
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Zentra Core Wallet").strong().size(12.0)
                        .color(self.theme.col(self.theme.accent)));
                    ui.add_space(8.0);
                    ui.menu_button("File", |ui| {
                        if ui.button("Backup Wallet…").clicked() {
                            ui.close_menu();
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("zentra-wallet-backup.txt")
                                .add_filter("Text", &["txt"])
                                .save_file()
                            {
                                match std::fs::write(&path, &self.mnemonic) {
                                    Ok(_)  => self.notification = Some((format!("Wallet backed up to {}", path.display()), false, 6.0)),
                                    Err(e) => self.notification = Some((format!("Backup failed: {}", e), true, 6.0)),
                                }
                            }
                        }
                        if ui.button("Sign Message…").clicked() { self.show_sign_dialog = true; ui.close_menu(); }
                        if ui.button("Verify Message…").clicked() { self.show_verify_dialog = true; ui.close_menu(); }
                        ui.separator();
                        if ui.button("Receiving Addresses…").clicked() { self.show_addresses_dialog = true; ui.close_menu(); }
                        if ui.button("Show / Backup Seed Phrase…").clicked() {
                            self.show_seed_dialog = true; self.seed_revealed = false; ui.close_menu();
                        }
                        ui.separator();
                        if ui.button("Exit").clicked() {
                            kill_daemon(&self.daemon);
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.menu_button("Settings", |ui| {
                        if ui.button("Options…").clicked() { self.show_options_dialog = true; ui.close_menu(); }
                        if ui.button("Theme & Appearance…").clicked() { self.current_tab = Tab::Theme; ui.close_menu(); }
                        ui.separator();
                        let enc_lbl = if self.encryption_enabled { "🔒  Wallet Security / Change Password…" } else { "🔒  Encrypt Wallet…" };
                        if ui.button(enc_lbl).clicked() {
                            self.show_encrypt_dialog = true;
                            self.encrypt_pw1.clear(); self.encrypt_pw2.clear(); self.encrypt_error.clear();
                            ui.close_menu();
                        }
                        if self.encryption_enabled {
                            if ui.button("Lock Wallet Now").clicked() {
                                self.wallet_locked = true;
                                self.address.clear();
                                self.mnemonic.clear();
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui.button("Show / Backup Seed Phrase…").clicked() {
                            self.show_seed_dialog = true; self.seed_revealed = false; ui.close_menu();
                        }
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("Debug Console").clicked() { self.current_tab = Tab::Console; ui.close_menu(); }
                        if ui.button("Command-line Options").clicked() { self.show_cmdline_dialog = true; ui.close_menu(); }
                        ui.separator();
                        if ui.button("About Zentra Core Wallet").clicked() { self.show_about = true; ui.close_menu(); }
                    });

                    // Right side: network + status
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        let net_col = match net_name.as_str() {
                            "mainnet" => col_green,
                            "testnet" => col_amber,
                            _ => col_purple,
                        };
                        ui.label(egui::RichText::new(&net_name).size(11.0).color(net_col).strong());
                        ui.label(egui::RichText::new("●").size(9.0).color(net_col));
                        ui.add_space(12.0);
                        if is_mining {
                            ui.label(egui::RichText::new(format!("Mining  {}", fmt_hashrate(hashrate))).size(11.0).color(col_green));
                            ui.add_space(10.0);
                        }
                    });
                });
            });

        // ── Toolbar ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("toolbar")
            .exact_height(48.0)
            .frame(egui::Frame::none()
                .fill(col_toolbar)
                .stroke(egui::Stroke::new(1.0, col_border)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let tabs: &[(Tab, &str, &str)] = &[
                        (Tab::Overview,     "Overview",     "⌂"),
                        (Tab::Send,         "Send",         "➤"),
                        (Tab::Receive,      "Receive",      "▼"),
                        (Tab::Transactions, "History",      "≡"),
                        (Tab::Mining,       "Mining",       "⛏"),
                        (Tab::Console,      "Console",      "▸"),
                        (Tab::Network,      "Network",      "⚯"),
                        (Tab::Theme,        "Theme",        "✦"),
                    ];
                    ui.add_space(4.0);
                    for (tab, label, icon) in tabs {
                        let sel = self.current_tab == *tab;
                        if toolbar_tab(ui, label, icon, sel, col_accent, col_surface, col_text, col_muted) {
                            self.current_tab = *tab;
                        }
                    }
                });
            });

        // ── Status bar ────────────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("statusbar")
            .exact_height(22.0)
            .frame(egui::Frame::none()
                .fill(col_toolbar)
                .stroke(egui::Stroke::new(1.0, col_border))
                .inner_margin(egui::vec2(8.0, 2.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Connections
                    let cc = if connected { col_green } else { col_red };
                    ui.label(egui::RichText::new(format!("{} active connections", peers.len())).size(10.5).color(cc));
                    sep(ui, col_faint);
                    // Block height
                    ui.label(egui::RichText::new(format!("Block {}", height)).size(10.5).color(col_muted));
                    sep(ui, col_faint);
                    // Mempool
                    ui.label(egui::RichText::new(format!("{} mempool transactions", mempool)).size(10.5).color(col_muted));
                    sep(ui, col_faint);
                    // Sync
                    let (sync_txt, sync_col) = if connected {
                        ("Up to date", col_muted)
                    } else {
                        ("Not connected", col_red)
                    };
                    ui.label(egui::RichText::new(sync_txt).size(10.5).color(sync_col));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new(format!("protocol v{}", proto_ver)).size(10.0).color(col_faint));
                    });
                });
            });

        // ── Main content ──────────────────────────────────────────────────────
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(col_bg))
            .show(ctx, |ui| {
                // Update available banner
                if let Some(ref new_ver) = self.update_available.clone() {
                    let bg = self.theme.col_a(self.theme.accent, 30);
                    let mut dismiss = false;
                    let mut do_update = false;
                    let downloading = self.update_status.as_deref()
                        .map(|s| !s.is_empty() && !s.starts_with("Update failed")).unwrap_or(false);
                    egui::Frame::none().fill(bg)
                        .stroke(egui::Stroke::new(1.0, col_accent))
                        .rounding(8.0)
                        .inner_margin(egui::vec2(14.0, 8.0)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(format!("🆕  New version {} is available", new_ver))
                                    .size(12.0).strong().color(col_text));
                                ui.add_space(10.0);
                                if let Some(st) = self.update_status.clone() {
                                    let c = if st.starts_with("Update failed") { col_red } else { col_amber };
                                    ui.label(egui::RichText::new(st).size(11.0).color(c));
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(egui::Button::new(egui::RichText::new("✕").size(10.0).color(col_muted)).frame(false)).clicked() {
                                        dismiss = true;
                                    }
                                    ui.add_space(6.0);
                                    // Open release page in browser
                                    if ui.add(egui::Button::new(egui::RichText::new(" Release notes ").size(11.0).color(col_muted))
                                        .frame(false)).clicked() {
                                        #[cfg(target_os = "windows")]
                                        { use std::os::windows::process::CommandExt;
                                          let _ = std::process::Command::new("cmd")
                                            .args(["/c","start","https://github.com/Jestag-CryptoJarcc/Zentra-Chain/releases/latest"])
                                            .creation_flags(0x08000000).spawn(); }
                                        #[cfg(not(target_os = "windows"))]
                                        { let _ = std::process::Command::new("xdg-open")
                                            .arg("https://github.com/Jestag-CryptoJarcc/Zentra-Chain/releases/latest").spawn(); }
                                    }
                                    ui.add_space(6.0);
                                    if !downloading {
                                        if ui.add(egui::Button::new(
                                            egui::RichText::new("  ⭳  Update & Restart  ").size(11.5).color(egui::Color32::WHITE).strong()
                                        ).fill(col_accent).rounding(6.0)).clicked() {
                                            do_update = true;
                                        }
                                    }
                                });
                            });
                        });
                    if do_update { self.start_self_update(); }
                    if dismiss { self.update_available = None; self.update_status = None; }
                }

                // Notification banner
                if let Some((msg, is_err, _)) = &self.notification {
                    let (bg, fc) = if *is_err {
                        (egui::Color32::from_rgba_unmultiplied(160, 40, 40, 45), col_red)
                    } else {
                        (egui::Color32::from_rgba_unmultiplied(50, 140, 90, 40), col_green)
                    };
                    let mut close = false;
                    egui::Frame::none()
                        .fill(bg)
                        .stroke(egui::Stroke::new(1.0, fc))
                        .inner_margin(egui::vec2(14.0, 7.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(msg.as_str()).size(12.0).color(col_text));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.add(egui::Button::new(
                                        egui::RichText::new("✕").size(10.0).color(col_muted)
                                    ).frame(false)).clicked() { close = true; }
                                });
                            });
                        });
                    if close { self.notification = None; }
                }

                // First-time wallet setup
                if !wallet_ready && self.current_tab != Tab::Console {
                    setup_screen(ui, &mut self.restore_input, &mut self.mnemonic, &mut self.address,
                        col_surface, col_border, col_text, col_muted, col_green, col_red, col_accent);
                    return;
                }

                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    egui::Frame::none().inner_margin(egui::vec2(20.0, 16.0)).show(ui, |ui| {
                        match self.current_tab {
                            Tab::Overview => tab_overview(
                                ui, height, ztr_bal, &self.address, &recent_blocks,
                                &pending_txs,
                                is_mining, hashrate, mined_blks, mempool, amm_ztr, amm_zusd,
                                connected, col_surface, col_surface2, col_border,
                                col_text, col_muted, col_faint,
                                col_green, col_amber, col_blue, col_red, col_accent,
                                &mut self.current_tab,
                                &mut self.tx_detail,
                            ),
                            Tab::Send => tab_send(
                                ui, &mut self.send_to, &mut self.send_amount,
                                &mut self.send_label, &mut self.send_asset,
                                &mut self.fee_priority,
                                ztr_bal, &self.mnemonic,
                                col_surface, col_border, col_text, col_muted,
                                col_green, col_amber, col_red, col_accent,
                                &mut self.notification,
                            ),
                            Tab::Receive => tab_receive(
                                ui, &self.address, &self.mnemonic,
                                col_surface, col_surface2, col_border, col_text, col_muted,
                                col_green, col_amber, col_accent,
                                &mut self.notification,
                            ),
                            Tab::Transactions => tab_transactions(
                                ui, &recent_blocks, &pending_txs, &self.address,
                                height,
                                col_surface, col_surface2, col_border,
                                col_text, col_muted, col_green, col_red, col_amber, col_accent,
                                &mut self.tx_detail,
                            ),
                            Tab::Mining => tab_mining(
                                ui, is_mining, mining_lane, &mining_addr, hashrate, hashes,
                                mined_blks, difficulty, target_ms, net_hash,
                                &mut self.mining_threads_local, max_threads, height,
                                block_reward, halving_blocks, halving_days,
                                &self.address, &self.daemon,
                                &self.cpu_info,
                                &mut self.pool_mining,
                                &pool_addr, pool_pending, pool_miners, pool_payout_ms,
                                pool_paid, pool_share,
                                col_surface, col_surface2, col_border,
                                col_text, col_muted,
                                col_green, col_amber, col_blue, col_red,
                                &mut self.notification,
                            ),
                            Tab::Console => tab_console(
                                ui, &mut self.console_input, &mut self.console_history,
                                &self.mnemonic, ctx,
                                col_surface, col_border, col_text, col_muted, col_green, col_blue,
                            ),
                            Tab::Network => tab_network(
                                ui, height, tips, &tip_hash, mempool, &peers, proto_ver, &net_name,
                                &mut self.peer_input,
                                col_surface, col_surface2, col_border,
                                col_text, col_muted, col_faint,
                                col_green, col_amber, col_blue,
                                &mut self.notification,
                            ),
                            Tab::Theme => tab_theme(
                                ui,
                                &mut self.theme,
                                &mut self.theme_tab,
                                &mut self.theme_custom_name,
                                &mut self.theme_harmony,
                                &mut self.theme_harmony_accent,
                                &mut self.theme_dark_mode,
                                &mut self.theme_export_buf,
                                &mut self.theme_import_buf,
                                col_surface, col_surface2, col_border,
                                col_text, col_muted, col_accent,
                                &mut self.notification,
                            ),
                        }
                    });
                });
            });
    }
}

// ─── Wallet setup screen ──────────────────────────────────────────────────────

fn setup_screen(
    ui: &mut egui::Ui,
    restore_input: &mut String,
    mnemonic: &mut String,
    address: &mut String,
    col_surface: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    col_red: egui::Color32,
    col_accent: egui::Color32,
) {
    egui::Frame::none().inner_margin(egui::vec2(40.0, 40.0)).show(ui, |ui| {
        ui.label(egui::RichText::new("Wallet Setup").size(20.0).strong().color(col_text));
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Create a new wallet or restore from a 24-word BIP-39 seed phrase.").size(13.0).color(col_muted));
        ui.add_space(24.0);

        group(ui, col_surface, col_border, |ui| {
            ui.label(egui::RichText::new("Create New Wallet").strong().size(13.5).color(col_text));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Generates a fresh HD wallet. Write down your seed phrase before closing!").size(12.0).color(col_muted));
            ui.add_space(10.0);
            if ui.add(egui::Button::new(egui::RichText::new("  Create New Wallet  ").color(egui::Color32::WHITE)).fill(col_accent)).clicked() {
                match call_rpc("generateMnemonic", json!([])) {
                    Ok(r) => {
                        let phrase = r.as_str().unwrap_or("").to_string();
                        std::fs::write(data_dir().join("wallet_mnemonic.txt"), &phrase).ok();
                        if let Ok(av) = call_rpc("deriveAddress", json!([phrase.clone()])) {
                            *address = av.as_str().unwrap_or("").to_string();
                        }
                        *mnemonic = phrase;
                    }
                    Err(e) => eprintln!("generateMnemonic: {}", e),
                }
            }
        });

        ui.add_space(14.0);

        group(ui, col_surface, col_border, |ui| {
            ui.label(egui::RichText::new("Restore Existing Wallet").strong().size(13.5).color(col_text));
            ui.add_space(4.0);
            ui.label(egui::RichText::new("Enter your 24-word mnemonic phrase (space-separated).").size(12.0).color(col_muted));
            ui.add_space(10.0);
            ui.add(egui::TextEdit::multiline(restore_input)
                .hint_text("word1 word2 word3 … word24")
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Monospace));
            ui.add_space(8.0);
            let count = restore_input.split_whitespace().count();
            let c = if count == 24 { col_green } else { col_red };
            ui.label(egui::RichText::new(format!("{} / 24 words", count)).size(11.0).color(c));
            ui.add_space(6.0);
            let ready = count == 24;
            let btn_fill = if ready { col_accent } else { egui::Color32::from_rgb(50, 50, 50) };
            if ui.add(egui::Button::new(egui::RichText::new("  Restore Wallet  ").color(egui::Color32::WHITE)).fill(btn_fill)).clicked() && ready {
                let phrase = restore_input.trim().to_string();
                std::fs::write(data_dir().join("wallet_mnemonic.txt"), &phrase).ok();
                match call_rpc("deriveAddress", json!([phrase.clone()])) {
                    Ok(r) => {
                        *address = r.as_str().unwrap_or("").to_string();
                        *mnemonic = phrase;
                        restore_input.clear();
                    }
                    Err(e) => eprintln!("deriveAddress: {}", e),
                }
            }
        });
    });
}

// ─── Overview Tab (Bitcoin Core two-column layout) ───────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_overview(
    ui: &mut egui::Ui,
    height: u64,
    ztr_bal: f64,
    address: &str,
    recent_blocks: &[serde_json::Value],
    pending_txs: &[serde_json::Value],
    is_mining: bool,
    hashrate: f64,
    mined_blks: u64,
    mempool: usize,
    amm_ztr: f64,
    amm_zusd: f64,
    connected: bool,
    col_surface: egui::Color32,
    col_surface2: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    _col_faint: egui::Color32,
    col_green: egui::Color32,
    col_amber: egui::Color32,
    col_blue: egui::Color32,
    col_red: egui::Color32,
    col_accent: egui::Color32,
    current_tab: &mut Tab,
    tx_detail: &mut Option<TxDetail>,
) {
    // Warning strip when offline
    if !connected {
        egui::Frame::none()
            .fill(egui::Color32::from_rgba_unmultiplied(210, 162, 62, 32))
            .stroke(egui::Stroke::new(1.0, col_amber))
            .rounding(8.0)
            .inner_margin(egui::vec2(14.0, 9.0))
            .show(ui, |ui| {
                ui.label(egui::RichText::new("⚠  Not connected to the Zentra network — waiting for the node to start…").size(11.5).color(col_amber));
            });
        ui.add_space(12.0);
    }

    // ── Compute balances ──────────────────────────────────────────────────────
    let pending_delta: f64 = pending_txs.iter().map(|tx| {
        let is_sender = tx["inputs"].as_array()
            .map(|ins| ins.iter().any(|i| i["sender_address"].as_str() == Some(address)))
            .unwrap_or(false);
        if is_sender {
            let sent: f64 = tx["outputs"].as_array()
                .map(|outs| outs.iter()
                    .filter(|o| o["address"].as_str() != Some(address))
                    .map(|o| o["amount"].as_f64().unwrap_or(0.0) / 1e8).sum())
                .unwrap_or(0.0);
            -sent
        } else {
            tx["outputs"].as_array()
                .map(|outs| outs.iter()
                    .filter(|o| o["address"].as_str() == Some(address))
                    .map(|o| o["amount"].as_f64().unwrap_or(0.0) / 1e8).sum())
                .unwrap_or(0.0)
        }
    }).sum();

    const COINBASE_MATURITY: i64 = 10;
    let immature: f64 = recent_blocks.iter().map(|block| {
        let confs = block["confirmations"].as_i64().unwrap_or(999);
        if confs >= COINBASE_MATURITY { return 0.0; }
        if block["is_selected"].as_bool() == Some(false) { return 0.0; }
        block["transactions"].as_array().map(|txs| {
            txs.iter().filter(|tx| tx["type"].as_str() == Some("Coinbase"))
                .flat_map(|tx| tx["outputs"].as_array().cloned().unwrap_or_default())
                .filter(|o| o["address"].as_str() == Some(address))
                .map(|o| o["amount"].as_f64().unwrap_or(0.0) / 1e8)
                .sum::<f64>()
        }).unwrap_or(0.0)
    }).sum();
    let available = (ztr_bal - immature).max(0.0);
    let total = ztr_bal + pending_delta.max(0.0);

    // ── Hero balance card ──────────────────────────────────────────────────────
    egui::Frame::none()
        .fill(col_surface)
        .stroke(egui::Stroke::new(1.0, col_border))
        .rounding(12.0)
        .inner_margin(egui::vec2(22.0, 20.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Left: total balance
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("TOTAL BALANCE").size(10.5).color(col_muted));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(fmt_amount(total)).size(34.0).strong().color(col_text));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("ZTR").size(15.0).color(col_muted));
                    });
                });
                // Right: status pill
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if connected {
                        pill(ui, "● SYNCED", col_green, col_green);
                    } else {
                        pill(ui, "○ OFFLINE", col_amber, col_amber);
                    }
                });
            });

            ui.add_space(16.0);
            ui.add(egui::Separator::default().spacing(2.0));
            ui.add_space(14.0);

            // Sub-balances
            ui.columns(3, |cols| {
                cols[0].label(egui::RichText::new("AVAILABLE").size(9.5).color(col_muted));
                cols[0].add_space(3.0);
                cols[0].label(egui::RichText::new(format!("{} ZTR", fmt_amount(available))).size(14.0).strong().color(col_green));

                let pen_col = if pending_delta > 0.0 { col_green } else if pending_delta < 0.0 { col_red } else { col_muted };
                cols[1].label(egui::RichText::new("PENDING").size(9.5).color(col_muted));
                cols[1].add_space(3.0);
                let pen_txt = if pending_delta == 0.0 { "0.00000000 ZTR".to_string() }
                    else if pending_delta > 0.0 { format!("+{} ZTR", fmt_amount(pending_delta)) }
                    else { format!("-{} ZTR", fmt_amount(-pending_delta)) };
                cols[1].label(egui::RichText::new(pen_txt).size(14.0).strong().color(pen_col));

                cols[2].label(egui::RichText::new("IMMATURE").size(9.5).color(col_muted));
                cols[2].add_space(3.0);
                let im_col = if immature > 0.0 { col_amber } else { col_muted };
                cols[2].label(egui::RichText::new(format!("{} ZTR", fmt_amount(immature))).size(14.0).strong().color(im_col));
            });

            ui.add_space(16.0);
            // Address row with copy
            egui::Frame::none()
                .fill(col_surface2)
                .rounding(8.0)
                .inner_margin(egui::vec2(12.0, 9.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Your address").size(10.5).color(col_muted));
                        ui.add_space(8.0);
                        let short = if address.len() > 36 {
                            format!("{}…{}", &address[..20], &address[address.len()-10..])
                        } else { address.to_string() };
                        ui.label(egui::RichText::new(short).size(11.5).monospace().color(col_text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if copy_btn(ui, address, col_muted, col_accent) {}
                        });
                    });
                });
        });

    ui.add_space(14.0);

    // ── Quick actions ──────────────────────────────────────────────────────────
    ui.horizontal(|ui| {
        let bw = (ui.available_width() - 16.0) / 3.0;
        let mk = |ui: &mut egui::Ui, label: &str, fill: egui::Color32| -> bool {
            ui.add_sized(egui::vec2(bw, 38.0),
                egui::Button::new(egui::RichText::new(label).size(13.0).color(egui::Color32::WHITE).strong())
                    .fill(fill).rounding(9.0)).clicked()
        };
        if mk(ui, "➤  Send", col_accent) { *current_tab = Tab::Send; }
        ui.add_space(8.0);
        if mk(ui, "▼  Receive", col_green) { *current_tab = Tab::Receive; }
        ui.add_space(8.0);
        if mk(ui, "⛏  Mine", col_amber) { *current_tab = Tab::Mining; }
    });

    ui.add_space(14.0);

    // ── Recent transactions + network status ───────────────────────────────────
    ui.columns(2, |cols| {
        // Left: Recent transactions (clickable)
        card_titled(&mut cols[0], col_surface, col_border, col_accent, col_text, "Recent Activity", |ui| {
            struct TxItem { dir: &'static str, addr: String, amount: f64, height: u64, txid: String, confs: i64 }
            let mut txs: Vec<TxItem> = vec![];
            'outer: for block in recent_blocks.iter().rev() {
                if block["is_selected"].as_bool() == Some(false) { continue; }
                let h = block["blue_score"].as_u64().unwrap_or(0);
                let confs = block["confirmations"].as_i64().unwrap_or(0);
                if let Some(arr) = block["transactions"].as_array() {
                    for tx in arr {
                        let ttype = tx["type"].as_str().unwrap_or("");
                        let txid = tx["txid"].as_str().unwrap_or("").to_string();
                        let mut amount = 0.0_f64;
                        let mut addr = String::new();
                        let mut dir: &'static str = if ttype == "Coinbase" { "mine" } else { "out" };
                        if let Some(outs) = tx["outputs"].as_array() {
                            for out in outs {
                                let a = out["amount"].as_f64().unwrap_or(0.0) / 1e8;
                                let oa = out["address"].as_str().unwrap_or("");
                                if oa == address {
                                    amount += a;
                                    dir = if ttype == "Coinbase" { "mine" } else { "in" };
                                    addr = oa.to_string();
                                } else {
                                    amount += a;
                                    if addr.is_empty() { addr = oa.to_string(); }
                                }
                            }
                        }
                        txs.push(TxItem { dir, addr, amount, height: h, txid, confs });
                        if txs.len() >= 6 { break 'outer; }
                    }
                }
            }

            if txs.is_empty() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("No transactions yet.").size(12.0).color(col_muted));
                ui.add_space(6.0);
            } else {
                for tx in &txs {
                    let (icon, ic, dirname) = match tx.dir {
                        "in"   => ("▼", col_green, "Received"),
                        "mine" => ("⛏", col_amber, "Mined"),
                        _      => ("▲", col_red,   "Sent"),
                    };
                    let resp = ui.scope(|ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(2.0);
                            ui.label(egui::RichText::new(icon).size(12.0).color(ic));
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new(dirname).size(11.5).color(col_text));
                                let sub = if tx.addr.is_empty() { format!("block #{}", tx.height) }
                                    else if tx.addr.len() > 18 { format!("{}…", &tx.addr[..18]) } else { tx.addr.clone() };
                                ui.label(egui::RichText::new(sub).size(9.5).monospace().color(col_muted));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let (sign, ac) = match tx.dir { "out" => ("-", col_red), "mine" => ("+", col_amber), _ => ("+", col_green) };
                                ui.label(egui::RichText::new(format!("{}{} ZTR", sign, fmt_amount(tx.amount))).size(12.0).color(ac).strong());
                            });
                        });
                    }).response.interact(egui::Sense::click());
                    if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                    if resp.clicked() {
                        *tx_detail = Some(TxDetail {
                            direction: dirname.to_string(), amount: tx.amount,
                            txid: tx.txid.clone(), counterparty: tx.addr.clone(),
                            block_height: tx.height, confirmations: tx.confs,
                        });
                    }
                    ui.add_space(3.0);
                }
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
            if ui.link(egui::RichText::new("View all transactions  →").size(11.5).color(col_accent)).clicked() {
                *current_tab = Tab::Transactions;
            }
        });

        // Right: Network status
        card_titled(&mut cols[1], col_surface, col_border, col_accent, col_text, "Network", |ui| {
            egui::Grid::new("ov_net_grid").num_columns(2).spacing([12.0, 9.0]).show(ui, |ui| {
                ui.label(egui::RichText::new("Block height").size(11.5).color(col_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{}", height)).size(11.5).strong().color(col_text)); });
                ui.end_row();
                ui.label(egui::RichText::new("Mempool").size(11.5).color(col_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{} tx", mempool)).size(11.5).strong().color(col_text)); });
                ui.end_row();
                ui.label(egui::RichText::new("AMM ZTR").size(11.5).color(col_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{:.2}", amm_ztr)).size(11.5).strong().color(col_blue)); });
                ui.end_row();
                ui.label(egui::RichText::new("AMM zUSD").size(11.5).color(col_muted));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{:.2}", amm_zusd)).size(11.5).strong().color(col_blue)); });
                ui.end_row();
                if is_mining {
                    ui.label(egui::RichText::new("Hashrate").size(11.5).color(col_muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(fmt_hashrate(hashrate)).size(11.5).strong().color(col_green)); });
                    ui.end_row();
                    ui.label(egui::RichText::new("Blocks found").size(11.5).color(col_muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(format!("{}", mined_blks)).size(11.5).strong().color(col_amber)); });
                    ui.end_row();
                }
            });
        });
    });
}

// ─── Send Tab ─────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_send(
    ui: &mut egui::Ui,
    send_to: &mut String,
    send_amount: &mut String,
    send_label: &mut String,
    send_asset: &mut String,
    fee_priority: &mut u8,
    ztr_bal: f64,
    mnemonic: &str,
    col_surface: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    _col_amber: egui::Color32,
    col_red: egui::Color32,
    col_accent: egui::Color32,
    notification: &mut Option<(String, bool, f64)>,
) {
    section_title(ui, "Send Payment", "Transfer ZTR to any Zentra address.", col_accent, col_text, col_muted);

    group(ui, col_surface, col_border, |ui| {
        ui.set_max_width(560.0);

        // Asset selection
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Asset:").size(12.0).color(col_muted));
            ui.add_space(4.0);
            ui.selectable_value(send_asset, "ZTR".into(),  "  ZTR  ");
            ui.selectable_value(send_asset, "zUSD".into(), "  zUSD  ");
        });
        ui.add_space(14.0);

        // Pay To
        ui.label(egui::RichText::new("Pay To:").size(12.0).color(col_muted));
        ui.add_space(3.0);
        ui.add(egui::TextEdit::singleline(send_to)
            .hint_text("zentra1…  or  zentradev1…")
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace));
        ui.add_space(10.0);

        // Label (optional)
        ui.label(egui::RichText::new("Label (optional):").size(12.0).color(col_muted));
        ui.add_space(3.0);
        ui.add(egui::TextEdit::singleline(send_label)
            .hint_text("Note for this transaction")
            .desired_width(f32::INFINITY));
        ui.add_space(10.0);

        // Amount
        ui.label(egui::RichText::new("Amount:").size(12.0).color(col_muted));
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(send_amount).desired_width(200.0));
            ui.label(egui::RichText::new(send_asset.as_str()).size(13.0).color(col_muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(
                    egui::RichText::new("Use Max").size(11.0).color(col_green)
                ).frame(false)).clicked() {
                    *send_amount = format!("{:.4}", (ztr_bal - 0.001).max(0.0));
                }
            });
        });
        ui.add_space(12.0);

        // Fee priority selector — higher fee = higher block priority.
        // zents: low=1000 (0.00001), normal=5000, high=20000
        let fee_zents: u64 = match *fee_priority { 0 => 1000, 2 => 20000, _ => 5000 };
        ui.label(egui::RichText::new("Network Fee (priority):").size(12.0).color(col_muted));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (i, (lbl, sub)) in [("Low", "0.00001"), ("Normal", "0.00005"), ("High", "0.00020")].iter().enumerate() {
                let sel = *fee_priority == i as u8;
                let fill = if sel { col_accent } else { col_surface };
                if ui.add(egui::Button::new(
                    egui::RichText::new(format!("  {}  ", lbl)).size(12.0)
                        .color(if sel { egui::Color32::WHITE } else { col_muted }))
                    .fill(fill).stroke(egui::Stroke::new(1.0, if sel { col_accent } else { col_border }))
                    .rounding(7.0)).clicked() {
                    *fee_priority = i as u8;
                }
                ui.label(egui::RichText::new(format!("{} ZTR", sub)).size(9.5).color(col_muted));
                ui.add_space(6.0);
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("Fee: {:.8} ZTR    Balance: {:.8} ZTR", fee_zents as f64 / 1e8, ztr_bal)).size(11.0).color(col_muted));

        if !send_to.is_empty() && !send_to.starts_with("zentra") && !send_to.starts_with("zentradev") {
            ui.label(egui::RichText::new("Invalid address — must start with zentra1 or zentradev1").size(11.0).color(col_red));
        }

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(10.0);

        let val: f64 = send_amount.parse().unwrap_or(0.0);
        let addr_ok = !send_to.is_empty() && (send_to.starts_with("zentra") || send_to.starts_with("zentradev"));
        let ready = addr_ok && val > 0.0 && val <= ztr_bal;

        ui.horizontal(|ui| {
            let send_col = if ready { col_accent } else { col_border };
            if ui.add(egui::Button::new(
                egui::RichText::new("  ➤  Send Payment  ").size(13.0).color(egui::Color32::WHITE).strong()
            ).fill(send_col).rounding(8.0)).clicked() && ready {
                if send_asset == "ZTR" {
                    let zents = (val * 1e8) as u64;
                    match call_rpc("sendTransfer", json!([mnemonic, send_to.trim(), zents, fee_zents])) {
                        Ok(r) => {
                            *notification = Some((format!("Sent! TxID: {} (in mempool, awaiting confirmation)", r.as_str().unwrap_or("")), false, 8.0));
                            send_to.clear();
                            *send_amount = "1.0".into();
                        }
                        Err(e) => *notification = Some((format!("Error: {}", e), true, 8.0)),
                    }
                } else {
                    let micro = (val * 1e6) as u64;
                    match call_rpc("vaultWithdraw", json!([send_to.trim(), micro])) {
                        Ok(_) => *notification = Some(("zUSD bridge exit successful.".into(), false, 6.0)),
                        Err(e) => *notification = Some((format!("Error: {}", e), true, 8.0)),
                    }
                }
            }
            ui.add_space(8.0);
            if ui.button("  Clear All  ").clicked() {
                send_to.clear();
                send_label.clear();
                *send_amount = "1.0".into();
            }
        });
    });
}

// ─── Receive Tab ──────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_receive(
    ui: &mut egui::Ui,
    address: &str,
    mnemonic: &str,
    col_surface: egui::Color32,
    col_surface2: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    col_amber: egui::Color32,
    col_accent: egui::Color32,
    notification: &mut Option<(String, bool, f64)>,
) {
    section_title(ui, "Receive", "Share this address to receive ZTR.", col_accent, col_text, col_muted);

    // Receiving address
    group(ui, col_surface, col_border, |ui| {
        ui.set_max_width(640.0);
        ui.label(egui::RichText::new("Your receiving address").size(13.0).strong().color(col_text));
        ui.add_space(10.0);
        let mut addr_buf = address.to_string();
        egui::Frame::none()
            .fill(col_surface2)
            .stroke(egui::Stroke::new(1.5, col_green))
            .rounding(8.0)
            .inner_margin(egui::vec2(14.0, 12.0))
            .show(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut addr_buf)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .frame(false));
            });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(egui::RichText::new("  ⧉  Copy Address  ").color(egui::Color32::WHITE).strong())
                .fill(col_green).rounding(8.0)).clicked() {
                ui.output_mut(|o| o.copied_text = address.to_string());
                *notification = Some(("Address copied to clipboard.".into(), false, 4.0));
            }
        });
    });

    ui.add_space(14.0);

    // Seed backup is intentionally NOT shown here for security.
    let _ = mnemonic;
    group(ui, col_surface, col_border, |ui| {
        ui.set_max_width(600.0);
        ui.label(egui::RichText::new("🔒  Seed phrase").size(13.0).strong().color(col_text));
        ui.add_space(4.0);
        ui.label(egui::RichText::new("For your safety, your recovery seed is not displayed here. Back it up from the menu — you'll get a warning before it's revealed.").size(11.5).color(col_muted));
        ui.add_space(10.0);
        ui.label(egui::RichText::new("File  ▸  Show / Backup Seed Phrase…").size(11.5).monospace().color(col_amber));
    });
}

// ─── Transactions Tab ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_transactions(
    ui: &mut egui::Ui,
    recent_blocks: &[serde_json::Value],
    pending_txs: &[serde_json::Value],
    address: &str,
    current_height: u64,
    col_surface: egui::Color32,
    col_surface2: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    col_red: egui::Color32,
    col_amber: egui::Color32,
    col_accent: egui::Color32,
    tx_detail: &mut Option<TxDetail>,
) {
    section_title(ui, "Transactions", "Click any row to view full details and copy.", col_accent, col_text, col_muted);

    struct TxRow {
        height: u64,
        confirmations: i64, // -1 = pending
        txid: String,
        direction: &'static str,
        amount: f64,
        counterparty: String,
    }

    // Helper: determine direction and amount for a tx relative to our address
    let parse_tx = |tx: &serde_json::Value, blk_height: u64, confs: i64| -> Option<TxRow> {
        let txid = tx["txid"].as_str().unwrap_or("-").to_string();
        let tx_type = tx["type"].as_str().unwrap_or("");
        let is_coinbase = tx_type == "Coinbase";

        if is_coinbase {
            // Show only if reward goes to our address
            let reward: f64 = tx["outputs"].as_array()
                .map(|outs| outs.iter()
                    .filter(|o| o["address"].as_str() == Some(address))
                    .map(|o| o["amount"].as_f64().unwrap_or(0.0) / 1e8)
                    .sum())
                .unwrap_or(0.0);
            if reward > 0.0 {
                return Some(TxRow { height: blk_height, confirmations: confs, txid, direction: "Mined", amount: reward, counterparty: String::new() });
            }
            return None;
        }

        // Check if we are sender via inputs
        let is_sender = tx["inputs"].as_array()
            .map(|ins| ins.iter().any(|i| i["sender_address"].as_str() == Some(address)))
            .unwrap_or(false);

        if is_sender {
            // Amount = what left us (non-change outputs)
            let sent: f64 = tx["outputs"].as_array()
                .map(|outs| outs.iter()
                    .filter(|o| o["address"].as_str() != Some(address))
                    .map(|o| o["amount"].as_f64().unwrap_or(0.0) / 1e8)
                    .sum())
                .unwrap_or(0.0);
            let dest = tx["outputs"].as_array()
                .and_then(|outs| outs.iter().find(|o| o["address"].as_str() != Some(address)))
                .and_then(|o| o["address"].as_str()).unwrap_or("").to_string();
            return Some(TxRow { height: blk_height, confirmations: confs, txid, direction: "Sent to", amount: sent, counterparty: dest });
        }

        // Check if we are receiver
        let received: f64 = tx["outputs"].as_array()
            .map(|outs| outs.iter()
                .filter(|o| o["address"].as_str() == Some(address))
                .map(|o| o["amount"].as_f64().unwrap_or(0.0) / 1e8)
                .sum())
            .unwrap_or(0.0);
        if received > 0.0 {
            return Some(TxRow { height: blk_height, confirmations: confs, txid, direction: "Received with", amount: received, counterparty: address.to_string() });
        }

        None
    };

    let mut rows: Vec<TxRow> = vec![];

    // Pending (mempool) rows first — only show ones that involve our address
    for tx in pending_txs {
        if let Some(row) = parse_tx(tx, 0, -1) {
            rows.push(row);
        }
    }

    // Confirmed rows (newest first)
    for block in recent_blocks.iter().rev() {
        if block["is_selected"].as_bool() == Some(false) { continue; }
        let blk_h = block["blue_score"].as_u64().unwrap_or(0);
        let confs = block["confirmations"].as_i64().unwrap_or(
            (current_height.saturating_sub(blk_h) + 1) as i64
        );
        if let Some(txs) = block["transactions"].as_array() {
            for tx in txs {
                if let Some(row) = parse_tx(tx, blk_h, confs) {
                    rows.push(row);
                }
            }
        }
    }

    group(ui, col_surface, col_border, |ui| {
        let pending_count = rows.iter().filter(|r| r.confirmations < 0).count();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{} transactions", rows.len())).size(12.0).color(col_muted));
            if pending_count > 0 {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(format!("({} unconfirmed)", pending_count)).size(12.0).color(col_amber));
            }
        });
        ui.add_space(8.0);

        // Column headers
        ui.horizontal(|ui| {
            tcell_h(ui, "Status",         100.0, col_muted);
            tcell_h(ui, "Block",           75.0, col_muted);
            tcell_h(ui, "Type",           120.0, col_muted);
            tcell_h(ui, "TxID",           230.0, col_muted);
            tcell_h(ui, "Amount",         130.0, col_muted);
        });
        ui.add(egui::Separator::default().spacing(4.0));

        if rows.is_empty() {
            ui.add_space(16.0);
            ui.label(egui::RichText::new("No transactions yet — start mining or send/receive ZTR.").size(12.0).color(col_muted));
        } else {
            egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                for (i, row) in rows.iter().enumerate() {
                    let row_bg = if i % 2 == 0 { egui::Color32::TRANSPARENT } else { col_surface2 };
                    let resp = egui::Frame::none().fill(row_bg).rounding(5.0).inner_margin(egui::vec2(4.0, 5.0)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // Status column
                            let (status_txt, status_col) = if row.confirmations < 0 {
                                ("● Unconfirmed".to_string(), col_amber)
                            } else if row.confirmations < 6 {
                                (format!("◐ {} conf", row.confirmations), col_amber)
                            } else {
                                ("● Confirmed".to_string(), col_green)
                            };
                            tcell(ui, &status_txt, 110.0, status_col);

                            // Block height
                            let blk_txt = if row.confirmations < 0 { "—".to_string() } else { format!("#{}", row.height) };
                            tcell(ui, &blk_txt, 70.0, col_muted);

                            // Direction type
                            let dir_col = match row.direction {
                                "Received with" => col_green,
                                "Mined"         => col_amber,
                                "Sent to"       => col_red,
                                _               => col_muted,
                            };
                            tcell(ui, row.direction, 115.0, dir_col);

                            // TxID (truncated) + copy button
                            let txid_s = if row.txid.len() > 20 {
                                format!("{}…{}", &row.txid[..10], &row.txid[row.txid.len()-8..])
                            } else { row.txid.clone() };
                            ui.add_sized(egui::vec2(195.0, 20.0),
                                egui::Label::new(egui::RichText::new(&txid_s).size(11.5).monospace().color(col_muted)));
                            copy_btn(ui, &row.txid, col_muted, col_accent);

                            // Amount
                            let (sign, ac) = match row.direction {
                                "Sent to"  => ("-", col_red),
                                "Mined"    => ("+", col_amber),
                                _          => ("+", col_green),
                            };
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new(format!("{}{} ZTR", sign, fmt_amount(row.amount))).size(11.5).strong().color(ac));
                            });
                        });
                    }).response.interact(egui::Sense::click());
                    if resp.hovered() { ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand); }
                    if resp.clicked() {
                        *tx_detail = Some(TxDetail {
                            direction: row.direction.to_string(), amount: row.amount,
                            txid: row.txid.clone(), counterparty: row.counterparty.clone(),
                            block_height: row.height, confirmations: row.confirmations,
                        });
                    }
                }
            });
        }
    });
}

// ─── Mining Tab ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_mining(
    ui: &mut egui::Ui,
    is_mining: bool,
    mining_lane: u8,
    mining_addr: &str,
    hashrate: f64,
    hashes: u64,
    mined_blks: u64,
    difficulty: f64,
    target_ms: u64,
    net_hash: f64,
    mining_threads_local: &mut u32,
    max_threads: u32,
    height: u64,
    block_reward: f64,
    halving_blocks: u64,
    halving_days: f64,
    payout_address: &str,
    daemon: &Arc<Mutex<Option<Child>>>,
    cpu_info: &CpuInfo,
    pool_mining: &mut bool,
    pool_addr: &str,
    pool_pending_zents: u64,
    pool_active_miners: usize,
    pool_payout_ms: u64,
    pool_my_paid_zents: u64,
    pool_my_share_pct: f64,
    col_surface: egui::Color32,
    col_surface2: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    col_amber: egui::Color32,
    col_blue: egui::Color32,
    col_red: egui::Color32,
    notification: &mut Option<(String, bool, f64)>,
) {
    // ── Header row: title + status badge ──────────────────────────────────────
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Mining").size(20.0).strong().color(col_text));
        ui.add_space(10.0);
        if is_mining {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(52, 211, 153, 22))
                .stroke(egui::Stroke::new(1.0, col_green)).rounding(12.0)
                .inner_margin(egui::vec2(10.0, 3.0)).show(ui, |ui| {
                ui.label(egui::RichText::new("● ACTIVE").size(11.0).strong().color(col_green));
            });
        } else {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(100,100,100,14))
                .stroke(egui::Stroke::new(1.0, col_border)).rounding(12.0)
                .inner_margin(egui::vec2(10.0, 3.0)).show(ui, |ui| {
                ui.label(egui::RichText::new("○ STOPPED").size(11.0).color(col_muted));
            });
        }
    });
    ui.add_space(14.0);

    // Stats grid (combined network hashrate from all P2P peers)
    let _ = (mining_lane, mining_addr); // used in mode panel below
    ui.columns(4, |cols| {
        stat_box(&mut cols[0], col_surface, col_border, "My Hashrate",   &fmt_hashrate(hashrate), col_green);
        stat_box(&mut cols[1], col_surface, col_border, "Network Total", &fmt_hashrate(net_hash), col_blue);
        let share = if net_hash > 0.0 { hashrate / net_hash * 100.0 } else if is_mining { 100.0 } else { 0.0 };
        stat_box(&mut cols[2], col_surface, col_border, "My Share",      &format!("{:.2}%", share), col_amber);
        stat_box(&mut cols[3], col_surface, col_border, "Blocks Found",  &format!("{}", mined_blks), col_amber);
    });
    ui.add_space(8.0);
    ui.columns(4, |cols| {
        stat_box(&mut cols[0], col_surface, col_border, "Height",       &format!("{}", height),            col_muted);
        stat_box(&mut cols[1], col_surface, col_border, "Difficulty",   &fmt_difficulty(difficulty),       col_muted);
        stat_box(&mut cols[2], col_surface, col_border, "Block Reward", &format!("{:.1} ZTR", block_reward), col_blue);
        let hs = if halving_days > 0.0 { format!("{:.1} days", halving_days) } else { "—".into() };
        stat_box(&mut cols[3], col_surface, col_border, "Next Halving", &hs,                               col_amber);
    });
    ui.add_space(14.0);

    // ── Hardware card ──────────────────────────────────────────────────────────
    card_titled(ui, col_surface, col_border, col_blue, col_text, "Hardware", |ui| {
        ui.label(egui::RichText::new(&cpu_info.brand).size(12.5).color(col_text));
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            kv_inline(ui, "Physical cores", &format!("{}", cpu_info.physical_cores), col_muted, col_blue);
            ui.add_space(24.0);
            kv_inline(ui, "Logical threads", &format!("{}", cpu_info.logical_cores), col_muted, col_blue);
            ui.add_space(24.0);
            kv_inline(ui, "Base frequency", &format!("{:.2} GHz", cpu_info.freq_ghz), col_muted, col_blue);
        });
    });
    ui.add_space(12.0);

    // ── Mining mode card (Solo / Pool) ──────────────────────────────────────────
    card_titled(ui, col_surface, col_border, col_amber, col_text, "Mining Mode", |ui| {
        ui.horizontal(|ui| {
            let solo_sel = !*pool_mining;
            let pool_sel = *pool_mining;
            let bw = (ui.available_width() - 8.0) / 2.0;
            if ui.add_sized(egui::vec2(bw, 38.0), egui::Button::new(
                egui::RichText::new("⛏  Solo Mining").size(13.0)
                    .color(if solo_sel { egui::Color32::WHITE } else { col_muted }))
                .fill(if solo_sel { egui::Color32::from_rgb(18, 90, 58) } else { col_surface2 })
                .stroke(egui::Stroke::new(if solo_sel {1.5} else {1.0}, if solo_sel { col_green } else { col_border }))
                .rounding(8.0)).clicked() { *pool_mining = false; }
            ui.add_space(8.0);
            if ui.add_sized(egui::vec2(bw, 38.0), egui::Button::new(
                egui::RichText::new("⇄  Pool Mining").size(13.0)
                    .color(if pool_sel { egui::Color32::WHITE } else { col_muted }))
                .fill(if pool_sel { egui::Color32::from_rgb(115, 80, 18) } else { col_surface2 })
                .stroke(egui::Stroke::new(if pool_sel {1.5} else {1.0}, if pool_sel { col_amber } else { col_border }))
                .rounding(8.0)).clicked() { *pool_mining = true; }
        });
        ui.add_space(12.0);
        if *pool_mining {
            ui.label(egui::RichText::new("Rewards collect in the pool wallet and pay back every 30 min, proportional to your hashrate (1% operator fee).").size(11.0).color(col_muted));
            if !pool_addr.is_empty() {
                ui.add_space(10.0);
                egui::Grid::new("pool_grid").num_columns(2).spacing([18.0, 7.0]).show(ui, |ui| {
                    ui.label(egui::RichText::new("Pool address").size(11.5).color(col_muted));
                    ui.horizontal(|ui| {
                        let short = if pool_addr.len() > 36 { format!("{}…{}", &pool_addr[..20], &pool_addr[pool_addr.len()-10..]) } else { pool_addr.to_string() };
                        ui.label(egui::RichText::new(short).size(11.0).monospace().color(col_amber));
                        copy_btn(ui, pool_addr, col_muted, col_amber);
                    });
                    ui.end_row();
                    ui.label(egui::RichText::new("Active miners").size(11.5).color(col_muted));
                    ui.label(egui::RichText::new(format!("{}", pool_active_miners)).size(11.5).strong().color(col_blue)); ui.end_row();
                    ui.label(egui::RichText::new("Your share").size(11.5).color(col_muted));
                    ui.label(egui::RichText::new(format!("{:.2}%", pool_my_share_pct)).size(11.5).strong().color(col_green)); ui.end_row();
                    ui.label(egui::RichText::new("Total earned").size(11.5).color(col_muted));
                    ui.label(egui::RichText::new(format!("{} ZTR", fmt_amount(pool_my_paid_zents as f64 / 1e8))).size(11.5).strong().color(col_green)); ui.end_row();
                    ui.label(egui::RichText::new("Pool balance").size(11.5).color(col_muted));
                    ui.label(egui::RichText::new(format!("{} ZTR", fmt_amount(pool_pending_zents as f64 / 1e8))).size(11.5).color(col_text)); ui.end_row();
                    if pool_payout_ms > 0 {
                        let secs = pool_payout_ms / 1000;
                        ui.label(egui::RichText::new("Next payout").size(11.5).color(col_muted));
                        ui.label(egui::RichText::new(format!("{}m {}s", secs/60, secs%60)).size(11.5).color(col_amber)); ui.end_row();
                    }
                });
            }
        } else {
            ui.label(egui::RichText::new("Every block you find pays its full reward directly to your wallet. Higher variance — you only earn when you personally solve a block.").size(11.0).color(col_muted));
        }
    });
    ui.add_space(12.0);

    // ── CPU Miner control card ──────────────────────────────────────────────────
    card_titled(ui, col_surface, col_border, col_green, col_text,
        if is_mining { "CPU Miner — Running" } else { "CPU Miner" }, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("Lane 0 · {} · {}",
                    if *pool_mining { "Pool" } else { "Solo" }, cpu_info.brand)).size(11.5).color(col_text));
                ui.label(egui::RichText::new(format!("Hashes: {}   ·   Target: {} ms/block",
                    fmt_big_num(hashes), target_ms)).size(10.5).color(col_muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_mining {
                    if ui.add(egui::Button::new(egui::RichText::new("  ■  Stop  ").size(13.0).color(egui::Color32::WHITE).strong())
                        .fill(egui::Color32::from_rgb(150, 40, 40)).rounding(8.0)).clicked() {
                        let _ = call_rpc("stopMining", json!([]));
                        if *pool_mining { let _ = call_rpc("poolSetMode", json!([false])); }
                        *notification = Some(("Mining stopped.".into(), false, 4.0));
                    }
                } else {
                    if ui.add(egui::Button::new(egui::RichText::new("  ▶  Start Mining  ").size(13.0).color(egui::Color32::WHITE).strong())
                        .fill(egui::Color32::from_rgb(20, 110, 70)).rounding(8.0)).clicked() {
                        if daemon.lock().unwrap().is_none() {
                            *daemon.lock().unwrap() = spawn_daemon();
                            std::thread::sleep(Duration::from_millis(1500));
                        }
                        if *pool_mining {
                            // Join as a POOL MEMBER: our node mines into the operator's
                            // shared pool wallet (learned from the seed over P2P) and the
                            // operator credits our payout address. We do NOT become our
                            // own operator, so every pool miner feeds the one VPS pool.
                            let _ = call_rpc("poolJoin", json!([payout_address]));
                            match call_rpc("startMining", json!([0u8, payout_address])) {
                                Ok(_)  => *notification = Some(("Pool mining started — earning shares.".into(), false, 4.0)),
                                Err(e) => *notification = Some((format!("Error: {}", e), true, 6.0)),
                            }
                        } else {
                            let _ = call_rpc("poolSetMode", json!([false]));
                            match call_rpc("startMining", json!([0u8, payout_address])) {
                                Ok(_)  => *notification = Some(("Solo mining started.".into(), false, 4.0)),
                                Err(e) => *notification = Some((format!("Error: {}", e), true, 6.0)),
                            }
                        }
                    }
                }
            });
        });

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(12.0);

        // Thread/core slider — full-width, value shown in the label (no overflow).
        let effective_max = max_threads.max(1);
        if *mining_threads_local < 1 { *mining_threads_local = 1; }
        if *mining_threads_local > effective_max { *mining_threads_local = effective_max; }
        ui.label(egui::RichText::new(
            format!("CPU Cores:  {} of {} physical cores", mining_threads_local, cpu_info.physical_cores)
        ).size(12.5).color(col_text));
        ui.add_space(8.0);
        let resp = ui.add(egui::Slider::new(mining_threads_local, 1..=effective_max).step_by(1.0).show_value(false));
        if resp.changed() {
            if let Err(e) = call_rpc("setMiningThreads", json!([*mining_threads_local as u8])) {
                *notification = Some((format!("Failed to set cores: {}", e), true, 5.0));
            }
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new("1 thread per physical core — no hyperthreading. Takes effect on the next block.").size(10.5).color(col_muted));
    });
}

// ─── AMM/Vault moved to web — see http://localhost:16112/dex and /vault ──────

#[allow(clippy::too_many_arguments)]
fn tab_amm(
    ui: &mut egui::Ui,
    swap_amount: &mut String,
    swap_from: &mut String,
    amm_ztr: f64,
    amm_zusd: f64,
    amm_lp_burned: f64,
    col_surface: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    col_amber: egui::Color32,
    col_blue: egui::Color32,
    col_accent: egui::Color32,
    notification: &mut Option<(String, bool, f64)>,
) {
    ui.label(egui::RichText::new("AMM Swap").size(18.0).strong().color(col_text));
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Constant-product liquidity pool. LP tokens are permanently burned.").size(12.0).color(col_muted));
    ui.add_space(14.0);

    ui.columns(3, |cols| {
        stat_box(&mut cols[0], col_surface, col_border, "Pool ZTR",   &format!("{:.4} ZTR", amm_ztr),  col_green);
        stat_box(&mut cols[1], col_surface, col_border, "Pool zUSD",  &format!("{:.4} zUSD", amm_zusd), col_blue);
        stat_box(&mut cols[2], col_surface, col_border, "LP Burned",  &format!("{:.4}", amm_lp_burned), col_amber);
    });
    ui.add_space(14.0);

    group(ui, col_surface, col_border, |ui| {
        ui.set_max_width(460.0);
        ui.label(egui::RichText::new("Swap Tokens").size(13.0).strong().color(col_text));
        ui.add_space(10.0);

        ui.label(egui::RichText::new("Direction:").size(12.0).color(col_muted));
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.selectable_value(swap_from, "ZTR".into(),  "  ZTR → zUSD  ");
            ui.selectable_value(swap_from, "zUSD".into(), "  zUSD → ZTR  ");
        });
        ui.add_space(10.0);

        ui.label(egui::RichText::new("Input Amount:").size(12.0).color(col_muted));
        ui.add_space(3.0);
        ui.add(egui::TextEdit::singleline(swap_amount).desired_width(180.0));
        ui.add_space(6.0);

        let val: f64 = swap_amount.parse().unwrap_or(0.0);
        let is_ztr = swap_from == "ZTR";
        let quote = if val > 0.0 && amm_ztr > 0.0 && amm_zusd > 0.0 {
            let fa = val * 0.998;
            if is_ztr { (amm_zusd * fa) / (amm_ztr + fa) }
            else      { (amm_ztr  * fa) / (amm_zusd + fa) }
        } else { 0.0 };
        let out_tick = if is_ztr { "zUSD" } else { "ZTR" };
        ui.label(egui::RichText::new(format!("Estimated output: {:.6} {}  (0.2% fee)", quote, out_tick)).size(11.5).color(col_muted));
        ui.add_space(14.0);

        if ui.add(egui::Button::new(egui::RichText::new("  Execute Swap  ").color(egui::Color32::WHITE).strong()).fill(col_accent)).clicked() {
            if val <= 0.0 {
                *notification = Some(("Enter a positive amount.".into(), true, 4.0));
            } else {
                let amount_in = if is_ztr { (val * 1e8) as u64 } else { (val * 1e6) as u64 };
                match call_rpc("swapTokens", json!([swap_from.as_str(), amount_in, 0u64])) {
                    Ok(r) => {
                        let out = r["amount_out"].as_u64().unwrap_or(0);
                        let disp = if is_ztr { format!("{:.4} zUSD", out as f64 / 1e6) } else { format!("{:.4} ZTR", out as f64 / 1e8) };
                        *notification = Some((format!("Swap complete! Received {}", disp), false, 6.0));
                    }
                    Err(e) => *notification = Some((format!("Swap failed: {}", e), true, 6.0)),
                }
            }
        }
    });
}

// ─── Omni-Vault Tab ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_vault(
    ui: &mut egui::Ui,
    vault_tx_hash: &mut String,
    vault_amount: &mut String,
    vault_burn_amount: &mut String,
    address: &str,
    col_surface: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    _col_amber: egui::Color32,
    col_red: egui::Color32,
    col_accent: egui::Color32,
    notification: &mut Option<(String, bool, f64)>,
) {
    ui.label(egui::RichText::new("Omni-Vault").size(18.0).strong().color(col_text));
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Cross-chain TSS bridge. Deposit external stablecoins to mint zUSD, or burn to redeem.").size(12.0).color(col_muted));
    ui.add_space(14.0);

    ui.columns(2, |cols| {
        group(&mut cols[0], col_surface, col_border, |ui| {
            ui.label(egui::RichText::new("Deposit — Mint zUSD").strong().size(13.0).color(col_text));
            ui.add_space(10.0);
            ui.label(egui::RichText::new("External Tx Hash (hex):").size(12.0).color(col_muted));
            ui.add_space(3.0);
            ui.add(egui::TextEdit::singleline(vault_tx_hash).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Stablecoin Amount (USDT):").size(12.0).color(col_muted));
            ui.add_space(3.0);
            ui.add(egui::TextEdit::singleline(vault_amount).desired_width(140.0));
            ui.add_space(14.0);
            if ui.add(egui::Button::new(egui::RichText::new("  Mint zUSD  ").color(egui::Color32::WHITE).strong()).fill(col_accent)).clicked() {
                let amt: f64 = vault_amount.parse().unwrap_or(0.0);
                if amt <= 0.0 { *notification = Some(("Enter positive amount.".into(), true, 4.0)); }
                else {
                    let micro = (amt * 1e6) as u64;
                    match call_rpc("vaultDeposit", json!([vault_tx_hash.trim(), micro])) {
                        Ok(r) => {
                            let minted = r["zusd_minted"].as_u64().unwrap_or(0) as f64 / 1e6;
                            *notification = Some((format!("Minted {:.2} zUSD via TSS ingest.", minted), false, 6.0));
                        }
                        Err(e) => *notification = Some((format!("Deposit failed: {}", e), true, 6.0)),
                    }
                }
            }
        });

        group(&mut cols[1], col_surface, col_border, |ui| {
            ui.label(egui::RichText::new("Exit Bridge — Burn zUSD").strong().size(13.0).color(col_text));
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Burn Amount (zUSD):").size(12.0).color(col_muted));
            ui.add_space(3.0);
            ui.add(egui::TextEdit::singleline(vault_burn_amount).desired_width(140.0));
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Burning zUSD triggers USDT unlock on the source chain.").size(11.0).color(col_muted));
            let br = if address.len() > 22 { format!("{}…", &address[..22]) } else { address.to_string() };
            ui.label(egui::RichText::new(format!("Bridge to: {}", br)).size(10.5).color(col_red));
            ui.add_space(14.0);
            if ui.add(egui::Button::new(egui::RichText::new("  Burn & Exit  ").color(egui::Color32::WHITE).strong())
                .fill(egui::Color32::from_rgb(140, 28, 28))).clicked() {
                let amt: f64 = vault_burn_amount.parse().unwrap_or(0.0);
                if amt <= 0.0 { *notification = Some(("Enter positive amount.".into(), true, 4.0)); }
                else {
                    let micro = (amt * 1e6) as u64;
                    match call_rpc("vaultWithdraw", json!([address, micro])) {
                        Ok(_)  => *notification = Some(("Burn successful. USDT will unlock on source chain.".into(), false, 6.0)),
                        Err(e) => *notification = Some((format!("Exit failed: {}", e), true, 6.0)),
                    }
                }
            }
            let _ = col_green;
        });
    });
}

// ─── Console Tab ──────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_console(
    ui: &mut egui::Ui,
    console_input: &mut String,
    console_history: &mut Vec<(String, String)>,
    mnemonic: &str,
    ctx: &egui::Context,
    col_surface: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_green: egui::Color32,
    col_blue: egui::Color32,
) {
    ui.label(egui::RichText::new("Debug Console").size(18.0).strong().color(col_text));
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Execute raw JSON-RPC calls against the background daemon.").size(12.0).color(col_muted));
    ui.add_space(12.0);

    egui::Frame::none()
        .fill(egui::Color32::from_rgb(14, 14, 14))
        .stroke(egui::Stroke::new(1.0, col_border))
        .rounding(4.0)
        .inner_margin(egui::vec2(12.0, 10.0))
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if console_history.is_empty() {
                        ui.label(egui::RichText::new("Type 'help' for available commands.").size(12.0).color(col_muted).monospace());
                    }
                    for (cmd, out) in console_history.iter() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("zentrad> ").size(12.0).color(col_blue).monospace().strong());
                            ui.label(egui::RichText::new(cmd).size(12.0).color(egui::Color32::WHITE).monospace().strong());
                        });
                        ui.label(egui::RichText::new(out).size(11.5).color(col_green).monospace());
                        ui.add_space(3.0);
                    }
                });
        });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(">").size(13.0).color(col_blue).monospace().strong());
        ui.add_space(4.0);
        let resp = ui.add(egui::TextEdit::singleline(console_input)
            .desired_width(ui.available_width() - 70.0)
            .font(egui::TextStyle::Monospace)
            .hint_text("help"));
        let enter = resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let run = ui.button(egui::RichText::new("Run").size(12.0));
        if run.clicked() || enter {
            let input = console_input.trim().to_string();
            if !input.is_empty() {
                let parts: Vec<String> = input.split_whitespace().map(|s| s.to_string()).collect();
                let result = match run_console_cmd(&parts[0], &parts, mnemonic) {
                    Ok(r)  => r,
                    Err(e) => format!("Error: {}", e),
                };
                console_history.push((input, result));
                console_input.clear();
                resp.request_focus();
            }
        }
    });

    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        for cmd in &["getdaginfo", "getbalance", "getmininginfo", "getminingstatus",
                     "getpoolstate", "getrecentblocks", "startmining", "stopmining",
                     "getmempool", "sendtoaddress", "help"] {
            ui.label(egui::RichText::new(*cmd).size(10.0).color(col_muted).monospace());
            ui.label(egui::RichText::new("·").size(10.0).color(egui::Color32::from_rgb(55, 55, 55)));
        }
    });
    let _ = col_surface;
}

// ─── Network Tab ──────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_network(
    ui: &mut egui::Ui,
    height: u64,
    tips: usize,
    tip_hash: &str,
    mempool: usize,
    peers: &[serde_json::Value],
    proto_ver: u64,
    net_name: &str,
    peer_input: &mut String,
    col_surface: egui::Color32,
    _col_surface2: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    _col_faint: egui::Color32,
    col_green: egui::Color32,
    col_amber: egui::Color32,
    col_blue: egui::Color32,
    notification: &mut Option<(String, bool, f64)>,
) {
    section_title(ui, "Node & Peers", "Connections, peers and the embedded web apps.", col_blue, col_text, col_muted);

    // Web Apps panel
    group(ui, col_surface, col_border, |ui| {
        ui.label(egui::RichText::new("Web Apps — open in browser").size(13.0).strong().color(col_text));
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Explorer, DEX, Vault, and Pool — opens the live site at zentrachain.xyz.").size(11.5).color(col_muted));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let open = |url: &str| {
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::process::CommandExt;
                    let _ = std::process::Command::new("cmd")
                        .args(&["/c", "start", url])
                        .creation_flags(0x08000000)
                        .spawn();
                }
                #[cfg(not(target_os = "windows"))]
                { let _ = std::process::Command::new("xdg-open").arg(url).spawn(); }
            };
            if ui.add(egui::Button::new(egui::RichText::new("  ⬡ Explorer  ").color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(60, 60, 120))).clicked() {
                open("https://zentrachain.xyz/explorer");
            }
            ui.add_space(4.0);
            if ui.add(egui::Button::new(egui::RichText::new("  ⇆ DEX  ").color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(30, 90, 100))).clicked() {
                open("https://zentrachain.xyz/dex");
            }
            ui.add_space(4.0);
            if ui.add(egui::Button::new(egui::RichText::new("  ⧖ Vault  ").color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(40, 90, 60))).clicked() {
                open("https://zentrachain.xyz/vault");
            }
            ui.add_space(4.0);
            if ui.add(egui::Button::new(egui::RichText::new("  ⛏ Pool  ").color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(110, 85, 25))).clicked() {
                open("https://zentrachain.xyz/pool");
            }
            ui.add_space(4.0);
            if ui.add(egui::Button::new(egui::RichText::new("  🏠 Home  ").color(egui::Color32::WHITE))
                .fill(egui::Color32::from_rgb(50, 50, 80))).clicked() {
                open("https://zentrachain.xyz");
            }
        });
    });
    ui.add_space(14.0);
    let _ = notification;

    ui.columns(2, |cols| {
        group(&mut cols[0], col_surface, col_border, |ui| {
            ui.label(egui::RichText::new("Node Diagnostics").size(13.0).strong().color(col_text));
            ui.add_space(8.0);
            kv(ui, "Client",          concat!("Zentra Core v", env!("CARGO_PKG_VERSION")), col_muted, col_text);
            kv(ui, "Protocol",        &format!("{}", proto_ver), col_muted, col_blue);
            kv(ui, "Network",          net_name, col_muted, col_blue);
            kv(ui, "Block Height",    &format!("{}", height), col_muted, col_green);
            kv(ui, "DAG Tips",        &format!("{}", tips), col_muted, col_green);
            kv(ui, "Mempool",         &format!("{} tx", mempool), col_muted, col_amber);
            kv(ui, "Peers",           &format!("{}", peers.len()), col_muted, col_amber);
        });
        group(&mut cols[1], col_surface, col_border, |ui| {
            ui.label(egui::RichText::new("Selected Tip").size(13.0).strong().color(col_text));
            ui.add_space(8.0);
            let th = if tip_hash.len() > 40 {
                format!("{}…{}", &tip_hash[..20], &tip_hash[tip_hash.len()-8..])
            } else { tip_hash.to_string() };
            ui.add(egui::Label::new(egui::RichText::new(&th).size(11.5).monospace().color(col_text)).wrap());
        });
    });

    ui.add_space(14.0);

    // Add peer panel
    group(ui, col_surface, col_border, |ui| {
        ui.label(egui::RichText::new("Add Peer").size(13.0).strong().color(col_text));
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Connect to another Zentra node to strengthen the network. Enter its host:port.").size(11.0).color(col_muted));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(peer_input)
                .hint_text("1.2.3.4:16110")
                .desired_width(260.0)
                .font(egui::TextStyle::Monospace));
            if ui.add(egui::Button::new(egui::RichText::new("  Add Peer  ").color(egui::Color32::WHITE)).fill(col_green)).clicked() {
                let addr = peer_input.trim().to_string();
                if addr.is_empty() {
                    *notification = Some(("Enter a peer address (host:port).".into(), true, 4.0));
                } else {
                    match call_rpc("addPeer", json!([addr])) {
                        Ok(_)  => { *notification = Some((format!("Peer {} added.", addr), false, 4.0)); peer_input.clear(); }
                        Err(e) => *notification = Some((format!("Failed: {}", e), true, 5.0)),
                    }
                }
            }
        });
    });
    ui.add_space(14.0);

    group(ui, col_surface, col_border, |ui| {
        ui.label(egui::RichText::new("Connected Peers").size(13.0).strong().color(col_text));
        ui.add_space(8.0);
        if peers.is_empty() {
            ui.label(egui::RichText::new("No peers yet. Add one above, or peers will appear as the P2P network grows.").size(12.0).color(col_muted));
        } else {
            ui.horizontal(|ui| {
                tcell_h(ui, "IP Address",  160.0, col_muted);
                tcell_h(ui, "Version",     180.0, col_muted);
                tcell_h(ui, "Ping",         70.0, col_muted);
                tcell_h(ui, "Height",       80.0, col_muted);
                tcell_h(ui, "Direction",    90.0, col_muted);
            });
            ui.add(egui::Separator::default().spacing(4.0));
            for peer in peers {
                let addr = peer["address"].as_str().unwrap_or("-");
                let ver  = peer["version"].as_str().unwrap_or("-");
                let ping = peer["ping_ms"].as_u64().unwrap_or(0);
                let h    = peer["height"].as_u64().unwrap_or(0);
                let dir  = peer["direction"].as_str().unwrap_or("-");
                ui.horizontal(|ui| {
                    tcell(ui, addr,                   160.0, col_green);
                    tcell_mono(ui, ver,               180.0, col_muted);
                    let pc = if ping < 50 { col_green } else if ping < 150 { col_amber } else { col_amber };
                    tcell(ui, &format!("{} ms", ping), 70.0, pc);
                    tcell(ui, &format!("{}", h),        80.0, col_text);
                    tcell(ui, dir,                      90.0, col_muted);
                });
            }
        }
    });
}

// ─── Logo, message signing helpers ───────────────────────────────────────────

/// Load the ZTR coin PNG (embedded at compile time) into an egui texture.
fn load_logo_texture(ctx: &egui::Context) -> egui::TextureHandle {
    const LOGO_BYTES: &[u8] = include_bytes!("../assets/coin-ztr.png");
    match image::load_from_memory(LOGO_BYTES) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            // Auto-crop transparent border so the coin fills the logo area.
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0u32, 0u32);
            for y in 0..h { for x in 0..w {
                if rgba.get_pixel(x, y)[3] > 16 {
                    if x < min_x { min_x = x; } if y < min_y { min_y = y; }
                    if x > max_x { max_x = x; } if y > max_y { max_y = y; }
                }
            }}
            let (pixels, cw, ch) = if max_x > min_x && max_y > min_y {
                let pad = ((max_x-min_x).max(max_y-min_y)/20).max(2);
                let x0 = min_x.saturating_sub(pad); let y0 = min_y.saturating_sub(pad);
                let x1 = (max_x+pad).min(w-1); let y1 = (max_y+pad).min(h-1);
                let c = image::imageops::crop_imm(&rgba, x0, y0, x1-x0+1, y1-y0+1).to_image();
                let (cw, ch) = c.dimensions();
                (c.into_raw(), cw, ch)
            } else { (rgba.into_raw(), w, h) };
            let color_img = egui::ColorImage::from_rgba_unmultiplied([cw as usize, ch as usize], &pixels);
            ctx.load_texture("ztr_logo", color_img, egui::TextureOptions::LINEAR)
        }
        Err(_) => {
            let color_img = egui::ColorImage::new([1, 1], egui::Color32::TRANSPARENT);
            ctx.load_texture("ztr_logo", color_img, egui::TextureOptions::LINEAR)
        }
    }
}

/// Infer the network from an address prefix.
fn network_from_address(addr: &str) -> zentra_types::NetworkType {
    if addr.starts_with("zentradev") { zentra_types::NetworkType::Devnet }
    else if addr.starts_with("zentratest") { zentra_types::NetworkType::Testnet }
    else { zentra_types::NetworkType::Mainnet }
}

/// Sign a message. Output = hex(pubkey[32] || signature[64]) so the verifier can
/// recompute the address (ed25519 has no pubkey recovery).
fn sign_message_with_wallet(mnemonic: &str, message: &str) -> String {
    use zentra_wallet::keygen::MasterKey;
    use ed25519_dalek::Signer;
    let master = match MasterKey::from_mnemonic(mnemonic) {
        Ok(m) => m, Err(_) => return "Error: invalid wallet".into(),
    };
    let kp = master.derive_keypair(0, 0);
    let sig = kp.signing_key().sign(message.as_bytes()).to_bytes();
    let pk = kp.public_key_bytes();
    let mut out = Vec::with_capacity(96);
    out.extend_from_slice(&pk);
    out.extend_from_slice(&sig);
    hex::encode(out)
}

/// Verify a signed message against an address.
fn verify_message_sig(address: &str, message: &str, sig_hex: &str) -> String {
    use ed25519_dalek::{Verifier, VerifyingKey, Signature};
    let bytes = match hex::decode(sig_hex.trim()) {
        Ok(b) => b, Err(_) => return "✗ Invalid signature format (expected hex)".into(),
    };
    if bytes.len() != 96 {
        return "✗ Invalid signature length".into();
    }
    let mut pk = [0u8; 32];
    pk.copy_from_slice(&bytes[..32]);
    let mut sig = [0u8; 64];
    sig.copy_from_slice(&bytes[32..]);

    // Recompute the address from the embedded pubkey and check it matches.
    let net = network_from_address(address);
    let derived = zentra_types::Address::from_public_key(&pk, net).to_string();
    if derived != address.trim() {
        return "✗ Signature does not match this address".into();
    }
    let vk = match VerifyingKey::from_bytes(&pk) {
        Ok(v) => v, Err(_) => return "✗ Invalid public key".into(),
    };
    match vk.verify(message.as_bytes(), &Signature::from_bytes(&sig)) {
        Ok(_)  => "✓ Valid signature — the message was signed by this address.".into(),
        Err(_) => "✗ Signature verification failed.".into(),
    }
}

// ─── Apply theme to egui visuals every frame ─────────────────────────────────

fn apply_theme_visuals(ctx: &egui::Context, t: &AppTheme) {
    let mut style = (*ctx.style()).clone();
    let [r,g,b] = t.bg;
    style.visuals.window_fill = egui::Color32::from_rgb(r,g,b);
    let [r,g,b] = t.toolbar;
    style.visuals.panel_fill  = egui::Color32::from_rgb(r,g,b);
    let [r,g,b] = t.text;
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(r,g,b));
    let [r,g,b] = t.surface;
    style.visuals.widgets.inactive.bg_fill  = egui::Color32::from_rgb(r,g,b);
    let [r2,g2,b2] = t.surface2;
    style.visuals.widgets.hovered.bg_fill   = egui::Color32::from_rgb(r2,g2,b2);
    style.visuals.widgets.active.bg_fill    = egui::Color32::from_rgb(
        r2.saturating_add(10), g2.saturating_add(10), b2.saturating_add(10));
    let [ar,ag,ab] = t.accent;
    style.visuals.selection.bg_fill = egui::Color32::from_rgba_unmultiplied(ar,ag,ab,55);
    style.visuals.selection.stroke  = egui::Stroke::new(1.0, egui::Color32::from_rgb(ar,ag,ab));
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(3.0);
    style.visuals.widgets.hovered.rounding  = egui::Rounding::same(3.0);
    style.visuals.widgets.active.rounding   = egui::Rounding::same(3.0);
    style.spacing.item_spacing   = t.density.item_spacing();
    style.spacing.button_padding = t.density.btn_padding();
    ctx.set_style(style);
}

// ─── Theme tab ────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn tab_theme(
    ui: &mut egui::Ui,
    theme: &mut AppTheme,
    tab: &mut ThemeTab,
    custom_name: &mut String,
    harmony: &mut Harmony,
    harmony_accent: &mut [u8; 3],
    dark_mode: &mut bool,
    export_buf: &mut String,
    import_buf: &mut String,
    col_surface: egui::Color32,
    col_surface2: egui::Color32,
    col_border: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
    col_accent: egui::Color32,
    notification: &mut Option<(String, bool, f64)>,
) {
    section_title(ui, "Theme & Appearance", "Pick a preset or craft your own colour scheme.", col_accent, col_text, col_muted);

    // Sub-tab selector
    ui.horizontal(|ui| {
        let t_style = |is: bool| {
            if is { egui::RichText::new("").strong() } else { egui::RichText::new("") }
        };
        let _ = t_style; // silence
        for (label, variant) in [("  Themes  ", ThemeTab::Presets), ("  Customize  ", ThemeTab::Customize)] {
            let sel = *tab == variant;
            let btn = egui::Button::new(
                egui::RichText::new(label).size(13.0).color(if sel { col_text } else { col_muted })
            )
            .fill(if sel { col_surface2 } else { egui::Color32::TRANSPARENT })
            .stroke(if sel { egui::Stroke::new(1.0, col_border) } else { egui::Stroke::NONE })
            .rounding(8.0);
            if ui.add(btn).clicked() { *tab = variant; }
        }
    });
    ui.add_space(16.0);

    match *tab {
        // ── PRESETS ──────────────────────────────────────────────────────────
        ThemeTab::Presets => {
            ui.label(egui::RichText::new("Default Themes").size(12.0).strong().color(col_muted));
            ui.add_space(12.0);

            let presets = AppTheme::all_presets();
            let cols = 4usize;
            let available = ui.available_width();
            let card_w = ((available - (cols as f32 - 1.0) * 12.0) / cols as f32).max(120.0);

            egui::Grid::new("theme_grid")
                .num_columns(cols)
                .spacing([12.0, 12.0])
                .show(ui, |ui| {
                    for (i, preset) in presets.iter().enumerate() {
                        let is_active = preset.name == theme.name;

                        // Draw a theme card using painter
                        let (card_rect, resp) = ui.allocate_exact_size(
                            egui::vec2(card_w, 80.0),
                            egui::Sense::click(),
                        );
                        let p = ui.painter();

                        // Card background
                        let border_col = if is_active { col_accent }
                            else if resp.hovered() { col_border }
                            else { egui::Color32::from_rgb(50, 50, 55) };
                        p.rect_filled(card_rect, egui::Rounding::same(10.0),
                            egui::Color32::from_rgb(preset.bg[0]/2, preset.bg[1]/2, preset.bg[2]/2));
                        p.rect_stroke(card_rect, egui::Rounding::same(10.0),
                            egui::Stroke::new(if is_active {2.0} else {1.0}, border_col));

                        // 3 color swatches
                        let swatch_y = card_rect.top() + 20.0;
                        let swatch_r = 9.0;
                        let sw_x_start = card_rect.center().x - swatch_r * 3.0;
                        for (j, &swatch_col) in [preset.bg, preset.accent, preset.text].iter().enumerate() {
                            let cx = sw_x_start + j as f32 * swatch_r * 2.2;
                            p.circle_filled(egui::pos2(cx, swatch_y),
                                swatch_r,
                                egui::Color32::from_rgb(swatch_col[0], swatch_col[1], swatch_col[2]));
                            p.circle_stroke(egui::pos2(cx, swatch_y), swatch_r,
                                egui::Stroke::new(0.8, egui::Color32::from_rgba_unmultiplied(255,255,255,40)));
                        }

                        // Theme name
                        p.text(
                            egui::pos2(card_rect.center().x, card_rect.bottom() - 18.0),
                            egui::Align2::CENTER_CENTER,
                            &preset.name,
                            egui::FontId::new(11.5, egui::FontFamily::Proportional),
                            if is_active { col_accent } else { col_muted },
                        );

                        if resp.clicked() {
                            *theme = preset.clone();
                            theme.save();
                            *notification = Some((format!("Theme set to {}", preset.name), false, 3.0));
                        }
                        if (i + 1) % cols == 0 { ui.end_row(); }
                    }
                });
        }

        // ── CUSTOMIZE ────────────────────────────────────────────────────────
        ThemeTab::Customize => {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // ── Colors ───────────────────────────────────────────────────
                group(ui, col_surface, col_border, |ui| {
                    ui.label(egui::RichText::new("Colors").size(13.5).strong().color(col_text));
                    ui.add_space(10.0);

                    egui::Grid::new("color_grid").num_columns(4).spacing([16.0, 10.0]).show(ui, |ui| {
                        // Helper: label + color picker + hex
                        macro_rules! color_row {
                            ($lbl:expr, $field:expr) => {{
                                ui.label(egui::RichText::new($lbl).size(12.0).color(col_muted));
                                let mut c = egui::Color32::from_rgb($field[0], $field[1], $field[2]);
                                egui::color_picker::color_edit_button_srgba(
                                    ui, &mut c,
                                    egui::color_picker::Alpha::Opaque,
                                );
                                $field = [c.r(), c.g(), c.b()];
                                ui.label(egui::RichText::new(
                                    format!("#{:02X}{:02X}{:02X}", $field[0], $field[1], $field[2])
                                ).size(10.5).monospace().color(col_muted));
                            }};
                        }
                        color_row!("Background", theme.bg);     ui.end_row();
                        color_row!("Panel/Toolbar", theme.toolbar); ui.end_row();
                        color_row!("Surface",    theme.surface);  ui.end_row();
                        color_row!("Border",     theme.border);   ui.end_row();
                        color_row!("Text",       theme.text);     ui.end_row();
                        color_row!("Muted",      theme.muted);    ui.end_row();
                        color_row!("Accent",     theme.accent);   ui.end_row();
                        color_row!("Green",      theme.green);    ui.end_row();
                    });
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new(
                        egui::RichText::new("  Apply Colors  ").color(egui::Color32::WHITE)
                    ).fill(col_accent)).clicked() {
                        theme.save();
                        *notification = Some(("Colors saved.".into(), false, 3.0));
                    }
                });
                ui.add_space(14.0);

                // ── Color Harmony ─────────────────────────────────────────────
                group(ui, col_surface, col_border, |ui| {
                    ui.label(egui::RichText::new("Color Harmony").size(13.5).strong().color(col_text));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Pick an accent color — auto-generate a complete palette.").size(11.5).color(col_muted));
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Accent Color").size(12.0).color(col_muted));
                        ui.add_space(8.0);
                        let mut c = egui::Color32::from_rgb(harmony_accent[0], harmony_accent[1], harmony_accent[2]);
                        egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque);
                        *harmony_accent = [c.r(), c.g(), c.b()];
                        ui.label(egui::RichText::new(
                            format!("#{:02X}{:02X}{:02X}", harmony_accent[0], harmony_accent[1], harmony_accent[2])
                        ).size(10.5).monospace().color(col_muted));
                    });
                    ui.add_space(8.0);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Harmony").size(12.0).color(col_muted));
                        ui.add_space(8.0);
                        egui::ComboBox::from_id_source("harmony_cb")
                            .selected_text(harmony.label())
                            .show_ui(ui, |ui| {
                                for h in [Harmony::Complementary, Harmony::Triadic, Harmony::Analogous,
                                          Harmony::SplitComplementary, Harmony::Tetradic] {
                                    ui.selectable_value(harmony, h, h.label());
                                }
                            });
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("Mode").size(12.0).color(col_muted));
                        ui.add_space(4.0);
                        egui::ComboBox::from_id_source("mode_cb")
                            .selected_text(if *dark_mode { "Dark" } else { "Light" })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(dark_mode, true, "Dark");
                                ui.selectable_value(dark_mode, false, "Light");
                            });
                    });
                    ui.add_space(12.0);
                    if ui.add(egui::Button::new(
                        egui::RichText::new("  Generate Theme  ").color(egui::Color32::WHITE)
                    ).fill(col_accent)).clicked() {
                        let mut gen = generate_theme_from_harmony(*harmony_accent, *harmony, *dark_mode);
                        gen.name = "Generated".into();
                        *theme = gen;
                        theme.save();
                        *notification = Some(("Theme generated from harmony!".into(), false, 4.0));
                    }
                });
                ui.add_space(14.0);

                // ── Font & Layout ─────────────────────────────────────────────
                group(ui, col_surface, col_border, |ui| {
                    ui.label(egui::RichText::new("Font & Layout").size(13.5).strong().color(col_text));
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Density").size(12.0).color(col_muted));
                        ui.add_space(8.0);
                        egui::ComboBox::from_id_source("density_cb")
                            .selected_text(theme.density.label())
                            .show_ui(ui, |ui| {
                                for d in [ThemeDensity::Comfortable, ThemeDensity::Compact, ThemeDensity::Cozy] {
                                    ui.selectable_value(&mut theme.density, d, d.label());
                                }
                            });
                    });
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(
                        "Compact = tighter spacing  ·  Comfortable = standard  ·  Cozy = extra room"
                    ).size(10.5).color(col_muted));
                    ui.add_space(8.0);
                    if ui.add(egui::Button::new(
                        egui::RichText::new("  Apply Layout  ").color(egui::Color32::WHITE)
                    ).fill(col_accent)).clicked() {
                        theme.save();
                        *notification = Some(("Layout saved.".into(), false, 3.0));
                    }
                });
                ui.add_space(14.0);

                // ── Save / Share ──────────────────────────────────────────────
                group(ui, col_surface, col_border, |ui| {
                    ui.label(egui::RichText::new("Save / Share").size(13.5).strong().color(col_text));
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Theme name").size(12.0).color(col_muted));
                        ui.add_space(8.0);
                        ui.add(egui::TextEdit::singleline(custom_name)
                            .hint_text("My Theme")
                            .desired_width(180.0));
                        ui.add_space(8.0);
                        if ui.add(egui::Button::new(
                            egui::RichText::new("  Save  ").color(egui::Color32::WHITE)
                        ).fill(col_accent)).clicked() {
                            if !custom_name.trim().is_empty() {
                                theme.name = custom_name.trim().to_string();
                            }
                            theme.save();
                            *notification = Some((format!("Theme '{}' saved!", theme.name), false, 4.0));
                        }
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("  Export JSON  ").clicked() {
                            *export_buf = theme.to_json();
                            ui.output_mut(|o| o.copied_text = export_buf.clone());
                            *notification = Some(("Theme JSON copied to clipboard!".into(), false, 4.0));
                        }
                        ui.add_space(8.0);
                        if ui.button("  Reset to Default  ").clicked() {
                            *theme = AppTheme::preset_zentra();
                            theme.save();
                            *notification = Some(("Reset to Zentra default theme.".into(), false, 3.0));
                        }
                    });
                    if !export_buf.is_empty() {
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Exported JSON (also copied to clipboard):").size(10.5).color(col_muted));
                        ui.add(egui::TextEdit::multiline(export_buf)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(6)
                            .desired_width(f32::INFINITY));
                    }
                    ui.add_space(10.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Import theme JSON:").size(12.0).color(col_muted));
                    ui.add_space(4.0);
                    ui.add(egui::TextEdit::multiline(import_buf)
                        .hint_text("Paste theme JSON here…")
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY));
                    ui.add_space(6.0);
                    if ui.add(egui::Button::new(
                        egui::RichText::new("  Import  ").color(egui::Color32::WHITE)
                    ).fill(egui::Color32::from_rgb(50, 120, 80))).clicked() {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(import_buf.trim()) {
                            fn arr(v: &serde_json::Value, k: &str) -> Option<[u8;3]> {
                                let a = v.get(k)?.as_array()?;
                                Some([a.get(0)?.as_u64()? as u8, a.get(1)?.as_u64()? as u8, a.get(2)?.as_u64()? as u8])
                            }
                            if let (Some(bg), Some(toolbar), Some(surface), Some(surface2),
                                    Some(border), Some(text), Some(muted), Some(accent), Some(green)) =
                                (arr(&v,"bg"), arr(&v,"toolbar"), arr(&v,"surface"), arr(&v,"surface2"),
                                 arr(&v,"border"), arr(&v,"text"), arr(&v,"muted"), arr(&v,"accent"), arr(&v,"green"))
                            {
                                let density = match v.get("density").and_then(|d| d.as_str()) {
                                    Some("Compact") => ThemeDensity::Compact,
                                    Some("Cozy")    => ThemeDensity::Cozy,
                                    _               => ThemeDensity::Comfortable,
                                };
                                *theme = AppTheme {
                                    name: v.get("name").and_then(|n| n.as_str()).unwrap_or("Imported").to_string(),
                                    bg, toolbar, surface, surface2, border, text, muted, accent, green, density,
                                };
                                theme.save();
                                import_buf.clear();
                                *notification = Some(("Theme imported!".into(), false, 4.0));
                            } else {
                                *notification = Some(("Invalid theme JSON format.".into(), true, 5.0));
                            }
                        } else {
                            *notification = Some(("Invalid JSON.".into(), true, 5.0));
                        }
                    }
                });
            });
        }
    }
}

// ─── UI helpers ───────────────────────────────────────────────────────────────

fn setup_visuals(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals.dark_mode = true;
    style.visuals.window_fill  = egui::Color32::from_rgb(44, 44, 44);
    style.visuals.panel_fill   = egui::Color32::from_rgb(32, 32, 32);
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(228, 228, 228));
    style.visuals.widgets.inactive.rounding = egui::Rounding::same(3.0);
    style.visuals.widgets.hovered.rounding  = egui::Rounding::same(3.0);
    style.visuals.widgets.active.rounding   = egui::Rounding::same(3.0);
    style.visuals.widgets.inactive.bg_fill  = egui::Color32::from_rgb(55, 55, 55);
    style.visuals.widgets.hovered.bg_fill   = egui::Color32::from_rgb(68, 68, 68);
    style.visuals.widgets.active.bg_fill    = egui::Color32::from_rgb(80, 80, 80);
    style.visuals.selection.bg_fill         = egui::Color32::from_rgba_unmultiplied(60, 120, 200, 60);
    style.visuals.selection.stroke          = egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 120, 200));
    style.spacing.item_spacing              = egui::vec2(8.0, 4.0);
    style.spacing.button_padding            = egui::vec2(10.0, 5.0);
    ctx.set_style(style);
}

// Toolbar button — returns true if clicked
fn toolbar_tab(
    ui: &mut egui::Ui,
    label: &str,
    icon: &str,
    selected: bool,
    accent: egui::Color32,
    surface: egui::Color32,
    col_text: egui::Color32,
    col_muted: egui::Color32,
) -> bool {
    let btn_w = 104.0_f32;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(btn_w, 44.0), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let pad = rect.shrink2(egui::vec2(3.0, 6.0));
        let bg = if selected {
            egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 32)
        } else if resp.hovered() {
            surface
        } else {
            egui::Color32::TRANSPARENT
        };
        p.rect_filled(pad, egui::Rounding::same(8.0), bg);
        if selected {
            p.rect_stroke(pad, egui::Rounding::same(8.0), egui::Stroke::new(1.0, accent));
        }
        let fg = if selected { accent } else if resp.hovered() { col_text } else { col_muted };
        // icon + label, centered
        let txt = format!("{}  {}", icon, label);
        p.text(pad.center(), egui::Align2::CENTER_CENTER, txt,
            egui::FontId::new(12.0, egui::FontFamily::Proportional), fg);
    }
    resp.clicked()
}

// Card frame — soft rounded panel with a subtle 1px border. The professional
// base container used across every tab.
fn group(ui: &mut egui::Ui, fill: egui::Color32, border: egui::Color32, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, border))
        .rounding(10.0)
        .inner_margin(egui::vec2(18.0, 16.0))
        .show(ui, content);
}

// Titled card — a card with a header row (accent tick + title) and the body
// below. Returns the inner response so callers can keep composing.
fn card_titled(
    ui: &mut egui::Ui,
    fill: egui::Color32, border: egui::Color32,
    accent: egui::Color32, text: egui::Color32,
    title: &str,
    content: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, border))
        .rounding(10.0)
        .inner_margin(egui::vec2(18.0, 16.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // accent tick
                let (rect, _) = ui.allocate_exact_size(egui::vec2(3.0, 15.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, egui::Rounding::same(2.0), accent);
                ui.add_space(7.0);
                ui.label(egui::RichText::new(title).size(13.5).strong().color(text));
            });
            ui.add_space(12.0);
            content(ui);
        });
}

// Section title with a left accent bar — used at the top of each tab.
fn section_title(ui: &mut egui::Ui, title: &str, subtitle: &str, accent: egui::Color32, text: egui::Color32, muted: egui::Color32) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(4.0, 26.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, egui::Rounding::same(2.0), accent);
        ui.add_space(10.0);
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(title).size(20.0).strong().color(text));
            if !subtitle.is_empty() {
                ui.label(egui::RichText::new(subtitle).size(11.5).color(muted));
            }
        });
    });
    ui.add_space(16.0);
}

// Reusable copy-to-clipboard icon button. Returns true if clicked.
fn copy_btn(ui: &mut egui::Ui, value: &str, muted: egui::Color32, accent: egui::Color32) -> bool {
    let resp = ui.add(egui::Button::new(egui::RichText::new("⧉").size(12.0).color(muted))
        .frame(false).min_size(egui::vec2(20.0, 20.0)))
        .on_hover_text("Copy");
    if resp.hovered() { ui.painter().rect_stroke(resp.rect, egui::Rounding::same(4.0), egui::Stroke::new(1.0, accent)); }
    if resp.clicked() {
        ui.output_mut(|o| o.copied_text = value.to_string());
        return true;
    }
    false
}

// Small rounded status pill (colored background).
fn pill(ui: &mut egui::Ui, label: &str, fg: egui::Color32, accent: egui::Color32) {
    egui::Frame::none()
        .fill(egui::Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 26))
        .stroke(egui::Stroke::new(1.0, accent))
        .rounding(11.0)
        .inner_margin(egui::vec2(10.0, 3.0))
        .show(ui, |ui| { ui.label(egui::RichText::new(label).size(11.0).strong().color(fg)); });
}

// Metric tile — a small card showing a label and a bold colored value, with a
// thin accent line under the value. Used in stat grids.
fn stat_box(ui: &mut egui::Ui, fill: egui::Color32, border: egui::Color32, label: &str, value: &str, val_col: egui::Color32) {
    let muted = egui::Color32::from_rgb(150, 150, 158);
    egui::Frame::none()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, border))
        .rounding(9.0)
        .inner_margin(egui::vec2(14.0, 12.0))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label.to_uppercase()).size(9.5).color(muted));
            ui.add_space(5.0);
            ui.label(egui::RichText::new(value).size(17.0).strong().color(val_col));
            ui.add_space(6.0);
            let w = ui.available_width().min(34.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 2.5), egui::Sense::hover());
            ui.painter().rect_filled(rect, egui::Rounding::same(2.0),
                egui::Color32::from_rgba_unmultiplied(val_col.r(), val_col.g(), val_col.b(), 130));
        });
}

// Balance row for a signed pending amount (shows +/- prefix)
fn bal_row_signed(ui: &mut egui::Ui, label: &str, delta: f64, col: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(12.5).color(col));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let txt = if delta == 0.0 {
                "0.00000000 ZTR".to_string()
            } else if delta > 0.0 {
                format!("+{:.8} ZTR", delta)
            } else {
                format!("{:.8} ZTR", delta)
            };
            ui.label(egui::RichText::new(txt).size(12.5).color(col).monospace());
        });
    });
    ui.add_space(2.0);
}

// Balance row — right-aligns the value
fn bal_row(ui: &mut egui::Ui, label: &str, amount: f64, lc: egui::Color32, vc: egui::Color32, bold: bool) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(12.5).color(lc));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let rt = egui::RichText::new(format!("{:.8} ZTR", amount)).size(12.5).color(vc).monospace();
            let rt = if bold { rt.strong() } else { rt };
            ui.label(rt);
        });
    });
    ui.add_space(2.0);
}

// Inline key: value in horizontal layout
fn kv_inline(ui: &mut egui::Ui, label: &str, val: &str, lc: egui::Color32, vc: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(11.5).color(lc));
        ui.label(egui::RichText::new(": ").size(11.5).color(lc));
        ui.label(egui::RichText::new(val).size(11.5).color(vc).strong());
    });
}

// Standard kv row (label left, value right)
fn kv(ui: &mut egui::Ui, key: &str, val: &str, kc: egui::Color32, vc: egui::Color32) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(key).size(12.0).color(kc));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(val).size(12.0).color(vc).strong());
        });
    });
}

// Table cell helpers
fn tcell_h(ui: &mut egui::Ui, label: &str, w: f32, col: egui::Color32) {
    ui.add_sized(egui::vec2(w, 16.0), egui::Label::new(egui::RichText::new(label).size(10.0).color(col).strong()));
}
fn tcell(ui: &mut egui::Ui, val: &str, w: f32, col: egui::Color32) {
    ui.add_sized(egui::vec2(w, 20.0), egui::Label::new(egui::RichText::new(val).size(12.0).color(col)));
}
fn tcell_mono(ui: &mut egui::Ui, val: &str, w: f32, col: egui::Color32) {
    ui.add_sized(egui::vec2(w, 20.0), egui::Label::new(egui::RichText::new(val).size(11.5).color(col).monospace()));
}

fn sep(ui: &mut egui::Ui, col: egui::Color32) {
    let _ = col;
    ui.add(egui::Separator::default().spacing(4.0));
}

fn fmt_hashrate(h: f64) -> String {
    if h < 1_000.0          { format!("{:.1} H/s", h) }
    else if h < 1_000_000.0 { format!("{:.2} KH/s", h / 1_000.0) }
    else                    { format!("{:.3} MH/s", h / 1_000_000.0) }
}

fn fmt_difficulty(d: f64) -> String {
    if d < 1_000.0          { format!("{:.1}", d) }
    else if d < 1_000_000.0 { format!("{:.2}K", d / 1_000.0) }
    else if d < 1e9         { format!("{:.3}M", d / 1_000_000.0) }
    else                    { format!("{:.3}G", d / 1e9) }
}

fn fmt_big_num(n: u64) -> String {
    if n < 1_000             { format!("{}", n) }
    else if n < 1_000_000   { format!("{:.1}K", n as f64 / 1_000.0) }
    else if n < 1_000_000_000 { format!("{:.2}M", n as f64 / 1_000_000.0) }
    else                    { format!("{:.3}B", n as f64 / 1_000_000_000.0) }
}

// Format a ZTR amount with thousands separators and 8 decimals,
// e.g. 862811.86016634 -> "862 811.86016634"
fn fmt_amount(v: f64) -> String {
    let neg = v < 0.0;
    let v = v.abs();
    let int_part = v.trunc() as u64;
    let frac = format!("{:.8}", v.fract());      // "0.86016634"
    let frac = frac.trim_start_matches('0');      // ".86016634"
    // group the integer part in threes
    let s = int_part.to_string();
    let bytes = s.as_bytes();
    let mut grouped = String::new();
    let len = bytes.len();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (len - i) % 3 == 0 { grouped.push(' '); }
        grouped.push(*b as char);
    }
    let out = format!("{}{}", grouped, frac);
    if neg { format!("-{}", out) } else { out }
}

// ─── RPC ──────────────────────────────────────────────────────────────────────

fn read_rpc_token() -> String {
    let path = data_dir().join("rpc_auth.token");
    if path.exists() {
        if let Ok(token) = std::fs::read_to_string(&path) {
            return token.trim().to_string();
        }
    }
    String::new()
}

/// Default node the wallet talks to: a node running on this machine.
const DEFAULT_NODE_RPC: &str = "http://127.0.0.1:16111";

/// The node RPC endpoint. Overridable with the `ZENTRA_NODE_URL` env var so a
/// user can point the wallet at a remote/seed node instead of a local one.
fn node_rpc_url() -> String {
    std::env::var("ZENTRA_NODE_URL").ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_NODE_RPC.to_string())
}

/// `host:port` form of the configured node, for raw TCP reachability checks.
fn node_rpc_hostport() -> String {
    node_rpc_url()
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .trim_end_matches('/')
        .to_string()
}

/// True when the wallet is pointed at a node other than the default local one;
/// in that case the wallet must NOT spawn or kill a local daemon.
fn using_remote_node() -> bool {
    node_rpc_url() != DEFAULT_NODE_RPC
}

/// Parse a "vX.Y.Z" (or "X.Y.Z") tag into a comparable (major, minor, patch)
/// tuple. Missing/garbage components become 0.
fn parse_semver(s: &str) -> (u64, u64, u64) {
    let s = s.trim().trim_start_matches('v').trim_start_matches('V');
    let mut it = s.split(|c| c == '.' || c == '-' || c == '+');
    let major = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let patch = it.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

/// True only when `remote` is a strictly-higher semver than `current`.
fn semver_gt(remote: &str, current: &str) -> bool {
    parse_semver(remote) > parse_semver(current)
}

fn call_rpc(method: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let body = json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": 1 });
    let token = read_rpc_token();
    let auth_header = format!("Bearer {}", token);
    let resp = ureq::post(&node_rpc_url())
        .timeout(std::time::Duration::from_secs(8))
        .set("Content-Type", "application/json")
        .set("Authorization", &auth_header)
        .send_json(body)
        .map_err(|e| e.to_string())?;
    let j: serde_json::Value = resp.into_json().map_err(|e| e.to_string())?;
    if let Some(err) = j.get("error") {
        return Err(err["message"].as_str().unwrap_or("RPC error").to_string());
    }
    j.get("result").cloned().ok_or_else(|| "No result".into())
}

// ─── Background poll ──────────────────────────────────────────────────────────

fn poll_node(state: &Arc<Mutex<NodeState>>) {
    let dag = match call_rpc("getDagInfo", json!([])) {
        Ok(r) => { state.lock().unwrap().poll_failures = 0; r }
        Err(_) => {
            // Only declare a real disconnect after several consecutive misses;
            // a lone failed poll while the node is busy must not flip the status.
            let mut s = state.lock().unwrap();
            s.poll_failures = s.poll_failures.saturating_add(1);
            if s.poll_failures >= 3 { s.connected = false; }
            return;
        }
    };
    let pool    = call_rpc("getPoolState",    json!([])).unwrap_or(json!({}));
    let mining  = call_rpc("getMiningStatus", json!([])).unwrap_or(json!({}));
    let net     = call_rpc("getNetworkInfo",  json!([])).unwrap_or(json!({}));
    let blocks  = call_rpc("getRecentBlocks", json!([])).unwrap_or(json!([]));
    let mempool = call_rpc("getMempool",      json!([])).unwrap_or(json!([]));

    // Fetch balance for the current wallet address (read address without holding lock)
    let mut wallet_addr = state.lock().unwrap().wallet_address.clone();
    // Self-heal: if the UI thread hasn't published the address yet, derive it
    // straight from the stored seed so the balance always loads.
    if wallet_addr.is_empty() {
        if let Ok(seed) = std::fs::read_to_string(data_dir().join("wallet_mnemonic.txt")) {
            let seed = seed.trim().to_string();
            if !seed.is_empty() {
                if let Ok(v) = call_rpc("deriveAddress", json!([seed])) {
                    if let Some(a) = v.as_str() {
                        if !a.is_empty() {
                            wallet_addr = a.to_string();
                            state.lock().unwrap().wallet_address = wallet_addr.clone();
                        }
                    }
                }
            }
        }
    }
    let ztr_balance = if !wallet_addr.is_empty() {
        call_rpc("getBalance", json!([wallet_addr]))
            .ok()
            .and_then(|r| r.as_u64())
            .map(|n| n as f64 / 1e8)
    } else {
        None
    };

    let mut s = state.lock().unwrap();
    s.connected       = true;
    s.blue_score      = dag["blue_score"].as_u64().unwrap_or(0);
    s.tips_count      = dag["tips"].as_array().map(|a| a.len()).unwrap_or(0);
    s.selected_tip    = dag["selected_tip"].as_str().unwrap_or("—").to_string();
    s.network_name    = dag["network"].as_str().unwrap_or("devnet").to_string();
    s.amm_reserve_ztr  = pool["reserve_ztr"].as_u64().unwrap_or(0) as f64 / 1e8;
    s.amm_reserve_zusd = pool["reserve_zusd"].as_u64().unwrap_or(0) as f64 / 1e6;
    s.amm_lp_burned    = pool["total_lp_burned"].as_f64().unwrap_or(0.0);
    s.is_mining         = mining["is_mining"].as_bool().unwrap_or(false);
    s.mining_lane       = mining["lane"].as_u64().unwrap_or(0) as u8;
    s.mining_address    = mining["address"].as_str().unwrap_or("").to_string();
    s.mining_hashrate   = mining["hashrate"].as_f64().unwrap_or(0.0);
    s.mining_hashes     = mining["hashes"].as_u64().unwrap_or(0);
    s.mined_blocks      = mining["mined_blocks"].as_u64().unwrap_or(0);
    s.mining_difficulty = mining["difficulty"].as_f64().unwrap_or(1.0);
    s.target_block_time_ms = mining["target_block_time_ms"].as_u64().unwrap_or(60_000);
    s.network_hashrate  = mining["network_hashrate"].as_f64().unwrap_or(0.0);
    s.mining_threads_daemon = mining["threads"].as_u64().unwrap_or(1) as u32;
    s.max_mining_threads    = mining["max_threads"].as_u64().unwrap_or(8) as u32;
    if let Some(em) = mining.get("emission") {
        s.block_reward         = em["initial_reward_ztr"].as_f64().unwrap_or(50.0);
        s.blocks_until_halving = em["blocks_until_next_halving"].as_u64().unwrap_or(0);
        s.days_until_halving   = em["days_until_next_halving"].as_f64().unwrap_or(0.0);
    }
    s.mempool_size    = net["mempool_size"].as_u64().unwrap_or(0) as usize;
    s.peers           = net["peers"].as_array().cloned().unwrap_or_default();
    s.protocol_version = net["protocol_version"].as_u64().unwrap_or(1);
    if let Some(arr) = blocks.as_array() { s.recent_blocks = arr.clone(); }
    if let Some(b) = ztr_balance { s.ztr_balance = b; }
    if let Some(arr) = mempool.as_array() { s.pending_txs = arr.clone(); }

    // ── Pool: fetch info; if pool mode + mining, send a heartbeat ──
    let pool_mode = s.pool_mode;
    let wallet = s.wallet_address.clone();
    let hashrate = s.mining_hashrate;
    let is_mining = s.is_mining;
    drop(s); // release lock before RPC calls

    if let Ok(info) = call_rpc("poolGetInfo", json!([])) {
        let mut s = state.lock().unwrap();
        s.pool_address = info["address"].as_str().unwrap_or("").to_string();
        s.pool_pending_zents = info["pending_balance_zents"].as_u64().unwrap_or(0);
        s.pool_active_miners = info["active_miners"].as_u64().unwrap_or(0) as usize;
        s.pool_ms_until_payout = info["ms_until_payout"].as_u64().unwrap_or(0);
    }

    // Heartbeat + per-miner stats when actively pool mining
    if pool_mode && is_mining && !wallet.is_empty() {
        if let Ok(hb) = call_rpc("poolHeartbeat", json!([wallet, hashrate])) {
            let mut s = state.lock().unwrap();
            s.pool_my_paid_zents = hb["total_paid_zents"].as_u64().unwrap_or(0);
            s.pool_my_share_pct = hb["share_percent"].as_f64().unwrap_or(0.0);
        }
    }
}

// ─── Console commands ─────────────────────────────────────────────────────────

fn run_console_cmd(cmd: &str, parts: &[String], mnemonic: &str) -> Result<String, String> {
    match cmd.to_lowercase().as_str() {
        "getdaginfo"      => Ok(pp(call_rpc("getDagInfo",      json!([]))?)),
        "getbalance"      => {
            let addr = parts.get(1).ok_or("Usage: getbalance <address>")?;
            let r = call_rpc("getBalance", json!([addr]))?;
            Ok(format!("{:.8} ZTR", r.as_u64().unwrap_or(0) as f64 / 1e8))
        }
        "getmininginfo"   => Ok(pp(call_rpc("getMiningInfo",   json!([]))?)),
        "getminingstatus" => Ok(pp(call_rpc("getMiningStatus", json!([]))?)),
        "getpoolstate"    => Ok(pp(call_rpc("getPoolState",    json!([]))?)),
        "getrecentblocks" => {
            let r = call_rpc("getRecentBlocks", json!([]))?;
            let n = r.as_array().map(|a| a.len()).unwrap_or(0);
            Ok(format!("{} blocks\n{}", n, pp(r)))
        }
        "getmempool"      => Ok(pp(call_rpc("getMempool",      json!([]))?)),
        "getnetworkinfo"  => Ok(pp(call_rpc("getNetworkInfo",  json!([]))?)),
        "stopmining"      => { call_rpc("stopMining",  json!([]))?; Ok("Mining stopped.".into()) }
        "startmining"     => {
            let lane: u8 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            let addr = parts.get(2).cloned().unwrap_or_default();
            let r = call_rpc("startMining", json!([lane, addr]))?;
            Ok(r.as_str().unwrap_or("ok").to_string())
        }
        "sendtoaddress"   => {
            let to    = parts.get(1).ok_or("Usage: sendtoaddress <addr> <amount>")?;
            let amt_s = parts.get(2).ok_or("Usage: sendtoaddress <addr> <amount>")?;
            let val: f64 = amt_s.parse().map_err(|_| "Invalid amount")?;
            let r = call_rpc("sendTransfer", json!([mnemonic, to, (val * 1e8) as u64, 1000u64]))?;
            Ok(format!("TxID: {}", r.as_str().unwrap_or("")))
        }
        "deriveaddress"   => {
            let mn = parts.get(1).cloned().unwrap_or_else(|| mnemonic.to_string());
            Ok(call_rpc("deriveAddress", json!([mn]))?.as_str().unwrap_or("").to_string())
        }
        "help" => Ok("Commands:\n  getdaginfo | getbalance <addr> | getmininginfo | getminingstatus\n  getpoolstate | getrecentblocks | getmempool | getnetworkinfo\n  stopmining | startmining [lane] [addr] | sendtoaddress <addr> <amt>\n  deriveaddress [mnemonic]".into()),
        _ => Err(format!("Unknown command '{}'. Type 'help'.", cmd)),
    }
}

fn pp(v: serde_json::Value) -> String {
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

// ─── Daemon management ────────────────────────────────────────────────────────

fn spawn_daemon() -> Option<Child> {
    // If the user pointed us at a remote node, never spawn a local daemon.
    if using_remote_node() { return None; }
    if TcpStream::connect(node_rpc_hostport().as_str()).is_ok() { return None; }
    let dir = data_dir();
    std::fs::create_dir_all(&dir).ok();
    let log = std::fs::File::create(dir.join("zentrad.log")).ok();
    let exe = std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.join("zentrad.exe")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("zentrad.exe"));
    let mut cmd = Command::new(&exe);
    cmd.arg("--network").arg("devnet").arg("--data-dir").arg(dir.to_str().unwrap_or("zentra-data"));
    if let Some(f) = log {
        cmd.stdout(f.try_clone().unwrap_or_else(|_| std::fs::File::create("NUL").unwrap())).stderr(f);
    } else {
        cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    }
    #[cfg(target_os = "windows")]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); }
    match cmd.spawn() {
        Ok(c) => { std::thread::sleep(Duration::from_millis(800)); Some(c) }
        Err(e) => { eprintln!("Failed to spawn zentrad: {}", e); None }
    }
}

fn kill_daemon(daemon: &Arc<Mutex<Option<Child>>>) {
    // Never stop/kill a node we don't own (remote node, or a separately-run one).
    if using_remote_node() { return; }
    let _ = call_rpc("stopNode", json!([]));
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if TcpStream::connect(node_rpc_hostport().as_str()).is_err() {
            *daemon.lock().unwrap() = None;
            return;
        }
    }
    let mut guard = daemon.lock().unwrap();
    if let Some(ref mut c) = *guard { let _ = c.kill(); let _ = c.wait(); }
    *guard = None;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill").args(&["/F", "/IM", "zentrad.exe"])
            .creation_flags(0x08000000).status();
    }
}

fn data_dir() -> std::path::PathBuf {
    std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.join("zentra-data")))
        .unwrap_or_else(|| std::path::PathBuf::from("./zentra-data"))
}

// ─── Entry point ──────────────────────────────────────────────────────────────

/// Decode the embedded coin PNG into an egui window icon, auto-cropping the
/// transparent border so the coin fills the icon canvas (otherwise it looks tiny).
fn load_window_icon() -> egui::IconData {
    const LOGO_BYTES: &[u8] = include_bytes!("../assets/coin-ztr.png");
    match image::load_from_memory(LOGO_BYTES) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            // Find the bounding box of non-transparent pixels.
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0u32, 0u32);
            for y in 0..h {
                for x in 0..w {
                    if rgba.get_pixel(x, y)[3] > 16 {
                        if x < min_x { min_x = x; }
                        if y < min_y { min_y = y; }
                        if x > max_x { max_x = x; }
                        if y > max_y { max_y = y; }
                    }
                }
            }
            if max_x <= min_x || max_y <= min_y {
                // Fully transparent fallback
                return egui::IconData { rgba: rgba.into_raw(), width: w, height: h };
            }
            // Add a tiny margin, clamp to bounds.
            let pad = ((max_x - min_x).max(max_y - min_y) / 20).max(2);
            let x0 = min_x.saturating_sub(pad);
            let y0 = min_y.saturating_sub(pad);
            let x1 = (max_x + pad).min(w - 1);
            let y1 = (max_y + pad).min(h - 1);
            let cropped = image::imageops::crop_imm(&rgba, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image();
            let (cw, ch) = cropped.dimensions();
            egui::IconData { rgba: cropped.into_raw(), width: cw, height: ch }
        }
        Err(_) => egui::IconData { rgba: vec![0, 0, 0, 0], width: 1, height: 1 },
    }
}

fn main() -> Result<(), eframe::Error> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Zentra Core Wallet")
            .with_icon(load_window_icon())
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([820.0, 560.0])
            .with_resizable(true),
        ..Default::default()
    };
    eframe::run_native("zentra_core", opts, Box::new(|cc| Ok(Box::new(ZentraApp::new(cc)))))
}
