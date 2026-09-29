# Qualidade da geração — auditoria e correções sistêmicas

Este documento registra uma passada de rigor sobre o **pipeline de geração**
(não sobre um módulo isolado): os defeitos encontrados atuavam sobre **todo**
elemento gerado, em **toda** região, e por isso degradavam o mundo inteiro
independentemente do capricho de cada gerador individual (prédios, vias,
Cerrado...). Cada item abaixo traz: o sintoma visível no mundo, a causa exata
no código (com o arquivo), a correção, e **como verificar** — testes unitários
novos cobrem cada um, e os testes de fumaça existentes (que rodam o binário
real com provedores locais) continuam passando.

Nenhuma flag nova foi criada: todas as correções valem para quem já roda
`pincelism --bbox ...` hoje. Comportamentos que dependiam de flags
(`--terrain`, `--no-ambient-forest`, `--city-boundaries`) continuam
respeitando-as.

---

## 1. Merge de provedores dizimava o próprio OSM

**Sintoma.** De duas ruas que se cruzam, só uma aparecia; de dois prédios
vizinhos com caixas envolventes encostadas, um sumia; polígonos de uso do solo
que se tocam eram descartados aos montes. O mundo saía "ralo" — e o efeito
crescia com a densidade da área (justamente no Guará/Plano Piloto).

**Causa.** `ProviderManager::resolve_collisions` (`src/providers/mod.rs`)
descartava **qualquer** feature cujo AABB intersectasse o AABB de outra já
aceita do mesmo `SemanticGroup`, sem olhar de onde cada uma veio. Com um único
provedor (o caso padrão: só OSM, prioridade 10 para tudo), a função — cujo
objetivo declarado era "dois provedores diferentes não devem gerar o mesmo
Building no mesmo lugar" — deduplicava o OSM contra ele mesmo. Todas as vias
são `SemanticGroup::Highway`; duas vias que se cruzam **sempre** têm AABBs que
se intersectam.

