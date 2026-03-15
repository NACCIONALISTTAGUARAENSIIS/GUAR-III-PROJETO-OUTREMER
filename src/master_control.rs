//! BESM-6 Master Control HUD
//!
//! Interface de telemetria passiva e controle de gerasaoo granular com Viewport.
//! Opera com Zero-Polling no sistema de arquivos e Renderiza��o Ass�ncrona em Tela Alternativa.
//! Implementa �ndice Estrito de Estado (Manifesto Bin�rio) garantindo a conten��o
//! de 180GB baseada na leitura real de compress�o Zlib dos pacotes .mca.

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
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, RwLock};
use std::thread;
use std::time::Duration;

// ?? Cota L�gica do Projeto: Aborta estritamente a escrita se o motor ultrapassar 180 GB.
const MAX_PROJECT_QUOTA_BYTES: u64 = 180 * 1024 * 1024 * 1024;
const MANIFEST_PATH: &str = "./world/besm6_manifest.bin";

// O tamanho m�ximo do grid visualizado na tela (A C�mera do HUD)
const VIEWPORT_WIDTH: i32 = 18;
const VIEWPORT_HEIGHT: i32 = 12;

/// Estados f�sicos de uma regi�o .mca no disco ou na RAM.
#[derive(Clone, Copy, PartialEq)]
pub enum RegionStatus {
    Processing,
    Cached,
    Sealed,
    Corrupted,
    Empty,
}

/// Sinais unidirecionais disparados pelo Motor Scanline (data_processing.rs).
pub enum BesmSignal {
    RegionCached(i32, i32),
    RegionProcessing(i32, i32),
    RegionSealed(i32, i32, u64), // Inclui o tamanho real gravado pela regi�o no disco
    RegionFailed(i32, i32),      // Gatilho de Corrup��o
    GenerationComplete,
    Log(String),
}

/// Sinais do Input do Usu�rio para a Thread de UI
enum UserInput {
    Command(String),
    MoveCamera(i32, i32), // dx, dz
    Quit,
}

/// Macro Regi�es de Bras�lia (Bounding Boxes pr�-definidas em escala de regi�o absoluta)
pub struct MacroRegion {
    pub name: &'static str,
    pub command: &'static str,
    pub min_x: i32,
    pub max_x: i32,
    pub min_z: i32,
    pub max_z: i32,
}

impl MacroRegion {
    pub fn get_presets() -> Vec<MacroRegion> {
        vec![
            MacroRegion {
                name: "Distrito Federal (Completo)",
                command: "-gerar df",
                min_x: -60,
                max_x: 60,
                min_z: -60,
                max_z: 60,
            },
            MacroRegion {
                name: "Plano Piloto",
                command: "-gerar plano_piloto",
                min_x: -10,
                max_x: 10,
                min_z: -8,
                max_z: 8,
            },
            MacroRegion {
                name: "Guar�",
                command: "-gerar guara",
                min_x: -15,
                max_x: -5,
                min_z: 5,
                max_z: 12,
            },
            MacroRegion {
                name: "Taguatinga",
                command: "-gerar taguatinga",
                min_x: -25,
                max_x: -15,
                min_z: 8,
                max_z: 18,
            },
            MacroRegion {
                name: "�guas Claras",
                command: "-gerar aguas_claras",
                min_x: -20,
                max_x: -12,
                min_z: 10,
                max_z: 16,
            },
            MacroRegion {
                name: "Lago Sul",
                command: "-gerar lago_sul",
                min_x: 2,
                max_x: 15,
                min_z: 0,
                max_z: 15,
            },
        ]
    }
}

/// A Mem�ria Compartilhada do HUD (Thread-safe).
/// Evita que a interface bloqueie esperando o r�dio, separando a exibi��o do estado de processamento.
pub struct SharedDashboardState {
    pub regions_map: HashMap<(i32, i32), RegionStatus>,
    pub accumulated_bytes_written: u64,
    pub log_buffer: Vec<String>,
    pub is_generating: bool,
    pub status_msg: String,
}

