<p align="center">
  <img src="assets/marca/logo-kalango-audio.png" alt="Logo do Kalango Audio" width="200">
</p>

# Kalango Audio

Ferramenta nativa do [KalangoOS](https://github.com/kalangoos) para capturar, tratar, rotear e gravar som.
Você monta uma **sessão** com blocos — **fontes** (um aplicativo, o microfone, o som do sistema), **efeitos**
(volume, equalizador, compressor, limpeza de voz, redução de ruído…) e **saídas** (alto-falantes, gravador,
rádio online, transmissão RTMP) —, liga a sessão e ouve ou grava o resultado. Também transcreve a fala com um
modelo local, roda em horários marcados e dispara automações.

Escrito em Rust, com interface em GTK 4 no visual do KalangoOS (tokens de cor, molas de animação e ícones do
sistema; segue o tema claro/escuro e o idioma do sistema). Licença GPL-3.0-or-later.

## Rodar

Precisa do GTK 4.14 ou mais novo (no Ubuntu: `libgtk-4-dev` para compilar).

```sh
cargo run --release        # abre a janela
./install.sh               # instala para o seu usuário (binário, ícones e atalho "Kalango Audio")
cargo test --release       # testes automáticos
```

## O que precisa no computador

- **PipeWire** (ou outro servidor de som acessível pelo ALSA). Sem ele o app mostra o estado "sistema de som não respondeu".
- `pw-record` e `pw-dump` (ferramentas do PipeWire): gravar só o som de um aplicativo.
- `ffmpeg` (opcional, recomendado): MP3, AAC, ALAC e FLAC, etiquetas, transmissões (Icecast, RTMP) e o player das gravações.
- O modelo de voz do KalangoOS (LFM3-Audio-2B, pela Central de Apps) e o `kalango-agente`: o bloco Transcrever
  e a transcrição das gravações. Sem ele, o bloco mostra que o modelo não está instalado.
- `gio` e `xdg-open` (opcional): Lixeira, "Mostrar em Arquivos" e abrir no player.
- Plug-ins LADSPA (opcional): aparecem no bloco "Plug-in LADSPA".

Em **Preferências** dá para ver o que foi encontrado; uma faixa no topo avisa quando falta algo importante.

## Primeiros passos

1. **Nova sessão** e escolha um modelo (por exemplo "Reunião online"), ou peça na busca (Ctrl K):
   "grave a reunião do navegador e transcreva" — o painel do agente mostra o plano e só age com a sua aprovação.
2. No editor, arraste blocos da biblioteca para o canvas (ou para cima de um fio, para inserir no meio) e clique
   num bloco para ver os ajustes dele no inspetor.
3. **Iniciar** (Ctrl+R ou Espaço). Para parar, **Parar**. As gravações, com forma de onda, etiquetas e transcrição,
   ficam em **Gravações**.

## Onde ficam as coisas

- Sessões e preferências: `~/.config/kalango/audio/`
- Gravações: a pasta de música do usuário (XDG) + `Kalango Audio`
- Transcrições: a pasta de documentos (XDG) + `Kalango Audio`

## Idioma e tema

Os textos da interface são escritos em inglês no código (`tr("…")`) e traduzidos pelos catálogos em
`locales/pt_BR/*.json`; o idioma é o do sistema (`LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`). O tema segue a
escolha do KalangoOS (`~/.config/kalango/tema.json`: escuro, claro ou automático, e a cor de destaque) e, fora dele,
a preferência de tema escuro do GTK.

## Para quem desenvolve

- `cargo test` roda os testes (o motor de áudio, o núcleo do app, as traduções e os estilos). Os testes nunca
  escrevem na pasta pessoal: usam pastas temporárias.
- Variáveis para conferir telas sem mexer nos seus dados: `KALANGO_AUDIO_DEMO=1` (ou `rodando`) abre com sessões
  e gravações de exemplo numa pasta temporária (`KALANGO_AUDIO_DEMO_DIR`); `KALANGO_AUDIO_PAGINA=<página>`
  (sessoes, modelos, editor, gravacoes, agenda, ajustes, boas-vindas, sem-som); `KALANGO_AUDIO_TEMA=claro|escuro`;
  `KALANGO_AUDIO_TAMANHO=760x900`; `KALANGO_AUDIO_SESSAO=<nome>` (+ `KALANGO_AUDIO_SESSAO_RODAR=1`,
  `KALANGO_AUDIO_BLOCO=<tipo>`); `KALANGO_AUDIO_PEDIDO="<pedido>"` abre o painel do agente;
  `KALANGO_AUDIO_SEM_SOM=1` mostra o estado sem sistema de som; `KALANGO_AUDIO_CAPTURA=<arquivo.png>` (+
  `KALANGO_AUDIO_CAPTURA_SAIR=1`) salva uma imagem da janela.

## Scripts

Automações podem rodar scripts em JavaScript com a nossa API (`kalango.*` e `ctx.*`). A ajuda completa está
em **Ajuda › Ajuda de scripts** dentro do app.

## Documentos

- [`docs/COMPORTAMENTO.md`](docs/COMPORTAMENTO.md) — como o Kalango Audio se comporta (blocos, medidores, gravação).
- [`docs/PROVENIENCIA.md`](docs/PROVENIENCIA.md) — auditoria de origem: o que foi verificado e trocado.
- [`docs/ISOLAR-VOZ.md`](docs/ISOLAR-VOZ.md) — porte em Rust do isolador de voz: paridade e velocidade medidas.
- Design: página "26 · Kalango Audio" do Design System do KalangoOS (FigCraft).
