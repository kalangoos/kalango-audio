# Kalango Audio

Ferramenta nativa do [KalangoOS](https://github.com/kalangoos) para capturar, tratar, rotear e gravar som.
Você monta uma **sessão** com blocos — **fontes** (um aplicativo, o microfone, o som do sistema), **efeitos**
(volume, equalizador, compressor, limpeza de voz, redução de ruído…) e **saídas** (alto-falantes, gravador,
rádio online, transmissão RTMP) —, liga a sessão e ouve ou grava o resultado. Também transcreve a fala com um
modelo local, roda em horários marcados e dispara automações.

Escrito em Rust (eframe/egui). Licença GPL-3.0-or-later.

## Rodar

```sh
cargo run --release        # abre a janela
./install.sh               # instala para o seu usuário (binário, ícones e atalho "Kalango Audio")
cargo test --release       # testes automáticos
```

## O que precisa no computador

- **PipeWire** (ou outro servidor de som acessível pelo ALSA). Sem ele o app mostra o estado "sistema de som não respondeu".
- `pw-record` e `pw-dump` (ferramentas do PipeWire): gravar só o som de um aplicativo.
- `ffmpeg` (opcional, recomendado): MP3, AAC, ALAC e FLAC, etiquetas, transmissões (Icecast, RTMP) e o player das gravações.
- `whisper-cli` e um modelo (opcional): o bloco Transcrever.
- `gio` e `xdg-open` (opcional): Lixeira, "Mostrar em Arquivos" e abrir no player.
- Plug-ins LADSPA (opcional): aparecem no bloco "Plug-in LADSPA".

Em **Preferências** dá para ver o que foi encontrado; uma faixa no topo avisa quando falta algo importante.

## Primeiros passos

1. **Nova sessão…** e escolha um modelo (por exemplo "Reunião online").
2. Clique num bloco para ver os ajustes dele. No Gravador, escolha pasta e formato.
3. **Iniciar** (Ctrl+R). Para parar, **Parar**. As gravações ficam em **Gravações**.

## Onde ficam as coisas

- Sessões e preferências: `~/.config/kalango/audio/`
- Gravações: a pasta de música do usuário (XDG) + `Kalango Audio`
- Transcrições: a pasta de documentos (XDG) + `Kalango Audio`

## Scripts

Automações podem rodar scripts em JavaScript com a nossa API (`kalango.*` e `ctx.*`). A ajuda completa está
em **Ajuda › Ajuda de scripts** dentro do app.

## Documentos

- [`docs/COMPORTAMENTO.md`](docs/COMPORTAMENTO.md) — como o Kalango Audio se comporta (blocos, medidores, gravação).
- [`docs/PROVENIENCIA.md`](docs/PROVENIENCIA.md) — auditoria de origem: o que foi verificado e trocado.
- Design: página "26 · Kalango Audio" do Design System do KalangoOS (FigCraft).
