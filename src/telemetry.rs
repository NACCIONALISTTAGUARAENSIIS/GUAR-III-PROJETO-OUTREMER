//! Telemetry Management (BESM-6 Government Tier).
//!
//! Módulo responsável pelo envio de métricas de geração e relatórios de crash.
//! Reescrito para compatibilidade total com `panic="abort"` (Release Mode),
//! eliminando vazamentos de memória (OOM) no canal MPSC e adotando I/O Assíncrono (Tokio).

use log::error;
use once_cell::sync::Lazy;
use serde::Serialize;
use std::backtrace::Backtrace;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::{self, Sender};
use uuid::Uuid;

/// 🚨 BESM-6: Telemetry endpoint URL (Atualizado para a arquitetura Pincelism)
const TELEMETRY_URL: &str = "https://telemetry.pincelism.com/api/v1/report";

/// Global flag to store user's telemetry consent
static TELEMETRY_CONSENT: AtomicBool = AtomicBool::new(false);

/// 🚨 BESM-6: Identificador de Sessão Único (UUID v4 Criptograficamente Seguro)
/// Essencial para vincular os logs normais ao Crash Report final sem colisões de thread_rng().
static SESSION_ID: Lazy<String> = Lazy::new(|| Uuid::new_v4().to_string());

/// Caches de alocação estática para evitar reconstrução de strings a cada log
static APP_VERSION: Lazy<String> = Lazy::new(|| env!("CARGO_PKG_VERSION").to_string());
static PLATFORM: Lazy<String> = Lazy::new(|| {
    match std::env::consts::OS {
        "windows" => "windows",
        "linux" => "linux",
        "macos" => "macos",
        _ => "unknown",
    }.to_string()
});

/// Sets the user's telemetry consent preference
pub fn set_telemetry_consent(consent: bool) {
    TELEMETRY_CONSENT.store(consent, Ordering::Relaxed);
}

/// Gets the user's telemetry consent preference
fn get_telemetry_consent() -> bool {
    TELEMETRY_CONSENT.load(Ordering::Relaxed)
}

// ============================================================================
// 🚨 BESM-6 PAYLOADS (Owned Strings para atravessar o canal em segurança)
// ============================================================================

#[derive(Serialize)]
struct CrashReport {
    r#type: &'static str,
    session_id: &'static str,
    error_message: String,
    stack_trace: String,
    platform: &'static str,
    app_version: &'static str,
}

#[derive(Serialize)]
struct GenerationClick {
    r#type: &'static str,
    session_id: &'static str,
}

#[derive(Serialize)]
struct LogEntry {
    r#type: &'static str,
    session_id: &'static str,
    log_level: &'static str,
    log_message: String,
    platform: &'static str,
    app_version: &'static str,
}

enum TelemetryEvent {
    Log(LogEntry),
    Click(GenerationClick),
}

// ============================================================================
// 🚨 O CANAL TOKIO MPSC: Assíncrono, Limite de Backpressure e Zero Vazamento
// Usamos Tokio MPSC com buffer restrito (1000 mensagens) para impedir que um
// estrangulamento da API trave a RAM do sistema.
// ============================================================================

static TELEMETRY_TX: Lazy<Sender<TelemetryEvent>> = Lazy::new(|| {
    // 🚨 Buffer de 1000 mensagens. Se lotar, as novas são descartadas em prol da estabilidade.
    let (tx, mut rx) = mpsc::channel::<TelemetryEvent>(1000);

    // O worker roda no runtime do Tokio (fornecido nativamente pela base do Tauri)
    tokio::spawn(async move {
        // Cliente assíncrono persistente (Reaproveitamento de Pool TCP/TLS)
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        while let Some(event) = rx.recv().await {
            let payload = match event {
                TelemetryEvent::Log(log) => serde_json::to_value(&log),
                TelemetryEvent::Click(click) => serde_json::to_value(&click),
            };

            if let Ok(json_payload) = payload {
                // Fire and forget assíncrono. Não bloqueia a extração do próximo log do canal.
                let client_clone = client.clone();
                tokio::spawn(async move {
                    let _ = client_clone.post(TELEMETRY_URL).json(&json_payload).send().await;
                });
            }
        }
    });

    tx
});

