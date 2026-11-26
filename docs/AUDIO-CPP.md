# audio.cpp como backend de modelos de áudio

Decisão do mantenedor: os modelos de áudio do KalangoOS rodam no [audio.cpp](https://github.com/0xShug0/audio.cpp)
(C++/ggml, CUDA/Vulkan/CPU, Apache-2.0) como **processo externo** controlado por Rust (crate `kalango-audiocpp`), como o
llama.cpp. A porta própria em Rust do Inworld Voice Isolator (`crates/kalango-isolar-voz`, ver `ISOLAR-VOZ.md`) fica como
referência de paridade e auditoria: não é apagada e não é o caminho de produção (CPU, RTF 3 a 4, não é tempo real).

## Versão fixada

| Item | Valor |
|---|---|
| Repositório | https://github.com/0xShug0/audio.cpp |
| Tag | `v0.9.6` (objeto da tag `cfcdc7af62b5db6168b5bac417ef28d74164fa9c`) |
| **Commit** | `04a37f2e80ff2dbcdd84cc67848a970bd02a73e0` (ggml 0.12.0, versão do projeto 0.2.1) |
| Família usada | `voice_isolator` (tarefa `s2s`) |

## Receita de build (Linux, CUDA, só o que usamos)

Máquina: RTX 3090 (sm_86), CUDA Toolkit 12.0 do apt (`nvcc 12.0.140`), gcc 13 como compilador principal. O nvcc 12.0 não
aceita gcc 13 como compilador de host, então o host do CUDA é o **g++-12** (`CUDAHOSTCXX`). Sem `glslc`/Vulkan SDK nesta
máquina: o backend Vulkan não foi construído nem testado.

```bash
git clone --depth 1 --branch v0.9.6 https://github.com/0xShug0/audio.cpp ~/.cache/kalango-audiocpp/src
cd ~/.cache/kalango-audiocpp/src            # git rev-parse HEAD deve dar 04a37f2e80ff...
export CUDAHOSTCXX=/usr/bin/g++-12
export CMAKE_C_COMPILER_LAUNCHER=sccache CMAKE_CXX_COMPILER_LAUNCHER=sccache
scripts/build_linux.sh --deployment-build --backend cuda --cuda-arch 86 \
  --model-set custom --models voice_isolator --with-warmbench \
  --build-dir ~/.cache/kalango-audiocpp/build-cuda --jobs 4 \
  --target audiocpp_cli --target audiocpp_server --target voice_isolator_warm_bench
```

- `--model-set custom --models voice_isolator` liga só a família que usamos (491 passos de build, cerca de 20 min com
  `-j4` e `nice`); `--deployment-build` embute o catálogo de especificações. Sem WebUI: o `dist` do WebUI é opcional e o
  servidor embute uma página vazia; **não precisa de node**.
- Tamanhos: cada binário tem 320 a 385 MB (runtime CUDA ligado estaticamente), árvore de fonte 370 MB, diretório de
  build cerca de 3,3 GB; tudo em `~/.cache/kalango-audiocpp` (4,0 GB com os pesos). Nada fora de `~/.cache`.
- Para empacotar mais famílias, mudar `--models` (lista separada por vírgula) e reconstruir.

## Pesos

Caminho usado: **os dois safetensors originais que já estavam na máquina** (`~/.cache/kalango-isolar-voz/weights`,
SHA-256 `647335a6aeb4dc706b895532c209d51e0e84a4849d17b339f00d4392a050ae53` offline e
`76598f05523c0669e76562a8b2f31c9d395fca316e80c907ee11775e7497ae0f` realtime, os mesmos que o audio.cpp fixa no pacote
`voice_isolator_safetensors`). O audio.cpp os carrega direto (`--model <diretório com os dois arquivos>`, ou o arquivo
`denoiser_realtime.safetensors`): sem conversão, sem rede.

Antes de a correção chegar, os dois GGUF F32 documentados (`Etherll/voice-isolator-GGUF`, revisão
`76a44d3262a3f248f02d34142857af0cec9c91ce`) já tinham sido baixados: `voice-isolator-offline-f32.gguf` 256 289 664 B
(`2ad7c614…ca54`) e `voice-isolator-realtime-f32.gguf` 232 808 896 B (`0849ffb2…a7e9`), SHA-256 conferido contra os
metadados do Hugging Face; 489 098 560 B no total. Medido: a saída GGUF é **idêntica bit a bit** à dos safetensors
(erro máximo 0). Os GGUF não são necessários e podem ser apagados.

## Resultados medidos (esta máquina)

Ryzen 7 5700G + RTX 3090 (24 GiB), compartilhados com outros agentes: a carga média ficou entre 3 e 30 durante as
medições (em CUDA isso quase não pesa; em CPU pesa, e as linhas marcadas com `*` foram medidas sob carga 30). Entrada:
os mesmos clipes da porta Rust (`~/.cache/kalango-isolar-voz/ref`, 48 kHz mono, sintéticos): o clipe de 60 s é o
`pink_5db_12s` repetido 5 vezes. `voice_isolator_warm_bench` (blocos de 20 ms) e o exemplo `stream` do crate (servidor
real, PCM por socket) deram o mesmo RTF, 0,146 a 0,149.

### Velocidade, memória e latência

| caminho | RTF | pico RSS | pico VRAM |
|---|---|---|---|
| **audio.cpp CUDA, fluxo, blocos de 20 ms, 60 s** | **0,147** | 352 MB | +1042 MiB |
| audio.cpp CUDA, fluxo, blocos de 10 ms, 60 s | 0,265 | 358 MB | igual |
| audio.cpp CUDA, offline, 60 s | **0,068** | 468 MB | +1156 MiB |
| audio.cpp CPU, fluxo, 8 threads, 20 s | 3,19 | 644 MB | |
| audio.cpp CPU, fluxo, 16 threads, 20 s `*` | 10,2 | 642 MB | |
| audio.cpp CPU, offline, 8 threads, 24 s `*` | 6,37 | 1,05 GB | |
| porta Rust (CPU), fluxo, 4 threads, 12 s (esta sessão, carga 10 a 30) | 3,1 | 1,09 GiB (doc anterior) | |
| porta Rust (CPU), fluxo 4 threads, 60 s (doc anterior) | 3,89 | 1,09 GiB | |
| PyTorch (CPU), fluxo 4 threads, 60 s (doc anterior) | 6,21 | 2,42 GiB | |

A VRAM é o total da placa menos a linha de base de 769 MiB de outros programas. A CPU do audio.cpp (RTF 3,19) é a
mesma ordem de grandeza da porta Rust: o ganho vem inteiro da GPU, cerca de 20 vezes mais rápida que a CPU, com RTF 0,147
no fluxo e 0,068 no offline.

Latência em tempo real, medida pelo crate (`stream ... pace`, um bloco de 20 ms a cada 20 ms durante 60 s, servidor
real em loopback, CUDA): o áudio isolado de cada trecho chega **p50 20,1 ms, p99 40,1 ms, máx 60,1 ms depois do envio
do último bloco de entrada de que ele precisava** (o servidor entrega em trechos de 40 ms). Somando a latência
algorítmica do modelo (2048 + 480 x lookahead amostras = **62,7 ms** com lookahead 2), o atraso de ponta a ponta do
processamento fica em torno de **83 ms (p50) a 103 ms (p99)**, sem contar captura e reprodução do dispositivo. Só a
computação por bloco de 20 ms (`voice_isolator_warm_bench`): p50 2,80 ms, p95 3,54 ms, máx 8,99 ms. A porta Rust tinha
p50 78 ms, p99 134 ms, máx 311 ms por bloco de 20 ms só de cálculo (4 threads, CPU).

### Paridade numérica (clipes de 3 s e de 12 s; SI-SDR do resultado contra a referência, e erro absoluto máximo)

| clipe | modo | audio.cpp CUDA vs PyTorch | audio.cpp CUDA vs porta Rust | porta Rust vs PyTorch |
|---|---|---|---|---|
| pink_0db_3s | fluxo | 73,9 dB / 3,6e-4 | 73,9 dB / 3,6e-4 | 108,1 dB / 1,8e-6 |
| pink_0db_3s | offline | 73,3 dB / 9,2e-5 | 73,3 dB / 9,2e-5 | 120,5 dB / 4,6e-7 |
| babble_10db_3s | fluxo | 72,7 dB / 2,6e-4 | 72,8 dB / 2,6e-4 | 94,2 dB / 1,3e-5 |
| babble_10db_3s | offline | 68,4 dB / 2,1e-4 | 68,1 dB / 1,8e-4 | 77,8 dB / 7,7e-5 |
| music_5db_3s | fluxo | 70,5 dB / 2,9e-4 | 70,7 dB / 2,9e-4 | 92,9 dB / 1,1e-5 |
| music_5db_3s | offline | 77,5 dB / 5,4e-5 | 77,5 dB / 5,5e-5 | 104,7 dB / 3,3e-6 |
| pink_5db_12s | fluxo | 74,2 dB / 4,6e-4 | 74,2 dB / 4,6e-4 | 115,2 dB / 1,4e-6 |
| pink_5db_12s | offline | 74,1 dB / 1,5e-4 | 74,1 dB / 1,5e-4 | 131,2 dB / 2,5e-7 |

Leitura: em CUDA o audio.cpp fica 68 a 77 dB de SI-SDR (erro máximo 5e-5 a 4,6e-4, em amostras de escala 1) da
referência, o que é o piso esperado de TF32 nos tensor cores (o doc do audio.cpp diz 1,5e-4) e, no fluxo pelo servidor,
da quantização para 16 bits da rota ao vivo. Inaudível, mas **não é a paridade de 99 dB da porta Rust**. Em CPU com
pesos F32 e cache F32 o audio.cpp bate a referência em 117,7 dB (erro 6,6e-7, offline, pink_0db_3s); o fluxo em CPU
usa por padrão cache de atenção Q8_0 e cai a 55,7 dB (7,4e-4), como o doc dele avisa. O SI-SDR do fluxo CUDA contra o
sinal limpo é 10,2 a 16,6 dB (os clipes são sintéticos; não substituem teste em fala real).

## Licenças

| Parte | Licença | Observação |
|---|---|---|
| audio.cpp (código) | Apache-2.0 | `LICENSE` no repositório; ggml e demais dependências em `external/` com as próprias licenças |
| Pesos Inworld Voice Isolator v1 (`inworld/voice-isolator`, revisão 9b0285b3…) | Apache-2.0 | `docs/model_licenses.md` do audio.cpp: uso comercial permitido, linha verificada em 2026-11-20 |
| GGUF `Etherll/voice-isolator-GGUF` | Apache-2.0 | conversão fiel dos mesmos pesos; mantém a licença do original |
| Créditos dos dados de treino | ver `PROVENIENCIA.md` | manter o aviso do Inworld e os créditos dos dados no nosso `PROVENIENCIA.md` ao distribuir |

A licença do código do audio.cpp não cobre os pesos: cada família mantém a licença do modelo de origem (tabela abaixo).

## Como será chamado

Processo, não biblioteca. O crate `kalango-audiocpp` sobe `audiocpp_server` (HTTP em 127.0.0.1) e fala com ele:

1. **Ciclo de vida**: caminho absoluto do binário, sem shell, ambiente limpo (só o que a configuração passa),
   `PR_SET_PDEATHSIG`=SIGKILL, sinais por pidfd (sem corrida com pid reaproveitado), `Drop` mata e colhe o filho,
   `stop(grace)` manda SIGTERM e depois SIGKILL. Diretório de trabalho 0700 nosso (de preferência sob
   `XDG_RUNTIME_DIR`; `Config::private_work_dir()`), configuração gravada 0600 com `O_NOFOLLOW`, log do servidor ao lado.
2. **Saúde e posse do socket**: o servidor recusa a porta 0 e não tem socket Unix nem chave de API. Escolhe-se uma porta
   livre, mas **nada é confiável pelo número da porta**: se outro processo a tomar antes, o bind do filho falha e ele
   sai (nova tentativa com outra porta); `/health` e o áudio só são usados quando o kernel diz (inode do socket em
   `/proc/<pid>/fd` contra as linhas LISTEN de `/proc/<pid>/net/tcp`) que o socket naquela porta é do próprio filho, e
   isso é reverificado antes de cada `/health` e de cada fluxo. Testes (`tests/ownership.rs`): um ouvinte falso que tomou
   a porta, ou que existe enquanto o filho não escuta, nunca recebe conexão.
3. **Fluxo ao vivo**: `POST /v1/audio/speech/live?model=isolator&sample_rate=48000&channels=1&sample_format=f32le&stream_format=audio`
   com corpo `Transfer-Encoding: chunked` de PCM f32 little endian; a resposta é PCM 16 bits mono 48 kHz em chunks, no
   mesmo socket, enquanto a entrada continua aberta. `Stream::push` envia amostras, `Stream::pull(timeout)` devolve o
   que chegou, `Stream::finish` fecha a entrada (chunk final) e recolhe o resto. Timeouts de leitura e escrita em todo
   socket (`Config::io_timeout`). A configuração do modelo vai no JSON gerado (`family: voice_isolator`,
   `task: s2s`, `mode: streaming`, `session_options: voice_isolator.lookahead_frames` 0, 2 ou 4).
4. **Por que não a CLI com stdin/stdout**: `audiocpp_cli --audio - --input-format f32le --input-rate 48000` aceita PCM ao
   vivo pela entrada padrão, mas só escreve o arquivo WAV de saída **quando a entrada termina** (verificado: com o stdin
   ainda aberto o arquivo não existe), então não devolve áudio incrementalmente. Serve para lote, não para tempo real.
   A rota `/live` do servidor é a que devolve áudio enquanto a entrada ainda está aberta.

Um servidor mantém o modelo carregado (cerca de 1 GiB de VRAM, 0,3 s para carregar em CUDA) e, segundo a documentação do servidor, a rota
trava o modelo durante o pedido (`busy_timeout_ms`), ou seja, um fluxo por vez por id de modelo: **não testado aqui**.
Para dois fluxos simultâneos, registrar dois ids ou subir dois servidores (a validar).

## Integrado no Kalango Audio (bloco "Isolar voz")

O audio.cpp é o backend do isolador de voz no app. O que existe, onde e como foi medido (mesma máquina, RTX 3090
compartilhada, carga entre 9 e 15 durante estas medições):

| Peça | Onde | O que faz |
|---|---|---|
| descoberta | `kalango-audiocpp::discovery` | programa em `KALANGO_AUDIOCPP_SERVER` ou `~/.local/share/kalango/audiocpp/bin/audiocpp_server`; modelo em `KALANGO_AUDIOCPP_MODEL` (arquivo ou pasta) ou `~/.local/share/kalango/modelos/voice-isolator`; backend por `KALANGO_AUDIOCPP_BACKEND` ou CUDA se há `/dev/nvidiactl`, senão CPU. Um valor de variável errado é apontado, não substituído. Nada é baixado. |
| serviço | `kalango-audiocpp::service` | um `audiocpp_server` por app, lazy (primeiro uso), uma thread gerente dona do filho (o `PR_SET_PDEATHSIG` vale para a thread que criou o filho), parada após 120 s sem uso e no `shutdown`, reinício com espera 1, 2, 4, 8 s (teto 30 s), desiste após 5 falhas seguidas (`retry()` recomeça), lê o backend real em `/health`, mede 1 s de som ao subir (RTF) e só libera o modo ao vivo com placa de vídeo e RTF < 0,8 |
| ao vivo | `kalango-audiocpp::live` | `LiveIsolator::process` roda na thread de áudio sem esperar: manda o mono em blocos de 20 ms a uma thread que fala com o servidor e mistura a voz isolada com o original por posição de amostra; o original é segurado pelo atraso do modelo; sem modelo pronto o original passa direto |
| gravação | `kalango-audiocpp::offline` | manda a gravação inteira o mais rápido possível, mais 8192 amostras de silêncio para esvaziar a antecipação do modelo, e corta no tamanho da entrada |
| app | `src/isolator.rs`, `src/dsp.rs`, `src/gui/editor_inspector.rs`, `src/gui/gravacoes.rs` | bloco "Isolar voz" (intensidade, estado, atraso, o que falta), ação "Isolar voz" em Gravações (`nome (voz isolada).wav`, 24 bits mono 48 kHz), `isolator::shutdown()` ao fechar |

Verificado à mão com o app e o servidor reais: `kill -9` no app mata o servidor no mesmo instante (sem órfão) e a pasta de trabalho que sobra é apagada na partida seguinte (`Config::remove_stale_work_dirs`).

O servidor é iniciado com `--config`, `--no-ui` e `--max-loaded-models 1`, `host` 127.0.0.1, sem `--ui-management` (sem
gerenciador de modelos nem download), pasta de trabalho 0700 sob `XDG_RUNTIME_DIR`, ambiente do filho limpo.

### Medido pelo serviço, com o binário e os pesos reais (`tests/real_service.rs`, clipe `pink_5db_12s`)

| Item | Resultado |
|---|---|
| pronto depois do primeiro uso | cerca de 1,0 s (carga do modelo + 1 s de som de teste de velocidade) |
| velocidade medida ao subir (RTF) | 0,19 a 0,49 (inclui o aquecimento do primeiro trecho; limite do modo ao vivo 0,8) |
| memória do servidor | 302 MiB de RAM (processo), só a família `voice_isolator` carregada |
| gravação (offline pelo serviço), 12 s | RTF 0,124 a 0,26 (1,5 a 3,1 s); SI-SDR 74,4 dB contra a referência armazenada no corpo do clipe; 30,6 dB nas últimas 8192 amostras (fim do fluxo: a referência foi gerada sem o silêncio extra); 44,6 dB no clipe todo |
| ao vivo pelo app (`LiveIsolator`, blocos de 10 ms em tempo real) | atraso fixo de 151,3 ms (p50 = p99) numa execução limpa, 72,4 dB de SI-SDR contra a referência nos últimos 4 s, 0 a 1 "resposta atrasada" |

Leituras honestas:

- O atraso ponta a ponta pelo app (cerca de 150 ms) é maior que os 83 a 103 ms do servidor sozinho: somam-se o lote de
  20 ms antes do envio, a espera de 20 ms que dá margem ao jitter (`PRIME` em `live.rs`), o tique de 10 ms do motor e o
  trecho de 40 ms em que o servidor entrega.
- Quando uma resposta do servidor chega tarde (GPU dividida com outros programas), o som daquele instante some por um
  momento e o atraso aumenta pelo tamanho da falha, de forma permanente até o bloco reiniciar; medi 4 a 9 falhas e
  atrasos de 151 a 231 ms com a máquina em carga 15, contra 0 a 1 falha em carga baixa. Passando de 0,6 s de som
  esperando o modelo, o bloco desiste e deixa o original passar (e tenta de novo).
- O modo "gravação" usa o checkpoint realtime pela rota ao vivo (RTF 0,13 a 0,26 nestas medições, 0,068 só foi medido
  com o `audiocpp_cli` e o checkpoint offline). Usar o checkpoint offline exigiria registrar um segundo modelo no
  servidor e uma rota que o wrapper ainda não fala: fica como melhoria.

### Testes

- `crates/kalango-audiocpp/tests/service.rs` (17 testes, com o servidor falso de `tests/support/fake_server.rs`, que
  entende `/health` e `/v1/audio/speech/live`): início lazy, parada por ociosidade e nova partida, `shutdown` sem processo
  nem pasta restantes, o filho morre se o `Service` é solto, backend lido do `/health`, teste de velocidade, servidor que
  morre e volta com espera, desistência e `retry`, servidor que sai logo, gravação com alinhamento amostra a amostra e
  cancelamento, mistura ao vivo alinhada sem eco (ganho 0,5, atraso do modelo, intensidade 1, 0,5 e 0), original
  inalterado enquanto o modelo não está pronto, "só gravação" em servidor só CPU, queda por lentidão, queda do servidor
  no meio do som e volta, soltar o bloco deixa o servidor parar.
- `src/isolator.rs` e `src/dsp.rs` (com o mesmo servidor falso): bloco sem arquivos deixa o original passar e diz o que
  falta; com servidor, a saída é o som isolado com o atraso informado; a ação sobre gravação grava ao lado sem
  sobrescrever, respeita a intensidade e cancela.
- `tests/real_service.rs` (`#[ignore]`, binário e pesos reais; `KALANGO_AUDIOCPP_REF_DIR` aponta para a pasta do clipe de
  referência): isola `pink_5db_12s` offline e ao vivo e exige SI-SDR acima de 60 dB no corpo (offline) e 30 dB nos últimos
  4 s (ao vivo, em até três tentativas por causa da GPU compartilhada).

```bash
KALANGO_AUDIOCPP_SERVER=$A/build-cuda/bin/audiocpp_server \
KALANGO_AUDIOCPP_MODEL=$A/models/voice-isolator/denoiser_realtime.safetensors \
  cargo test -p kalango-audiocpp --test real_service -- --ignored --nocapture
```

Capturas do bloco no app (tema escuro, pt_BR): `capturas/isolar-voz-sem-modelo.jpg` (o que falta e onde colocar),
`capturas/isolar-voz-pronto.jpg` (instalado, sessão parada) e `capturas/isolar-voz-rodando.jpg` (sessão rodando com o
servidor real: "Isolando a voz", atraso de 150 ms, placa de vídeo CUDA).

### Empacotamento (proposta, nada foi adicionado a ISO nem pacote)

O `audiocpp_server` com CUDA tem 320 a 385 MB (runtime CUDA estático) e os pesos 0,5 GB: não entram na ISO. Opções:

1. **Download opcional na Central de Apps**, com tela de consentimento (licença Apache-2.0 do audio.cpp e dos pesos
   Inworld, créditos de `PROVENIENCIA.md`, tamanho, destino), que grava o programa em
   `~/.local/share/kalango/audiocpp/bin/` e os pesos em `~/.local/share/kalango/modelos/voice-isolator/` (é onde o app já
   procura), conferindo SHA-256. É o caminho recomendado: o app não muda, só passa a achar os arquivos.
2. **Compilação Vulkan** (sem CUDA, algumas dezenas de MB), a mesma receita com `--backend vulkan`; funciona em placas
   AMD, Intel e NVIDIA. Precisa de `glslc`/Vulkan SDK na máquina de build e **não foi construída nem testada**; o app já
   aceita `KALANGO_AUDIOCPP_BACKEND=vulkan`, mas a velocidade ao vivo é desconhecida.
3. **Compilação só CPU**, pequena, serve à ação sobre gravação (RTF cerca de 3 a 6: um minuto de som leva de 3 a 6
   minutos) e o bloco ao vivo fica desligado, como já acontece quando o servidor informa CPU.

Em qualquer opção a ISO continua sem o programa e sem os pesos; o bloco mostra o que falta e onde colocar.

## Outras famílias para avaliar (não foram executadas)

Do `docs/model_licenses.md` do audio.cpp v0.9.6; "uso comercial" é o resumo do projeto, vale a licença do texto original.
Para as legendas do Kalango Cast (ASR em fluxo) e decisões de áudio:

| Família | Uso | Licença | Comercial | Nota |
|---|---|---|---|---|
| `lfm3_audio` | ASR, TTS (38 vozes), conversa fala a fala (S2S); experimental; fluxo para TTS e S2S | LFM Open License v1.0 | **Condicional** | só organizações com receita anual < USD 10M |
| `lfm2_audio` | ASR/TTS/S2S 1,5B | LFM Open License v1.0 | **Condicional** | idem; codificador baseado em canary-180m-flash (CC-BY-4.0) |
| `lfm_d1_omni` | decisões sobre áudio (perguntas nomeadas, resposta JSON); não gera fala nem transcrição | LFM Open License v1.0 | **Condicional** | idem |
| `voxtral_realtime` | ASR em fluxo (Mistral Voxtral Mini 4B Realtime) | Apache-2.0 | Sim | candidato a legenda ao vivo |
| `nemotron_asr` | ASR em fluxo 0,6B | OpenMDW-1.1 | Sim | candidato a legenda ao vivo |
| `moonshine_asr` | ASR em fluxo tiny/small/medium | MIT | Sim | leve, bom para CPU |
| `vibevoice_asr_streaming` | ASR em fluxo | MIT | Sim | 1,5B e 7B |
| `qwen3_asr` | ASR 0,6B e 1,7B | Apache-2.0 | Sim | |
| `parakeet_tdt` | ASR | CC-BY-4.0 (v3 NVIDIA) | Sim | exige atribuição |
| `cohere_asr`, `granite5asr`, `granite_speech5`, `fun_asr_nano` | ASR | Apache-2.0 | Sim | |
| `canary_asr` | ASR | CC-BY-4.0 | Sim | atribuição |
| `sense_asr` | ASR SenseVoice | FunASR Model Open Source License v1.1 | Sim | creditar fonte e autores |
| `nemotron_3_diar`, `sortformer_diar_v2` | diarização | OpenMDW-1.1; NVIDIA Open Model License | Sim | `sortformer_diar` (v1) é CC-BY-NC-4.0: não |
| `silero_vad`, `earshot_vad`, `firered_vad`, `pulsevad` | detecção de voz | MIT; MIT OR Apache-2.0; Apache-2.0; MIT | Sim | |
| `mossformer2` | separação, realce 48 kHz e super-resolução | Apache-2.0 | Sim | alternativa de realce |
| `kokoro_tts`, `qwen3_tts`, `pocket_tts` | TTS | Apache-2.0; Apache-2.0; CC-BY-4.0 | Sim | |

Evitar (não comercial ou sem licença clara): `crisperwhisper` (inclui restrição sobre as saídas), `omnivoice`,
`voxtral_tts`, `sortformer_diar` v1 (todos NC), `marblenet_vad`, `zipvoice`, `rvc` e `piper_tts` (licença não clara).
As três LFM só valem se a receita da organização ficar abaixo do limite: decidir antes de depender delas.

## Limites (honestos)

- A paridade em CUDA é de 68 a 77 dB, não 99 dB: TF32 nos tensor cores e saída de 16 bits na rota ao vivo. Para
  auditoria numérica exata use a CPU com pesos e cache F32 (117 dB), ou a porta Rust (92 a 131 dB contra o PyTorch).
- Só áudio sintético (ruído rosa, "música" sintética, "babble"); nenhuma fala real gravada, nenhuma avaliação perceptiva.
- Medições em máquina compartilhada. CUDA quase não foi afetada (a repetição deu o mesmo RTF); os números de CPU sob
  carga 30 (marcados com `*`) são pessimistas. A latência ponta a ponta exclui captura/reprodução e o agendamento do
  PipeWire. Loopback, uma só placa.
- O servidor não tem autenticação nem socket Unix e recusa a porta 0: o isolamento depende da prova de posse do socket
  (acima) e de ouvir só em 127.0.0.1. Outro usuário local ainda poderia conectar à porta do servidor e usá-lo; o fluxo
  de áudio de uma pessoa só fica protegido contra impostores, não contra um cliente extra.
- A saída da rota ao vivo é PCM de 16 bits (o servidor codifica assim); a entrada pode ser f32.
- Mono 48 kHz de saída; o checkpoint realtime atende só os últimos 12 s de contexto.
- Vulkan e CPU-only não foram construídos (sem `glslc`); só CUDA 12.0 + g++-12. O app aceita os dois (`KALANGO_AUDIOCPP_BACKEND`), mas nenhum foi exercitado. Nada foi instalado. Outro toolkit
  mudaria a regra do compilador de host.
- Binários de 320 a 385 MB cada (CUDA estático); em ISO, preferir uma compilação com só as famílias usadas.
- O audio.cpp é jovem (v0.9.x, 0.2.1 interno, formato dos GGUF e das especificações ainda muda): fixar o commit e
  revalidar a paridade a cada atualização (`examples/compare`, `examples/stream`).

## Como repetir as medições

```bash
A=~/.cache/kalango-audiocpp
# velocidade, blocos de 20 ms, 60 s, CUDA (sintético, só tempo)
$A/build-cuda/bin/voice_isolator_warm_bench --model $A/models/voice-isolator/denoiser_realtime.safetensors \
  --backend cuda --seconds 60 --push 960
# fluxo real pelo crate, tempo real (latência) e o mais rápido possível (RTF); saída em f32 cru
cargo run --release -p kalango-audiocpp --example stream -- $A/build-cuda/bin/audiocpp_server \
  $A/models/voice-isolator/denoiser_realtime.safetensors cuda ~/.cache/kalango-isolar-voz/ref/pink_5db_12s.in.f32 out.f32 60 960 2944 pace
# paridade
cargo run --release -p kalango-audiocpp --example compare -- out.f32 ~/.cache/kalango-isolar-voz/ref/pink_5db_12s.realtime.f32
# testes contra o binário real
KALANGO_AUDIOCPP_SERVER=$A/build-cuda/bin/audiocpp_server \
KALANGO_AUDIOCPP_MODEL=$A/models/voice-isolator/denoiser_realtime.safetensors \
  cargo test -p kalango-audiocpp -- --ignored
```
