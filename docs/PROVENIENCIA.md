# Proveniência — auditoria de origem

O Kalango Audio nasceu de uma reconstrução em Rust, feita pelo próprio autor, de um programa comercial de
gravação e roteamento de som para macOS. Essa reconstrução usava valores, nomes, textos e tabelas tirados do
programa comercial. Este documento registra o que foi verificado antes do primeiro commit deste repositório e o
que foi removido ou trocado por equivalentes nossos. O histórico do git começa já no estado limpo: nada do que
está listado como removido entrou em nenhum commit.

## O que foi importado

- `src/` (Rust; a interface era em eframe/egui e foi trocada por GTK 4 depois, na interface nova), `Cargo.toml`, `Cargo.lock`, `install.sh` e `README.md` — todos reescritos ou
  revisados como descrito abaixo.
- **Não importado:** a pasta de build (`target/`), as capturas de tela da reconstrução (`shots/`: mostravam
  textos e nomes de modelos copiados), a ficha de mecânica (`MECHANICS.md` e `mechanics.json`: descreviam o
  funcionamento interno do programa comercial, com endereços e constantes lidas do binário) e todo o espaço de
  trabalho de engenharia reversa ao redor da reconstrução.
- No lugar da ficha de mecânica há o [`COMPORTAMENTO.md`](COMPORTAMENTO.md), escrito do zero a partir do nosso código.

## O que foi verificado

1. Todas as strings e constantes de `src/` (busca por nome do produto e do fabricante, "original", endereços
   `0x…`, nomes de classes do programa, arquivos `.js` da interface dele, chaves de plist, formato de sessão).
2. `src/eq_presets.json` e `src/templates.json`.
3. Os algoritmos de processamento de som (`src/dsp.rs`) e as regras dos medidores (`src/ui.rs`).
4. A API de scripts (`src/script.rs`) e os scripts que vinham embutidos.
5. Os textos da interface, diálogos e janelas.
6. Nomes do pacote, do binário, do app id, das pastas de configuração e das variáveis de ambiente.
7. Licenças das dependências (`cargo metadata`; `deny.toml` para a CI).

Busca final, sem nenhuma ocorrência que se refira ao programa comercial:

```sh
# PADRAO = nome do produto | nome do fabricante | original | 0x[0-9a-f]{4,} | extensão de sessão dele | Mac | Finder | Music.app
# (os nomes de terceiros ficam fora deste repositório de propósito)
grep -rniE "$PADRAO" .
# única ocorrência: a semente 0x9E3779B97F4A7C15 do gerador de ruído do dither (constante de razão áurea, de uso geral)
```

## O que foi removido ou trocado (e por quê)

| Item | Antes | Agora | Motivo |
|---|---|---|---|
| Predefinições do equalizador | 23 curvas lidas do binário do programa (nomes e valores) | 12 curvas nossas em dB (`EQ_PRESETS_DB` em `src/ui/extras.rs`): Plano, Voz clara, Podcast quente, Mais/Menos graves, Mais/Menos agudos, Fones, Alto-falante de notebook, Rádio antigo, Música ao vivo, Volume baixo à noite | dados extraídos |
| Modos do compressor | tabela lida do binário (nomes e valores) | modos nossos: Desligado, Fala, Música, Transmissão, Filme, com valores nossos | dados extraídos |
| Predefinições do Gravador | nomes e taxas do programa | as nossas: AAC voz 64 kbps, MP3 96/192/320 kbps, AAC 160 kbps, FLAC, WAV | textos copiados |
| Modelos de sessão | 17 modelos extraídos (nomes, textos, blocos e posições) | 12 modelos nossos em pt-BR (`src/templates.json`), com categoria e ícone, posições calculadas | dados e textos extraídos |
| Importação de sessões do programa | leitor do formato de sessão dele (plist com as chaves dele) e a dependência `plist` | removido; o app só lê o próprio formato JSON | formato de arquivo e chaves copiados |
| Identificadores dos blocos | nomes internos das classes do programa | ids nossos em inglês (`app_source`, `volume`, `loudness`…) | nomes copiados |
| Nomes e descrições dos blocos | textos da biblioteca do programa | títulos e descrições nossos em pt-BR (`KINDS` em `src/model.rs`) | textos copiados |
| Chaves de parâmetros | três chaves com os nomes do programa | `amount`, `boost`, `file_name` | nomes copiados |
| Eventos de automação | nomes da API dele | `recording_finished`, `session_starting`, `session_stopped`, `timer_starting`, `timer_stopped` | API copiada |
| API de scripts | objetos, métodos e marcadores com os mesmos nomes da API do programa, e os scripts embutidos dele | API nossa: `kalango.*` (sessions, session, start, stop, stopAll, focus, quit, notify, log) e `ctx.*` (kind, session, recording com trash, showInFiles, openWith, transcribe); cabeçalho `// requer: gravação`; 8 scripts de exemplo nossos | API e textos copiados |
| Volume com reforço | limitador com janela, teto e liberação lidos do binário | limitador nosso: antecipação de 5 ms, teto −1 dBFS, liberação de 80 ms | algoritmo e constantes extraídos |
| Balanço e panorama | leis lidas do binário | balanço linear do lado oposto; panorama de potência constante (seno/cosseno, −3 dB no centro) | algoritmo extraído |
| Graves e agudos | cantos e escalas do binário | prateleiras do "Audio EQ Cookbook" (RBJ) em 120 Hz e 8 kHz, ±12 dB | constantes extraídas |
| Equalizador gráfico | centros e largura do binário | centros ISO de oitava (31,5 Hz … 16 kHz), Q = √2 | constantes extraídas |
| Remover zumbido, compressor, encorpar, alargar estéreo, medidor de ponteiro, redução de ruído, remover cliques, ducking, atraso | fórmulas, tabelas e tempos do binário | projetos nossos, descritos em `COMPORTAMENTO.md` | algoritmos e constantes extraídos |
| Medidores | regra de queda, retenção e "clip" lida do binário | queda de 20 dB/s, pico retido 1,5 s, luz de clip em 0 dBFS por 2 s, VU de 300 ms | constantes extraídas |
| Silêncio do Gravador | −60 dB / 2 s (padrões do programa) | −50 dB / 10 s | valores extraídos |
| Qualidade do RTMP | rótulos e taxas do programa | Leve 1 Mbps, Padrão 3 Mbps, Alta 6 Mbps | textos copiados |
| Textos da interface | em inglês, muitos copiados (diálogo de saída, página de leitor de tela, faixa de teste grátis, "Sobre", nomes de janelas) | tudo em pt-BR com redação nossa; faixa de teste, página de leitor de tela e aviso do editor removidos; "Sobre" nosso | textos copiados |
| Tema | "cores do tema escuro do programa" | tokens do KalangoOS | aparência copiada |
| Nomes | pacote, binário, janela, menu, pasta de configuração e variáveis de ambiente com o nome do produto | `kalango-audio`, app id `os.kalango.Audio`, "Kalango Audio", `~/.config/kalango/audio/`, `KALANGO_AUDIO_*` | marca de terceiros |
| Ícone | desenhado em código | arte do KalangoOS (`assets/icons/`, `assets/marca/`) | identidade nossa |

