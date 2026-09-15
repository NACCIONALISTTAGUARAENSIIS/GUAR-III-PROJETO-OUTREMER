//! Integração com o [BlueMap](https://bluemap.bluecolored.de/) — um
//! renderizador de mundos Minecraft real, maduro e de código aberto (não uma
//! invenção nossa), usado via `--view-world-bluemap <PASTA_DO_MUNDO>`.
//!
//! 🚨 BESM-6: por que isto existe ao lado de `world_viewer.rs`. O relevo
//! caseiro (`--view-world`) é rápido e não depende de nada além do próprio
//! binário, mas é uma amostra (altura+cor do topo de cada coluna, uma malha
//! só) — o suficiente para ver a skyline e o traçado urbano do mundo inteiro,
//! mas não para inspecionar blocos de verdade de perto. O BlueMap faz o
//! trabalho oposto: renderiza os blocos REAIS, texturizados, com os modelos
//! 3D de verdade do Minecraft (paredes, portas, telhados — tudo que este
//! motor desenha), em várias vistas (topo/isométrica/perspectiva/livre) e
//! níveis de zoom, exatamente como um administrador de servidor Minecraft
//! real usaria para inspecionar o próprio mundo. É uma ferramenta JVM externa
//! (não Rust) — este módulo só orquestra o processo `java -jar
//! bluemap-cli.jar`, ele não reimplementa nada da renderização.
//!
//! ## O que este módulo automatiza (documentado aqui porque foi descoberto
//! manualmente nesta sessão, testando de verdade contra o Guará I+II)
//!
//! 1. **Localizar um Java compatível.** A versão testada do BlueMap CLI
//!    (5.25) foi compilada contra bytecode de classe versão 69 — Java 25.
//!    Rodar com Java 21 (a versão de `java` no PATH desta máquina) falha com
//!    `UnsupportedClassVersionError` na hora. `find_compatible_java` procura
//!    primeiro no override explícito (`--java`), depois checa se `java` do
//!    PATH já serve, e só então varre um punhado de caminhos conhecidos de
//!    instalações JVM lado a lado (ex.: `/usr/lib/jvm/jdk-25`).
//! 2. **Gerar a configuração mínima do mapa.** Na primeira execução para uma
//!    pasta de configuração nova, roda `bluemap-cli.jar -c <config>` uma vez
//!    (gera `core.conf`/`webapp.conf`/`webserver.conf`/`storages/*.conf` —
//!    isso é boilerplate genuíno do BlueMap, não vale a pena reescrever à
//!    mão), remove os mapas `nether`/`end` que vêm por padrão (este motor
//!    nunca gera essas dimensões) e escreve um `maps/<id>.conf` PRÓPRIO,
//!    mínimo (confirmado nesta sessão: um arquivo de 3 linhas — `world`,
//!    `dimension`, `name` — é suficiente; os demais campos assumem os
//!    defaults documentados no arquivo gerado pelo BlueMap).
//! 3. **Aceitar o download de assets.** BlueMap precisa das texturas oficiais
//!    do Minecraft (baixadas da própria Mojang) para desenhar os blocos —
//!    `core.conf` tem uma flag `accept-download` que representa aceitar a
//!    EULA da Mojang (<https://account.mojang.com/documents/minecraft_eula>).
//!    Isso só é ligado na primeira geração da configuração, e avisado no
//!    console — nunca silenciosamente.
//! 4. **Renderizar + servir numa só chamada.** `-r -w -m <id>` faz o BlueMap
//!    renderizar o mapa configurado e, ao terminar, subir o próprio
//!    webserver embutido — o processo roda em primeiro plano (stdio
//!    herdado, o usuário vê o progresso real do BlueMap, incluindo o ETA)
//!    até `Ctrl+C`, igual ao contrato de `--view-world`.
//!
//! Não baixa o `.jar` do BlueMap automaticamente: buscar e EXECUTAR um
//! executável externo baixado da rede sem o usuário ver esse passo
//! explicitamente é um risco de cadeia de suprimentos que preferimos não
//! automatizar em silêncio — se o `.jar` não for encontrado, este módulo
//! imprime o link oficial do GitHub e o caminho esperado, e para.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Versão mínima de Java confirmada capaz de rodar o BlueMap CLI 5.25 nesta
/// sessão (class file version 69 = Java 25). Se uma versão futura do BlueMap
/// relaxar esse requisito, isso só faz a varredura de candidatos ser mais
/// permissiva do que precisa — não quebra nada.
const MIN_JAVA_MAJOR_VERSION: u32 = 25;

