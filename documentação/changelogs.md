ATENÇÃO: aqui serão dispostos todos os changelogs, tudo o que foi mudado, por que, razão etc de forma extremamente documentada

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
