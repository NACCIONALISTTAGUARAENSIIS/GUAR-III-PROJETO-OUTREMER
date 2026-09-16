//! Servidor de inspeção visual: renderiza o mundo INTEIRO gerado (ou
//! qualquer recorte dele) em VOXELS DE VERDADE (`--view-world-voxels
//! <PASTA_DO_MUNDO>`), com navegação em primeira pessoa (voar pelo mundo
//! como um personagem), complementando `world_viewer.rs` (relevo amostrado
//! só da superfície) e `bluemap_viewer.rs` (render real via ferramenta
//! externa).
//!
//! 🚨 BESM-6: por que este módulo virou um servidor de streaming de chunks,
//! não um extrator de recorte único. A primeira versão deste módulo exigia
//! `--crop <minX> <minZ> <maxX> <maxZ>` e pré-computava tudo de uma vez —
//! funcionava, mas o usuário pediu explicitamente o mundo INTEIRO (e
//! qualquer cidade futura gerada), sem nenhum bloco visível omitido, com
//! navegação livre tipo Minecraft. Pré-computar o mundo inteiro em voxels
//! de uma vez é inviável (dezenas de milhões de blocos expostos, gigabytes
//! de JSON) — a solução real, do mesmo jeito que o Minecraft e o BlueMap
//! fazem, é carregar só os chunks (16×16 colunas) perto de onde o jogador
//! está, sob demanda, e descartar os que ficam longe. Isso também resolve
//! "cidades futuras" de graça: o servidor não sabe nada sobre o Guará
//! especificamente, só lê região por região do `world_dir` passado.
//!
//! **"Sem nenhum bloco omitido":** todo bloco não-transparente com pelo
//! menos uma face exposta é enviado, faixa de altura completa do Minecraft
//! moderno por padrão (`-64..320`, ajustável via `--min-y`/`--max-y`). A
//! ÚNICA coisa descartada são blocos totalmente cercados por 6 outros
//! blocos opacos — esses são, por definição, invisíveis de qualquer ângulo
//! possível (nunca aparecem no Minecraft real também), então "descartar"
//! aqui não tira nenhum detalhe visível, só evita enviar geometria que
//! nunca seria vista. Ver `map_renderer::compute_voxel_crop`/
//! `extract_exposed_voxels`, reaproveitados aqui por chunk.
//!
//! **Trade-off assumido conscientemente:** a exposição de um bloco é
//! calculada só dentro do chunk que o contém (16×16 colunas, altura
//! inteira) — um bloco bem na borda de um chunk cujo vizinho do lado de
//! fora fica no chunk adjacente é tratado como exposto mesmo que esse
//! vizinho (ainda não carregado) o cubra. Na pior hipótese isso mantém uns
//! poucos triângulos extras nas costuras entre chunks (overhead de render
//! irrelevante) — nunca omite um bloco que devia aparecer.
//!
//! **Navegação:** primeira pessoa via `PointerLockControls` do Three.js —
//! clique pra travar o cursor, mouse pra olhar, WASD pra andar, Espaço/Shift
//! pra subir/descer (modo voo — sem colisão/gravidade nesta versão; ver
//! `voxel_viewer_page.html`).

use crate::map_renderer;
use crate::world_viewer;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Faixa de altura padrão: o piso e o teto absolutos do Minecraft moderno
/// (1.18+) — cobre QUALQUER mundo gerado por este motor sem truncar nada,
/// ajustável via `--min-y`/`--max-y` só se um recorte vertical mais estreito
/// for desejado (menos dado por chunk, carregamento mais rápido).
const DEFAULT_MIN_Y: i32 = -64;
const DEFAULT_MAX_Y: i32 = 320;

/// Lado de um chunk em blocos — fixo pelo formato Anvil do Minecraft, não
/// um parâmetro nosso.
const CHUNK_SIZE: i32 = 16;

const VOXEL_VIEWER_PAGE: &str = include_str!("voxel_viewer_page.html");

type ChunkCache = Mutex<HashMap<(i32, i32), Arc<Vec<[i32; 6]>>>>;

/// Recorte opcional (`--crop`) restringindo a área streamável — sem ele, o
/// mundo inteiro (todas as regiões presentes em `region/`) é navegável.
#[derive(Clone, Copy)]
struct CropBounds {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
}

struct ServerContext {
    world_dir: PathBuf,
    min_y: i32,
    max_y: i32,
    crop: Option<CropBounds>,
    cache: ChunkCache,
}