## Licenças

- O Kalango Audio é **GPL-3.0-or-later** (`LICENSE`).
- Dependências (linux x86_64): MIT, Apache-2.0, BSD-2/3, ISC, Zlib, Unicode-3.0, Unlicense, 0BSD,
  Apache-2.0 WITH LLVM-exception; `kalango-movimento` (as molas do KalangoOS) é GPL-3.0-or-later, como nós. O GTK 4
  é a biblioteca do sistema, ligada dinamicamente (aceita pelo KalangoOS); os crates `gtk4`/`glib` são MIT.
- Fora da lista, só em alternativas OR (escolhemos a permissiva): `ryu-js` (Apache-2.0 OR BSL-1.0),
  `r-efi` (MIT OR Apache-2.0 OR LGPL-2.1-or-later), `anymap3` (BlueOak-1.0.0 OR MIT OR Apache-2.0).
- Os ícones de linha (`assets/ui/`) e os tokens de cor (`assets/css/tokens-*.css`) são do KalangoOS.
- Isolar voz (`crates/kalango-isolar-voz`): porte em Rust do **Inworld Voice Isolator** v1
  (<https://github.com/inworld-ai/voice-isolator>, <https://huggingface.co/inworld/voice-isolator>). Código **e pesos**
  são **Apache-2.0** (README e MODEL_CARD do projeto; `NOTICE`: Copyright 2026 Inworld AI). Os pesos foram treinados
  também com VocalSet (CC BY 4.0), IndicVoices-R (CC BY 4.0) e VCTK (ODC-By 1.0), citados no `NOTICE` deles. Os pesos
  **não** entram no git: ficam em `~/.cache/kalango-isolar-voz/weights` (SHA-256 em `src/lib.rs`, conferidos).
  Os testes só usam sinais sintéticos e a referência gerada localmente com o PyTorch deles.
- `nnnoiseless` (BSD-3-Clause) é uma reescrita do RNNoise; os pesos do modelo vêm do projeto RNNoise (BSD).

## O que esta auditoria não resolve

- A **estrutura** do app (sessões feitas de blocos fonte → efeito → saída, o conjunto de tipos de bloco, a janela
  com lista de sessões, quadro e inspetor) veio da reconstrução, que foi escrita olhando o programa comercial.
  Ideias e funcionalidades não são protegidas por direito autoral, e nenhum código, texto, dado ou imagem dele
  ficou; mas isto não é uma reconstrução "sala limpa" documentada. A interface nova, desenhada no Design System
  do KalangoOS (página "26 · Kalango Audio"), substituiu a disposição herdada (interface em GTK 4).
- O repositório foi tornado público por quem mantém o projeto, depois de ler este documento.
