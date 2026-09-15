# Compilação e Validação — Registro da Sessão (BESM-6)

Este documento registra, de forma rigorosa e reprodutível, o processo de
compilação e validação do motor Pincelism (BESM-6) realizado nesta sessão.
Antes dela, o projeto **nunca havia compilado com sucesso** desde o commit
inicial — nenhum dos gates de CI (`cargo check`, `cargo clippy`, `cargo fmt`,
`cargo test`, `cargo build --release`) jamais havia sido validado de ponta a
ponta. Este registro existe para que o estado atual (e as decisões tomadas no
caminho) sejam auditáveis, não apenas "funciona".

## Como reproduzir

```bash
# Necessário neste ambiente (ver "Achado #1" abaixo) — não necessário em CI,
# que usa toolchain "stable" recém-instalado com libclang consistente.
export LIBCLANG_PATH=/usr/lib/llvm-14/lib

cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --all-targets --all-features --release
```

Todos os quatro comandos acima — os mesmos usados por `.github/workflows/ci-build.yml`
— terminam limpos ao fim desta sessão (0 erros, 0 avisos, 44/44 testes
passando). O build de release completo (LTO `fat`, `codegen-units = 1`,
`overflow-checks = true`, conforme `Cargo.toml`) também conclui com sucesso.

---

## Achado #1 — Ambiente: bug de bindgen/libclang no `proj-sys`

A primeira tentativa de compilar falhava com 29 erros dentro da dependência
`proj-sys` (`E0609`/`E0560`, campos como `major`/`minor`/`xy` "inexistentes"
em `PJ_INFO`/`PJ_COORD`). Investigação:

- `proj-sys` compila a biblioteca PROJ 9.2.1 a partir do código-fonte
  empacotado (o PROJ do sistema, 8.2.1, é mais antigo que o mínimo exigido).
- O cabeçalho C gerado pelo CMake está correto e completo.
- O `bindgen` (via `clang-sys`), no entanto, gerava bindings **opacos**
  (`PJ_INFO { _address: u8 }`) para esses tipos específicos — não por causa do
  cabeçalho, mas por um comportamento de parsing sensível à versão do
  `libclang` escolhida. O ambiente tem libclang 14, 15 e 22 instalados lado a
  lado.
- Forçar `LIBCLANG_PATH=/usr/lib/llvm-14/lib` (a mesma versão do `clang`
  default do sistema) resolve completamente — os bindings passam a expandir os
  campos reais e o restante da árvore de dependências compila normalmente.

Isto é uma particularidade **deste ambiente de desenvolvimento**, não do
projeto: o CI (`dtolnay/rust-toolchain@v1` com `toolchain: stable`, Ubuntu
padrão do runner) não tem essa mistura de libclangs e não deve precisar da
variável. Documentado aqui para economizar a próxima pessoa que reproduzir
localmente numa máquina parecida.

---

## Achado #2 — Bug sistêmico: API do `rand` 0.9 usada contra `rand` 0.8

