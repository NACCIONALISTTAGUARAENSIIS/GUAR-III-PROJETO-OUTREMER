//! Servidor de inspeção visual: renderiza um recorte retangular do mundo em
//! VOXELS DE VERDADE (`--view-world-voxels <PASTA_DO_MUNDO> --crop <minX>
//! <minZ> <maxX> <maxZ>`), complementando `world_viewer.rs` (relevo
//! amostrado do mundo inteiro, rápido mas só a superfície) e
//! `bluemap_viewer.rs` (render real via ferramenta externa).
//!
//! 🚨 BESM-6: por que este módulo existe apesar de já termos dois
//! visualizadores. Nesta sessão o usuário pediu explicitamente pra ver o
//! mundo gerado "em detalhe" — cada bloco, parede, estrutura — pra poder
//! iterar no código de geração do Pincelism depois. `--view-world` (relevo
//! amostrado) mostra só a cor do bloco do TOPO de cada coluna, sem paredes
//! nem estrutura interna. `--view-world-bluemap` mostraria blocos reais
//! texturizados, mas se mostrou frágil demais nesta sessão (uma
//! investigação extensa não encontrou a causa raiz de por que o cliente do
//! BlueMap não conseguia renderizar/navegar até a área correta, apesar dos
//! dados no servidor estarem comprovadamente corretos). Este módulo é o
//! caminho totalmente sob nosso controle: sem dependência externa, sem
//! nenhuma lógica de cliente que não escrevemos — reaproveita o mesmo leitor
//! de NBT/paleta já usado pelo minimapa 2D e pelo relevo amostrado
//! (`map_renderer`), mas decodificando TODO bloco de um recorte (não só o
//! topo de cada coluna) via `map_renderer::compute_voxel_crop`.
//!
//! **Por que exige um recorte (`--crop`), não o mundo inteiro:** um render
//! voxel completo cresce em O(largura × altura × profundidade). O Guará
//! I+II inteiro (~5700×9200 blocos) estouraria qualquer navegador. O
//! recorte é deliberadamente uma responsabilidade do usuário (escolher uma
//! área de interesse), com um teto de segurança (`map_renderer::MAX_VOXEL_CELLS`)
//! que recusa recortes grandes demais com uma mensagem explicando o porquê,
//! em vez de travar silenciosamente.
//!
//! **Só blocos com face exposta são enviados** (`extract_exposed_voxels` em
//! `map_renderer.rs`): blocos totalmente cercados por outros blocos opacos
//! nunca aparecem visualmente e só inflariam o payload — mantém isto "leve"
//! (pedido explícito do usuário) sem perder nenhum detalhe visível.
//!
//! Mesmo padrão de servidor manual (thread-per-conexão, bloqueia até
//! `Ctrl+C`) que `world_viewer.rs`/`bluemap_viewer.rs` já estabeleceram.

use crate::map_renderer;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

/// Faixa de altura padrão quando `--min-y`/`--max-y` não são passados.
/// `-64` é o piso absoluto do Minecraft moderno (1.18+). `100` foi
/// escolhido observando o relevo real do Guará I+II nesta sessão (topo do
/// relevo entre Y=-63 e Y=30) com folga suficiente pra qualquer prédio
/// razoável construído acima do chão — não é um limite do formato, só um
/// default conveniente; passe `--min-y`/`--max-y` pra ajustar.
const DEFAULT_MIN_Y: i32 = -64;
const DEFAULT_MAX_Y: i32 = 100;

const VOXEL_VIEWER_PAGE: &str = include_str!("voxel_viewer_page.html");

