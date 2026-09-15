//! Servidor de inspeção visual: serve um relevo 3D do mundo INTEIRO já gerado,
//! num navegador local (`--view-world <PASTA_DO_MUNDO>`), complementando o
//! módulo de testes (`tests/multi_provider_smoke.rs`) e o extrator de voxels
//! avulso usado nesta sessão para os relatórios de campo do Guará.
//!
//! 🚨 BESM-6: por que um relevo amostrado, não um voxel completo. O Guará I+II
//! inteiro tem ~50 milhões de colunas (x,z) — renderizar cada uma como um voxel
//! individual no navegador (o que o visualizador de recorte pequeno já faz,
//! artefato à parte) estouraria a memória de qualquer WebGL real. Este servidor
//! computa, uma vez na inicialização, a altura+cor do bloco do TOPO de cada
//! coluna (reaproveitando exatamente o mesmo scanner de NBT/paleta que já
//! gera o minimapa 2D do GUI — ver `map_renderer::compute_heightfield`),
//! amostra numa grade cujo tamanho é limitado por um orçamento de células, e
//! serve isso como uma única malha de relevo (não uma foto plana: a altura de
//! cada vértice é a altura real do bloco) — o suficiente para orbitar e ver a
//! skyline e o traçado urbano do mundo inteiro em 3D de verdade.
//!
//! Não é um `#[test]`: sobe um servidor HTTP persistente (até `Ctrl+C`), o que
//! não se encaixa no contrato de um teste que precisa terminar sozinho.
//! Interceptado cru em `main()` antes do parsing normal do clap (ver o
//! comentário em `main.rs`), porque este modo não usa `--bbox` (obrigatório em
//! todo outro modo) e mudar isso exigiria tornar `Args::bbox` opcional em toda
//! a base de código só para este caso.

use crate::map_renderer;
use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

/// Sentinela de "sem dado" para colunas fora da área realmente gerada dentro
/// do retângulo delimitador (buracos reais, não um erro) — bem longe de
/// qualquer Y real do Minecraft (-64..320).
const NO_DATA: i32 = -999_999;

/// Orçamento de células da grade de relevo. ~350k células é o ponto de
/// equilíbrio encontrado nesta sessão entre nitidez (ainda dá pra distinguir
/// quarteirões e vias) e um payload/malha que o navegador monta em segundos,
/// não minutos, mesmo para o Guará I+II inteiro (~50M colunas reais).
const TARGET_CELLS: u64 = 350_000;

#[derive(Serialize)]
struct HeightfieldResponse {
    #[serde(rename = "originX")]
    origin_x: i32,
    #[serde(rename = "originZ")]
    origin_z: i32,
    width: u32,
    height: u32,
    step: u32,
    #[serde(rename = "noData")]
    no_data: i32,
    /// Linha a linha (row-major, `dz * width + dx`): `[y, r, g, b]`.
    cells: Vec<[i32; 4]>,
}

#[derive(serde::Deserialize)]
struct WorldMetadataRaw {
    #[serde(rename = "minMcX")]
    min_mc_x: i32,
    #[serde(rename = "maxMcX")]
    max_mc_x: i32,
    #[serde(rename = "minMcZ")]
    min_mc_z: i32,
    #[serde(rename = "maxMcZ")]
    max_mc_z: i32,
}

/// Descobre os limites do mundo em blocos absolutos. Prefere `metadata.json`
/// (gravado pelo próprio motor ao final da geração, `WorldMetadata`); se
/// ausente (mundo gerado por uma versão antiga, ou copiado sem o arquivo),
/// cai para inferir a partir dos nomes dos arquivos `region/r.<rx>.<rz>.mca`
/// (cada região cobre exatamente 512×512 blocos).
fn discover_bounds(world_dir: &Path) -> Result<(i32, i32, i32, i32), String> {
    let metadata_path = world_dir.join("metadata.json");
    if let Ok(raw) = std::fs::read_to_string(&metadata_path) {
        if let Ok(m) = serde_json::from_str::<WorldMetadataRaw>(&raw) {
            return Ok((m.min_mc_x, m.max_mc_x, m.min_mc_z, m.max_mc_z));
        }
    }

    let region_dir = world_dir.join("region");
    let entries = std::fs::read_dir(&region_dir)
        .map_err(|e| format!("Falha ao ler {}: {e}", region_dir.display()))?;

    let mut min_rx = i32::MAX;
    let mut max_rx = i32::MIN;
    let mut min_rz = i32::MAX;
    let mut max_rz = i32::MIN;
    let mut found = false;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix("r.") else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(".mca") else {
            continue;
        };
        let mut parts = rest.split('.');
        let (Some(rx_str), Some(rz_str)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(rx), Ok(rz)) = (rx_str.parse::<i32>(), rz_str.parse::<i32>()) else {
            continue;
        };
        min_rx = min_rx.min(rx);
        max_rx = max_rx.max(rx);
        min_rz = min_rz.min(rz);
        max_rz = max_rz.max(rz);
        found = true;
    }

    if !found {
        return Err(format!(
            "Nenhum arquivo de região .mca válido encontrado em {}",
            region_dir.display()
        ));
    }

    Ok((
        min_rx * 512,
        max_rx * 512 + 511,
        min_rz * 512,
        max_rz * 512 + 511,
    ))
}

/// Escolhe o menor fator de downsample (potência de 2) que mantém o total de
/// células dentro de `TARGET_CELLS` — mesmo espírito do auto-downsample de
/// `map_renderer::render_world_map`, mas orçado por células, não por pixels de
/// imagem (uma área muito comprida-e-estreita teria um limite de pixel bem
/// diferente de um limite de célula útil para a malha 3D).
fn pick_downsample_factor(width_blocks: u64, height_blocks: u64) -> u32 {
    let mut factor: u64 = 1;
    while (width_blocks / factor).max(1) * (height_blocks / factor).max(1) > TARGET_CELLS {
        factor *= 2;
    }
    factor as u32
}

