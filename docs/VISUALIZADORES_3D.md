# Visualizadores 3D do mundo gerado

Este documento registra, de forma reprodutível, os dois visualizadores 3D
embutidos no motor — quando usar cada um, como funcionam por baixo dos panos,
e os achados reais desta sessão ao integrar o segundo deles (BlueMap).

## 1. `--view-world` — relevo caseiro (rápido, sem dependências externas)

```bash
pincelism --view-world "<pasta do mundo>" [--port <porta>]
```

Sobe um servidor HTTP local (`src/world_viewer.rs`) que serve uma página
Three.js mostrando um **relevo amostrado** do mundo inteiro: para cada coluna
`(x, z)` do mundo, computa a altura e a cor do bloco do **topo** (reaproveitando
o mesmo scanner de NBT/paleta que já gera o minimapa 2D do GUI —
`map_renderer::compute_heightfield`), amostra numa grade limitada a
~350.000 células (orçamento fixo em `world_viewer::TARGET_CELLS`) e monta
**uma única malha de relevo** (não *boxes* individuais) com altura real por
vértice.

**Por que uma amostra, não um voxel completo:** o Guará I+II tem ~50 milhões
de colunas reais. Renderizar cada uma como um voxel individual no navegador
estouraria a memória de qualquer WebGL. O trade-off é documentado no próprio
código (`world_viewer.rs`, comentário de módulo).

**Quando usar:** visão geral instantânea (segundos, não minutos) de um mundo
recém-gerado, sem precisar de nada além do próprio binário — bom para
iteração rápida durante o desenvolvimento.

## 2. `--view-world-bluemap` — render real com [BlueMap](https://bluemap.bluecolored.de/)

```bash
pincelism --view-world-bluemap "<pasta do mundo>" \
  [--bluemap-jar <caminho/para/bluemap-cli.jar>] \
  [--java <caminho/para/java>] \
  [--bluemap-config <pasta de config>]
```

Delega a renderização de verdade para o **BlueMap**, um renderizador de
mundos Minecraft real, maduro e de código aberto (não uma invenção deste
projeto) — o mesmo tipo de ferramenta que administradores de servidor
Minecraft usam para inspecionar seus próprios mundos. Ao contrário do relevo
caseiro, o BlueMap desenha os **blocos reais texturizados**, com os modelos
3D verdadeiros do Minecraft (paredes, portas, telhados, tudo que este motor
gera), em múltiplas vistas (topo / isométrica / perspectiva / livre) e
níveis de zoom.

Este projeto **não reimplementa nada do BlueMap** — `src/bluemap_viewer.rs`
só orquestra o processo externo `java -jar bluemap-cli.jar`, cuidando de:

1. **Encontrar um Java compatível** (`find_compatible_java`).
2. **Gerar/atualizar a configuração mínima do mapa** (`ensure_config`).
3. **Renderizar + subir o servidor web numa só chamada** (`run_render_and_serve`).

Os três passos abaixo documentam exatamente o que foi descoberto **testando
de verdade** contra o Guará I+II nesta sessão — não suposições.

### Achado #1: BlueMap 5.25 exige Java 25, não a versão "estável" mais comum

A primeira tentativa de rodar `bluemap-5.25-cli.jar` com o `java` padrão do
PATH desta máquina (OpenJDK 21.0.12) falhou imediatamente:

```
Error: LinkageError occurred while loading main class de.bluecolored.bluemap.cli.BlueMapCLI
	java.lang.UnsupportedClassVersionError: de/bluecolored/bluemap/cli/BlueMapCLI
	has been compiled by a more recent version of the Java Runtime
	(class file version 69.0), this version of the Java Runtime only
	recognizes class file versions up to 65.0
```

