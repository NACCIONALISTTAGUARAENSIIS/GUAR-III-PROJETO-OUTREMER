ATENÇÃO: aqui serão dispostos todos os changelogs, tudo o que foi mudado, por que, razão etc de forma extremamente documentada

## 2026-09-29 — Escala de roleplay: metrô, vias, quadras, comércio e auditoria dos providers

Segunda rodada após a geração real do Guará I+II. Detalhes, causa e
verificação em `docs/QUALIDADE_GERACAO.md` (Parte II). Resumo:

1. **Metrô-DF** (`railways.rs`, `stations.rs` novo): túnel/viaduto/superfície
   decididos pelas tags (`tunnel`, `bridge`, `layer`) e não pelo tipo — a
   Linha Verde deixa de ser enterrada; estações e acessos (nós
   `railway=station|subway_entrance`, antes descartados) ganham plataformas
   alinhadas ao trilho, cobertura, placas e poços de acesso; terminais de
   ônibus reutilizam a Rodoviária de `landmarks.rs`; paradas usam o abrigo
   existente.
2. **Vias** (`highways.rs`): asfalto real, largura por `width`/`lanes`,
   `surface` respeitado, faixas (branca tracejada em mão única, dupla amarela
   em duas mãos), meio-fio + calçada, iluminação pública com o poste
   Neoenergia único do motor (`amenities::place_neoenergia_pole`).
3. **Quadras esportivas** (`sports.rs`, `oriented_frame.rs` novos): marcações
   por modalidade em retângulo de área mínima, traves/cestas/redes/rampas,
   alambrado via `barriers`, postes de canto.
4. **Comércio** (`retrieve_data.rs`, `poi_enrichment.rs` novo, `buildings.rs`,
   `amenities.rs`): Overpass passa a baixar `shop/office/craft/healthcare/
   public_transport/sport/playground`; POIs (nós e lojas indoor) injetam uso e
   nome no prédio que os contém antes do dispatch; categoria comercial/
   institucional correta e térreo de vitrine; pátios institucionais
   (`amenity=school|hospital|...` como área) ganham piso e alambrado
   reaproveitando `landuse` e `barriers`.
5. **Providers** (`providers/mod.rs`, `gdf/geojson/gpkg/postgis/osm/pbf`):
   tradutores de atributos preservados; Shapefile GDF passa a projetar com o
   `CoordTransformer` do mundo; `uso_to_tag` impede que lotes virem prédios
   (e substituam os prédios reais do OSM no merge); grupos semânticos
   canônicos (`semantic_group_from_tags`, `railway` → `Railway`).
6. **Chão × elementos** (`world_editor/mod.rs`, `data_processing.rs`,
   `highways.rs`): o passe de chão por região escrevia a superfície antes
   dos elementos e a escrita padrão é "só se vazio", então asfalto, pisos de
   quadra/pátio/landuse e água eram descartados em silêncio; e `highways`
   misturava Y relativo com absoluto (`ground.max(current_y)`), enterrando
   ruas em terreno positivo e fazendo-as flutuar em terreno negativo.
   Superfície de terreno intocada agora conta como vazia para a primeira
   escrita de elemento; a precedência do elemento (`get_priority`, escada
   refinada: prédio > via > trilho > água > cerca > piso esportivo >
   equipamento > lazer/natural > landuse > place) decide quem repinta o piso
   mesmo entre regiões (o `landuse` da QE 17 chegava pelo Halo antes do
   asfalto das ruas da região vizinha); `paint_y_at` unifica a cota de
   pintura das vias; o canteiro central só é pintado com raio > 0 (antes
   deixava uma linha de grama no eixo de toda via), o eixo recebe asfalto
   quando não há canteiro, o pincel sub-amostra as diagonais para não
   deixar buracos em xadrez no asfalto e as linhas de vaga passam a cada
   7 blocos (≈5,5 m).

## 2026-09-29 — Qualidade da geração: sete defeitos sistêmicos corrigidos

Auditoria de rigor sobre o pipeline inteiro (não sobre um gerador isolado).
Detalhes, causa exata, correção e verificação de cada item em
`docs/QUALIDADE_GERACAO.md`. Resumo:

1. **Merge de provedores** (`providers/mod.rs::resolve_collisions`): descartava
   qualquer feature cujo AABB tocasse outra do mesmo grupo semântico, mesmo da
   mesma fonte — de duas ruas que se cruzam só uma sobrevivia. Agora só
   deduplica ENTRE fontes, por prioridade estrita, com ≥50% de cobertura de
   AABB. Coordenadas negativas passam a usar buckets consistentes.
2. **Terreno plano com `--terrain`** (`ground.rs`, `data_processing.rs`):
   cache de elevação populada com chaves absolutas e consultada com chaves
   relativas ao bbox (que aqui nunca começa em zero). `Ground` agora guarda a
   origem e expõe as duas famílias de consulta; grade única para o mundo
   inteiro (sem fatiamento por região); DEM local e DSM re-baseados para o
   datum do SRTM; `elevation_enabled` só com dado real.
3. **Grade de elevação** (`elevation_data.rs`): dimensões exatas do `XZBBox`;
   amostragem bilinear dos tiles Terrarium via inversa exata do
   `CoordTransformer`; sem corte de percentis (picos reais preservados);
   preenchimento BFS O(N); teto vertical coerente com o escritor (319).
4. **Bordas de região** (`data_processing.rs`, `world_editor/mod.rs`):
   elemento executado na região do canto mínimo do AABB (com margem) em vez
   do centroide — todo vazamento passa a ser "para frente" e nada se perde em
   região já selada; features de provedor executadas uma vez; Halo virou log
   de operações com a semântica do Core (if-absent/whitelist/blacklist/force
   e propriedades preservadas), replayado em ordem após o chão existir.
   Segunda passada no fim da geração: regiões já seladas que ainda receberam
   vazamentos (copas da floresta ambiente na borda oeste/norte — 1,25 M de
   operações medidas no Guará I+II) são relidas do disco, atualizadas e
   regravadas (`flush_pending_halo`/`load_java_region_from_disk`).
5. **Floresta ambiente** (`tree.rs`, `floodfill_cache.rs`): só nasce em
   `GRASS_BLOCK` natural, fora de qualquer área mapeada (bitmap de cobertura
   de uso do solo/lazer/amenidades/natureza/água/prédios) e a >2 blocos de
   prédios — acabou a mata dentro de quadra residencial, campo e pátio.
6. **Spawn** (`main.rs`, `world_utils.rs`, `gui.rs`): Y gravado uma única
   vez, na cota real do terreno + 3, nos dois builds; removidas as duas
   funções de `gui.rs` que sobrescreviam o Y com 150 fixo.
7. **`describe_sources`** concluído (o stub anterior quebrava
   `clippy -D warnings`): todo provedor reporta arquivo/endpoint, logado ao
   iniciar e gravado em `provenance_summary.json` (`dataSources`).

Cobertura de testes nova: `providers::tests` (8), `ground::tests` (7),
`elevation_data::tests` (9), `data_processing::anchor_tests` (5),
`world_editor::halo_tests` (2), `floodfill_cache::mapped_area_tests` (4),
mais a asserção de `dataSources` em `provenance::tests`.
