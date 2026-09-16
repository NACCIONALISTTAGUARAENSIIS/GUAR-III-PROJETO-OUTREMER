# Visualizadores 3D do mundo gerado

Este documento registra, de forma reprodutível, os três visualizadores 3D
embutidos no motor — quando usar cada um, como funcionam por baixo dos panos,
e os achados reais desta sessão ao integrar o segundo deles (BlueMap) e ao
construir o terceiro (`--view-world-voxels`) depois que o BlueMap se mostrou
frágil demais pra este mundo.

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

## 2. `--view-world-voxels` — voxels reais, mundo inteiro, sob demanda

```bash
pincelism --view-world-voxels "<pasta do mundo>" \
  [--crop <minX> <minZ> <maxX> <maxZ>] \
  [--min-y <N>] [--max-y <N>] \
  [--port <porta>]
```

Sobe um servidor HTTP (`src/voxel_viewer.rs`) que serve uma página Three.js
com navegação em **primeira pessoa** (voo livre, `PointerLockControls`) por
cima de **voxels reais** — todo bloco não-transparente com pelo menos uma
face exposta, não uma amostra do topo de cada coluna. Ao contrário de
`--view-world`, dá pra ver paredes, telhados e entrar em interiores.

### Por que isto existe apesar de já termos dois visualizadores

Nesta sessão o usuário pediu explicitamente pra inspecionar o mundo gerado
"em detalhe, cada bloco", pra poder iterar no código de geração do
Pincelism depois — e pediu navegação livre tipo personagem, como o
BlueMap oferece. `--view-world-bluemap` foi a primeira tentativa de
resolver isso, mas uma investigação extensa (documentada nos Achados #5–#7
acima) não encontrou a causa raiz de por que o **cliente** do BlueMap não
conseguia renderizar/navegar até a área correta — mesmo depois de
confirmar, por fora do BlueMap (gerando uma imagem direto do heightfield
real do mundo), que os dados e a posição estavam certos. Em vez de continuar
depurando uma ferramenta externa cujo código não escrevemos, construímos
este visualizador — sem nenhuma dependência externa, sem nenhuma lógica de
cliente que não é nossa.

### Por que "mundo inteiro" virou um servidor de streaming de chunks

A primeira versão deste módulo exigia um `--crop` obrigatório e pré-computava
tudo de uma vez (funcionou, mas é um recorte, não o mundo todo). Pré-computar
o mundo inteiro do Guará I+II (~5700×9200 blocos) em voxels de uma vez geraria
dezenas de milhões de blocos expostos — gigabytes de JSON, inviável pro
navegador montar de uma vez. A solução, do mesmo jeito que o próprio
Minecraft e o BlueMap fazem: carregar só os **chunks** (colunas de 16×16
blocos, formato nativo do Anvil) perto de onde o "jogador" está, sob
demanda, via `GET /chunk/<cx>/<cz>`, e descartar (com uma margem de
histerese, `RENDER_DIST_CHUNKS` vs `UNLOAD_DIST_CHUNKS` no HTML) os que
ficam longe conforme você anda. Isso resolve "cidades futuras" de graça — o
servidor não sabe nada sobre o Guará especificamente, só lê o `world_dir`
passado região por região; qualquer mundo gerado por este motor funciona
sem nenhuma mudança de código.

`--crop` continua disponível, agora **opcional**: sem ele, todas as regiões
presentes em `region/` são navegáveis; com ele, restringe a área streamável
(útil pra focar numa cidade específica quando várias existem na mesma pasta).

### Por que "sem nenhum bloco omitido" ainda descarta alguma coisa

A única geometria descartada (`map_renderer::extract_exposed_voxels`) é a
de blocos **totalmente cercados** por 6 outros blocos opacos — esses são,
por definição, invisíveis de qualquer ângulo possível, inclusive no
Minecraft real (o próprio jogo nunca desenha essas faces). Omitir isso não
tira nenhum detalhe visível, só evita mandar geometria que nunca apareceria
na tela — o mesmo tipo de *face culling* que qualquer motor de voxels faz.
A faixa de altura padrão é a completa do Minecraft moderno (`-64` a `320`),
então nada é cortado verticalmente por padrão.

**Trade-off assumido conscientemente:** a exposição de um bloco é calculada
só dentro do chunk que o contém — um bloco bem na borda de um chunk cujo
vizinho fica no chunk adjacente (ainda não carregado nesse momento) é
tratado como exposto mesmo que esse vizinho o cubra. Na pior hipótese isso
mantém uns poucos triângulos extras nas costuras entre chunks — nunca
omite um bloco que devia aparecer.