fn build_heightfield_json(world_dir: &Path) -> Result<String, String> {
    let (min_x, max_x, min_z, max_z) = discover_bounds(world_dir)?;
    let width_blocks = (max_x - min_x + 1).max(1) as u64;
    let height_blocks = (max_z - min_z + 1).max(1) as u64;
    let step = pick_downsample_factor(width_blocks, height_blocks);

    println!(
        "[INFO] 🗺️  Escaneando relevo do mundo: {}×{} blocos, amostrado a cada {} bloco(s)...",
        width_blocks, height_blocks, step
    );

    let raw_cells = map_renderer::compute_heightfield(world_dir, min_x, max_x, min_z, max_z, step);

    let width = width_blocks.div_ceil(step as u64) as u32;
    let height = height_blocks.div_ceil(step as u64) as u32;

    let mut grid = vec![[NO_DATA, 0, 0, 0]; (width as usize) * (height as usize)];
    for (world_x, world_z, world_y, [r, g, b]) in raw_cells {
        let dx = (world_x - min_x) / step as i32;
        let dz = (world_z - min_z) / step as i32;
        if dx < 0 || dz < 0 || dx as u32 >= width || dz as u32 >= height {
            continue;
        }
        let idx = (dz as usize) * (width as usize) + (dx as usize);
        grid[idx] = [world_y, r as i32, g as i32, b as i32];
    }

    println!(
        "[INFO] ✅ Relevo pronto: {} células ({}×{}).",
        grid.len(),
        width,
        height
    );

    let response = HeightfieldResponse {
        origin_x: min_x,
        origin_z: min_z,
        width,
        height,
        step,
        no_data: NO_DATA,
        cells: grid,
    };

    serde_json::to_string(&response).map_err(|e| format!("Falha ao serializar relevo: {e}"))
}

const VIEWER_PAGE: &str = include_str!("world_viewer_page.html");

fn write_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// Trata uma única conexão HTTP/1.1: lê só a linha de requisição (método +
/// caminho), ignora cabeçalhos/corpo (o navegador só faz GETs simples aqui),
/// e responde. Servidor de inspeção local só — não foi hardened contra
/// tráfego hostil de propósito (bind em 127.0.0.1, uso manual).
fn handle_connection(mut stream: TcpStream, page: &str, heights_json: &[u8]) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drena e descarta o resto dos cabeçalhos (até a linha em branco) para não
    // deixar lixo na conexão antes de escrever a resposta.
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
        "/heights.json" => write_response(&mut stream, "200 OK", "application/json", heights_json),
        _ => write_response(&mut stream, "404 Not Found", "text/plain", b"not found"),
    }
}

/// Ponto de entrada de `--view-world`. Bloqueia para sempre servindo o mundo
/// em `world_dir`; encerra com `Ctrl+C`.
pub fn serve(world_dir: PathBuf, port: u16) {
    if !world_dir.join("region").is_dir() {
        eprintln!(
            "Erro: {} não parece um diretório de mundo Pincelism válido (falta a pasta 'region/').",
            world_dir.display()
        );
        std::process::exit(1);
    }

    let heights_json = match build_heightfield_json(&world_dir) {
        Ok(json) => json,
        Err(e) => {
            eprintln!("Erro ao computar o relevo do mundo: {e}");
            std::process::exit(1);
        }
    };
    let heights_bytes = heights_json.into_bytes();

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Erro ao abrir o servidor local na porta {port}: {e}");
            std::process::exit(1);
        }
    };
    let actual_port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    println!();
    println!("  🗺️  Visualizador 3D do Pincelism no ar:");
    println!("     http://127.0.0.1:{actual_port}");
    println!();
    println!("  Abra esse endereço no navegador. Ctrl+C aqui encerra o servidor.");
    println!();

    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let page = VIEWER_PAGE;
        let bytes = heights_bytes.clone();
        std::thread::spawn(move || {
            handle_connection(stream, page, &bytes);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_factor_stays_within_budget() {
        // Guará I+II real: ~5704x8833 blocos (~50M colunas) — precisa de um
        // fator > 1 pra caber no orçamento de células.
        let factor = pick_downsample_factor(5704, 8833);
        assert!(factor > 1);
        let cells = (5704u64 / factor as u64) * (8833u64 / factor as u64);
        assert!(cells <= TARGET_CELLS * 4); // folga: a última potência de 2 pode passar um pouco antes de estabilizar
    }

    #[test]
    fn downsample_factor_is_one_for_small_world() {
        // Um recorte de teste pequeno (80x80, como os fixtures desta sessão)
        // não precisa de nenhum downsample.
        assert_eq!(pick_downsample_factor(80, 80), 1);
    }

    #[test]
    fn discover_bounds_falls_back_to_region_filenames() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        let region_dir = tmp.path().join("region");
        std::fs::create_dir_all(&region_dir).unwrap();
        // Duas regiões reais: r.-1.0 cobre X em [-512,-1], r.0.1 cobre Z em [512,1023].
        std::fs::write(region_dir.join("r.-1.0.mca"), b"").unwrap();
        std::fs::write(region_dir.join("r.0.1.mca"), b"").unwrap();

        let (min_x, max_x, min_z, max_z) = discover_bounds(tmp.path()).expect("bounds");
        assert_eq!(min_x, -512);
        assert_eq!(max_x, 511);
        assert_eq!(min_z, 0);
        assert_eq!(max_z, 1023);
    }
}
