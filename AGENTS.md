# AD3 Editing — regras do operador (Antonio)

## 100% local, zero online/com tokens

Os tokens de geração acabaram e não serão repostos. Este projeto opera
**somente local**, sempre:

- **Proibido** a qualquer agente e a qualquer fluxo usar: `generate.*`
  (imagem, vídeo, voz, áudio), transcrição automática sem `src` local,
  `models`, `voices`, `whoami`/login/conta, telas de planos, créditos,
  upgrade, checkout e callbacks de auth. Nada de upload, nuvem ou provedor
  pago.
- **Liberado** (local, sem créditos): `check`, `capture`, `export`, `probe`,
  `filmstrip`, `waveform`, `fonts`, transcrição local com timestamps e
  ferramentas FFmpeg/FFprobe locais.
- Exceção só com aprovação explícita do Antonio para aquele caso concreto.
  Pedido genérico ("gera aí", "transcreve") **não** autoriza online: o agente
  deve fazer local ou dizer que não dá.
- O formato de projeto TSX é o documento; não reescrever nem migrar formato
  por conveniência.

## Nome

O produto é **AD3 Editing**. Não reintroduzir "Diffusion Studio" em nomes
visíveis (executável, pastas de dados, pipe, protocolo `ad3-editing://`,
documentação) nem o escopo npm `@diffusionstudio/*` (atual: `@ad3-editing/*`).
Exceções intencionais e imutáveis: URLs externas reais (ex.: bucket S3
`diffusion-studio-public`) e histórico de releases já publicados.