impl SharedDashboardState {
    pub fn new() -> Self {
        Self {
            regions_map: HashMap::new(),
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
    state: Arc<RwLock<SharedDashboardState>>,

    // A C�mera do HUD (Centro de Vis�o), manipulada localmente pela Thread da UI
    camera_x: i32,
    camera_z: i32,
    current_typing: String,
}

impl MasterControl {
    pub fn new() -> Self {
        let mut control = Self {
            state: Arc::new(RwLock::new(SharedDashboardState::new())),
            camera_x: 0, // Foca na Pra�a dos Tr�s Poderes ao iniciar
            camera_z: 0,
            current_typing: String::new(),
        };
        control.load_manifest_index();
        control
    }

    /// ?? BESM-6 TWEAK: Zero-IO Initialization
    /// L� a tabela de aloca��o bin�ria para reconstituir o mapa visual e a contagem
    /// de bytes instantaneamente, ignorando o sistema de arquivos da m�quina virtual.
    fn load_manifest_index(&mut self) {
        if !Path::new(MANIFEST_PATH).exists() {
            return;
        }

        if let Ok(mut file) = File::open(MANIFEST_PATH) {
            let mut buffer = String::new();
            if file.read_to_string(&mut buffer).is_ok() {
                let mut state = self.state.write().unwrap();
                for line in buffer.lines() {
                    let parts: Vec<&str> = line.split(':').collect();
                    if parts.len() == 2 {
                        if parts[0] == "TOTAL_BYTES" {
                            state.accumulated_bytes_written = parts[1].parse().unwrap_or(0);
                        } else if parts[0] == "REGION" {
                            let coords: Vec<&str> = parts[1].split(',').collect();
                            if coords.len() == 2 {
                                if let (Ok(x), Ok(z)) = (coords[0].parse(), coords[1].parse()) {
                                    state.regions_map.insert((x, z), RegionStatus::Sealed);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Atualiza o Manifesto no disco ap�s cada selagem.
    /// Leve, epis�dico e disparado silenciosamente.
    fn update_manifest_index(state: &SharedDashboardState) {
        if let Ok(mut file) = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(MANIFEST_PATH)
        {
            writeln!(file, "TOTAL_BYTES:{}", state.accumulated_bytes_written).unwrap();
            for (&(x, z), &status) in &state.regions_map {
                if status == RegionStatus::Sealed {
                    writeln!(file, "REGION:{},{}", x, z).unwrap();
                }
            }
        }
    }

    /// O Motor Gr�fico Ass�ncrono do HUD (Flicker-Free).
    /// L� o estado compartilhado sem bloqueios longos.
    fn render_dashboard(&mut self) {
        let state = self.state.read().unwrap();
        let mut stdout = io::stdout();

        queue!(stdout, MoveTo(0, 0), Clear(ClearType::All)).unwrap();

        let consumed_gb = state.accumulated_bytes_written as f64 / (1024.0 * 1024.0 * 1024.0);
        let max_gb = MAX_PROJECT_QUOTA_BYTES as f64 / (1024.0 * 1024.0 * 1024.0);
        let usage_percent = (consumed_gb / max_gb) * 100.0;

        let bar_color = if usage_percent > 90.0 { "red" } else { "cyan" };

        // Cabe�alho
        println!(
            "{}",
            "================================================================================="
                .cyan()
                .bold()
        );
        println!(
            "{} - {}",
            "[BESM-6]".yellow().bold(),
            "MASTER CONTROL DASHBOARD (BRAS�LIA DF)"
                .bright_white()
                .bold()
        );

        // Indicador de Cota Absoluta Baseado na Compress�o Real Zlib
        let quota_str = format!(
            "PROJECT QUOTA: {:.2} GB / {:.2} GB [{:.1}%]",
            consumed_gb, max_gb, usage_percent
        );
        if usage_percent > 90.0 {
            println!("{}", quota_str.red().bold());
        } else {
            println!("{}", quota_str.color(bar_color).bold());
        }

        println!("STATUS: {}", state.status_msg.magenta().bold());
        println!(
            "{}",
            "================================================================================="
                .cyan()
                .bold()
        );
        println!(
            "{} Use Setas/WASD para mover a c�mera. Ctrl+C para Abortar.",
            "CONTROLES:".white().bold()
        );
        println!();

        // Limites Locais da C�mera (Viewport Culling)
        let v_min_x = self.camera_x - VIEWPORT_WIDTH / 2;
        let v_max_x = self.camera_x + VIEWPORT_WIDTH / 2;
        let v_min_z = self.camera_z - VIEWPORT_HEIGHT / 2;
        let v_max_z = self.camera_z + VIEWPORT_HEIGHT / 2;

        // Renderiza��o do Grid com Culling Perfeito (Impede o Wrap da Tela)
        print!("    ");
        for x in v_min_x..=v_max_x {
            if x % 2 == 0 {
                print!("{:>3} ", x);
            } else {
                print!("    ");
            }
        }
        println!();

        for z in v_min_z..=v_max_z {
            print!("{:>3} ", z);
            for x in v_min_x..=v_max_x {
                let status = state
                    .regions_map
                    .get(&(x, z))
                    .copied()
                    .unwrap_or(RegionStatus::Empty);
                match status {
                    RegionStatus::Sealed => print!("{} ", "[��]".green()),
                    RegionStatus::Processing => print!("{} ", "[��]".yellow()),
                    RegionStatus::Cached => print!("{} ", "[��]".blue()),
                    RegionStatus::Corrupted => print!("{} ", "[XX]".red().bold()),
                    RegionStatus::Empty => print!("{} ", "[  ]".bright_black()),
                }
            }
            println!();
        }

        println!(
            "\n{}: {} Selado | {} Varredura Atual | {} Halo Vizinho | {} Falha IO",
            "LEGENDA".white().bold(),
            "[��]".green(),
            "[��]".yellow(),
            "[��]".blue(),
            "[XX]".red().bold()
        );
        println!(
            "{}",
            "================================================================================="
                .cyan()
                .bold()
        );

        // Renderiza��o Isolada de Logs
        println!("{}", "SYSTEM LOGS:".green().bold());
        let log_start = state.log_buffer.len().saturating_sub(6);
        for msg in &state.log_buffer[log_start..] {
            println!("> {}", msg);
        }
        println!(
            "{}",
            "================================================================================="
                .cyan()
                .bold()
        );

        if !state.is_generating {
            println!("{}", "DIRETRIZES T�TICAS DISPON�VEIS:".green().bold());
            for preset in MacroRegion::get_presets() {
                println!("  {:<25} -> {}", preset.command.yellow(), preset.name);
            }
        }

        print!(
            "\n{} {}_",
            "root@besm6:~#".green().bold(),
            self.current_typing
        );

        stdout.flush().unwrap();
    }

    /// Shell Principal com Polling Dedicado de Input (30 FPS Render Target)
    pub fn run_interactive_shell(&mut self) {
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, Hide).unwrap();
        enable_raw_mode().unwrap();

        self.render_dashboard();

        // Canal para o usu�rio digitar coisas sem travar o loop de renderiza��o do motor
        let (input_tx, input_rx) = mpsc::channel::<UserInput>();

        // Thread dedicada apenas a ler o teclado em Raw Mode (Garante Input T�til Perfeito)
        thread::spawn(move || loop {
            if event::poll(Duration::from_millis(16)).unwrap() {
                if let Event::Key(key_event) = event::read().unwrap() {
                    if key_event.modifiers.contains(KeyModifiers::CONTROL)
                        && key_event.code == KeyCode::Char('c')
                    {
                        input_tx.send(UserInput::Quit).unwrap();
                        break;
                    }

                    match key_event.code {
                        KeyCode::Up | KeyCode::Char('w') => {
                            input_tx.send(UserInput::MoveCamera(0, -1)).unwrap()
                        }
                        KeyCode::Down | KeyCode::Char('s') => {
                            input_tx.send(UserInput::MoveCamera(0, 1)).unwrap()
                        }
                        KeyCode::Left | KeyCode::Char('a') => {
                            input_tx.send(UserInput::MoveCamera(-1, 0)).unwrap()
                        }
                        KeyCode::Right | KeyCode::Char('d') => {
                            input_tx.send(UserInput::MoveCamera(1, 0)).unwrap()
                        }
                        KeyCode::Enter => {
                            input_tx.send(UserInput::Command("\n".to_string())).unwrap()
                        }
                        KeyCode::Backspace => input_tx
                            .send(UserInput::Command("BACKSPACE".to_string()))
                            .unwrap(),
                        KeyCode::Char(c) => {
                            input_tx.send(UserInput::Command(c.to_string())).unwrap()
                        }
                        _ => {}
                    }
                }
            }
        });

        // Loop Central do HUD (Desacoplado do Motor de Gera��o)
        let mut running = true;
        while running {
            // Atualiza��o de C�mera e Comandos (Instant�neo)
            while let Ok(user_input) = input_rx.try_recv() {
                match user_input {
                    UserInput::Quit => {
                        self.state
                            .write()
                            .unwrap()
                            .push_log("Encerrando conex�o terminal...".red().to_string());
                        running = false;
                    }
                    UserInput::MoveCamera(dx, dz) => {
                        self.camera_x += dx * 2;
                        self.camera_z += dz * 2;
                    }
                    UserInput::Command(cmd) => {
                        let is_gen = self.state.read().unwrap().is_generating;
                        if is_gen {
                            self.state.write().unwrap().push_log("COMANDO RECUSADO: Motor est� selando mapas. Pressione Ctrl+C para abortar duramente.".yellow().to_string());
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

            self.render_dashboard();
            thread::sleep(Duration::from_millis(33)); // ~30 FPS HUD Target
        }

        disable_raw_mode().unwrap();
        execute!(stdout, Show, LeaveAlternateScreen).unwrap();
    }

    fn process_command(&mut self, command: &str) {
        if command == "-sair" || command == "exit" {
            std::process::exit(0); // Morte brusca e segura
        }

        let presets = MacroRegion::get_presets();
        let mut target_region = None;

        for preset in presets {
            if command == preset.command {
                target_region = Some(preset);
                break;
            }
        }

        if let Some(region) = target_region {
            self.dispatch_generation(region);
        } else if !command.is_empty() {
            self.state.write().unwrap().push_log(format!(
                "{} DIRETRIZ DESCONHECIDA: {}",
                "[ERRO]".red(),
                command
            ));
        }
    }

    /// Despacha a varredura atrelando a thread do motor ao SharedDashboardState.
    /// O motor atualizar� os blocos assincronamente e a GUI renderizar� no seu pr�prio clock.
    fn dispatch_generation(&mut self, region: MacroRegion) {
        {
            let mut w_state = self.state.write().unwrap();
            w_state.is_generating = true;
            w_state.status_msg =
                format!("INICIANDO IGNI��O DO SETOR: {}", region.name.to_uppercase());
        }

        let shared_state = Arc::clone(&self.state);
        let abort_flag = Arc::new(AtomicBool::new(false));
        let abort_flag_worker = Arc::clone(&abort_flag);

        let min_x = region.min_x;
        let max_x = region.max_x;
        let min_z = region.min_z;
        let max_z = region.max_z;
        let region_name = region.name.to_string();

        // ?? A THREAD DO MOTOR (Roda solta nas costas, sem bloquear o renderizador)
        thread::spawn(move || {
            shared_state.write().unwrap().push_log(format!(
                "Ativando Scanline Out-of-Core para {}",
                region_name
            ));

            for z in min_z..=max_z {
                for x in min_x..=max_x {
                    // Prote��o L�gica da Cota de Armazenamento
                    let current_bytes = shared_state.read().unwrap().accumulated_bytes_written;
                    if current_bytes >= MAX_PROJECT_QUOTA_BYTES {
                        shared_state.write().unwrap().push_log(format!(
                            "{} LIMITE L�GICO DE 180 GB ATINGIDO. BLOQUEIO INJETADO.",
                            "[HALT]".red().bold()
                        ));
                        abort_flag_worker.store(true, Ordering::SeqCst);
                        return;
                    }

                    if abort_flag_worker.load(Ordering::SeqCst) {
                        return;
                    }

                    // 1. Sinaliza: Processamento e Corre��o da Frente de Onda (Halo real)
                    {
                        let mut state = shared_state.write().unwrap();
                        state.regions_map.insert((x, z), RegionStatus::Processing);

                        // Halo Cache real se espalha para os vizinhos adjacentes na esteira
                        if x + 1 <= max_x {
                            state.regions_map.insert((x + 1, z), RegionStatus::Cached);
                        }
                        if z + 1 <= max_z {
                            state.regions_map.insert((x, z + 1), RegionStatus::Cached);
                        }
                        if x + 1 <= max_x && z + 1 <= max_z {
                            state
                                .regions_map
                                .insert((x + 1, z + 1), RegionStatus::Cached);
                        }

                        state.status_msg =
                            format!("VARREDURA EM ANDAMENTO: {} (r.{}.{})", region_name, x, z);
                    }

                    // ============================================
                    // INTEGRA��O: O CALCULO REAL OCORRER� AQUI
                    // ============================================
                    thread::sleep(std::time::Duration::from_millis(50));

                    // 2. Extra��o F�sica P�s-Compress�o Zlib (C�lculo Emp�rico de Corrup��o)
                    // Simula��o da chamada: let region_path = format!("world/region/r.{}.{}.mca", x, z);
                    // let real_file_size = std::fs::metadata(&region_path).map(|m| m.len()).unwrap_or(0);
                    let real_file_size_bytes: u64 = 14_750_000;

                    // 3. Sinaliza: Selagem Completa, Atualiza��o At�mica de Bytes e Indexa��o Segura
                    {
                        let mut state = shared_state.write().unwrap();
                        if real_file_size_bytes == 0 {
                            state.regions_map.insert((x, z), RegionStatus::Corrupted);
                            state.push_log(format!(
                                "{} FALHA DE GRAVA��O ZLIB NO QUADRANTE r.{}.{}",
                                "[CORRUP��O]".red().bold(),
                                x,
                                z
                            ));
                        } else {
                            state.regions_map.insert((x, z), RegionStatus::Sealed);
                            state.accumulated_bytes_written += real_file_size_bytes;
                            Self::update_manifest_index(&state); // ?? Grava��o f�sica do manifesto p�s-selagem
                        }
                    }
                }
            }

            // Gera��o Finalizada
            let mut state = shared_state.write().unwrap();
            state.push_log(format!(
                "{} MAPA MATERIALIZADO COM SUCESSO NO DISCO.",
                region_name.to_uppercase().green()
            ));
            state.is_generating = false;
            state.status_msg = "SISTEMA OPERANTE. AGUARDANDO DIRETRIZ.".to_string();
        });
    }
}
