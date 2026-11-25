# Isolar voz (porte em Rust do Inworld Voice Isolator v1)

Crate `crates/kalango-isolar-voz`: lê os safetensors oficiais por mmap e roda a rede em Rust puro (AVX2/FMA, sem
Python nem libtorch). Pesos e código de origem: Apache-2.0 (ver `PROVENIENCIA.md`); os pesos não ficam no git.

## Estado

- `cargo check`, `cargo clippy --all-targets -- -D warnings` e `cargo fmt --check` limpos no crate.
- Testes: unitários (kernels, pool) e `tests/reference.rs` contra o PyTorch com os pesos reais (pulam, avisando, se
  `~/.cache/kalango-isolar-voz` não existir; `scripts/make_reference.py` gera a referência).
- Só CPU x86_64 com AVX2+FMA. **Não há caminho de GPU.**

## Paridade com o PyTorch (CPU, f32, medida)

Por estágio, clipe `pink_0db_3s` (SNR = energia da referência / energia do erro):

| estágio | offline: erro máx / SNR | tempo real: erro máx / SNR |
|---|---|---|
| STFT | 1,5e-5 / 134,9 dB | (mesma) |
| encoder | 4,0e-4 / 111,4 dB | 3,1e-4 / 113,0 dB |
| blocos 0–5 | 2,9e-3 a 3,9e-3 / 99,7 a 104,8 dB | 1,3e-3 a 5,2e-3 / 100,2 a 109,6 dB |
| bloco 6 | 3,6e-1 / 106,4 dB | 3,0e-1 / 112,9 dB |
| decoder (`raw`) | 1,6e-5 / 118,8 dB | 3,5e-5 / 115,9 dB |
| saída | 4,6e-7 / 120,5 dB | 1,9e-6 / 106,8 dB |

(Erros máximos absolutos grandes nos blocos são em ativações de magnitude alta; o SNR mostra o erro relativo.)

Ponta a ponta, quatro clipes (SI-SDR entre Rust e PyTorch; erro máx absoluto): offline 77,8 a 131,2 dB (pior:
`babble_10db_3s`, 7,7e-5); fluxo em tempo real 92,9 a 115,2 dB (pior 1,1e-5). A diferença de SI-SDR contra o sinal
limpo entre Rust e PyTorch é de no máximo 0,0006 dB.

Limiares dos testes: 40 dB de paridade ponta a ponta, 0,1 dB de diferença de SI-SDR, 60 dB por estágio. Não foram
ajustados para passar: a aritmética f32 com outra ordem de soma (tiles AVX2 aqui, MKL no PyTorch) deixa ~1e-6 de erro
relativo por camada (acima de 100 dB, como medido), e um erro de lógica (cabeça de atenção, direção do scan, epsilon
de norma) custa dezenas de dB. 60 dB deixa folga de 40 dB sobre o ruído de arredondamento. A paridade foi medida só
em clipes sintéticos (ruído rosa, música sintética, "babble") de 3 e 12 s; não houve fala real gravada nem a bateria
dourada do projeto original (`tests/fixtures` dele).

Bug achado e corrigido pelos testes: o fluxo em tempo real estourava uma subtração no início (o `trim` do
overlap-add passava do primeiro quadro ainda por sintetizar).

## Velocidade (medida, clipe de 60 s = 12 s repetidos 5 vezes, 48 kHz)

Máquina: Ryzen 7 5700G (8 núcleos/16 threads), compartilhada com outros agentes durante as medições (carga entre 5
e 30): os números são pessimistas e variam. Build release. Python: PyTorch 2.7.1+cu126 em CPU, pacote
`inworld-voice-isolation` do clone local, mesmos pesos, mesmo clipe, mesma máquina, ao mesmo tempo da carga dos outros.

| modo | threads | RTF Rust | RTF Python | pico de RSS Rust | pico de RSS Python |
|---|---|---|---|---|---|
| tempo real (blocos de 20 ms) | 4 | 3,89 | 6,21 | 1,09 GiB | 2,42 GiB |
| tempo real | 8 | 3,47 | não medido | 1,09 GiB | não medido |
| offline (janelas de 8 s) | 4 | 3,71 | 10,01 | 0,53 GiB | 7,2 GiB |

Latência por bloco de 20 ms (Rust, 4 threads): p50 78 ms, p99 134 ms, máx 311 ms (Python: p50 115 ms, p99 325 ms, máx
1005 ms). A latência algorítmica do fluxo é 2944 amostras (61,3 ms), fora o tempo de cálculo.

**Conclusão honesta: em CPU não roda em tempo real (RTF > 1) nem em Rust nem em Python.** O modelo tem ~58 M de
parâmetros e processa 46 bandas x 100 quadros/s; estimativa grosseira de ~0,5 TFLOP/s necessários contra ~1 TFLOP/s
de pico teórico f32 do 5700G (estimativa, não medição). O Rust é 1,6x a 2,7x mais rápido que o PyTorch na CPU e usa
menos memória, mas para tempo real ao vivo é preciso GPU (a RTX 3090 desta máquina não é usada por este código) ou
quantização/modelo menor. Uso viável hoje: offline, em lote.

## O que falta

- Caminho de GPU (CUDA/Vulkan) ou quantização int8/bf16 para chegar a RTF < 1.
- Medir em fala real e rodar os fixtures dourados do projeto original.
- Medições com a máquina ociosa (as de cima disputaram CPU com outros processos).
- Integração no app (bloco "isolar voz") e textos pt-BR.