O erro isolado mais numeroso (189 de 241 erros iniciais do `cargo check`): o
código chamava `.random_range(...)`, `.random_bool(...)` e `.random()` — a
API do **rand 0.9** — em 12 arquivos, enquanto `Cargo.toml` fixa
deliberadamente `rand = "0.8"` (comentário no próprio arquivo: *"BESM-6: Rand
fixado na versão 0.8 (Estabilidade Governamental)"*). rand 0.8 usa
`.gen_range(...)`, `.gen_bool(...)`, `.gen()`.

Corrigido mecanicamente (rename 1:1, sem mudança de comportamento) em:
`block_definitions.rs`, `master_control.rs`, e `element_processing/{buildings,
tree, natural, amenities, landuse, man_made, barriers, highways, advertising,
leisure, historic}.rs`. Também: `use rand::prelude::IndexedRandom` (só existe
em 0.9) trocado por `rand::prelude::SliceRandom` (equivalente em 0.8) em 4
arquivos; `rand::rngs::SmallRng` precisava da feature `small_rng` do crate,
adicionada em `Cargo.toml`.

---

## Achado #3 — Reconexões reais (não supressões)

Vários módulos e providers estavam **prontos, corretos, mas nunca chamados**
— o padrão que os próprios commits anteriores já vinham descrevendo ("BESM-6:
Reconecta..."). Nesta sessão, sempre que uma peça estava desconectada, a
prioridade foi religar de verdade, não apenas silenciar o aviso do compilador.
Reconectado nesta sessão:

| Peça | Estava pronta em | Ligada em |
|---|---|---|
| `generate_building_interior` (25 funções: Bairro/Tipologia/EdificioBrasília, layouts de metrô/igreja/shopping/hospital/etc.) | `element_processing/subprocessor/buildings_interior.rs` | `buildings.rs` — a chamada existia comentada como *"disabled due to signature mismatch"*; reconciliada com o escopo real (`bounds`, `config`, `floor_levels`) |
| DSM (Modelo de Superfície — telhados/copas) | `providers/dsm_provider.rs` | Novo `GenerationOptions.surface_data`, populado em `main.rs`, consumido por `Ground::surface_level` (antes sempre vazio) |
| DEM local explícito (terreno nu alternativo ao SRTM) | `providers/dem_provider.rs` | Novo `GenerationOptions.dem_override`, sobrepõe `bare_earth_cache` quando fornecido |
| `advertising.rs` (MUB, totens, outdoors) | Arquivo inteiro | Roteado por `SemanticGroup::Advertising` no loop de `provider_features` de `data_processing.rs`, igual ao já existente para CAESB |
| `IndoorUtilityProvider` (pipeline CAESB/CEB) | `providers/indoor_utility_provider.rs` | Registrado em `main::register_providers` sob nova flag `--local-caesb-geojson` |
| `PostGisProvider`, `MeshProvider`, `MvtProvider`, `LidarProvider` (como fonte de Features, além do heightmap) | `providers/*.rs` | Registrados em `main::register_providers`; PostGIS ganhou novas flags `--postgis-table`/`--postgis-geom-column` |
| `CsvProvider`, `KmlProvider`, `Tiles3DProvider` | `providers/*.rs` | Registrados sob novas flags `--local-csv`, `--local-kml`, `--tiles3d-endpoint` |
| `map_renderer::render_world_map` + `emit_map_preview_ready` | `map_renderer.rs`, `progress.rs` | Chamados em `main.rs` logo após a geração de um mundo Java (build com GUI) |
| `world_utils::calculate_default_spawn` / `SessionLock` / `add_localized_world_name` / `gui::set_player_spawn_in_level_dat` | `gui.rs` (nunca chamados) | Fallback de spawn padrão, lock do diretório do mundo, e nome de mundo com área reverse-geocoded, todos ligados em `main::run_generation_pipeline` |
| `osm_parser::get_priority` | Existia, nunca usado | `data_processing.rs` agora ordena os elementos de cada região por prioridade antes do dispatch |
| `master_control::MasterControl` (dashboard TUI de Tile Streaming) | Existia, chamada quebrada (`MasterControl::new()` com 0 argumentos, assinatura real precisa de `Arc<ProviderManager>` + `Arc<Args>`) | `main()` agora monta um `ProviderManager` real (via `register_providers`, extraída para ser reaproveitada) e o passa corretamente — validado com `cargo check --no-default-features` (é o único modo em que este dashboard é o ponto de entrada) |
| `OSMProvider` ignorava `--file`, `--offline` e `--downloader` | `providers/osm_provider.rs` fazia a chamada Overpass hardcoded | Agora usa `--file` (JSON local) se fornecido, falha rápido em `--offline` sem arquivo local, e respeita `--downloader` |
| `power::generate_power` (postes/torres/linhas aéreas) | Nunca chamado para ways `power=*` | `data_processing.rs` despacha `way.tags.contains_key("power")` para ele |

Oito flags de CLI novas foram adicionadas como consequência direta dessas
reconexões (`--local-dsm` + geo-transform, `--local-dem` geo-transform,
`--local-caesb-geojson`, `--local-csv`, `--local-kml`, `--tiles3d-endpoint`,
`--postgis-table`, `--postgis-geom-column`) — todas opcionais, sem alterar o
comportamento padrão de quem não as usa.

## Achado #4 — Onde a reconexão foi propositalmente **adiada**

Um pequeno número de itens ficou com `#[allow(dead_code)]` documentado em vez
de conectado, porque a conexão exigiria uma decisão de design ou teste visual
que não é seguro improvisar numa passada de compilação:

- **`clip_water_ring_to_bbox`** (`clipping.rs`): útil como otimização de
  `water_areas::generate_water_areas_from_relation`, mas exige um adaptador de
  tipos (`ProcessedNode` vs. tuplas cruas) — tarefa de acompanhamento
  sugerida na sessão.
- **Providers puramente cfg-dependentes** (`master_control.rs` inteiro,
  `map_renderer.rs` inteiro, `world_utils::set_spawn_in_level_dat`,
  `world_editor::get_halo_metrics`, etc.): genuinamente usados, só que apenas
  numa das duas configurações de build (`gui` ligada vs. desligada) — o
  `#[allow]` é condicional (`cfg_attr`) e documenta exatamente isso, não
  esconde uma lacuna real.
- **`RasterProvider`**, `GmlSurfaceType::Roof/Ground` (CityGML): sobreposição
  não resolvida com providers já conectados (`VegetationProvider`); marcado
  para revisão futura em vez de arriscar classificação duplicada.

`doors::carve_and_place_door` (talha portas em paredes) foi conectado em
`buildings.rs` (`place_entrance_doors`) durante a sessão — ver seção seguinte.

## Nota sobre trabalho concorrente

Uma segunda sessão do Claude Code trabalhou no mesmo repositório em paralelo
durante parte desta sessão (mesmo servidor, mesmo diretório de trabalho),
focada em: `water_areas.rs` (implementação do zero — o arquivo antes só tinha
um protótipo de floresta obsoleto e nunca compilava), fetch real de
elevação/bioma em `main.rs`/`data_processing.rs`, novas espécies de árvore em
`tree.rs`, categoria `BuildingCategory::Government` e ligação de
`place_entrance_doors`/`carve_and_place_door` em `buildings.rs`, e pipeline de
tubulação CAESB em `man_made.rs`. As mudanças de ambas as sessões foram
verificadas juntas nesta validação final.

---

## Estado final

```
cargo fmt -- --check                                           → OK
cargo clippy --all-targets --all-features -- -D warnings       → OK (0 erros)
cargo clippy --no-default-features --all-targets -- -D warnings→ OK (0 erros)
cargo test --all-targets --all-features                        → OK (44 passed; 0 failed)
cargo build --all-targets --all-features --release             → OK
cargo check --no-default-features                               → OK
```

`rust-version` em `Cargo.toml` foi atualizado de `1.75` para `1.89` — o
projeto já usa APIs estabilizadas depois de 1.75 (`Iterator::is_multiple_of`,
`File::unlock` em `fs2`); o MSRV declarado estava desatualizado em relação ao
código real, não o contrário.

Cinco testes em `args.rs` (`test_bedrock_flag`, `test_wfs_dependency`,
`test_offline_wfs_conflict`, `test_required_options`,
`test_spawn_point_both_required`) usavam uma bbox de teste ("1,2,3,4", uma
área real de ~500km×500km) maior que os limites de segurança
(`--max-area-km2`, cota de armazenamento de 180GB) adicionados nesta rodada de
"BESM-6" — os testes nunca haviam rodado contra essas regras novas. Corrigidos
para usar uma bbox pequena e realista (ou `--max-area-km2` explícito) sem
enfraquecer as validações em si.