// ============================================================================
// FUNÇÕES DE DISPARO (O(1) sem alocação bloqueante)
// ============================================================================

/// Sends a crash report to the telemetry server
/// 🚨 Roda de forma SÍNCRONA e BLOQUEANTE (Reqwest Blocking) apenas para o momento da morte.
/// Em `panic="abort"`, a thread morre na mesma instrução. A única forma de despachar a caixa preta
/// é travar a destruição até o SO confirmar o pacote TCP (ou bater o timeout de 3 segundos).
fn send_crash_report(error_message: String, stack_trace: String) {
    let _ = (|| -> Result<(), reqwest::Error> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(3)) // 3 Segundos para o último fôlego
            .build()?;

        let payload = CrashReport {
            r#type: "crash",
            session_id: &SESSION_ID,
            error_message,
            stack_trace,
            platform: &PLATFORM,
            app_version: &APP_VERSION,
        };

        let _res = client
            .post(TELEMETRY_URL)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()?;

        Ok(())
    })();
}

/// Sends a generation click event to the telemetry server
pub fn send_generation_click() {
    if !get_telemetry_consent() || cfg!(debug_assertions) {
        return;
    }

    let payload = GenerationClick {
        r#type: "generation_click",
        session_id: &SESSION_ID,
    };

    // 🚨 Tenta enviar sem bloquear. Se o buffer do Tokio (1000) estiver cheio, ignora.
    let _ = TELEMETRY_TX.try_send(TelemetryEvent::Click(payload));
}

/// Log levels for telemetry
#[allow(dead_code)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

impl LogLevel {
    fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
        }
    }
}

/// 🚨 Trunca strings a nível de bytes sem invalidar UTF-8 (Performance O(1))
fn truncate_safe(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut boundary = max_bytes;
    while !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    s[..boundary].to_string()
}

/// Sends a log entry to the telemetry server
pub fn send_log(level: LogLevel, message: &str) {
    if !get_telemetry_consent() || cfg!(debug_assertions) {
        return;
    }

    // Truncamento veloz na camada de bytes (O(1)) vs iterador de chars lento
    let truncated_message = truncate_safe(message, 1024);

    let payload = LogEntry {
        r#type: "log",
        session_id: &SESSION_ID,
        log_level: level.as_str(),
        log_message: truncated_message,
        platform: &PLATFORM,
        app_version: &APP_VERSION,
    };

    // 🚨 Despacho não-bloqueante para a fila do Tokio.
    let _ = TELEMETRY_TX.try_send(TelemetryEvent::Log(payload));
}

/// Installs a panic hook that logs panics and sends crash reports
/// 🚨 Adaptado para arquitetura `panic="abort"`. Não há unwinding. O processo
/// será obliterado logo após a execução desta closure.
pub fn install_panic_hook() {
    panic::set_hook(Box::new(|panic_info| {
        error!("Application panicked: {:?}", panic_info);

        if let Some(location) = panic_info.location() {
            if location.file().contains("panicking.rs") {
                return;
            }
        }

        if !get_telemetry_consent() || cfg!(debug_assertions) {
            return;
        }

        let payload = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Unknown panic".to_string()
        };

        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let error_message = truncate_safe(&format!("{} @ {}", payload, location), 512);

        // 🚨 Captura do Rastro Completo da Pilha (Stack Trace)
        // Necessário RUST_BACKTRACE=1 no ambiente para ativação total
        let backtrace = Backtrace::capture();
        let stack_trace = format!("{}", backtrace);

        // Disparo Síncrono de Emergência (Garante o envio antes do OS dar SIGKILL)
        send_crash_report(error_message, stack_trace);
    }));
}