/// Caminhos conhecidos de instalações JVM lado a lado a tentar se `java` do
/// PATH for velho demais — descobertos nesta própria sessão
/// (`update-alternatives --list java`, `ls /usr/lib/jvm`).
const FALLBACK_JAVA_CANDIDATES: &[&str] = &[
    "/usr/lib/jvm/jdk-25/bin/java",
    "/usr/lib/jvm/java-25-openjdk-amd64/bin/java",
    "/usr/lib/jvm/java-25-openjdk-arm64/bin/java",
];

/// Roda `<candidate> -version` e extrai a versão MAJOR (ex.: "21.0.12" -> 21,
/// "1.8.0_392" (formato antigo pré-Java 9) -> 8). Retorna `None` se o
/// binário não existir ou a saída não puder ser interpretada.
fn probe_java_major_version(candidate: &str) -> Option<u32> {
    let output = Command::new(candidate).arg("-version").output().ok()?;
    // Convenção do próprio `java`: a linha de versão vai para STDERR, não STDOUT.
    let text = String::from_utf8_lossy(&output.stderr);
    let first_line = text.lines().next()?;
    let quoted = first_line.split('"').nth(1)?;

    let mut parts = quoted.split('.');
    let first: u32 = parts.next()?.parse().ok()?;
    if first == 1 {
        // Formato antigo "1.8.0_392" -> major real é o segundo componente.
        parts.next()?.parse().ok()
    } else {
        Some(first)
    }
}

/// Encontra um binário `java` cuja versão seja >= `MIN_JAVA_MAJOR_VERSION`.
/// Ordem: override explícito do usuário -> `java` do PATH -> candidatos
/// conhecidos de instalações lado a lado.
fn find_compatible_java(java_override: Option<&str>) -> Result<String, String> {
    let mut tried = Vec::new();

    let mut candidates: Vec<String> = Vec::new();
    if let Some(j) = java_override {
        candidates.push(j.to_string());
    }
    candidates.push("java".to_string());
    candidates.extend(FALLBACK_JAVA_CANDIDATES.iter().map(|s| s.to_string()));

    for candidate in candidates {
        match probe_java_major_version(&candidate) {
            Some(major) if major >= MIN_JAVA_MAJOR_VERSION => return Ok(candidate),
            Some(major) => tried.push(format!(
                "{candidate} (Java {major}, < {MIN_JAVA_MAJOR_VERSION})"
            )),
            None => tried.push(format!("{candidate} (não encontrado)")),
        }
    }

    Err(format!(
        "Nenhum Java >= {MIN_JAVA_MAJOR_VERSION} encontrado. Tentativas:\n  - {}\n\
         Instale um JDK {MIN_JAVA_MAJOR_VERSION}+ ou aponte um com --java <caminho>.",
        tried.join("\n  - ")
    ))
}

/// Sanitiza o nome da pasta do mundo num id de mapa BlueMap válido
/// (`[a-z0-9_-]+`, tudo em minúsculas — o formato HOCON dos arquivos de
/// config do BlueMap não aceita espaços/maiúsculas livres no nome do arquivo
/// sem se arriscar a colidir com outra convenção interna dele).
fn sanitize_map_id(world_dir: &Path) -> String {
    let raw = world_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("world");
    let mut id: String = raw
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if id.is_empty() {
        id = "world".to_string();
    }
    id
}

