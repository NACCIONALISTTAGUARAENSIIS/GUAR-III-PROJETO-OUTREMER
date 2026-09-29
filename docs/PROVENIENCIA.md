# Auditoria de proveniência

Todo mundo gerado por `pincelism --bbox ...` (formato Java Anvil) agora produz,
automaticamente, dois arquivos ao lado de `metadata.json`, na pasta do mundo:

- **`provenance.ndjson`** — um registro JSON por linha, um por feature
  efetivamente despachada para geração de blocos.
- **`provenance_summary.json`** — agregados: contagem por provider, por
  módulo, por par provider×módulo, por grupo semântico, e a lista de
  features processadas por **mais de um módulo** na mesma geração.

Não é uma flag opcional — roda sempre, sem custo perceptível (é só
contabilidade em memória sobre dados que já existem no pipeline).

## Por que isto existe

Pedido explícito do usuário: saber o que foi gerado por qual provider e por
qual módulo `.rs`, incluindo onde há mistura entre eles, sem precisar
consultar o código toda vez — para poder auditar e iterar na geração com
segurança mais adiante.

## Granularidade: por feature/estrutura, não por bloco

Decisão tomada em conjunto com o usuário. Rastrear por bloco individual
exigiria instrumentar os ~820 pontos de chamada de `WorldEditor::set_block*`
espalhados por 21 módulos de `element_processing` — uma mudança muito mais
invasiva e arriscada. Por feature aproveita os campos que já existem em
`providers::Feature` (`id`/`source`/`semantic_group`) e responde à pergunta
real ("o que gerou isto aqui") sem reescrever a geração inteira.

## O achado que motivou o desenho

`Feature::into_processed_element()` descarta `source`/`semantic_group`/
`priority` — só `id`/`geometry`/`attributes` sobrevivem na conversão pra
`ProcessedElement`. Sem instrumentação, o dispatcher genérico
(`data_processing::dispatch_element`, que roteia a maior parte da geração
conforme as tags OSM) não tinha **nenhuma** forma de saber de onde um
elemento veio.

`src/provenance.rs` resolve isso com um `ProvenanceLedger` que:

1. **`register_origin(feature)`** — chamado em `main.rs` (e, por segurança,
   de novo em `data_processing.rs` para features de provedor que passam pelo
   dispatcher genérico) **antes** da conversão, guardando `source`/
   `semantic_group` por `feature.id`.
2. **`record_dispatch(feature_id, modules)`** — chamado ao fim de
   `dispatch_element`, depois que se sabe (pelas tags) qual(is) módulo(s)
   trataram o elemento. Um `Vec<String>`, não uma única string: o pipeline
   atual já tem casos reais de uma feature passar por mais de um módulo na
   mesma chamada (ver exemplo abaixo).
3. **`record_direct(feature, module)`** — para as features de provedor que
   nunca passam por `dispatch_element` (CAESB/fotogrametria/advertising,
   despachadas direto pro motor especializado).

## Exemplo real (capturado gerando um recorte pequeno do Guará)

```
{"featureId":478660365,"source":"osm","semanticGroup":"Infrastructure","modules":["power","underground_infrastructure"]}
```

Uma via de energia (`power=*`) real do OSM foi desenhada tanto pelo módulo
`power` (o poste/estrutura de superfície) quanto por
`generate_underground_infrastructure` (o cabo subterrâneo) — **na mesma
geração, para a mesma feature**. Sem a auditoria, essa mistura só seria
visível lendo `dispatch_element` linha por linha; com ela, aparece direto em
`provenance_summary.json`:

```json
"mixedModuleFeatures": [
  { "featureId": 478660365, "source": "osm", "semanticGroup": "Infrastructure",
    "modules": ["power", "underground_infrastructure"] }
]
```

## Cobertura

Só o pipeline principal (`data_processing::generate_world_with_options`,
chamado por `main::run_generation_pipeline` — o caminho de
`pincelism --bbox ...`). Dois casos ficam de fora, documentados como
limitação real:

- **Bedrock (`--bedrock`)**: `output_path` já é o `.mcworld` empacotado (zip)
  no momento em que o relatório seria escrito — não uma pasta onde dá pra
  soltar arquivos. Os relatórios só são gravados para Java Anvil.
- **Modo HUD interativo** (`generate_region_from_global`, usado só sem a
  feature `gui`, streaming região-por-região): recebe um `ProvenanceLedger`
  novo e descartado a cada chamada — não há um ponto final único onde
  acumular um relatório coerente para o mundo inteiro sem replanejar esse
  modo por completo.

## Próximo passo combinado com o usuário

Um modo visual nos visualizadores 3D (cor = provider/módulo em vez da cor
real do bloco), pra inspecionar a proveniência sem precisar ler
`provenance.ndjson` — ainda não implementado.

## Fontes exatas por provedor (`dataSources`)

Além de "qual provider", o relatório responde "**qual arquivo/endpoint**":
`DataProvider::describe_sources()` é implementado por todos os provedores de
arquivo local (caminho exato que abriram) e de rede (endpoint configurado; o
PostGIS omite credenciais e reporta só a tabela; o OSM reporta o `--file`
local ou "Overpass API" — vários servidores são tentados em sequência e qual
respondeu não é rastreado). `ProviderManager::fetch_all` loga cada fonte ao
iniciar o provedor (`↳ fonte: ...`), e `provenance_summary.json` ganhou a
seção:

```json
"dataSources": [
  { "provider": "OpenStreetMap (Overpass API)", "sources": ["./guara.json"] },
  { "provider": "GDF GeoPackage (SQLite Spatial DB)", "sources": ["./dados/lotes.gpkg"] }
]
```