### Navegação: dois modos de câmera alternáveis

Pedido explícito do usuário: poder separar uma "visão normal" (câmera solta,
pra ver o conjunto de fora) de uma "visão corporificada" (andar por dentro,
como um personagem). Um botão no painel de opções alterna entre os dois, a
qualquer momento, mantendo a orientação da câmera ao trocar:

- **🛰 Visão livre** — `OrbitControls` (a mesma usada em `--view-world`):
  arraste pra orbitar, scroll pra zoom, botão direito desloca o alvo. Boa
  pra ver o traçado urbano inteiro de fora, sem se preocupar em "cair" em
  algum lugar.
- **🚶 Corporificada** — voo livre em primeira pessoa: `WASD` anda na
  direção que a câmera olha (plano horizontal), **arrastar o mouse** olha
  em qualquer direção, `Espaço`/`Shift` sobem/descem. **Não há colisão nem
  gravidade nesta versão** — é voo livre, não andar-sobre-o-chão;
  suficiente pra inspecionar cada bloco/parede/interior de perto, mas
  documentado aqui como uma limitação real, não escondida.

### Achado real: por que a primeira versão usava Pointer Lock e não funcionava

A primeira versão deste visualizador usava a
[Pointer Lock API](https://developer.mozilla.org/en-US/docs/Web/API/Pointer_Lock_API)
do navegador (clique trava o cursor) pra olhar em primeira pessoa — o
padrão usado por jogos em WebGL. Testado ao vivo pelo usuário: clicar não
fazia nada, sem nenhum erro visível. Causa raiz: a Pointer Lock API exige
um **contexto seguro** (HTTPS, ou `localhost`) nos navegadores modernos —
servido por HTTP simples num IP público (o mesmo padrão de exposição já
usado pelo BlueMap e pelo relevo caseiro nesta sessão), o navegador recusa
o pedido de trava **silenciosamente**, sem disparar nenhum evento de erro
que o código pudesse capturar. Corrigido substituindo por
"arrastar-o-mouse-pra-olhar" (`mousedown`+`mousemove`+`mouseup` normais,
usando `event.movementX/Y`) — funciona em qualquer contexto, HTTP incluído,
sem exigir nenhuma permissão especial do navegador.

### Carregamento: pré-carga em massa, não só reativo ao andar

O usuário pediu explicitamente pra não depender de streaming reativo —
"melhor renderizar tudo de uma vez". Isso é fisicamente inviável de forma
literal para o mundo INTEIRO (dezenas de milhões de blocos expostos,
gigabytes de dados, travaria qualquer navegador) — mas o visualizador agora
**pré-carrega em massa** um raio generoso de chunks já na inicialização
(ajustável no painel, padrão 15 chunks = 240 blocos de raio, com uma barra
de progresso real enquanto carrega), em vez de só reagir a movimento. Isso
cobre uma área grande o suficiente pra sentir como "está tudo ali" pra
inspeção normal. O painel também tem um interruptor **"carregamento
dinâmico"**: desligado, nenhum chunk novo carrega além do que já foi
pré-carregado, mesmo andando — uma aproximação honesta de "tudo de uma vez,
sem surpresas ao se mover", dentro do que o navegador aguenta.

### Teto de segurança por chunk

`map_renderer::MAX_VOXEL_CELLS` (6 milhões de células) protege contra um
`--crop` absurdamente grande sendo pedido de uma vez — mas por chunk
(16×16×385 = 98.560 células na faixa de altura completa) isso nunca chega
nem perto do limite, então o streaming normal nunca esbarra nele.

## 3. `--view-world-bluemap` — render real com [BlueMap](https://bluemap.bluecolored.de/)

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

### Achado #5: o `start-pos` default do BlueMap abre olhando pro Marco Zero de Brasília, não pro mundo gerado

Descoberto ao mostrar o Guará I+II renderizado pela primeira vez pro usuário:
a câmera abria numa superfície plana, cinza, sem nenhum prédio/via visível —
parecia um bug de renderização (textura quebrada), mas era outra coisa. O
`start-pos` default do BlueMap é `{x: 0, z: 0}` — o **zero absoluto da malha
Minecraft deste motor**, que é o Marco Zero fixo de Brasília
(`DF_ORIGIN_LAT`/`DF_ORIGIN_LON` em `transformation.rs`), não necessariamente
um ponto dentro do bbox pedido. O Guará I+II vai de X=-18525 a X=-12821 — o
zero fica a mais de 12.800 blocos de distância do mundo real, numa área nunca
gerada (vazio/void). `bluemap_viewer::compute_start_pos` corrige isso
computando o meio do retângulo delimitador real do mundo (reaproveitando
`world_viewer::discover_bounds`, a mesma descoberta de limites já usada pelo
relevo caseiro) e escrevendo esse valor no `maps/<id>.conf` gerado — testado
com uma regressão automatizada (`compute_start_pos_centers_on_real_world_bounds`)
que fixa exatamente esse caso real (Guará I+II: meio em `(-15673, 4419)`,
bem longe de `(0, 0)`).

**Detalhe operacional encontrado ao corrigir isso manualmente pela primeira
vez:** mudar só o `start-pos` de um `maps/<id>.conf` já renderizado e rodar
`bluemap-cli.jar -s` (`--generate-websettings`, documentado como a forma de
atualizar isso) **não** atualizou o `startPos` no `settings.json` do mapa
nos testes desta sessão — só uma re-renderização completa (`-r`) fez
efeito. Como `bluemap_viewer::run_render_and_serve` sempre roda com `-r`
antes de `-w`, isto nunca é um problema pelo caminho normal (Rust) — só
importa se alguém for editar a config manualmente depois de um render já
feito, como aconteceu aqui.

### Achado #6: a correção do Achado #5 só vale para uma URL sem `#hash` — um link com posição salva sempre tem prioridade absoluta

Depois de corrigir e confirmar (via `curl` no `settings.json` servido) que o
`start-pos` do mapa `guara_full` estava correto (`[-15673, 4419]`), o usuário
ainda reportou ver só vazio/preto — inclusive numa aba anônima do Tor, o que
descartava cache/cookies como explicação. A causa raiz real (achada lendo o
JS da própria interface do BlueMap, função `loadPageAddress`, já que o
navegador MCP estava indisponível nesta sessão) é esta lógica, na ordem
exata em que ela roda a cada carregamento de página:

```js
let t = (location.hash.substring(1) || this.settings.startLocation || "").split(":");
if (t.length === 1 && /* mapa ainda não é o pedido */) { try { switchMap(t[0]) } catch { return false } }
if (t.length !== 10) return false;
// ... usa t[1..9] (x, y, z, distance, rotation, angle, tilt, ortho, viewMode)
// LITERALMENTE, sem nunca consultar o `start-pos` do mapa.
```

e, no ponto de entrada da aplicação:

```js
await this.loadPageAddress() ||
  (this.maps.length > 0 && await this.switchMap(this.maps[0].data.id), this.resetCamera());
```

Ou seja: **o `start-pos` por-mapa (nosso `compute_start_pos`) só é consultado
dentro de `resetCamera()`, que só roda quando `loadPageAddress()` retorna
`false`** — e isso só acontece quando a URL não tem `#` nenhum (hash vazio) E
`webapp.conf`'s `start-location` também está vazio (nosso caso, nunca
setado). **Qualquer URL com um hash de exatamente 10 campos
(`mapa:x:y:z:distância:rotação:ângulo:tilt:ortho:modo`) é usada
literalmente, ignorando `start-pos` por completo** — mesmo que aponte para
fora do mundo gerado.