#[derive(Serialize)]
struct WorldInfo {
    #[serde(rename = "minX")]
    min_x: i32,
    #[serde(rename = "maxX")]
    max_x: i32,
    #[serde(rename = "minZ")]
    min_z: i32,
    #[serde(rename = "maxZ")]
    max_z: i32,
    #[serde(rename = "minY")]
    min_y: i32,
    #[serde(rename = "maxY")]
    max_y: i32,
    #[serde(rename = "chunkSize")]
    chunk_size: i32,
    #[serde(rename = "spawnX")]
    spawn_x: i32,
    #[serde(rename = "spawnY")]
    spawn_y: i32,
    #[serde(rename = "spawnZ")]
    spawn_z: i32,
}

#[derive(Serialize)]
struct ChunkResponse {
    #[serde(rename = "chunkX")]
    chunk_x: i32,
    #[serde(rename = "chunkZ")]
    chunk_z: i32,
    /// `[x, y, z, r, g, b]` por voxel exposto, coordenadas absolutas do
    /// mundo — o cliente recentraliza em torno do spawn pra evitar
    /// problemas de precisão do WebGL.
    voxels: Vec<[i32; 6]>,
}

/// Calcula (e cacheia) os voxels expostos de um chunk, reaproveitando
/// `compute_voxel_crop` com um recorte de exatamente 16×16 colunas — a
/// mesma função que já usamos pro modo de recorte único, só chamada uma
/// vez por chunk em vez de uma vez pro mundo inteiro.
fn get_or_compute_chunk(ctx: &ServerContext, chunk_x: i32, chunk_z: i32) -> Arc<Vec<[i32; 6]>> {
    if let Ok(cache) = ctx.cache.lock() {
        if let Some(cached) = cache.get(&(chunk_x, chunk_z)) {
            return Arc::clone(cached);
        }
    }

    let min_x = chunk_x * CHUNK_SIZE;
    let max_x = min_x + CHUNK_SIZE - 1;
    let min_z = chunk_z * CHUNK_SIZE;
    let max_z = min_z + CHUNK_SIZE - 1;

    let voxels = if let Some(crop) = ctx.crop {
        if max_x < crop.min_x || min_x > crop.max_x || max_z < crop.min_z || min_z > crop.max_z {
            Vec::new()
        } else {
            compute_chunk_voxels(
                &ctx.world_dir,
                min_x,
                max_x,
                min_z,
                max_z,
                ctx.min_y,
                ctx.max_y,
            )
        }
    } else {
        compute_chunk_voxels(
            &ctx.world_dir,
            min_x,
            max_x,
            min_z,
            max_z,
            ctx.min_y,
            ctx.max_y,
        )
    };

    let arc = Arc::new(voxels);
    if let Ok(mut cache) = ctx.cache.lock() {
        cache.insert((chunk_x, chunk_z), Arc::clone(&arc));
    }
    arc
}

#[allow(clippy::too_many_arguments)]
fn compute_chunk_voxels(
    world_dir: &Path,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    min_y: i32,
    max_y: i32,
) -> Vec<[i32; 6]> {
    match map_renderer::compute_voxel_crop(world_dir, min_x, max_x, min_z, max_z, min_y, max_y) {
        Ok(raw) => raw
            .into_iter()
            .map(|(x, y, z, [r, g, b])| [x, y, z, r as i32, g as i32, b as i32])
            .collect(),
        Err(e) => {
            eprintln!("[AVISO] Falha ao decodificar chunk ({min_x},{min_z}): {e}");
            Vec::new()
        }
    }
}