/// Centraliza a câmera inicial do BlueMap no MEIO do retângulo delimitador
/// real do mundo gerado, em vez de aceitar o `{x: 0, z: 0}` default do
/// BlueMap — reaproveita a mesma descoberta de limites que
/// `world_viewer::discover_bounds` já faz para o relevo caseiro (via
/// `metadata.json`, com fallback pros nomes dos arquivos `region/*.mca`), não
/// duplicada aqui. Cai para `(0, 0)` (com aviso explícito, nunca em silêncio)
/// só se os limites não puderem ser descobertos de jeito nenhum.
fn compute_start_pos(world_dir: &Path) -> (i32, i32) {
    match crate::world_viewer::discover_bounds(world_dir) {
        Ok((min_x, max_x, min_z, max_z)) => {
            let center_x = (min_x + max_x) / 2;
            let center_z = (min_z + max_z) / 2;
            println!(
                "[INFO] Centralizando a câmera inicial do BlueMap em ({center_x}, {center_z}) \
                 (meio do mundo gerado — o padrão do BlueMap, {{0, 0}}, é o Marco Zero de \
                 Brasília, quase nunca dentro da área realmente gerada)."
            );
            (center_x, center_z)
        }
        Err(e) => {
            eprintln!(
                "[AVISO] Não foi possível descobrir os limites do mundo para centralizar a \
                 câmera ({e}); usando o default do BlueMap (0, 0) — pode abrir olhando pro vazio."
            );
            (0, 0)
        }
    }
}

/// Garante que `config_dir` tem uma configuração BlueMap válida com um mapa
/// apontando para `world_dir`. Gera a configuração padrão do zero na
/// primeira vez (e só na primeira vez — chamadas seguintes reaproveitam o
/// que já existe, para não perder customizações manuais do usuário no
/// `core.conf`/`webserver.conf`).
fn ensure_config(
    config_dir: &Path,
    jar_path: &Path,
    java_bin: &str,
    world_dir: &Path,
    map_id: &str,
) -> Result<(), String> {
    let core_conf = config_dir.join("core.conf");

    if !core_conf.exists() {
        println!("[INFO] Gerando configuração padrão do BlueMap em {config_dir:?}...");
        let status = Command::new(java_bin)
            .arg("-jar")
            .arg(jar_path)
            .arg("-c")
            .arg(config_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| format!("Falha ao executar o BlueMap CLI para gerar a config: {e}"))?;
        if !status.success() {
            return Err(format!(
                "BlueMap CLI saiu com erro ({status}) ao gerar a configuração padrão."
            ));
        }

        // Este motor nunca gera Nether/End — remove os mapas de exemplo
        // correspondentes para o BlueMap não tentar (e falhar) carregá-los.
        for extra in ["nether.conf", "end.conf"] {
            let _ = std::fs::remove_file(config_dir.join("maps").join(extra));
        }

        // 🚨 Aceita a EULA da Mojang (necessária para o BlueMap baixar as
        // texturas oficiais) — feito só nesta primeira geração, e avisado
        // aqui explicitamente, nunca em silêncio.
        println!(
            "[INFO] Aceitando a Mojang EULA (https://account.mojang.com/documents/minecraft_eula) \
             em nome do usuário, necessária para o BlueMap baixar as texturas oficiais do Minecraft."
        );
        let core_text = std::fs::read_to_string(&core_conf)
            .map_err(|e| format!("Falha ao ler {core_conf:?}: {e}"))?;
        let patched = core_text.replace("accept-download: false", "accept-download: true");
        std::fs::write(&core_conf, patched)
            .map_err(|e| format!("Falha ao escrever {core_conf:?}: {e}"))?;
    }

    // 🚨 CORREÇÃO (achado real, testando contra o Guará I+II): o `start-pos`
    // default do BlueMap é `{x: 0, z: 0}` — o ZERO ABSOLUTO da malha
    // Minecraft deste motor, que é o Marco Zero fixo de Brasília
    // (`DF_ORIGIN_LAT/LON` em `transformation.rs`), não necessariamente um
    // ponto dentro do bbox pedido. Qualquer geração fora do Plano Piloto
    // central (Guará, Ceilândia, Taguatinga, ...) abre o BlueMap olhando pro
    // VAZIO — nenhum bloco gerado por perto — parecendo (ao olho, sem
    // contexto) uma superfície plana travada/quebrada, quando na verdade é
    // só a câmera longe de qualquer coisa. Ver `compute_start_pos`.
    let start_pos = compute_start_pos(world_dir);

    // Config mínima confirmada suficiente nesta sessão: `world`/`dimension`/
    // `name`/`start-pos` bastam, o resto assume os defaults do próprio
    // BlueMap. Reescrita toda vez (idempotente) para sempre refletir o
    // `world_dir` pedido nesta chamada, mesmo que a config já existisse de
    // uma chamada anterior com outro mundo.
    let map_conf_path = config_dir.join("maps").join(format!("{map_id}.conf"));
    let map_conf = format!(
        "world: \"{}\"\ndimension: \"minecraft:overworld\"\nname: \"{}\"\nstart-pos: {{ x: {}, z: {} }}\n",
        world_dir.display(),
        map_id,
        start_pos.0,
        start_pos.1,
    );
    std::fs::write(&map_conf_path, map_conf)
        .map_err(|e| format!("Falha ao escrever {map_conf_path:?}: {e}"))?;

    Ok(())
}