Foi exatamente isso que aconteceu: a segunda captura de tela do usuário
mostrava a URL
`#guara_full:-19764:0:10239:12974:0.04:0:0:0:perspective` — um hash
plenamente válido (10 campos), mas com `x=-19764, z=10239`, **fora do bbox
real do Guará I+II** (`x: -18525..-12821`, `z: 3..8836`) — por isso vazio/
preto, sem nenhum bloco por perto. A `distância=12974` (bem maior que o
`1500` que `resetCamera()` usaria) sugere que a câmera tinha sido afastada
manualmente (zoom out) tentando "achar" o mundo, e o BlueMap grava essa
posição de volta na URL automaticamente a cada movimento de câmera
(`updatePageAddress`) — por isso o hash "gruda" e sobrevive a uma nova aba/
navegador anônimo (a posição está na própria URL digitada/copiada, não em
cookie ou cache).

**Confirmado nesta sessão, via `settings.json` servido ao vivo:**
`curl http://localhost:8100/settings.json` mostra
`"maps": ["guara_full", "overworld", "nether", "end"]` (`guara_full` é
`this.maps[0]`) e
`curl http://localhost:8100/maps/guara_full/settings.json` mostra
`"startPos": [-15673, 4419]` — exatamente o meio do mundo, como esperado.
Isso prova que a correção do Achado #5 está de fato ativa e correta; o
sintoma que persistia era inteiramente devido ao hash da URL, não a uma
falha na correção.