/// Escolhe um ponto de spawn: o centro do mundo (ou do recorte, se houver),
/// numa altura logo acima do bloco mais alto encontrado no chunk de spawn
/// — computa esse UM chunk já na inicialização (rápido) só pra achar uma
/// altura seguramente acima do chão, em vez de chutar um Y fixo que pode
/// ficar embaixo da terra em mundos com relevo mais alto.
fn choose_spawn(ctx: &ServerContext, center_x: i32, center_z: i32) -> (i32, i32, i32) {
    let chunk_x = center_x.div_euclid(CHUNK_SIZE);
    let chunk_z = center_z.div_euclid(CHUNK_SIZE);
    let voxels = get_or_compute_chunk(ctx, chunk_x, chunk_z);
    let top_y = voxels.iter().map(|v| v[1]).max().unwrap_or(ctx.min_y);
    (center_x, top_y + 20, center_z)
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

fn write_json<T: Serialize>(stream: &mut TcpStream, value: &T) {
    match serde_json::to_vec(value) {
        Ok(bytes) => write_response(stream, "200 OK", "application/json", &bytes),
        Err(e) => write_response(
            stream,
            "500 Internal Server Error",
            "text/plain",
            format!("Falha ao serializar: {e}").as_bytes(),
        ),
    }
}

fn handle_connection(mut stream: TcpStream, ctx: &ServerContext, world_info: &WorldInfo) {
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

    if path == "/" || path == "/index.html" {
        write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            VOXEL_VIEWER_PAGE.as_bytes(),
        );
        return;
    }

    if path == "/world-info" {
        write_json(&mut stream, world_info);
        return;
    }

    if let Some(rest) = path.strip_prefix("/chunk/") {
        let mut parts = rest.split('/');
        let parsed = parts
            .next()
            .and_then(|s| s.parse::<i32>().ok())
            .zip(parts.next().and_then(|s| s.parse::<i32>().ok()));
        if let Some((chunk_x, chunk_z)) = parsed {
            let voxels = get_or_compute_chunk(ctx, chunk_x, chunk_z);
            write_json(
                &mut stream,
                &ChunkResponse {
                    chunk_x,
                    chunk_z,
                    voxels: (*voxels).clone(),
                },
            );
            return;
        }
    }

    write_response(&mut stream, "404 Not Found", "text/plain", b"not found");
}

/// Ponto de entrada de `--view-world-voxels`. Bloqueia até `Ctrl+C`.
pub fn serve(
    world_dir: PathBuf,
    crop: Option<(i32, i32, i32, i32)>,
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
    if min_y > max_y {
        eprintln!("Erro: --min-y ({min_y}) precisa ser <= --max-y ({max_y}).");
        std::process::exit(1);
    }

    let crop_bounds = crop.map(|(min_x, min_z, max_x, max_z)| CropBounds {
        min_x,
        max_x,
        min_z,
        max_z,
    });

    // Descobre os limites reais do mundo (via `metadata.json`, com fallback
    // pros nomes dos arquivos `region/*.mca`) só pra reportar a extensão e
    // escolher um spawn central — nunca pra pré-computar voxels do mundo
    // inteiro (é aí que este módulo evita o erro do BlueMap desta sessão:
    // zero suposições sobre "onde está o conteúdo", carrega sob demanda).
    let (world_min_x, world_max_x, world_min_z, world_max_z) = match world_viewer::discover_bounds(
        &world_dir,
    ) {
        Ok(bounds) => bounds,
        Err(e) => {
            eprintln!(
                    "[AVISO] Não foi possível descobrir os limites do mundo ({e}); usando (0,0) como centro."
                );
            (0, 0, 0, 0)
        }
    };

    let (min_x, max_x, min_z, max_z) = match crop_bounds {
        Some(c) => (c.min_x, c.max_x, c.min_z, c.max_z),
        None => (world_min_x, world_max_x, world_min_z, world_max_z),
    };
    let center_x = (min_x + max_x) / 2;
    let center_z = (min_z + max_z) / 2;

    let ctx = ServerContext {
        world_dir,
        min_y,
        max_y,
        crop: crop_bounds,
        cache: Mutex::new(HashMap::new()),
    };

    println!("[INFO] 🧊 Calculando ponto de spawn (Y acima do relevo no centro do mundo)...");
    let (spawn_x, spawn_y, spawn_z) = choose_spawn(&ctx, center_x, center_z);

    let world_info = WorldInfo {
        min_x,
        max_x,
        min_z,
        max_z,
        min_y,
        max_y,
        chunk_size: CHUNK_SIZE,
        spawn_x,
        spawn_y,
        spawn_z,
    };

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
    println!(
        "     Mundo: x[{min_x}..{max_x}] z[{min_z}..{max_z}] — spawn em ({spawn_x}, {spawn_y}, {spawn_z})"
    );
    println!("     Chunks carregam sob demanda ao andar — o mundo inteiro é navegável.");
    println!();
    println!("  Abra esse endereço no navegador. Ctrl+C aqui encerra o servidor.");
    println!();

    let ctx = Arc::new(ctx);
    let world_info = Arc::new(world_info);
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let ctx = Arc::clone(&ctx);
        let world_info = Arc::clone(&world_info);
        std::thread::spawn(move || {
            handle_connection(stream, &ctx, &world_info);
        });
    }
}
