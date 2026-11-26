# Como o Kalango Audio se comporta

Descrição do comportamento do nosso código, para quem usa e para quem mexe nele. Os valores vêm de
`src/dsp.rs`, `src/engine.rs`, `src/model.rs` e `src/ui.rs`; se mudar lá, mude aqui.

## Sessões e blocos

- Uma **sessão** é um grafo de blocos. **Fontes** (Aplicativo, Microfone ou entrada, Som do sistema, Tom de
  teste) não têm entrada; **saídas** (Saída de áudio, Gravador, Rádio online, Transmissão RTMP) não têm saída
  de áudio para outros blocos; **efeitos** e blocos **avançados** ficam no meio.
- Com **ligação automática** (padrão), um bloco novo é ligado ao vizinho; com "Ligar à mão" você puxa os fios.
  Um bloco recebe vários fios e soma o que chega.
- Um bloco desligado deixa o som passar sem mudança.
- O motor roda a 48 kHz, em estéreo de ponto flutuante. A carga do motor aparece na barra.
- Sessões e preferências ficam em `~/.config/kalango/audio/`. Os modelos estão em `src/templates.json`.

## Efeitos

| Bloco | O que faz |
|---|---|
| Volume | ganho de 0 a 100 %; o "reforço" multiplica por 1 a 4 e passa por um limitador com antecipação de 5 ms, teto de −1 dBFS e liberação de 80 ms |
| Balanço | abaixa o lado oposto de forma linear (−1 só esquerda … +1 só direita) |
| Panorama | potência constante (seno/cosseno), −3 dB no centro; em 0 o estéreo passa intacto |
| Graves e agudos | prateleiras (RBJ, inclinação 1) em 120 Hz e 8 kHz, ±12 dB |
| Equalizador gráfico | 10 filtros de pico nos centros ISO de oitava (31,5 Hz a 16 kHz), Q = √2, ±12 dB; o controle 0..1 tem o 0 dB em 0,5 |
| Passa-baixas / passa-altas / passa-faixa / prateleiras | biquads de 2ª ordem (Butterworth onde se aplica) |
| EQ paramétrico | até 16 faixas, cada uma com tipo, frequência, ganho e Q |
| Compressor | detector RMS de 5 ms, joelho suave de 6 dB, antecipação de 2 ms. "Quantidade" leva o limiar de 0 a −30 dBFS e a razão de 1:1 a 6:1; "Velocidade" vai de ataque 25 ms / liberação 400 ms a 1 ms / 50 ms; "Compensação" é um ganho linear de 1 a 4 |
| Encorpar | prateleira de graves (100 Hz) e de agudos (10 kHz) proporcionais à quantidade, compressão 3:1 e o limitador de −1 dBFS |
| Alargar estéreo | só para som mono: cria um sinal lateral atrasado 12 ms e filtrado acima do cruzamento (500 Hz por padrão), largura 0,5; os graves ficam no centro e a soma volta ao mono sem perdas |
| Canais | troca, junta ou isola os canais |
| Remover zumbido | cortes de Q 30 na frequência da rede (50 ou 60 Hz) e em 8 harmônicos; cada passo de "quantidade" corta 5 dB |
| Remover cliques | limita a velocidade de variação da onda (passo máximo 0,5 ÷ sensibilidade por amostra) |
| Limpar voz | RNNoise (biblioteca `nnnoiseless`), opcionalmente em mono |
| Isolar voz | mantém só a voz com o modelo de IA do audio.cpp (Inworld Voice Isolator), neste computador; "intensidade" mistura a voz isolada com o original, alinhados por amostra; veja "Isolar voz" abaixo |
| Redução de ruído | aprende 2 s do ruído de fundo e subtrai o espectro dele (fator 2) |
| Abaixar sob voz | quando a fonte de voz passa do limiar, as outras descem ao nível escolhido; mantém 0,5 s e muda em rampas de 50 ms |
| Convolução | aplica uma resposta ao impulso de arquivo (reamostrada para 48 kHz), com ganho de entrada em dB |
| Atraso | atraso em milissegundos, arredondado para a amostra mais próxima |
| Voltar no tempo | guarda até 30 min do som ao vivo para pausar e voltar |
| Alternar fonte | escolhe qual de duas entradas passa |
| Mixer | até 5 entradas com ganho, balanço, panorama, mudo, solo e fade de 1 s |
| Plug-in LADSPA | carrega um efeito LADSPA do sistema |

## Isolar voz

O bloco **Isolar voz** (e a ação **Isolar voz** em Gravações) usam o `audiocpp_server` do audio.cpp como processo
filho (crate `kalango-audiocpp`; detalhes e medidas em `AUDIO-CPP.md`). A porta em Rust (`kalango-isolar-voz`) não é
usada aqui: fica só como referência de paridade.

- **Onde estão os arquivos**: o programa em `KALANGO_AUDIOCPP_SERVER` ou `~/.local/share/kalango/audiocpp/bin/audiocpp_server`;
  o modelo (`denoiser_realtime.safetensors` ou um `.gguf` realtime) em `KALANGO_AUDIOCPP_MODEL` (arquivo ou pasta) ou
  `~/.local/share/kalango/modelos/voice-isolator`. Faltando um deles, o bloco diz o que falta e onde colocar; **nada é
  baixado** pelo app (o download com a licença dos pesos é outra etapa).