/// Executa `java -jar <jar> -c <config> -r -w -m <map_id>`, deixando o
/// stdout/stderr do BlueMap fluir direto pro terminal (o usuário vê o
/// progresso real do render, incluindo o ETA), até o processo terminar
/// (renderiza, depois sobe o servidor web embutido e bloqueia até
/// `Ctrl+C`).
fn run_render_and_serve(
    jar_path: &Path,
    java_bin: &str,
    config_dir: &Path,
    map_id: &str,
) -> Result<(), String> {
    let mut child = Command::new(java_bin)
        .arg("-jar")
        .arg(jar_path)
        .arg("-c")
        .arg(config_dir)
        .arg("-r")
        .arg("-w")
        .arg("-m")
        .arg(map_id)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Falha ao executar o BlueMap CLI: {e}"))?;

    // Repassa o stdout do BlueMap linha a linha em vez de herdar diretamente,
    // pra poder prefixar e deixar claro no terminal que essas linhas vêm de
    // um processo externo, não do próprio Pincelism.
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            println!("[bluemap] {line}");
        }
    }

    let status = child
        .wait()
        .map_err(|e| format!("Falha ao esperar o processo do BlueMap CLI: {e}"))?;
    if !status.success() {
        return Err(format!("BlueMap CLI saiu com erro ({status})."));
    }
    Ok(())
}

