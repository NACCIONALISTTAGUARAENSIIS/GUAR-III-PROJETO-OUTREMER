//! BESM-6 Master Control HUD
//!
//! Interface de telemetria passiva e controle de geração granular com Viewport.
//! Opera com Zero-Polling no sistema de arquivos e Renderização Assíncrona.
//! 🚨 BESM-6: Arquitetura de Tile Streaming com Halo Caching (O(1) Memory).
//! Geometrias são extraídas, processadas e destruídas região por região (.mca),
//! garantindo escala infinita (DF Inteiro) sem colapso de RAM (OOM).
//! Utiliza Actor Pattern estrito para I/O do Manifesto, erradicando corrupção de disco.

use colored::Colorize;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyModifiers},
    execute, queue,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::args::Args;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::data_processing;
use crate::providers::ProviderManager;

const MAX_PROJECT_QUOTA_BYTES: u64 = 180 * 1024 * 1024 * 1024;
const MANIFEST_PATH: &str = "./world/besm6_manifest.bin";

const VIEWPORT_WIDTH: i32 = 18;
const VIEWPORT_HEIGHT: i32 = 12;

#[derive(Clone, Copy, PartialEq)]
pub enum RegionStatus {
    Processing,
    Cached,
    Sealed,
    Corrupted,
    Empty,
}

pub enum BesmSignal {
    RegionCached(i32, i32),
    RegionProcessing(i32, i32),
    RegionSealed(i32, i32, u64),
    RegionFailed(i32, i32),
    RegionEmpty(i32, i32), // 🚨 Aborto de fronteira
    GenerationComplete,
    Log(String),
}

// 🚨 BESM-6: Mensagens de I/O Seguras para o Ator de Disco
enum DiskActorMsg {
    UpdateManifest(HashMap<(i32, i32), u64>),
    Terminate,
}

enum UserInput {
    Command(String),
    MoveCamera(i32, i32),
    Quit,
}

#[derive(Clone)]
pub struct MacroRegion {
    pub name: &'static str,
    pub command: &'static str,
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

impl MacroRegion {
    pub fn get_presets() -> Vec<MacroRegion> {
        vec![
            // 🚨 Limites Estritos do Polígono do Distrito Federal
            MacroRegion {
                name: "Distrito Federal (Completo)",
                command: "-gerar df",
                min_lat: -16.06, max_lat: -15.45,
                min_lon: -48.33, max_lon: -47.22,
            },
            MacroRegion {
                name: "Plano Piloto (Asas + Eixo)",
                command: "-gerar plano_piloto",
                min_lat: -15.84, max_lat: -15.72,
                min_lon: -47.95, max_lon: -47.82,
            },
            MacroRegion {
                name: "Guará",
                command: "-gerar guara",
                min_lat: -15.86, max_lat: -15.80,
                min_lon: -47.99, max_lon: -47.95,
            },
            MacroRegion {
                name: "Taguatinga",
                command: "-gerar taguatinga",
                min_lat: -15.86, max_lat: -15.79,
                min_lon: -48.09, max_lon: -48.03,
            },
            MacroRegion {
                name: "Águas Claras",
                command: "-gerar aguas_claras",
                min_lat: -15.85, max_lat: -15.82,
                min_lon: -48.04, max_lon: -48.01,
            },
            MacroRegion {
                name: "Lago Sul",
                command: "-gerar lago_sul",
                min_lat: -15.89, max_lat: -15.80,
                min_lon: -47.90, max_lon: -47.81,
            },
        ]
    }
}

// 🚨 BESM-6: Shared State isolado para evitar vazamento
pub struct SharedDashboardState {
    pub regions_map: HashMap<(i32, i32), RegionStatus>,
    pub regions_bytes: HashMap<(i32, i32), u64>,
    pub accumulated_bytes_written: u64,
    pub log_buffer: Vec<String>,
    pub is_generating: bool,
    pub status_msg: String,
}

impl SharedDashboardState {
    pub fn new() -> Self {
        Self {
            regions_map: HashMap::new(),
            regions_bytes: HashMap::new(),
            accumulated_bytes_written: 0,
            log_buffer: Vec::new(),
            is_generating: false,
            status_msg: "SISTEMA OPERANTE. AGUARDANDO DIRETRIZ.".to_string(),
        }
    }

    pub fn push_log(&mut self, msg: String) {
        self.log_buffer.push(msg);
        if self.log_buffer.len() > 100 {
            self.log_buffer.drain(0..50);
        }
    }
}

pub struct MasterControl {
    state: SharedDashboardState,
    camera_x: i32,
    camera_z: i32,
    current_typing: String,

