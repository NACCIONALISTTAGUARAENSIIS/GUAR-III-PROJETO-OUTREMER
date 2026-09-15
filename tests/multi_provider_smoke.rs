//! Teste de fumaça multi-provider — roda o binário real do Pincelism com
//! VÁRIOS provedores de dados locais ligados ao mesmo tempo (não um só, como
//! os testes manuais desta sessão até aqui) e confere:
//!
//! 1. que o processo termina com sucesso (nenhum provedor derruba os outros);
//! 2. que o mundo gerado tem pelo menos um chunk com conteúdo real;
//! 3. que a estrutura NBT dos chunks é a do formato Java ATUAL (pós-1.18) —
//!    `DataVersion`/`Status`/`yPos` soltos na raiz, sem o wrapper `"Level"`
//!    (o bug crítico corrigido nesta sessão em `world_editor/java.rs`) — lida
//!    de volta com a MESMA crate (`fastanvil`/`fastnbt`) que o motor usa para
//!    escrever, não uma reimplementação própria do formato.
//!
//! É um teste de PROCESSO (`std::process::Command` sobre o binário
//! compilado via `env!("CARGO_BIN_EXE_pincelism")`), não uma chamada direta às
//! APIs internas dos provedores — este crate não expõe um alvo de biblioteca
//! (só o binário), então essa é a forma real de testar a integração completa
//! sem reestruturar o `Cargo.toml`.
//!
//! ## O que este teste cobre hoje, e o que fica de fora (documentado, não
//! silenciado)
//!
//! Cobertos com fixtures locais reais (sem rede, 100% determinístico):
//! `--file` (OSM offline), `--local-csv`, `--local-kml`, `--local-geojson`,
//! `--local-gpkg`, `--local-mesh`, `--local-citygml`, `--local-ifc`,
//! `--local-pbf`.
//!
//! Coberto com um servidor HTTP mock local (`std::net::TcpListener`, sem
//! nenhuma crate nova) rodando DENTRO do processo de teste, servindo uma
//! resposta fixa em `127.0.0.1:<porta efêmera>`: `--tiles3d-endpoint`.
//!
//! Cobertos numa segunda função de teste (`terrain_providers_smoke_test`),
//! separada da principal, com `--terrain` + `--local-lidar` + `--local-dem` +
//! `--mapbiomas-tiff`: ver a seção própria mais abaixo para o porquê da
//! separação.
//!
//! ### `--local-citygml`: `predio.gml`
//! Um `bldg:Building` LOD1 único com um `gml:Solid` fechado de verdade (base +
//! topo + 4 paredes, 6 `gml:Polygon`), coordenadas reais em EPSG:31983
//! (SIRGAS 2000 / UTM 23S) calculadas com `cs2cs` para cair dentro do mesmo
//! bbox de teste (Guará I) usado pelos outros provedores. Nenhuma mudança foi
//! necessária em `citygml_provider.rs` — o parser SAX já lida corretamente
//! com `gml:posList` aninhado em qualquer profundidade dentro de `Building`.
//!
//! ### `--local-ifc`: `predio.ifc`
//! Uma IFC4/STEP mínima com `IFCWALL` + `IFCSLAB`. Corrigido nesta rodada em
//! `ifc_provider.rs`: um bug real em `extract_coordinates` — a sintaxe padrão
//! do IFC para `IFCCARTESIANPOINT` é uma lista ANINHADA (`NAME((x,y,z))`), e
//! o parser só removia o parêntese externo, deixando o primeiro e o último
//! valor com um `(`/`)` colado que quebrava o `str::parse::<f64>`; qualquer
//! IFC gerado por uma ferramenta BIM real (que sempre usa essa sintaxe) tinha
//! 100% dos seus pontos descartados silenciosamente.
//!
//! ### `--local-pbf`: `predio.osm.pbf`
//! Um `.osm.pbf` mínimo, montado byte a byte (varints/zigzag/length-delimited
//! na mão, sem crate de protobuf — o mesmo espírito de `landuse.gpkg`),
//! seguindo exatamente `osmformat.proto`/`fileformat.proto` da versão
//! `osmpbf` 0.3.8 que o projeto usa: 4 `Node`s formando um quadrado fechado
//! e 1 `Way` com `building=yes` referenciando-os. Corrigido nesta rodada em
//! `pbf_provider.rs`: o teste de "a via está dentro da bbox pedida" usava
//! `xz.x >= 0 && xz.z >= 0` — mas a origem (0,0) da malha Minecraft NÃO é o
//! canto da bbox pedida, é o Marco Zero FIXO de Brasília. Qualquer bbox a
//! OESTE desse marco (Guará, Ceilândia, Taguatinga) produzia X
//! sistematicamente NEGATIVO, então TODA via de todo arquivo PBF nessas
//! regiões era descartada. A correção usa `XZBBox::contains()`.
//!
//! Fora do escopo desta rodada — cada um por um motivo concreto, não por
//! preguiça:
//! - `--wfs-endpoint`: TECNICAMENTE testável com o mesmo mock HTTP usado para
//!   `--tiles3d-endpoint`, mas com uma pegadinha real: `args.rs` PROÍBE
//!   `--enable-underground-wfs` (obrigatório para registrar o WFSProvider)
//!   junto de `--offline`, e sem `--offline` o binário faz reverse-geocoding
//!   AO VIVO contra o Nominatim para nomear o mundo — testar WFS por este
//!   binário sempre traria uma dependência de rede genuína, quebrando o "sem
//!   rede" desta suíte.
//! - `--mvt-endpoint`: `mvt_provider.rs::fetch_features` é um placeholder
//!   puro (documentado como tal no próprio arquivo), nunca abre conexão de
//!   rede; um mock aqui não testaria nada real.
//! - `--postgis-url`: precisa de um Postgres/PostGIS real rodando.
//!
//! ## Segundo teste: `terrain_providers_smoke_test` (`--terrain`)
//!
//! `--local-dem`, `--local-lidar` e `--mapbiomas-tiff` NÃO entram na função de
//! teste principal acima, de propósito: `--terrain` muda o nível do chão para
//! TODO o pipeline de geração, não só para o próprio provedor — misturar isso
//! com os outros provedores vetoriais na mesma chamada acoplaria a validação
//! deles (já testados e passam sem terreno real) ao comportamento do
//! terreno, sem necessidade. Uma segunda função de teste, com seu próprio
//! bbox e sua própria chamada ao binário, mantém os dois conjuntos de
//! garantias independentes — e ainda reaproveita a mesma
//! `validate_generated_world_structure`.
//!
//! Fixtures em `tests/fixtures/multi_provider/`, geradas byte a byte por
//! programas Rust standalone (usando as MESMAS versões de `las`/`tiff` que o
//! projeto já depende):
//! - `predio.las`: pontos LAS 1.2 formato 0. Uma grade 11x11 classificada
//!   `Ground` (classe 2, usada por `elevation_data::load_local_lidar`) e uma
//!   grade 6x6 classificada `Building` (classe 6, usada por `LidarProvider`)
//!   — ambas cobertas pelo MESMO arquivo, como um LiDAR real teria.
//! - `predio_dem.tif`/`predio_mapbiomas.tif`: GeoTIFF single-band sem
//!   compressão (`Gray32Float`/`Gray8`), sem tags de georreferenciamento — o
//!   georreferenciamento inteiro vem de flags de CLI (`--dem-top-left-*`/
//!   `--mapbiomas-*`), então um TIFF puro já basta.
//!
//! ## Anomalia do GPKG (0 feições) — investigada e resolvida
//!
//! Uma rodada anterior desta suíte via `GDF GeoPackage` sempre extrair 0
//! feições do fixture `landuse.gpkg`, apesar do arquivo ser um GeoPackage
//! válido (`SELECT COUNT(*)`/`SELECT *` via `sqlite3` CLI e via `rusqlite`
//! isolado sempre encontravam a única linha de `lotes_fixture`). A hipótese
//! de trabalho na época era algum conflito em tempo de link/execução entre o
//! SQLite *bundled* do `rusqlite` (3.45.0, estático) e o `libsqlite3.so`
//! dinâmico do sistema (3.37.2) que o WebKitGTK traz transitivamente pela
//! feature `gui` — e esse conflito de fato EXISTE e foi confirmado (via
//! `nm -D`/`LD_DEBUG=bindings`: o binário exporta ~268 símbolos `sqlite3_*`
//! do bundled, e é `libwebkit2gtk-4.1.so.0` quem acaba resolvendo suas
//! próprias chamadas `sqlite3_step`/`sqlite3_prepare_v2`/etc contra a NOSSA
//! cópia estática). Mas essa pista era um red herring: instrumentação
//! temporária direto em `GpkgProvider::fetch_features` (já removida) mostrou
//! que `stmt_data.query([]).next()` SEMPRE encontrava a linha corretamente —
//! `Ok(Some(row))` com todas as colunas certas — e `GpkgWkb::to_geo()`
//! decodificava a geometria sem erro. O SQLite nunca foi o problema, em
//! nenhum dos dois binários (com ou sem a feature `gui`/WebKit).
//!
//! A causa real: o polígono desenhado à mão no `landuse.gpkg` (via WKB bruto)
//! tinha longitude na faixa `[-47.98275, -47.98265]`, enquanto o bbox deste
//! teste (`-15.8182,-47.9832,-15.8178,-47.9828`, ordem
//! `min_lat,min_lng,max_lat,max_lng` — ver `LLBBox::from_str`) cobre
//! `lng ∈ [-47.9832, -47.9828]`. O polígono inteiro caía de 5 a 16 metros a
//! LESTE do limite máximo do bbox — fora da área, mas por uma margem pequena
//! o bastante para passar despercebida a olho nu. `GpkgProvider` lia e
//! decodificava a feição perfeitamente, e o "Early-Z Culling"
//! (`if is_completely_outside { continue; }`) descartava ela silenciosamente
//! por estar, de fato, fora do bbox — comportamento correto do código diante
//! de um fixture com coordenadas erradas. Corrigido deslocando a longitude do
//! polígono (e o `min_x`/`max_x` de `gpkg_contents`, para manter o arquivo
//! internamente consistente) para dentro do bbox.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

