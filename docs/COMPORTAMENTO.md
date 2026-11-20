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
| Redução de ruído | aprende 2 s do ruído de fundo e subtrai o espectro dele (fator 2) |
| Abaixar sob voz | quando a fonte de voz passa do limiar, as outras descem ao nível escolhido; mantém 0,5 s e muda em rampas de 50 ms |
| Convolução | aplica uma resposta ao impulso de arquivo (reamostrada para 48 kHz), com ganho de entrada em dB |
| Atraso | atraso em milissegundos, arredondado para a amostra mais próxima |
| Voltar no tempo | guarda até 30 min do som ao vivo para pausar e voltar |
| Alternar fonte | escolhe qual de duas entradas passa |
| Mixer | até 5 entradas com ganho, balanço, panorama, mudo, solo e fade de 1 s |
| Plug-in LADSPA | carrega um efeito LADSPA do sistema |

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

Usa `whisper-cli` com um modelo local; o texto é salvo na pasta de documentos (XDG) + `Kalango Audio`.
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