**Correção prática (não é uma mudança de código — é operacional):** para ver
o mundo do jeito certo, a URL usada para abrir o BlueMap **não pode ter nada
depois da porta** — nem um `#` sozinho. Ex.: `http://<host>:8100/`, nunca um
link salvo/compartilhado que já tenha um `#mapa:x:y:z:...` gravado. Se a
câmera "se perder" (zoom/pan excessivo), a forma confiável de recomeçar é
apagar tudo após a porta na barra de endereço e recarregar — não usar o
botão "voltar", que reaproveita o hash salvo no histórico.

### Achado #7 (a causa raiz real): `remove-caves-below-y` apaga o mundo inteiro quando o relevo fica abaixo de Y=55

Depois dos Achados #5/#6 (posição da câmera e hash da URL) não resolverem o
sintoma reportado pelo usuário — mapa preto, só um "quadradinho" borrado
visível de longe que **desaparece** ao se aproximar — a hipótese de câmera
foi descartada com evidência concreta: a posição confirmada (`-15670:4419`)
está numa área onde nosso próprio heightfield (`map_renderer::compute_heightfield`)
mostra relevo real e variado (263 cores distintas, altura entre Y=-63 e
Y=30, nada plano). O problema não era posição — era o que o BlueMap
realmente desenha ali.

Decodificando um tile hi-res "vazio" (formato `.prbm.gz`, o formato binário
próprio do BlueMap para geometria de tile) dessa mesma área:

```
$ gunzip -c tiles/0/x-4/9/0/z1/3/1.prbm.gz | xxd
00000000: 0107 0000 0000 0000 706f 7369 7469 6f6e  ........position
00000010: 0021 0000 6e6f 726d 616c 0063 636f 6c6f  .!..normal.ccolo
...
```

— só os nomes dos atributos (`position`, `normal`, `color`, `uv`, `ao`,
`blocklight`, `sunlight`), **sem nenhum vértice real**: geometria
zerada. Medindo o tamanho de todos os tiles hi-res numa amostra de 2870
arquivos na área central do Guará I+II: **62% tinham exatamente esse
tamanho mínimo (~81 bytes)** — vazios — apesar do heightfield confirmar
relevo real ali.

A causa: `maps/<id>.conf` do BlueMap tem uma opção
[`remove-caves-below-y`](https://bluemap.bluecolored.de/wiki/customization/Map-Settings.html)
com **default 55** — calibrada para o nível do mar do Minecraft vanilla
(~63), pensada para esconder cavernas escondendo todo bloco abaixo desse Y
que não recebe luz do céu. O relevo que este motor gera usa um datum de
altura completamente diferente do vanilla: o Guará I+II inteiro fica entre
Y=-63 e Y=30 — **sempre abaixo de 55**. Com o default, o BlueMap classifica
a CIDADE INTEIRA como "caverna" (por estar abaixo do limiar, mesmo exposta
ao céu) e remove a geometria do render — não é falha de dado nem de posição
de câmera, é uma opção de renderização do BlueMap calibrada para um mundo
com outro referencial de altura.

**Correção:** `ensure_config` agora escreve `remove-caves-below-y: -10000`
(o valor que a própria documentação do BlueMap recomenda para desligar essa
remoção por completo) em todo `maps/<id>.conf` gerado — extraído para uma
função pura (`build_map_conf`), testada
(`build_map_conf_disables_cave_removal`). Como a própria doc do BlueMap
avisa ("Changing this value requires a re-render of the map"), **um mapa já
renderizado com o valor antigo precisa ser re-renderizado do zero** — só
reescrever a config não corrige tiles já gravados.

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

| | `--view-world` | `--view-world-bluemap` | `--view-world-voxels` |
|---|---|---|---|
| Dependências externas | nenhuma | JVM (Java 25+) + `bluemap-cli.jar` (baixado manualmente) | nenhuma |
| Tempo até visualizar | segundos | minutos (proporcional ao tamanho do mundo) | segundos (chunks sob demanda) |
| Fidelidade visual | relevo amostrado (altura+cor do topo, downsample) | blocos reais texturizados, múltiplas vistas | blocos reais (cor por bloco, sem textura), voo em 1ª pessoa |
| Cobertura | mundo inteiro, sempre | mundo inteiro (uma vez renderizado) | mundo inteiro, chunk a chunk sob demanda |
| Bom para | iteração rápida, visão geral da skyline/traçado | inspeção de perto, apresentação/demonstração | inspecionar cada bloco/parede/interior pra iterar no código de geração |
