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
- No fim da geração, operações do Halo nunca aplicadas (que agora devem ser
  zero) são contadas e avisadas no log, em vez de sumir em silêncio.

**Verificação.** `data_processing::anchor_tests::*` (5 testes, incluindo a
propriedade "a âncora nunca vem depois de qualquer região tocada na ordem da
varredura") e `world_editor::halo_tests::*` (replay com cada modo contra um
chão pré-existente; propriedades preservadas; estado sombra em ordem).

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