/// Sobe um servidor HTTP mínimo (`std::net::TcpListener`, sem dependência
/// nova) numa porta efêmera local, aceita EXATAMENTE uma conexão, devolve o
/// `body` fornecido como um `200 OK application/json` e encerra. Suficiente
/// para os provedores de endpoint que fazem só uma requisição GET por
/// geração (ex.: `Tiles3DProvider` busca o `tileset.json` uma única vez —
/// não baixa o conteúdo `.b3dm`/`.glb` referenciado, só registra a URI como
/// tag). Retorna a URL base (`http://127.0.0.1:<porta>/`) para passar como
/// argumento de CLI ao binário.
fn spawn_single_response_mock_server(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind mock HTTP server");
    let port = listener
        .local_addr()
        .expect("failed to read mock server local addr")
        .port();

    std::thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let mut reader = BufReader::new(stream.try_clone().expect("failed to clone stream"));
            let mut stream = stream;
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
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    format!("http://127.0.0.1:{port}/")
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/multi_provider")
}

#[test]
fn multi_provider_generation_produces_valid_modern_chunks() {
    let fixtures = fixtures_dir();
    let out_dir = tempfile::tempdir().expect("failed to create temp output dir");

    let bin = PathBuf::from(env!("CARGO_BIN_EXE_pincelism"));

    // tileset.json mínimo válido segundo a spec OGC 3D Tiles: um nó `root`
    // com `boundingVolume.region` (radianos: west,south,east,north,minH,maxH)
    // cobrindo o mesmo bbox de teste do Guará I, e um `content.uri` — o
    // provider só lê este JSON (nunca baixa o .b3dm referenciado).
    let tiles3d_url = spawn_single_response_mock_server(
        r#"{"asset":{"version":"1.0"},"geometricError":500,"root":{"boundingVolume":{"region":[-0.8374700616306991,-0.276084907726723,-0.8374526083381792,-0.2760674544342031,1080.0,1090.0]},"geometricError":100,"refine":"ADD","content":{"uri":"fixture_tile.b3dm"},"children":[]}}"#,
    );

    // Bbox minúscula e real (Guará I), cobrindo exatamente onde as fixtures
    // GeoJSON/KML/GPKG foram desenhadas — ver os arquivos em
    // tests/fixtures/multi_provider/.
    let output = Command::new(&bin)
        .arg("--bbox")
        .arg("-15.8182,-47.9832,-15.8178,-47.9828")
        .arg("--output-dir")
        .arg(out_dir.path())
        .arg("--offline")
        .arg("--file")
        .arg(fixtures.join("osm_offline.json"))
        .arg("--local-csv")
        .arg(fixtures.join("mobiliario_urbano.csv"))
        .arg("--local-kml")
        .arg(fixtures.join("tombamento.kml"))
        .arg("--local-geojson")
        .arg(fixtures.join("gdf_extra.geojson"))
        .arg("--local-gpkg")
        .arg(fixtures.join("landuse.gpkg"))
        .arg("--local-mesh")
        .arg(fixtures.join("monument"))
        .arg("--local-citygml")
        .arg(fixtures.join("predio.gml"))
        .arg("--local-ifc")
        .arg(fixtures.join("predio.ifc"))
        .arg("--tiles3d-endpoint")
        .arg(&tiles3d_url)
        .arg("--local-pbf")
        .arg(fixtures.join("predio.osm.pbf"))
        .output()
        .expect("failed to execute pincelism binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "geração com múltiplos provedores falhou (status {:?}).\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );

    // Confirma que cada provedor local realmente foi inicializado (não
    // silenciosamente ignorado) — protege contra o tipo de regressão achado
    // nesta sessão (provider registrado mas cujas Features nunca chegam a
    // lugar nenhum).
    for expected in [
        "OpenStreetMap",
        "CSV",
        "KML",
        "GeoJSON",
        "GeoPackage",
        "Fotogrametria",
        "CityGML",
        "BIM IFC",
        "3D Tiles",
        "OSM PBF",
    ] {
        assert!(
            stdout.contains(expected) || stderr.contains(expected),
            "log de geração não menciona o provedor esperado: {expected:?}\n--- stdout ---\n{stdout}"
        );
    }

    // Confirma que cada provedor não só rodou, mas EXTRAIU pelo menos 1
    // feição real do seu fixture — mais rigoroso que só checar o nome no log
    // (que aparece mesmo quando o provedor devolve 0). Achado real desta
    // sessão: o mesh provider sempre falhava com uma pasta (o uso documentado
    // em --help) até ser corrigido para varrer `*.obj` dentro dela.
    //
    // `GDF GeoPackage` esteve de fora desta lista por uma rodada inteira
    // (extraía sempre 0) até a causa real ser encontrada e corrigida: o
    // polígono do fixture `landuse.gpkg` estava desenhado fora do bbox deste
    // teste — ver o comentário de módulo no topo deste arquivo.
    for (provider_label, extracted_marker) in [
        (
            "GDF Open Data (CSV Point Cloud)",
            "features extraídas de GDF Open Data",
        ),
        (
            "GDF KML/KMZ Provider",
            "features extraídas de GDF KML/KMZ Provider",
        ),
        ("GDF GeoJSON", "features extraídas de GDF GeoJSON"),
        (
            "GDF GeoPackage (SQLite Spatial DB)",
            "features extraídas de GDF GeoPackage",
        ),
        (
            "Photogrammetry Mesh Voxelizer 3D",
            "features extraídas de Photogrammetry Mesh Voxelizer 3D",
        ),
        (
            "CityGML 3D Provider (LOD3 SAX Stream with Facade Matrix)",
            "features extraídas de CityGML 3D Provider (LOD3 SAX Stream with Facade Matrix)",
        ),
        (
            "BIM IFC Provider (LOD4/LOD5 Niemeyer Architecture)",
            "features extraídas de BIM IFC Provider (LOD4/LOD5 Niemeyer Architecture)",
        ),
        (
            "OGC 3D Tiles Streamer (Cesium HLOD)",
            "features extraídas de OGC 3D Tiles Streamer (Cesium HLOD)",
        ),
        (
            "Local OSM PBF (Multi-Pass Topological Stream)",
            "features extraídas de Local OSM PBF (Multi-Pass Topological Stream)",
        ),
    ] {
        let has_nonzero = stdout
            .lines()
            .any(|line| line.contains(extracted_marker) && !line.trim_start().starts_with("-> 0 "));
        assert!(
            has_nonzero,
            "provedor {provider_label:?} não extraiu nenhuma feição do fixture local (esperado >0)\n--- stdout ---\n{stdout}"
        );
    }

    validate_generated_world_structure(out_dir.path());
}