**Correção.** Uma feature só é descartada se uma feature já aceita (1) veio de
**outra fonte**, (2) tem prioridade **estritamente** maior, (3) é do mesmo
grupo semântico e (4) cobre pelo menos 50% do AABB da candidata (é "o mesmo
objeto", não um vizinho encostado — `aabb_coverage_ratio`). A mesma fonte
nunca colide consigo mesma. Buckets espaciais agora usam `div_euclid`
(coordenadas negativas — o caso normal a oeste do Marco Zero — antes caíam em
buckets inconsistentes em torno de zero).

**Verificação.** `providers::tests::*` (8 testes): ruas cruzadas mantidas,
prédios adjacentes mantidos, lote GDF (prioridade 1) substitui o prédio OSM
equivalente, sobreposição parcial (<50%) mantém ambos, grupos diferentes nunca
colidem, prioridades iguais de fontes diferentes mantêm ambos, coordenadas
negativas.

---

## 2. Terreno: cache absoluta, consulta relativa → mundo sempre plano

**Sintoma.** Com `--terrain`, os tiles SRTM eram baixados, processados,
logados ("Realistic elevation: ... stretched to N blocks") — e o mundo saía
**plano** em `ground_level`. LiDAR local idem.

**Causa.** `Ground::level(XZPoint)` (`src/ground.rs`) sempre foi consumido com
coordenadas **relativas** ao canto mínimo do `XZBBox` (`WorldEditor::
get_ground_level` subtrai `min_x`/`min_z`; `gui.rs`, `bedrock.rs` e
`buildings.rs` fazem o mesmo). A cache, porém, era populada em
`data_processing.rs` com chaves **absolutas** `(x, z)`. No Arnis original a
bbox começa em (0, 0) e as duas convenções coincidem; neste projeto o `XZBBox`
é ancorado no Marco Zero de Brasília, e o Guará fica ~15.000 blocos a oeste —
**nenhuma** consulta encontrava a chave e todas caíam no fallback plano.

**Correção.** `Ground` guarda a **origem** do grid e converte internamente:
`level`/`surface_level` seguem aceitando coordenadas relativas (contrato
legado, sem quebrar chamadores) e `level_abs`/`surface_level_abs`/`get_biome`
aceitam absolutas. `advertising.rs`, que já passava coordenadas absolutas para
`surface_level`, usa a variante absoluta explícita.

Consequências arrastadas pela mesma correção:

- **Grade única para o mundo inteiro.** O fatiamento por região (~262 mil
  inserções de hash por região de 512×512) foi eliminado: `Ground` referencia
  a grade densa inteira por `Arc`, sem cópia. Geometria que "vaza" para a
  região vizinha (pontes, avenidas, telhados) consultava um chão que não
  existia naquela fatia e ganhava degraus exatamente na borda de região;
  agora consulta a mesma grade. Fora do bbox, devolve a cota da borda mais
  próxima em vez de "plano".
- **`elevation_enabled` só quando há dado.** Antes, `--terrain` com falha de
  download deixava o `Ground` "habilitado" com cache vazia — módulos como
  `highways.rs` (pontes sobre vale) tomavam decisões de terreno sobre um chão
  chato. Agora há aviso explícito no log e o `Ground` é plano de verdade.
- **DEM local re-baseado.** `DemProvider` produz cotas em datum **absoluto**
  (`ground_level + metros × scale_v`, ~1.300 blocos para o Planalto Central);
  eram fundidas cruas sobre o SRTM (datum relativo, mínimo em `ground_level`)
  — o trecho coberto pelo DEM virava um platô acima do teto do mundo,
  truncado em Y=319. `Ground::assemble` re-baseia o DEM pelo deslocamento
  mediano nas colunas em comum com o SRTM (ou pelo próprio mínimo, sem SRTM);
  o DSM recebe o mesmo deslocamento.

**Verificação.** `ground::tests::*` (7 testes): consulta relativa e absoluta
concordam com origem não nula; clamp fora do grid; plano sem fontes; DEM
re-baseado sobre SRTM; DEM sozinho ancora o mínimo em `ground_level`; DSM;
reamostragem de grade de outra dimensão. O teste de fumaça
`terrain_providers_smoke_test` (LiDAR + DEM + MapBiomas reais, sem rede)
continua passando.

---

## 3. Grade de elevação: dimensão errada, amostragem por "salpico", picos cortados

**Causas e correções** (`src/elevation_data.rs`):

| Antes | Agora |
|---|---|
| Grade dimensionada por `geo_distance × scale_h` (Haversine), diferente da fórmula ENU do `XZBBox` — divergiam por alguns blocos, deixando uma faixa plana numa borda. | Grade com **exatamente** `width × height` do `XZBBox`. |
| Cada pixel Terrarium (~4,8 m no zoom 15) era jogado na célula mais próxima; com células de 0,75 m, >95% da grade ficava vazia e era preenchida por dilatação 3×3 iterativa (degraus), depois disfarçada por um blur cujo σ crescia com o tamanho do mapa (~22 blocos num mapa de 2 km). | **Interpolação bilinear** dos 4 pixels vizinhos (também através de bordas de tile), por célula, com a **inversa exata** do `CoordTransformer` (a mesma projeção das ruas e prédios). Blur residual pequeno e constante (σ = 6 blocos SRTM, 1,5 LiDAR). |
| Filtro de outliers descartava tudo fora dos percentis 1–99: num bbox com um único morro, o topo dele (1% da área) era achatado. | Só valores fisicamente impossíveis (`< -500 m`, `> 9.000 m` — os voids do SRTM) e picos isolados de 1 célula (> 25 m acima da mediana dos 8 vizinhos) são removidos. |
| Preenchimento de buracos O(iterações × N) (uma falha de tile de 256 px exigia centenas de passadas sobre a grade inteira). | Preenchimento pelo vizinho válido mais próximo em BFS multi-fonte, O(N). |
| Teto vertical `MAX_Y = 4064` ("datapack"), inexistente no escritor de chunks (que trunca em 319). | `ground::TERRAIN_MAX_Y = 319 − 100` (folga para edificações); a compressão vertical usa esse teto real. |
| Pontos LiDAR mapeados por interpolação linear em graus. | Projetados pela transformação direta do `CoordTransformer` para a coluna exata. |

**Verificação.** `elevation_data::tests::*` (9 testes): decode Terrarium,
continuidade bilinear através da borda de tile, tile ausente cai nos taps
disponíveis, BFS de preenchimento, spike vs. crista, saneamento de valores
implausíveis, blur normalizado, quantização (mínimo → `ground_level`,
`scale_v`), compressão sob o teto.

---

## 4. Bordas de região: metade dos vazamentos era perdida

**Sintoma.** Ruas cortadas ao meio exatamente em múltiplos de 512 blocos;
prédios com uma fatia ausente; lagos com um quadrante faltando. Elementos
grandes parcialmente fora do bbox (um lago, uma rodovia) às vezes não
apareciam **nada**.

**Causa.** O Scanline (`data_processing.rs`) executa cada elemento **uma vez**,
na região do seu **centroide**, e roteia os blocos que caem fora dela para o
Halo Cache, replayado quando a região destino vira ativa. A varredura é `rz`
crescente e, na linha, `rx` crescente — então tudo o que o elemento pintasse
em regiões "anteriores" (à esquerda/acima do centroide) ia para o Halo de uma
região **já selada em disco** e nunca mais era escrito. Centroide fora do bbox
⇒ região nunca varrida ⇒ elemento nunca executado. Além disso, `Feature`s de
provedor (CAESB, advertising) eram executadas em **toda** região que o AABB
intersectasse — um duto de 2 km era desenhado 4–5 vezes, com proveniência em
dobro.

**Correção.**

- **Região-âncora = canto mínimo do AABB** do elemento, recuado por uma margem
  de alcance (`ELEMENT_REACH_MARGIN = 64` blocos: brush do Eixo Monumental,
  copas, beirais, pilares) e **recortado** ao intervalo varrido. Prova
  simples: toda coordenada que o elemento toca tem `z ≥ min_z − margem`
  (logo `rz ≥ âncora`) e, na mesma linha, `x ≥ min_x − margem` (logo `rx ≥
  âncora`) — **todo vazamento é para frente**, para uma região que ainda vai
  replayar o Halo antes de selar. Provedores diretos usam a mesma âncora, uma
  vez só.
- **Halo como log de operações com a semântica do Core.** O Halo antigo
  guardava só "posição → bloco", sobrescrevendo incondicionalmente e
  descartando whitelists/blacklists e **propriedades** (orientação de escada,
  meia-laje): na borda de região, uma via sobrescrevia a parede de um prédio
  que dentro da região ela respeitaria, e escadas viravam blocos sem
  orientação. Agora cada escrita adiada guarda seu modo (`IfAbsent`,
  `Whitelist`, `Blacklist`, `Force`) e propriedades, e é replayada **na
  ordem**, **depois** do chão da região existir — exatamente a decisão que
  teria sido tomada in-core. Um estado "sombra" mantém as leituras
  antecipadas (`check_for_block_absolute`) coerentes.
- **Segunda passada para o que ainda vaza para trás.** A âncora cobre os
  ELEMENTOS; a floresta ambiente, porém, é gerada por chunk (não por
  elemento) e planta copas/troncos caídos na borda oeste/norte de cada região
  — que caem na região anterior, já selada. Medido na geração real do Guará
  I+II: **1,25 milhão** de operações nessa situação, antes simplesmente
  perdidas (copas cortadas em linha reta a cada 512 blocos). Agora, no fim da
  varredura, `WorldEditor::flush_pending_halo` relê cada região afetada do
  disco (`load_java_region_from_disk`: paleta + índices empacotados,
  propriedades de blockstate e `block_entities` preservados), replaya as
  operações com a semântica normal e regrava. Só no formato Java; no Bedrock
  o log avisa a quantidade não aplicada.

**Verificação.** `data_processing::anchor_tests::*` (5 testes, incluindo a
propriedade "a âncora nunca vem depois de qualquer região tocada na ordem da
varredura") e `world_editor::halo_tests::*` (replay com cada modo contra um
chão pré-existente; propriedades preservadas; estado sombra em ordem; e a
segunda passada gravando/relendo um `.mca` real e conferindo, com o parser da
`fastanvil`, que o bloco vazado apareceu e os originais — inclusive uma
escada com `facing` — sobreviveram).

---

## 5. Floresta ambiente do Cerrado nascia dentro da cidade

**Sintoma.** Árvores e sub-bosque em quintais de quadra residencial, campos
de futebol, estacionamentos de terra, cemitérios, pátios industriais,
canteiros de obra — em qualquer bloco cuja superfície não fosse uma de meia
dúzia de "proibidas" (asfalto, concreto, água).

**Causa.** `tree::generate_chunk` usava uma **lista curta de superfícies
proibidas**; qualquer outra (grama, terra, farmland, podzol, areia...) era
considerada mata. Uma quadra `landuse=residential` é pintada de grama/terra
pelo gerador de uso do solo — e virava floresta.

**Correção.** A regra foi invertida para uma **lista positiva**
(`tree::is_wild_ground`): a mata só nasce onde (a) a superfície ainda é o
`GRASS_BLOCK` base do mundo — qualquer gerador que já passou deixou outra
superfície; (b) a coluna está **fora de toda área mapeada**
(`FloodFillCache::collect_mapped_area_coverage`: um bitmap de 1 bit/coluna
com todo polígono fechado/multipolígono de `landuse`, `leisure`, `amenity`,
`natural`, `water`, `waterway`, `aeroway`, `man_made`, `power`, `military`,
`tourism`, `shop`, `historic`, `place`, `parking`, `highway`+`area=yes` e
prédios); (c) está a mais de 2 blocos de qualquer prédio (copa não atravessa
parede). O bitmap só é construído quando a floresta está ligada. A árvore
também passa a usar a cota real da coluna final (depois do jitter), não a da
coluna de origem.

**Verificação.** `floodfill_cache::mapped_area_tests::*` (4 testes: polígono
residencial cobre o interior; via aberta não é área, praça `area=yes` é;
polígono não fechado é ignorado; cerca não conta).

---

## 6. Spawn do jogador a 150 blocos de altura

**Sintoma.** O jogador nascia no ar e caía dezenas de blocos (com `--terrain`,
morria na queda).

**Causa.** `data_processing.rs` corrigia o Y do spawn para o terreno **durante**
a geração (com um `Ground` fictício plano, ainda por cima) e, logo depois,
`main.rs` chamava `set_player_spawn_in_level_dat`/`set_spawn_in_level_dat`,
que **sobrescreviam** o Y com um 150 fixo — nos dois builds (com e sem GUI).

**Correção.** Um único ponto (`main.rs`, após a geração) grava X, **Y real do
terreno + 3** e Z via `world_utils::set_spawn_in_level_dat`, usando o mesmo
`Ground` definitivo do mundo. As duas funções redundantes de `gui.rs` foram
removidas.

---

## 7. `describe_sources` (WIP que quebrava o CI)

O commit anterior deixou `DataProvider::describe_sources()` como stub sem
chamadores — `cargo clippy -D warnings` (o gate do CI) falhava com
`method describe_sources is never used`. Concluído: todos os provedores de
arquivo/endpoint reportam a fonte exata (o PostGIS omite credenciais),
`ProviderManager::fetch_all` loga cada fonte ao iniciar o provedor, e
`provenance_summary.json` ganhou a seção `dataSources` (provedor → fontes).

---

## Como reproduzir a validação

```bash
export LIBCLANG_PATH=/usr/lib/llvm-18/lib   # só neste ambiente (ver COMPILACAO_E_VALIDACAO.md)
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test --all-targets --all-features
cargo test --all-targets --no-default-features
```

Os testes de fumaça (`tests/multi_provider_smoke.rs`) rodam o binário real,
offline, com fixtures de todos os provedores locais — inclusive
`--terrain` com LiDAR + DEM + MapBiomas — e validam que os chunks gravados são
legíveis pelo parser de produção da `fastanvil`.

## O que NÃO foi alterado (de propósito)

- Nenhum gerador individual (`buildings.rs`, `highways.rs`, ...) teve sua
  estética mudada: as correções são de infraestrutura (roteamento, terreno,
  merge, spawn). O ganho visível vem de esses geradores finalmente receberem
  os dados certos (terreno real, todas as features, blocos completos nas
  bordas).
- O modo HUD interativo (`generate_region_from_global`, só sem `gui`) continua
  com chão plano e sem floresta ambiente, como documentado em
  `data_processing.rs`.

---

# Parte II — Escala de roleplay: metrô, vias, quadras, comércio e providers

Segunda rodada, disparada pela geração real do Guará I+II (bbox
`-15.86,-47.99,-15.80,-47.95`, 341 mil elementos OSM). O bairro era
reconhecível, mas faltavam exatamente as coisas que o tornam o Guará: o
Metrô-DF e suas estações, as quadras esportivas das QE/QI, as ruas com cara de
rua e o comércio das entrequadras. A auditoria foi feita **contra os dados**
(contagem por tag do que o Overpass devolve × o que o `dispatch_element`
efetivamente desenha, via `provenance.json`), não por impressão visual.

## 8. Metrô-DF: enterrado por regra de tipo, estações nunca desenhadas

**Sintoma.** Nenhuma linha, nenhuma estação (Feira, Guará, Arniqueiras,
Shopping), nenhum acesso.

**Causa.** `railways.rs` tratava `railway=subway` como "sempre túnel" (regra
por TIPO) e cavava a via 10 blocos abaixo do chão mesmo onde o OSM marca
`tunnel` ausente, `bridge=viaduct` ou `layer=1` — no Guará a Linha Verde é
quase toda de superfície/elevada. As estações são **nós** (`railway=station`,
`railway=subway_entrance`) e o `dispatch_element` não tinha ramo para
`railway` em nós: descartados em silêncio (0 despachados na proveniência).

**Correção.**
- `railways.rs`: `is_tunnel_way`/`is_elevated_way`/`vertical_offset(layer,
  tunnel, elevated)` decidem pela TAG (`tunnel=*`, `bridge=*`, `layer`,
  `location`), não pelo tipo. Túnel: `layer.min(-1)·10` (piso ≥ −40); elevado:
  `layer.max(1)·6+1` com tabuleiro de viaduto (`LIGHT_GRAY_CONCRETE`, guarda-
  corpo `STONE_BRICK_WALL`, pares de pilares a cada 14 pontos); superfície com
  cerca de faixa de domínio (`IRON_BARS`). `subway|light_rail|monorail`
  contam como metrô para o desenho do trilho.
- `stations.rs` (novo): `RailIndex` (segmentos, entradas, estações) constrói
  a estação a partir do nó e do trilho mais próximo: plataformas laterais
  alinhadas ao eixo (`OrientedFrame`), 60 blocos de meia-extensão, faixa
  tátil amarela, bancos, pilares; caixa subterrânea iluminada quando o trilho
  está em túnel; cobertura pelo `landmarks::generate_unique_landmark` quando a
  estação tem nome conhecido (mesma lógica de marcos), cobertura genérica
  caso contrário; placas "METRÔ-DF"; acessos (`subway_entrance`) ligados por
  poço com escada + corredor até a plataforma. `buffer_stop` e `halt` também.
- **Leito contínuo.** Medido no mundo v7: o laço de `generate_railways`
  pinta um disco de leito por PONTO da polilinha, e a clotoide devolve só os
  vértices (2 pontos numa reta, amostras espaçadas nas curvas) — o metrô saía
  pontilhado e os viadutos de 2 nós da Feira e do Shopping não existiam (a
  seção transversal no meio deles era chão urbano). `densify_polyline`
  (Bresenham entre vértices) alimenta os dois laços de desenho; pilares e
  luminárias passam a ser espaçados em blocos, como o comentário já dizia.
- Terminais de ônibus (`amenity=bus_station`, `public_transport=station` em
  way/relation) reutilizam `landmarks::generate_terminal_rodoviario` (a
  Rodoviária do Plano já existia; só foi parametrizada em altura).
- Paradas (`public_transport=platform` em nó) usam o abrigo já existente,
  extraído para `highways::generate_bus_shelter`.

## 9. Vias: tudo despachado, nada parecia rua

**Sintoma.** "Parece que não gerou as vias." A proveniência mostrava 100% das
`highway=*` despachadas — o problema era o desenho: cinza uniforme, largura
fixa, sem meio-fio, sem calçada, sem faixa, sem poste.

**Correção (`highways.rs`).**
- Asfalto real (`BLACK_CONCRETE`) para toda via motorizada; `surface=*` do OSM
  respeitado (paralelepípedo, terra, cascalho, concreto).
- Largura por dado: `width=*` em metros ou `lanes=*`
  (`lanes_to_half_width`, compartilhada com `bridges.rs`); tipologias do DF
  (Via Guará, via local com faixa de estacionamento e canteiro de 3, motorway
  com acostamento); `service=parking_aisle|driveway` estreitos.
- Sinalização: `oneway` → tracejado branco; duas mãos com ≥6 de largura →
  linha dupla amarela contínua (`lane_divider_offset`); faixas laterais.
- Zona 3 de meio-fio (`POLISHED_ANDESITE` + `SMOOTH_STONE_SLAB`) e calçada de
  2 blocos, desligáveis por `sidewalk=no`.
- Iluminação pública a cada 34 blocos, lados alternados, com o poste padrão
  Neoenergia extraído de `amenities::place_neoenergia_pole` (o ÚNICO poste do
  motor — vias, estacionamentos, quadras); `lit=no` respeitado.
- `highway=corridor` (indoor) ignorado; `street_lamp` em nó usa o mesmo poste.

## 10. Quadras esportivas por modalidade

**Sintoma.** `leisure=pitch` virava um retângulo verde.

**Correção.** `sports.rs` (novo): `classify` por `sport=*` (sem tag: >3500
blocos → futebol, senão poliesportiva), `default_surface` por modalidade,
`generate_pitch` desenha marcações em coordenadas locais do
`OrientedFrame::from_polygon` (retângulo de área mínima — as quadras do Guará
raramente estão alinhadas aos eixos): futebol/society (grande área, círculo
central, traves), basquete (garrafão, cestas), vôlei/tênis (rede), futsal,
skate (rampas). Alambrado via `barriers::generate_barriers` (way sintético
`barrier=fence`+`fence_type=chain_link`, altura 3) e quatro postes de canto.
`oriented_frame.rs` também substitui a estimativa de ângulo de
`landmarks::get_oriented_bounds`.

## 11. Comércio: o uso estava nos nós, não nos prédios

**Sintoma.** "Problema com a estrutura dos negócios." Dos 24.600 prédios do
Guará, 24.410 são `building=yes` sem uso; o uso está em **nós** dentro deles
(`shop=*`, `amenity=restaurant`, `office=*`...). O Overpass nem baixava
`shop`/`office`/`craft`/`healthcare`, e o dispatcher não desenha nada para um
nó `amenity=restaurant` solto — o comércio inteiro sumia e cada loja era uma
casa genérica.

**Correção.**
- `retrieve_data.rs`: a consulta pede `shop`, `office`, `craft`,
  `healthcare`, `public_transport`, `sport`, `playground`.
- `poi_enrichment.rs` (novo, pré-passe antes do dispatch): índice espacial
  dos POIs (nós e também ÁREAS de uso sem `building` — lojas indoor do
  shopping/feira com `indoor`/`level`, pátios institucionais — pelo
  centróide); para cada prédio, o POI dominante contido no polígono (ray
  casting) injeta as chaves de uso que faltam, o `name` e `poi:count`.
  Assim a lógica que JÁ existia passa a valer sem código novo:
  `BuildingCategory::from_element`, `buildings_interior::detect_tipologia`,
  `eh_uso_misto_comercio_terreo`.
- `buildings.rs`: `building=yes|commercial|retail` com `amenity`/`shop`/
  `office`/`tourism`/`healthcare` cai na categoria certa (hospital, escola,
  escritório, hotel, comércio); prédio comercial baixo ganha térreo de
  vitrine (vidro com pilares, toldo colorido por `VITRINE_COLORS`).
- `amenities::generate_institutional_grounds` (novo): áreas `amenity=school|
  hospital|place_of_worship|police|...` sem `building` (36 escolas no Guará)
  caíam no `_ => {}` e não geravam nada; agora o piso sai de
  `landuse::generate_landuse` (estilo `education`/`religious` já existente) e
  o alambrado de `barriers` via `sports::place_fence`, que copia `amenity`/
  `landuse`/`name` para `barriers` aplicar a semântica de escola (4 m) que ele
  já conhecia. O piso pavimentado só em lotes de até 2.500 blocos (pátio);
  campi maiores (o Batalhão do Guará tem 83 mil blocos, a mediana dos 57
  terrenos é 6,5 mil) ficam gramados e só ganham a cerca — pavimentar tudo
  virava uma mancha branca do tamanho de uma quadra na geração real.

## 12. Auditoria dos providers: onde eles conflitavam

Os **tradutores de atributos** de cada provider (Shapefile GDF, GeoJSON,
GeoPackage, PostGIS, KML, CSV...) foram mantidos intactos: são conhecimento de
domínio de cada fonte e a organicidade do resultado depende deles. O que foi
corrigido é a **interface** entre eles e o merge.

| Conflito | Efeito | Correção (aditiva) |
|---|---|---|
| Shapefile GDF projetava com equirretangular própria (`project_to_minecraft_xz`), não com o `CoordTransformer` do mundo | Lotes/edificações do SITURB deslocados dezenas de blocos em relação ao OSM; culling por bbox errado | `gdf_provider.rs` usa `llbbox_to_xzbbox` + `transform_point` e descarta pelo `XZBBox` real |
| Quatro tradutores mapeavam `USO=Residencial` → `building=residential` | Camada "Lotes Registrados" virava um prédio por LOTE; com prioridade 1 e grupo `Building`, esses lotes ainda **substituíam** os prédios reais do OSM no merge | `providers::uso_to_tag(uso, has_structure)`: prédio só com evidência estrutural (pavimentos/altura) ou camada de edificações; senão `landuse=residential|retail|institutional|industrial`. Fallback `building=yes` não é mais aplicado quando há `landuse` |
| Grupos semânticos divergentes (OSM punha `railway` em `Highway`, KML em `Railway`; PBF `natural` em `Terrain`, GeoJSON em `Natural`) | `resolve_collisions` só deduplica no MESMO grupo → o dado prioritário nunca substituía o duplicado | `providers::semantic_group_from_tags` canônico; OSM/PBF `railway` → `Railway`; GeoJSON/GPKG/PostGIS usam o canônico como fallback (override explícito respeitado) |
| OSM (10) e PBF (10) com a mesma prioridade | Duplicatas quando os dois estão ativos: empate não é resolvido | Documentado; não alterado (o PBF é alternativa offline ao OSM, não complemento) |

Prioridades em vigor (`main.rs::register_providers`, menor = vence):
LiDAR, CityGML, IFC, Mesh, PostGIS, GDF Shapefile, GeoJSON, GeoPackage,
Indoor/Utility = **1**; WFS, KML, 3D Tiles = **2**; CSV, MVT = **5**;
OSM, PBF = **10**. Regra: o dado governamental/levantado vence o
crowdsourced no mesmo grupo semântico e com ≥50% de sobreposição de AABB.

## 13. O chão nascia antes dos elementos e engolia toda pintura de piso

**Sintoma.** Depois de tudo acima, a geração real do Guará v2 ainda mostrava
as ruas da QE 17 como uma faixa contínua de andesito polido — sem asfalto — e
quadras/pátios sem piso. Medido no mundo gerado (seção transversal de uma
`highway=residential`, via o endpoint `/chunk` do visualizador): 20 colunas
com a mesma cor de superfície do chão urbano; e uma rua da Candangolândia
(terreno a −11) com asfalto **flutuando na cota 0**.

**Causa (duas, anteriores a este branch).**
1. O Scanline preenche o chão de cada região (passo 2) ANTES de despachar os
   elementos (passo 4), e a escrita padrão do motor,
   `set_block(..., None, None)`, é "só se vazio". No Arnis original o chão
   nasce DEPOIS dos elementos, então toda pintura de piso vencia o terreno;
   aqui ela batia no bloco de chão e era descartada em silêncio — 19 pontos
   de escrita em 8 módulos (asfalto e faixas de `highways`, pisos de
   `landuse`/`leisure`/`sports`/`stations`, água, pátios). Enquanto o mundo
   era plano (defeito 2), o problema ficou mascarado; com terreno real ele
   apareceu em todo lugar.
2. `highways.rs` calculava a cota de pintura como `ground.max(current_y)`,
   com `current_y` RELATIVO (0 no nível da rua, >0 em rampa): com terreno
   positivo a via caía exatamente na cota do chão já preenchido (e era
   descartada pelo item 1); com terreno negativo a via ficava na cota 0,
   flutuando.

**Correção.**
- `WorldEditor` ganha um registro de **superfície de terreno intocada**
  (`terrain_surface_y`, 512×512 por região): o passe de chão escreve por
  `set_terrain_surface_absolute`, e para qualquer escrita de ELEMENTO
  (if-absent, whitelist, blacklist, com propriedades, replay do Halo) essa
  coluna conta como VAZIA até a primeira escrita consumi-la. Blocos postos por
  elementos continuam protegidos exatamente como antes. Na 2ª passada do Halo
  (região relida do disco) a superfície é reconhecida pela heurística "bloco
  de chão do passe de terreno na cota exata do `Ground`".
- `highways.rs`: `paint_y_at(x, z) = solo local + current_y` (ou a cota
  absoluta do tabuleiro em ponte de vale) é a única fonte de verdade para
  asfalto, bordas, faixas, divisórias e pilares; o aterro de rampas é
  relativo ao solo. `SAFE_FOR_SIDEWALK` inclui o andesito do chão urbano para
  meio-fio e calçada existirem também dentro da mancha urbana.
- **Precedência de piso entre regiões.** Medido de novo depois da correção
  acima: a rua da QE 17 virou GRAMA. O polígono `landuse=residential` "QE 17"
  está ancorado na região (−34, 9) e a rua na (−33, 9); o gramado do landuse
  chega à região da rua pelo Halo, que é replayado ANTES dos elementos da
  própria região — então a primeira escrita era a do landuse, e a via (que
  respeita o que já existe) perdia. A ordenação por prioridade dentro da
  região nunca valeu ENTRE regiões. Agora cada coluna guarda também a
  precedência (`osm_parser::get_priority`) de quem pintou a superfície
  (`surface_writer_priority`), a precedência viaja com cada `HaloOp`, e um
  elemento mais específico repinta o piso de um mais genérico qualquer que
  seja a ordem de chegada; empate = o primeiro fica (Arnis). A escada de
  prioridade foi refinada para isso: prédio > via > trilho > rio > lago >
  cerca > piso esportivo/estacionamento > equipamento (`amenity`,
  `man_made`, ...) > lazer e natural > `landuse` > `place` (antes toda ÁREA
  empatava em 6).
- **Dois artefatos do pincel de via que o chão pré-preenchido escondia.**
  (a) A Zona 1 ("canteiro central") era pintada com `dist <= raio` mesmo com
  raio 0 — uma linha de grama no eixo de TODA via; agora só com raio > 0.
  (b) Em trechos diagonais, pincéis perpendiculares saindo de pontos de
  Bresenham 8-conexos deixam buracos em xadrez (a célula entre duas linhas
  paralelas vizinhas nunca é amostrada), que apareciam como grama no meio do
  asfalto; o pincel agora sub-amostra a meio bloco na largura e ao longo da
  via apenas nas diagonais (raio de cobertura do reticulado 0,5 ≈ 0,35 <
  0,5, logo toda célula do retângulo varrido é atingida). (c) A Zona 2
  (asfalto) começava em `dist > raio do canteiro`, que com raio 0 exclui a
  coluna do eixo — a linha central ficava sem pavimento (grama, depois
  andesito do chão urbano); sem canteiro o eixo agora é asfalto. (d) As
  linhas de vaga junto à guia eram pintadas a cada 4 blocos (zebra contínua);
  agora a cada 7 (≈5,5 m, o comprimento de uma vaga paralela).
- Testes: `untouched_terrain_surface_is_replaceable_exactly_once` (quatro
  modos, fast-path, replay pelo Halo e o caso QE 17 landuse-via) e
  `floor_precedence_puts_specific_before_generic`.

## 14. Árvores "comendo" prédios e ruas (LiDAR, `landuse`, `natural`)

**Sintoma.** Copas atravessando telhados, troncos no meio do asfalto, paredes
com buracos onde uma árvore chegou antes — típico onde a vegetação vem da
nuvem LiDAR, que não distingue copa de telhado, mas também de `landuse` e
`natural=wood` do OSM.

**Causa.** Três mecanismos somados: (1) a árvore chega antes da estrutura
(pelo Halo de uma região vizinha, ou porque a feature de provedor é
despachada depois e o prédio do OSM "só se vazio" já não consegue ocupar o
espaço da copa); (2) a semente da árvore usava uma lista NEGATIVA de blocos
proibidos, e brotava em qualquer superfície fora dela (andesito de calçada,
laje, cascalho); (3) nada removia a copa que ficava sobre a pista.

**Correção.**
- `Block::is_vegetation` (copa, tronco, arbusto, capim) e a regra em
  `WorldEditor::existing_for_write`: para um escritor ESTRUTURAL
  (`get_priority` ≤ 8: prédio, via, trilho, água, cerca, piso esportivo,
  equipamento) a vegetação existente conta como vazia — a parede atravessa a
  copa em vez de nascer com buraco. Vegetação sobre vegetação segue "o
  primeiro fica". Vale in-core e no replay do Halo.
- `Tree::create_*`: a semente só pega em solo natural (lista positiva
  `NATURAL_GROUND`: grama, terra, podzol, areia, musgo); coluna ainda sem
  bloco (região vizinha) continua permitida para não perder copas de borda.
- `highways.rs` chama `clear_vegetation_above` sobre pista e passeio (até 14
  blocos, tolerando vãos de copa).
- Teste em `halo_tests` (copa × parede por prioridade, limpeza sobre a via).

## 15. Merge de provedores: o vencedor herda a semântica que não tem

**Sintoma (mesma família do §14).** Um prédio da nuvem LiDAR (classe 6 →
`building=yes` + `source`, prioridade 1) cobre o prédio do OSM e o substitui
no merge — levando junto nome, pavimentos, `amenity`/`shop`. A mata LiDAR
(`natural=wood`) apaga `leaf_type`/`name` da mata do OSM; a água LiDAR, o
nome do lago. A fonte que **não sabe o que vê** comia a que sabe.

**Correção.** `resolve_collisions` passa a chamar `inherit_missing_semantics`
no momento em que descarta a candidata: o vencedor mantém geometria,
prioridade e tudo que já declarava, e recebe as chaves que não tinha (nunca
`source`/`density`/ids); `merged:source` registra de onde veio a herança
para a auditoria de proveniência. Assim `BuildingCategory`, o enriquecimento
por POIs e as espécies do Cerrado continuam funcionando sobre o contorno
exato do LiDAR/CityGML. Teste
`superseded_feature_lends_its_missing_semantics_to_the_winner`.

## Validação da Parte II

Mesmos comandos da seção "Como reproduzir a validação". Testes novos:
`providers::tests` (`uso_to_tag`, `semantic_group_from_tags`),
`poi_enrichment::tests` (ponto-em-polígono com coordenadas negativas,
dominância institucional > loja, unidade indoor enriquecendo o shopping),
`sports::tests` (classificação), `oriented_frame::tests`,
`railways::tests` (offset por tag), `stations::tests` (índice de trilhos).