#[derive(Serialize)]
struct VoxelResponse {
    #[serde(rename = "minX")]
    min_x: i32,
    #[serde(rename = "minY")]
    min_y: i32,
    #[serde(rename = "minZ")]
    min_z: i32,
    #[serde(rename = "maxX")]
    max_x: i32,
    #[serde(rename = "maxY")]
    max_y: i32,
    #[serde(rename = "maxZ")]
    max_z: i32,
    /// `[x, y, z, r, g, b]` por voxel exposto — coordenadas absolutas do
    /// mundo (não relativas ao recorte), pra bater com o que o BlueMap/
    /// relevo caseiro já mostram, facilitando comparar os três.
    voxels: Vec<[i32; 6]>,
}

fn build_voxel_json(
    world_dir: &Path,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    min_y: i32,
    max_y: i32,
) -> Result<String, String> {
    let raw =
        map_renderer::compute_voxel_crop(world_dir, min_x, max_x, min_z, max_z, min_y, max_y)?;
    let voxels: Vec<[i32; 6]> = raw
        .into_iter()
        .map(|(x, y, z, [r, g, b])| [x, y, z, r as i32, g as i32, b as i32])
        .collect();

    println!(
        "[INFO] ✅ Recorte voxel pronto: {} blocos expostos.",
        voxels.len()
    );

    let response = VoxelResponse {
        min_x,
        min_y,
        min_z,
        max_x,
        max_y,
        max_z,
        voxels,
    };
    serde_json::to_string(&response).map_err(|e| format!("Falha ao serializar JSON: {e}"))
}

fn write_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn handle_connection(mut stream: TcpStream, page: &str, voxels_json: &[u8]) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line == "\r\n" || line == "\n" => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    let path = request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .to_string();

    match path.as_str() {
        "/" | "/index.html" => write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            page.as_bytes(),
        ),
        "/voxels.json" => write_response(&mut stream, "200 OK", "application/json", voxels_json),
        _ => write_response(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

/// Ponto de entrada de `--view-world-voxels`. Bloqueia até `Ctrl+C`.
#[allow(clippy::too_many_arguments)]
pub fn serve(
    world_dir: PathBuf,
    min_x: i32,
    min_z: i32,
    max_x: i32,
    max_z: i32,
    min_y: Option<i32>,
    max_y: Option<i32>,
    port: u16,
) {
    if !world_dir.join("region").is_dir() {
        eprintln!(
            "Erro: {} não parece um diretório de mundo Pincelism válido (falta a pasta 'region/').",
            world_dir.display()
        );
        std::process::exit(1);
    }

    let min_y = min_y.unwrap_or(DEFAULT_MIN_Y);
    let max_y = max_y.unwrap_or(DEFAULT_MAX_Y);

    println!(
        "[INFO] 🧊 Extraindo recorte voxel: x[{min_x}..{max_x}] z[{min_z}..{max_z}] y[{min_y}..{max_y}]..."
    );
    let voxels_json = match build_voxel_json(&world_dir, min_x, max_x, min_z, max_z, min_y, max_y) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Erro ao extrair o recorte voxel: {e}");
            std::process::exit(1);
        }
    };
    let voxels_bytes = voxels_json.into_bytes();

    // Bind em todas as interfaces (0.0.0.0), mesmo padrão adotado em
    // `world_viewer.rs` nesta sessão — servidor de inspeção manual, não
    // hardened, só aceitável porque o usuário já expõe outras portas iguais
    // na mesma máquina; para uso estritamente local prefira 127.0.0.1/túnel
    // SSH em vez de expor a porta.
    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Erro ao abrir o servidor na porta {port}: {e}");
            std::process::exit(1);
        }
    };
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    println!();
    println!("  🧊 Visualizador de voxels do Pincelism no ar (todas as interfaces):");
    println!("     http://127.0.0.1:{actual_port}  (local)");
    println!("     http://<ip-desta-máquina>:{actual_port}  (rede/remoto)");
    println!();
    println!("  Abra esse endereço no navegador. Ctrl+C aqui encerra o servidor.");
    println!();

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let page = VOXEL_VIEWER_PAGE;
        let voxels = voxels_bytes.clone();
        std::thread::spawn(move || {
            handle_connection(stream, page, &voxels);
        });
    }
}