Classe versão 69 = Java 25 (65=21, 66=22, 67=23, 68=24, 69=25). Esta máquina
já tinha um JDK 25 instalado lado a lado em `/usr/lib/jvm/jdk-25/` (não era o
`java` padrão do `update-alternatives`) — usá-lo resolveu na hora.
`find_compatible_java` automatiza essa descoberta: tenta o override explícito
(`--java`), depois `java` do PATH, depois uma pequena lista de caminhos
conhecidos de instalações lado a lado (`FALLBACK_JAVA_CANDIDATES`), sondando
a versão real de cada candidato via `java -version` (cuja saída, por
convenção do próprio `java`, vai para STDERR, não STDOUT — um detalhe fácil
de errar ao escrever o parser).

**Implicação prática:** se você rodar isto numa máquina só com Java 21 (ou
mais antigo) e sem um JDK 25+ instalado em algum lugar, vai precisar instalar
um e/ou apontar `--java <caminho>` explicitamente. O erro impresso já diz
isso.

### Achado #2: a configuração mínima de um mapa BlueMap é genuinamente mínima

Testado isolando um mapa de config só com 3 linhas:

```hocon
world: "/caminho/para/o/mundo"
dimension: "minecraft:overworld"
name: "algum-nome"
```

e confirmando que renderiza igual a um `maps/overworld.conf` gerado
(que tem ~150 linhas de opções documentadas, todas com defaults sensatos).
Isso permite que `ensure_config` escreva o arquivo de config do mapa direto,
programaticamente, sem precisar reimplementar/copiar as ~150 linhas de
comentários do BlueMap.

### Achado #3: a EULA da Mojang precisa ser aceita para baixar as texturas

`core.conf` tem uma flag `accept-download` (default `false`) que representa
aceitar a [EULA da Mojang](https://account.mojang.com/documents/minecraft_eula)
— necessária porque o BlueMap baixa o `client.jar` oficial do Minecraft (via
`piston-data.mojang.com`) para extrair texturas/modelos de bloco reais.
`ensure_config` liga essa flag automaticamente **só na primeira geração da
configuração**, e imprime um aviso explícito no console dizendo que fez isso
em nome do usuário — nunca em silêncio.

### Achado #4: tempo de render é proporcional ao tamanho real do mundo, não um detalhe

| Mundo | Tamanho em disco | Regiões `.mca` | Tempo de render BlueMap |
|---|---|---|---|
| Guará I (recorte de teste) | 21 MB | 4 | ~28 segundos |
| Guará I+II completo | ~908 MB | 217 | ~24 minutos (medido) |

Ambos renderizaram **sem nenhum erro de compatibilidade** — confirmação
independente (de uma ferramenta madura e completamente alheia a este
projeto) de que o formato NBT que este motor escreve é um mundo Java Edition
genuinamente válido.

### Por que o `.jar` do BlueMap não é baixado automaticamente

`bluemap_viewer.rs` exige que o usuário baixe o `.jar` manualmente (a
[release oficial no GitHub](https://github.com/BlueMap-Minecraft/BlueMap/releases/latest),
arquivo `bluemap-<versão>-cli.jar`) e o coloque em `./bluemap/bluemap-cli.jar`
(ou aponte outro caminho com `--bluemap-jar`). Isso é deliberado: buscar E
EXECUTAR automaticamente um executável externo baixado da rede, sem o
usuário ver esse passo explicitamente, é uma categoria de risco (cadeia de
suprimentos) que este projeto prefere não automatizar em silêncio — mesmo
sendo o BlueMap um projeto de código aberto confiável e amplamente usado.

## Comparação rápida

| | `--view-world` | `--view-world-bluemap` |
|---|---|---|
| Dependências externas | nenhuma | JVM (Java 25+) + `bluemap-cli.jar` (baixado manualmente) |
| Tempo até visualizar | segundos | minutos (proporcional ao tamanho do mundo) |
| Fidelidade visual | relevo amostrado (altura+cor do topo, downsample) | blocos reais texturizados, múltiplas vistas |
| Bom para | iteração rápida, visão geral da skyline/traçado | inspeção de perto, apresentação/demonstração |