/// Segundo teste de fumaça, isolado do principal: `--terrain` ligado com
/// `--local-lidar` + `--local-dem` + `--mapbiomas-tiff` simultâneos. Ver o
/// comentário de módulo no topo deste arquivo ("Segundo teste:
/// terrain_providers_smoke_test") para o porquê da separação da função
/// principal.
#[test]
fn terrain_providers_smoke_test() {
    let fixtures = fixtures_dir();
    let out_dir = tempfile::tempdir().expect("failed to create temp output dir");
    let bin = PathBuf::from(env!("CARGO_BIN_EXE_pincelism"));

    // Mesmo bbox de teste (Guará I) usado no teste principal — as
    // coordenadas de `predio.las`/`predio_dem.tif`/`predio_mapbiomas.tif`
    // foram calculadas para caírem dentro dele.
    let output = Command::new(&bin)
        .arg("--bbox")
        .arg("-15.8182,-47.9832,-15.8178,-47.9828")
        .arg("--output-dir")
        .arg(out_dir.path())
        .arg("--offline")
        // `--offline` exige uma fonte vetorial local mesmo aqui (DEM/LiDAR/
        // mapbiomas não contam para essa checagem em `args.rs`) — reusa o
        // mesmo OSM offline mínimo do teste principal.
        .arg("--file")
        .arg(fixtures.join("osm_offline.json"))
        .arg("--terrain")
        .arg("--local-lidar")
        .arg(fixtures.join("predio.las"))
        .arg("--local-dem")
        .arg(fixtures.join("predio_dem.tif"))
        .arg("--dem-top-left-lat=-15.8168")
        .arg("--dem-top-left-lon=-47.9842")
        .arg("--dem-pixel-size-deg=0.00002")
        .arg("--mapbiomas-tiff")
        .arg(fixtures.join("predio_mapbiomas.tif"))
        .arg("--mapbiomas-top-left-lat=-15.8168")
        .arg("--mapbiomas-top-left-lon=-47.9842")
        .arg("--mapbiomas-pixel-size-deg=0.00002")
        .output()
        .expect("failed to execute pincelism binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "geração com provedores de terreno falhou (status {:?}).\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );

    // LidarProvider ("Concave Vectorizer"): extrai feições vetoriais
    // (prédios/vegetação/água) da nuvem de pontos — segue o mesmo padrão
    // rigoroso ">0 features" do teste principal.
    let has_lidar_features = stdout.lines().any(|line| {
        line.contains("features extraídas de High-Density LiDAR Concave Vectorizer")
            && !line.trim_start().starts_with("-> 0 ")
    });
    assert!(
        has_lidar_features,
        "LidarProvider não extraiu nenhuma feição vetorial (prédio) do fixture LAS\n--- stdout ---\n{stdout}"
    );

    // Os três abaixo não passam por `Feature`/`ProviderManager` — são grids
    // de quantização lidos diretamente em `main.rs` (`elevation_data`,
    // `dem_provider`, `vegetation_provider`), cada um com sua própria
    // linha de log de sucesso. Extrai o número reportado e confere que é >0.
    for (label, needle) in [
        (
            "grid de elevação LiDAR (elevation_data::load_local_lidar)",
            "alocados no relevo de solo.",
        ),
        (
            "grid MapBiomas (vegetation_provider)",
            "sementes base de fitofisionomia",
        ),
        (
            "grid DEM (dem_provider)",
            "blocos de terreno ancorados no Grid Absoluto.",
        ),
    ] {
        let count = stdout
            .lines()
            .find_map(|line| {
                if !line.contains(needle) {
                    return None;
                }
                // Extrai o ÚLTIMO token puramente numérico da linha: nas
                // duas variantes de mensagem usadas aqui ("N alocados..." e
                // "N pixels processados, M blocos...") é sempre o último
                // número que representa a contagem de interesse.
                line.split(|c: char| !c.is_ascii_digit())
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| s.parse::<u64>().ok())
                    .next_back()
            })
            .unwrap_or_else(|| {
                panic!("linha de log com {needle:?} não encontrada\n--- stdout ---\n{stdout}")
            });
        assert!(
            count > 0,
            "{label} reportou 0 (esperado >0)\n--- stdout ---\n{stdout}"
        );
    }

    validate_generated_world_structure(out_dir.path());
}