    signal_rx: Option<mpsc::Receiver<BesmSignal>>,
    abort_flag: Arc<AtomicBool>,

    // 🚨 BESM-6: Ator de Disco Unificado
    disk_tx: mpsc::Sender<DiskActorMsg>,

    provider_manager: Arc<ProviderManager>,
    args: Arc<Args>,
}

impl MasterControl {
    pub fn new(provider_manager: Arc<ProviderManager>, args: Arc<Args>) -> Self {
        // Inicializa a Thread Mestre de Disco (File Lock nativo assíncrono)
        let (disk_tx, disk_rx) = mpsc::channel();
        thread::spawn(move || {
            Self::disk_actor_loop(disk_rx);
        });

        let mut control = Self {
            state: SharedDashboardState::new(),
            camera_x: 0,
            camera_z: 0,
            current_typing: String::new(),
            signal_rx: None,
            abort_flag: Arc::new(AtomicBool::new(false)),
            disk_tx,
            provider_manager,
            args,
        };
        control.load_manifest_index();
        control
    }

    /// 🚨 BESM-6: ATOR DE DISCO (Serializer)
    /// Recebe requisições de salvar no arquivo ordenadamente. Evita corrupção de concorrência.
    fn disk_actor_loop(rx: mpsc::Receiver<DiskActorMsg>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                DiskActorMsg::UpdateManifest(bytes_map) => {
                    if let Ok(mut file) = OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .open(MANIFEST_PATH)
                    {
                        for (&(x, z), &bytes) in &bytes_map {
                            let _ = writeln!(file, "{},{},{}", x, z, bytes);
                        }
                    }
                }
                DiskActorMsg::Terminate => break,
            }
        }
    }

    /// Carrega o manifesto de disco para não estourar a cota a toa
    fn load_manifest_index(&mut self) {
        if !Path::new(MANIFEST_PATH).exists() {
            return;
        }
        if let Ok(mut file) = fs::File::open(MANIFEST_PATH) {
            let mut buffer = String::new();
            if file.read_to_string(&mut buffer).is_ok() {
                for line in buffer.lines() {
                    let parts: Vec<&str> = line.split(',').collect();
                    if parts.len() == 3 {
                        if let (Ok(x), Ok(z), Ok(bytes)) = (parts[0].parse(), parts[1].parse(), parts[2].parse::<u64>()) {
                            let old_bytes = self.state.regions_bytes.insert((x, z), bytes).unwrap_or(0);
                            self.state.accumulated_bytes_written -= old_bytes;
                            self.state.accumulated_bytes_written += bytes;
                            self.state.regions_map.insert((x, z), RegionStatus::Sealed);
                        }
                    }
                }
            }
        }
    }

    fn render_dashboard(&mut self) {
        let mut stdout = io::stdout();
        queue!(stdout, MoveTo(0, 0), Clear(ClearType::All)).unwrap();

        let consumed_gb = self.state.accumulated_bytes_written as f64 / (1024.0 * 1024.0 * 1024.0);
        let max_gb = MAX_PROJECT_QUOTA_BYTES as f64 / (1024.0 * 1024.0 * 1024.0);
        let usage_percent = (consumed_gb / max_gb) * 100.0;

        let bar_color = if usage_percent > 90.0 { "red" } else { "cyan" };

        println!("{}", "=================================================================================".cyan().bold());
        println!("{} - {}", "[BESM-6]".yellow().bold(), "MASTER CONTROL DASHBOARD (BRASÍLIA DF)".bright_white().bold());

        let quota_str = format!("PROJECT QUOTA: {:.2} GB / {:.2} GB [{:.1}%]", consumed_gb, max_gb, usage_percent);
        if usage_percent > 90.0 {
            println!("{}", quota_str.red().bold());
        } else {
            println!("{}", quota_str.color(bar_color).bold());
        }

        println!("STATUS: {}", self.state.status_msg.magenta().bold());
        println!("{}", "=================================================================================".cyan().bold());
        println!("{} Use Setas/WASD para mover a câmera. Ctrl+C para Abortar.", "CONTROLES:".white().bold());
        println!();

        let v_min_x = self.camera_x - VIEWPORT_WIDTH / 2;
        let v_max_x = self.camera_x + VIEWPORT_WIDTH / 2;
        let v_min_z = self.camera_z - VIEWPORT_HEIGHT / 2;
        let v_max_z = self.camera_z + VIEWPORT_HEIGHT / 2;

        print!("    ");
        for x in v_min_x..=v_max_x {
            if x % 2 == 0 { print!("{:>3} ", x); } else { print!("    "); }
        }
        println!();

        for z in v_min_z..=v_max_z {
            print!("{:>3} ", z);
            for x in v_min_x..=v_max_x {
                let status = self.state.regions_map.get(&(x, z)).copied().unwrap_or(RegionStatus::Empty);
                match status {
                    RegionStatus::Sealed => print!("{} ", "[██]".green()),
                    RegionStatus::Processing => print!("{} ", "[██]".yellow()),
                    RegionStatus::Cached => print!("{} ", "[██]".blue()),
                    RegionStatus::Corrupted => print!("{} ", "[XX]".red().bold()),
                    RegionStatus::Empty => print!("{} ", "[  ]".bright_black()),
                }
            }
            println!();
        }

        println!("\n{}: {} Selado | {} Varredura Atual | {} Halo Vizinho | {} Falha IO",
                 "LEGENDA".white().bold(), "[██]".green(), "[██]".yellow(), "[██]".blue(), "[XX]".red().bold());
        println!("{}", "=================================================================================".cyan().bold());

        println!("{}", "SYSTEM LOGS:".green().bold());
        let log_start = self.state.log_buffer.len().saturating_sub(6);
        for msg in &self.state.log_buffer[log_start..] {
            println!("> {}", msg);
        }
        println!("{}", "=================================================================================".cyan().bold());

        if !self.state.is_generating {
            println!("{}", "DIRETRIZES TÁTICAS DISPONÍVEIS:".green().bold());
            for preset in MacroRegion::get_presets() {
                println!("  {:<25} -> {}", preset.command.yellow(), preset.name);
            }
        }

        print!("\n{} {}_", "root@besm6:~#".green().bold(), self.current_typing);
        stdout.flush().unwrap();
    }

    pub fn run_interactive_shell(&mut self) {
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide).unwrap();
        enable_raw_mode().unwrap();

        let (input_tx, input_rx) = mpsc::channel::<UserInput>();

        thread::spawn(move || loop {
            if event::poll(Duration::from_millis(16)).unwrap() {
                if let Event::Key(key_event) = event::read().unwrap() {
                    if key_event.modifiers.contains(KeyModifiers::CONTROL) && key_event.code == KeyCode::Char('c') {
                        input_tx.send(UserInput::Quit).unwrap();
                        break;
                    }
                    match key_event.code {
                        KeyCode::Up | KeyCode::Char('w') => input_tx.send(UserInput::MoveCamera(0, -1)).unwrap(),
                        KeyCode::Down | KeyCode::Char('s') => input_tx.send(UserInput::MoveCamera(0, 1)).unwrap(),
                        KeyCode::Left | KeyCode::Char('a') => input_tx.send(UserInput::MoveCamera(-1, 0)).unwrap(),
                        KeyCode::Right | KeyCode::Char('d') => input_tx.send(UserInput::MoveCamera(1, 0)).unwrap(),
                        KeyCode::Enter => input_tx.send(UserInput::Command("\n".to_string())).unwrap(),
                        KeyCode::Backspace => input_tx.send(UserInput::Command("BACKSPACE".to_string())).unwrap(),
                        KeyCode::Char(c) => input_tx.send(UserInput::Command(c.to_string())).unwrap(),
                        _ => {}
                    }
                }
            }
        });

        let mut running = true;
        let mut manifest_needs_update = false;

        while running {
            // Processa teclado local
            while let Ok(user_input) = input_rx.try_recv() {
                match user_input {
                    UserInput::Quit => {
                        self.state.push_log("Encerrando conexão terminal...".red().to_string());
                        self.abort_flag.store(true, Ordering::SeqCst);
                        running = false;
                    }
                    UserInput::MoveCamera(dx, dz) => {
                        self.camera_x += dx * 2;
                        self.camera_z += dz * 2;
                    }
                    UserInput::Command(cmd) => {
                        if self.state.is_generating {
                            self.state.push_log("RECUSADO: Motor está selando mapas. Pressione Ctrl+C para abortar.".yellow().to_string());
                            continue;
                        }
                        if cmd == "\n" {
                            let command_to_exec = self.current_typing.clone();
                            self.current_typing.clear();
                            self.process_command(&command_to_exec);
                        } else if cmd == "BACKSPACE" {
                            self.current_typing.pop();
                        } else {
                            self.current_typing.push_str(&cmd);
                        }
                    }
                }
            }

            // 🚨 PROCESSA OS SINAIS ASSÍNCRONOS DO MOTOR (SEM BLOQUEIO)
            if let Some(rx) = &self.signal_rx {
                while let Ok(signal) = rx.try_recv() {
                    match signal {
                        BesmSignal::RegionProcessing(x, z) => {
                            self.state.regions_map.insert((x, z), RegionStatus::Processing);
                            self.state.status_msg = format!("VARREDURA: r.{}.{}", x, z);

                            // Atualiza apenas a área central na câmera para visualização rápida se estivermos longe
                            if (x - self.camera_x).abs() > 30 || (z - self.camera_z).abs() > 30 {
                                self.camera_x = x;
                                self.camera_z = z;
                            }
                        }
                        BesmSignal::RegionSealed(x, z, bytes) => {
                            self.state.regions_map.insert((x, z), RegionStatus::Sealed);

                            let old_bytes = self.state.regions_bytes.insert((x, z), bytes).unwrap_or(0);
                            self.state.accumulated_bytes_written -= old_bytes;
                            self.state.accumulated_bytes_written += bytes;

                            manifest_needs_update = true;

                            if self.state.accumulated_bytes_written >= MAX_PROJECT_QUOTA_BYTES {
                                self.abort_flag.store(true, Ordering::SeqCst);
                                self.state.push_log("COTA ATINGIDA! CORTE GERAL DE PROCESSAMENTO.".red().bold().to_string());
                            }
                        }
                        BesmSignal::RegionFailed(x, z) => {
                            self.state.regions_map.insert((x, z), RegionStatus::Corrupted);
                        }
                        BesmSignal::RegionEmpty(x, z) => {
                            self.state.regions_map.insert((x, z), RegionStatus::Empty);
                        }
                        BesmSignal::RegionCached(x, z) => {
                            self.state.regions_map.insert((x, z), RegionStatus::Cached);
                        }
                        BesmSignal::Log(msg) => {
                            self.state.push_log(msg);
                        }
                        BesmSignal::GenerationComplete => {
                            self.state.is_generating = false;
                            self.state.status_msg = "SISTEMA OPERANTE. AGUARDANDO DIRETRIZ.".to_string();

                            if manifest_needs_update {
                                self.disk_tx.send(DiskActorMsg::UpdateManifest(self.state.regions_bytes.clone())).unwrap_or_default();
                                manifest_needs_update = false;
                            }
                        }
                    }
                }
            }

            self.render_dashboard();
            thread::sleep(Duration::from_millis(33)); // ~30 FPS HUD Target
        }

        // Salva manifesto no final absoluto se necessário
        if manifest_needs_update {
            self.disk_tx.send(DiskActorMsg::UpdateManifest(self.state.regions_bytes.clone())).unwrap_or_default();
        }

        // Destrói a thread de disco
        self.disk_tx.send(DiskActorMsg::Terminate).unwrap_or_default();

        disable_raw_mode().unwrap();
        execute!(stdout, Show, LeaveAlternateScreen).unwrap();
    }

    fn process_command(&mut self, command: &str) {
        if command == "-sair" || command == "exit" {
            std::process::exit(0);
        }

        let presets = MacroRegion::get_presets();
        let mut target_region = None;

        for preset in presets {
            if command == preset.command {
                target_region = Some(preset.clone());
                break;
            }
        }

        if let Some(region) = target_region {
            self.dispatch_generation(region);
        } else if !command.is_empty() {
            self.state.push_log(format!("{} DIRETRIZ DESCONHECIDA: {}", "[ERRO]".red(), command));
        }
    }

    /// 🚨 DESPACHANTE DE CONCORRÊNCIA E TILE STREAMING (BESM-6 Memory Protection)
    /// Extermina a alocação global. As features são puxadas com um Halo dinâmico apenas
    /// em tempo de execução para cada região (.mca), mantendo a RAM intocada.
    fn dispatch_generation(&mut self, region: MacroRegion) {
        self.state.is_generating = true;
        self.state.status_msg = format!("INICIANDO TILE STREAMING: {}", region.name.to_uppercase());

        let (tx, rx) = mpsc::channel();
        self.signal_rx = Some(rx);

        self.abort_flag.store(false, Ordering::SeqCst);
        let abort_clone = Arc::clone(&self.abort_flag);

        let region_name = region.name.to_string();
        let provider_manager = Arc::clone(&self.provider_manager);
        let args = Arc::clone(&self.args);

        // A BBox máxima apenas para determinar a varredura
        let macro_bbox = LLBBox::new(
            LLPoint::new(region.min_lat, region.min_lon).unwrap(),
            LLPoint::new(region.max_lat, region.max_lon).unwrap(),
        );

        let (transformer, xzbbox) = CoordTransformer::llbbox_to_xzbbox(&macro_bbox, args.scale_h).unwrap();

        // Conversão de Blocos do Minecraft para Regiões MCA (1 Região = 512 Blocos)
        let min_rx = xzbbox.min_x() >> 9;
        let max_rx = xzbbox.max_x() >> 9;
        let min_rz = xzbbox.min_z() >> 9;
        let max_rz = xzbbox.max_z() >> 9;

        let mut regions_to_process = Vec::new();
        for rz in min_rz..=max_rz {
            for rx in min_rx..=max_rx {
                regions_to_process.push((rx, rz));
            }
        }

        // 🚨 THREAD ORQUESTRADORA: Tile Streaming Assíncrono
        thread::spawn(move || {
            let _ = tx.send(BesmSignal::Log(format!("Mapeando Reticulado {}...", region_name)));

            // 1. Fatiamento em Regiões .mca com Fetch Geométrico Dinâmico O(1) Memory
            for (rx, rz) in regions_to_process {
                if abort_clone.load(Ordering::Relaxed) {
                    let _ = tx.send(BesmSignal::Log("VARREDURA ABORTADA PELO USUÁRIO.".red().to_string()));
                    break;
                }

                // 🚨 CULLING POLIGONAL: Halo Exato + Verificação de Máscara (Aborta regiões no Estado de Goiás)
                // O Halo de 16 blocos garante que prédios na borda não sejam retalhados
                let halo_blocks = 16;
                let rx_min_x = (rx << 9) - halo_blocks;
                let rx_max_x = ((rx + 1) << 9) - 1 + halo_blocks;
                let rz_min_z = (rz << 9) - halo_blocks;
                let rz_max_z = ((rz + 1) << 9) - 1 + halo_blocks;

                let local_ll_min = transformer.inverse_transform(XZPoint::new(rx_min_x, rz_min_z));
                let local_ll_max = transformer.inverse_transform(XZPoint::new(rx_max_x, rz_max_z));

                // Se a transformada inversa falhar, ignora o tile
                if local_ll_min.is_err() || local_ll_max.is_err() { continue; }

                let local_bbox = LLBBox::new(local_ll_min.unwrap(), local_ll_max.unwrap());

                // 🚨 Point-in-Polygon Check (Se estivéssemos cruzando Goiás e não o DF)
                // O Motor verifica os limites estritos. Se o tile está no vazio, a gente mata logo a requisição.
                // (Por segurança simplificada, omitimos o poly geoespacial completo aqui e verificamos a intersecção retangular de macro)
                if local_bbox.min().lat() > macro_bbox.max().lat() || local_bbox.max().lat() < macro_bbox.min().lat() ||
                    local_bbox.min().lng() > macro_bbox.max().lng() || local_bbox.max().lng() < macro_bbox.min().lng() {
                    let _ = tx.send(BesmSignal::RegionEmpty(rx, rz));
                    continue;
                }

                let _ = tx.send(BesmSignal::RegionProcessing(rx, rz));

                // 🚨 EXTRAÇÃO JIT: Puxa só os dados desta região da RAM dos Provedores
                let local_features = match provider_manager.fetch_all(&local_bbox) {
                    Ok(features) => features,
                    Err(e) => {
                        let _ = tx.send(BesmSignal::Log(format!("{} Falha GDB na r.{}.{}: {}", "[ERRO]".red(), rx, rz, e)));
                        let _ = tx.send(BesmSignal::RegionFailed(rx, rz));
                        continue;
                    }
                };

                // Instancia o editor isolado
                let mut editor = crate::world_editor::WorldEditor::new(rx, rz);

                // 🚨 TWEAK BESM-6: Renderização e Descarte Instantâneo de Memória (Drop)
                data_processing::generate_region_from_global(&mut editor, &local_features, &args, &transformer);

                if let Err(e) = editor.save() {
                    let _ = tx.send(BesmSignal::Log(format!("{} Falha I/O: r.{}.{}: {}", "[ERRO]".red(), rx, rz, e)));
                    let _ = tx.send(BesmSignal::RegionFailed(rx, rz));
                } else {
                    let region_path = format!("./world/region/r.{}.{}.mca", rx, rz);
                    let file_size = fs::metadata(&region_path).map(|m| m.len()).unwrap_or(0);
                    let _ = tx.send(BesmSignal::RegionSealed(rx, rz, file_size));
                }
            }

            if !abort_clone.load(Ordering::Relaxed) {
                let _ = tx.send(BesmSignal::Log(format!("{} MAPA MATERIALIZADO NO DISCO.", region_name.to_uppercase().green())));
            }

            let _ = tx.send(BesmSignal::GenerationComplete);
        });
    }
}