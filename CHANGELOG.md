# Mudanças

## Não lançado

- **Isolar voz** (audio.cpp): novo bloco "Isolar voz" nas sessões, ao vivo no microfone (voz isolada misturada com o
  original por posição de amostra, intensidade de 0 a 100 %, estado e atraso na tela) e ação "Isolar voz" em Gravações
  que salva `nome (voz isolada).wav` ao lado da gravação. O backend é o `audiocpp_server` do audio.cpp, controlado
  pelo crate `kalango-audiocpp` (um servidor por app, só no primeiro uso, parado ao ficar ocioso e ao fechar o app,
  reinício com espera crescente). O programa e o modelo não vêm no app e **nada é baixado**: sem eles o bloco diz o que
  falta e onde colocar (`KALANGO_AUDIOCPP_SERVER`, `KALANGO_AUDIOCPP_MODEL`, ou `~/.local/share/kalango/`). Só com placa
  de vídeo o modo ao vivo é liberado; só com processador o bloco deixa o original passar e a ação sobre gravação segue
  valendo. A porta em Rust (`kalango-isolar-voz`) fica só como referência de paridade. Textos nos 9 idiomas
  (traduções de IA, sem revisão por nativos).
- `kalango-audiocpp`: descoberta de arquivos, serviço com ciclo de vida, `LiveIsolator`, isolamento de gravação,
  servidor falso com `/health` e a rota ao vivo para os testes, e o servidor agora sobe com `--no-ui` e
  `--max-loaded-models 1`.