/// Abre o mundo gerado e confere, região por região e chunk por chunk, que a
/// estrutura NBT é a do formato Java ATUAL — a regressão concreta que motivou
/// este teste. Usa `fastanvil::Region` + `fastnbt::from_bytes::<CurrentJavaChunk>`
/// (a MESMA dupla de crates que o motor usa para escrever) em vez de um parser
/// próprio: se `CurrentJavaChunk` conseguir desserializar, um cliente Minecraft
/// real também conseguiria.
fn validate_generated_world_structure(world_root: &std::path::Path) {
    let world_dirs: Vec<PathBuf> = std::fs::read_dir(world_root)
        .expect("failed to read output dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    assert!(
        !world_dirs.is_empty(),
        "nenhuma pasta de mundo foi criada em {world_root:?}"
    );

    let region_dir = world_dirs[0].join("region");
    let region_files: Vec<PathBuf> = std::fs::read_dir(&region_dir)
        .unwrap_or_else(|e| panic!("failed to read region dir {region_dir:?}: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "mca"))
        .collect();
    assert!(
        !region_files.is_empty(),
        "nenhum arquivo de região .mca foi escrito em {region_dir:?}"
    );

    let mut total_chunks_checked = 0usize;
    let mut total_non_air_sections = 0usize;

    for region_path in &region_files {
        let file = std::fs::File::open(region_path)
            .unwrap_or_else(|e| panic!("failed to open region file {region_path:?}: {e}"));
        let mut region = fastanvil::Region::from_stream(file)
            .unwrap_or_else(|e| panic!("failed to parse region file {region_path:?}: {e}"));

        for cx in 0..32usize {
            for cz in 0..32usize {
                let Ok(Some(raw_chunk)) = region.read_chunk(cx, cz) else {
                    continue;
                };
                if raw_chunk.is_empty() {
                    continue;
                }
                total_chunks_checked += 1;

                // A asserção central: o parser de PRODUÇÃO da própria crate
                // `fastanvil` para o formato Java ATUAL consegue ler o chunk.
                // Isso falha imediatamente se alguém reintroduzir o wrapper
                // "Level" (formato pré-1.18) ou remover DataVersion/Status/yPos.
                let chunk: fastanvil::CurrentJavaChunk = fastnbt::from_bytes(&raw_chunk)
                    .unwrap_or_else(|e| {
                        panic!(
                            "chunk ({cx},{cz}) em {region_path:?} não está no formato Java atual \
                             (regressão do bug do wrapper \"Level\"?): {e}"
                        )
                    });

                if let Some(sections) = &chunk.sections {
                    for section in sections.sections() {
                        if section.block_states.palette().len() > 1 {
                            total_non_air_sections += 1;
                        }
                    }
                }
            }
        }
    }

    assert!(
        total_chunks_checked > 0,
        "nenhum chunk com conteúdo foi encontrado em nenhuma região — a geração não escreveu nada"
    );
    assert!(
        total_non_air_sections > 0,
        "todos os {total_chunks_checked} chunks verificados estavam vazios (só ar/paleta única) — \
         os provedores locais não geraram nenhum bloco de verdade"
    );
}