/// Ponto de entrada de `--view-world-bluemap`. Ver o comentário de módulo
/// para o fluxo completo.
#[allow(clippy::too_many_arguments)]
pub fn serve(
    world_dir: PathBuf,
    jar_path: Option<PathBuf>,
    java_override: Option<String>,
    config_dir_override: Option<PathBuf>,
) {
    if !world_dir.join("region").is_dir() {
        eprintln!(
            "Erro: {} não parece um diretório de mundo Pincelism válido (falta a pasta 'region/').",
            world_dir.display()
        );
        std::process::exit(1);
    }

    let jar_path = jar_path.unwrap_or_else(|| PathBuf::from("./bluemap/bluemap-cli.jar"));
    if !jar_path.is_file() {
        eprintln!(
            "Erro: BlueMap CLI não encontrado em {jar_path:?}.\n\n\
             Baixe manualmente (não é feito automaticamente de propósito — ver o \
             comentário de módulo em bluemap_viewer.rs):\n\
             https://github.com/BlueMap-Minecraft/BlueMap/releases/latest \
             (arquivo 'bluemap-<versão>-cli.jar')\n\n\
             Salve em {jar_path:?} ou aponte outro caminho com --bluemap-jar <caminho>."
        );
        std::process::exit(1);
    }

    let java_bin = match find_compatible_java(java_override.as_deref()) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Erro: {e}");
            std::process::exit(1);
        }
    };
    println!("[INFO] Usando Java: {java_bin}");

    let config_dir = config_dir_override.unwrap_or_else(|| {
        jar_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("config")
    });
    let map_id = sanitize_map_id(&world_dir);

    if let Err(e) = ensure_config(&config_dir, &jar_path, &java_bin, &world_dir, &map_id) {
        eprintln!("Erro ao preparar a configuração do BlueMap: {e}");
        std::process::exit(1);
    }

    println!();
    println!("  🗺️  Renderizando '{map_id}' com BlueMap — isso pode levar minutos");
    println!("     em mundos grandes (o progresso real aparece abaixo, prefixado");
    println!("     '[bluemap]'). O servidor web sobe automaticamente ao terminar.");
    println!();

    if let Err(e) = run_render_and_serve(&jar_path, &java_bin, &config_dir, &map_id) {
        eprintln!("Erro ao renderizar/servir com o BlueMap: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_map_id_lowercases_and_strips_special_chars() {
        let path = PathBuf::from("/tmp/xyz/Pincelism World 1");
        assert_eq!(sanitize_map_id(&path), "pincelism_world_1");
    }

    #[test]
    fn sanitize_map_id_falls_back_when_empty() {
        let path = PathBuf::from("/");
        assert_eq!(sanitize_map_id(&path), "world");
    }

    #[test]
    fn probe_java_major_version_parses_modern_format() {
        // Não dá pra testar contra um binário `java` real de forma
        // determinística (a versão instalada varia por máquina), mas o
        // parser em si (extração da major version da string entre aspas) é
        // testável isoladamente via uma função auxiliar — replicada aqui
        // como uma unidade pura, já que `probe_java_major_version` combina
        // spawn de processo + parsing.
        fn parse_major(quoted: &str) -> Option<u32> {
            let mut parts = quoted.split('.');
            let first: u32 = parts.next()?.parse().ok()?;
            if first == 1 {
                parts.next()?.parse().ok()
            } else {
                Some(first)
            }
        }

        assert_eq!(parse_major("21.0.12"), Some(21));
        assert_eq!(parse_major("25.0.3"), Some(25));
        assert_eq!(parse_major("1.8.0_392"), Some(8));
    }

    /// Regressão do bug real achado gerando o Guará I+II: `compute_start_pos`
    /// precisa devolver o MEIO do mundo, não `(0, 0)`, sempre que os limites
    /// puderem ser descobertos — ver o comentário de `compute_start_pos` e
    /// `docs/VISUALIZADORES_3D.md` para o achado completo (o BlueMap abria
    /// olhando pro Marco Zero de Brasília, bem fora do bbox real do Guará).
    #[test]
    fn compute_start_pos_centers_on_real_world_bounds() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        std::fs::write(
            tmp.path().join("metadata.json"),
            r#"{"minMcX":-18525,"maxMcX":-12821,"minMcZ":3,"maxMcZ":8836,
                "minGeoLat":-15.86,"maxGeoLat":-15.8,"minGeoLon":-47.99,"maxGeoLon":-47.95}"#,
        )
        .unwrap();

        let (x, z) = compute_start_pos(tmp.path());
        assert_eq!((x, z), (-15673, 4419));
        // O bug real: a posição default do BlueMap fica bem fora do mundo.
        assert_ne!((x, z), (0, 0));
    }

    #[test]
    fn compute_start_pos_falls_back_to_origin_when_bounds_unknown() {
        let tmp = tempfile::tempdir().expect("tmp dir");
        // Sem metadata.json e sem pasta region/: discover_bounds falha, e
        // compute_start_pos precisa cair pro (0, 0) sem entrar em pânico.
        assert_eq!(compute_start_pos(tmp.path()), (0, 0));
    }
}
