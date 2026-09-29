ATENÇÃO: aqui serão dispostos todos os changelogs, tudo o que foi mudado, por que, razão etc de forma extremamente documentada

## 2026-09-29 — Auditoria do Guará I+II completo: inconsistências entre módulos

Detalhes, medições e verificação em `docs/QUALIDADE_GERACAO.md` (Parte IV,
§21–§27). Com o relevo real funcionando, apareceram defeitos que o chão plano
escondia. Medido no Guará completo antes → depois: buracos na cota do chão
19.448 → 443; asfalto flutuando (amostra) 12.053 → 207.

1. **Linha aérea enterrada** (`data_processing.rs`): `power=line` deixa de
   ganhar duto de cobre subterrâneo; `is_buried_network` decide por
   `location`/túnel/`layer` e pelo tipo (cabo e adutora enterram).
2. **Equipamentos 25 blocos no ar** (`amenities.rs`): 28 escritas com cota
   absoluta passada à API relativa (Y = 2 × chão); todas para `*_absolute`.
3. **Lajes na caixa envolvente** (`world_editor/mod.rs`, `buildings.rs`):
   máscara de escrita = contorno real do prédio durante o interior.
4. **Prédio de encosta flutuando** (`buildings.rs`): embasamento sólido até o
   chão (o ramo de "vão livre" escrevia ar e apagava o chão).
5. **Merge de infraestrutura** (`providers/mod.rs`): estacionamentos, cercas e
   equipamentos deduplicados entre provedores comparando o mesmo tipo de objeto
   e de geometria, com sobreposição real de polígono; cercas com tolerância de
   4 blocos.
6. **Trilho em encosta** (`railways.rs`): aterro por célula até o leito dela.
7. **Árvore cortada pelo estacionamento** (`amenities.rs`): limpa a vegetação
   acima das vagas.

## 2026-09-29 — Vias entre provedores: conflação por linha, pistas duplicadas e sinalização

Terceira rodada, feita no servidor Oracle sobre o trabalho da nuvem. Detalhes,
números medidos e verificação em `docs/QUALIDADE_GERACAO.md` (Parte III, §16–§20).

1. **Merge de linhas** (`providers/mod.rs`): vias, trilhos, cercas e cursos
   d'água são deduplicados por comprimento coberto (≥70% a ≤6 blocos, somando
   todos os trechos aceitos), não por caixa envolvente. No Guará a regra por
   caixa descartava 1.711 vias do OSM que não eram a mesma rua (serviço,
   calçadas, trechos da EPTG/EPIA) e deixava passar 457 duplicatas. A
   duplicata empresta nome/sentido/classe só a quem ela cobre; ponte/túnel do
   OSM substitui o eixo do GDF; tags por nó nunca são herdadas.
2. **Pistas duplicadas** (`highways.rs`): via de mão única não tem canteiro
   nem mureta próprios (o canteiro é o vão entre as duas pistas mapeadas);
   `dual_carriageway` dá canteiro; canteiro soma à largura vinda de
   `lanes`; `kerb=no` deixa a guia rente.
3. **Sinalização** (`highways.rs`): eixo amarelo, divisórias, bordos e zebra
   passam a ser pintados (whitelist = leito da própria via); antes a escrita
   empatava com o leito e era descartada — nenhuma rua tinha eixo.
4. **GDF** (`geojson_provider.rs`): camada curada nunca vira prédio pelo
   fallback (2.317 faixas de passeio e 22 lotes sem uso viravam "prédios");
   `edificacao` é prédio de base; eixo de arruamento traduz faixas, pistas,
   revestimento e meio-fio; calçada construída é área preenchida, em
   andesito/concreto claro.
5. **Toolchain** (`floodfill_cache.rs`): `as_chunks` no lugar de
   `chunks_exact` com tamanho constante.

## 2026-09-29 — Escala de roleplay: metrô, vias, quadras, comércio e auditoria dos providers

Segunda rodada após a geração real do Guará I+II. Detalhes, causa e
verificação em `docs/QUALIDADE_GERACAO.md` (Parte II). Resumo:

1. **Metrô-DF** (`railways.rs`, `stations.rs` novo): túnel/viaduto/superfície
   decididos pelas tags (`tunnel`, `bridge`, `layer`) e não pelo tipo — a
   Linha Verde deixa de ser enterrada; estações e acessos (nós
   `railway=station|subway_entrance`, antes descartados) ganham plataformas
   alinhadas ao trilho, cobertura, placas e poços de acesso; terminais de
   ônibus reutilizam a Rodoviária de `landmarks.rs`; paradas usam o abrigo
   existente; o leito passa a ser contínuo (Bresenham entre os vértices da
   clotoide — antes só os vértices eram pintados e os viadutos de 2 nós da
   Feira/Shopping não existiam).
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
8. **Merge de provedores** (`providers/mod.rs`): a feature vencedora
   (LiDAR/CityGML, contorno exato) herda as chaves semânticas que não tem da
   feature OSM que substitui (nome, pavimentos, amenity/shop, leaf_type),
   com `merged:source` para a auditoria — a fonte que não sabe o que vê
   deixa de apagar a que sabe.
7. **Árvores × estruturas** (`block_definitions.rs`, `world_editor/mod.rs`,
   `tree.rs`, `highways.rs`): vegetação existente cede a escritores
   estruturais (prédio, via, trilho...) em vez de deixar paredes com buracos;
   a semente da árvore só pega em solo natural (lista positiva); a via limpa
   a copa acima da pista e do passeio. Resolve o caso clássico do LiDAR, que
   não distingue copa de telhado.
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