- **Placa de vídeo ou processador**: pede CUDA quando há driver NVIDIA (ou o que `KALANGO_AUDIOCPP_BACKEND` disser) e
  confia no que o servidor informa em `/health`. Ao subir, mede 1 s de som; só com placa de vídeo e velocidade abaixo de
  0,8 s por segundo de som o modo ao vivo é liberado. Só processador (RTF perto de 3) ou lento demais: o bloco avisa,
  deixa o som original passar e a ação sobre gravação continua valendo.
- **Ao vivo (bloco)**: o som estéreo vira mono, vai ao servidor em blocos de 20 ms numa thread à parte (o áudio nunca
  espera o modelo) e a voz isolada volta e é misturada com o original **por posição de amostra**: o original é segurado
  pelo mesmo atraso do modelo, então não há eco. O atraso mostrado é esse (cerca de 150 ms medidos no app; o servidor
  sozinho dá 83 a 103 ms). Intensidade 100 % é só a voz isolada; menos deixa parte do original voltar. Estados:
  iniciando, isolando, "sem isolar por enquanto" (reinício, atraso do modelo: o original passa direto), indisponível
  (desistiu depois de 5 falhas seguidas; botão "Tentar de novo") e "só gravação" (lento demais). Ligar/desligar o bloco é
  o interruptor dele; desligado, o modelo é solto.
- **Gravação (ação)**: decodifica com o `ffmpeg` (sem ele, só WAV), manda tudo ao servidor o mais rápido possível (RTF
  cerca de 0,13 a 0,25 em CUDA) e grava ao lado `nome (voz isolada).wav` (mono, 48 kHz, 24 bits), sem sobrescrever.
- **Ciclo de vida**: um servidor por app, subido só no primeiro uso, parado 2 min depois do último uso e ao fechar o app
  (se o app morrer, até com `kill -9`, o kernel mata o filho: `PR_SET_PDEATHSIG`; a pasta de trabalho que uma queda deixa é removida na próxima partida). Cai e volta com espera crescente (1, 2, 4, 8 s),
  até 5 falhas seguidas. Só a família do isolador é carregada (cerca de 300 MB de RAM e 1 GiB de VRAM).
- **Segurança**: o som não sai do computador; o servidor escuta só 127.0.0.1, sem WebUI (`--no-ui`) nem gerenciador de
  modelos, pasta de trabalho 0700, ambiente do filho limpo, e o app só fala com o socket que o kernel prova ser do filho.

## Medidores

- Barras: sobem na hora e caem 20 dB por segundo; o pico fica parado 1,5 s e depois cai na mesma velocidade.
- A luz de clip acende quando uma amostra chega a 0 dBFS e fica acesa 2 s.
- O medidor de ponteiro integra o nível em 300 ms; 0 VU corresponde a −18 dBFS e a escala vai de −20 a +3 VU.

## Gravador

- Formatos: WAV e AIFF direto; MP3, AAC, ALAC e FLAC pelo `ffmpeg` (sem ele, o Gravador cai para WAV e avisa).
- Pasta padrão: a pasta de música do usuário (XDG) + `Kalango Audio`. Nome do arquivo com fichas como
  `%name`, `%date`, `%time` e `%counter`.
- Pode dividir arquivos por tempo ou tamanho, parar sozinho num limite e pausar no silêncio
  (padrão −50 dB por 10 s; desligado por padrão).
- Etiquetas (título, artista, álbum, capa…) são gravadas pelo `ffmpeg` nos formatos que aceitam.

## Transcrever

Usa o modelo de voz local do KalangoOS (LFM3-Audio-2B), pelo `kalango-agente --json transcrever` (procurado em
`$KALANGO_AGENTE`, `/usr/libexec/kalango-agente` e no PATH). O som vai em trechos de 15 s, então o texto aparece
enquanto grava, uma linha `[mm:ss] texto` por trecho; trechos em silêncio não são enviados. O idioma é o do sistema.
O texto é salvo na pasta de documentos (XDG) + `Kalango Audio`; `ctx.recording.transcribe()` grava `<gravação>.txt`.
Nada sai do computador.

## Agenda e automações

- **Agenda:** horários por dias da semana ou numa data; a sessão inicia e para sozinha.
- **Automações:** disparam em `recording_finished`, `session_starting`, `session_stopped`, `timer_starting` ou
  `timer_stopped` e rodam um script em JavaScript com a nossa API:
  - `kalango.sessions()`, `kalango.session(nome)`, `kalango.start(nome)`, `kalango.stop(nome)`,
    `kalango.stopAll()`, `kalango.focus()`, `kalango.quit()`, `kalango.notify(texto)`, `kalango.log(texto)`;
  - `ctx.kind`, `ctx.session` (`name`, `running`, `blocks[]` com `bypassed` alterável, `start()`, `stop()`,
    `open()`) e `ctx.recording` (`path`, `name`, `seconds`, `bytes`, `trash()`, `showInFiles()`,
    `openWith('editor' | 'player')`, `transcribe()`).
  - Uma linha `// requer: gravação` ou `// requer: sessão` no topo diz o que o script espera.
- Os scripts rodam isolados (limite de laços e de recursão); o que eles pedem vira uma lista de ações que o app executa.
